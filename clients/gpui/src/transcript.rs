//! An agent pane's conversation as the daemon streams it: `agent_attached` carries the
//! transcript so far, then `agent` events follow, text arriving in chunks.
//!
//! The fold mirrors WheelKit's `Transcript` (`apple/WheelKit/Sources/WheelKit/Transcript.swift`):
//! a message the person sends sits at the bottom as queued until its `turn_start` moves it into
//! place, the turn's rows follow it, a retract takes a message back, tool calls update in place
//! by id, the one plan row is replaced whole, and a turn's end or an error closes everything open.
use std::collections::HashMap;
use wheel_proto::{AgentEvent, AgentFooter, AgentState, ConfigOption, ContextUse, PlanItem, ServerMsg, ToolContent};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A message the person sent; `queued` while it waits for its turn to start.
    User { text: String, queued: bool },
    /// The agent's reply text; streamed chunks are merged into one row.
    Reply(String),
    /// The agent's reasoning; streamed chunks are merged into one row.
    Thought(String),
    /// A tool call; later updates change its title, status and content in place. `detail` is what
    /// the call ran on when `title` is the agent's own description of it.
    Tool { id: String, title: String, kind: String, status: String, content: Vec<ToolContent>, detail: Option<String> },
    Plan(Vec<PlanItem>),
    /// The agent switched its own mode and has no `mode` option to show it in.
    Mode(String),
    /// A turn that stopped for a reason other than finishing.
    Notice(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: usize,
    pub kind: Kind,
    /// The turn a `User` row's message was queued as; none for every other kind.
    pub turn: Option<u64>,
    /// A tool call or a thought still in progress: no final status yet and its turn has not ended.
    pub open: bool,
}

/// Statuses after which a tool call does no more work.
fn is_final(status: &str) -> bool {
    matches!(status, "completed" | "failed")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    pub rows: Vec<Row>,
    /// The agent's state as its wire phase (`starting`, `ready`, `busy`, `failed`, `switching`,
    /// `gave_up`).
    pub state: String,
    /// How full the context window is, as the agent last reported it; none until it has.
    pub context: Option<ContextUse>,
    /// What the status line shows besides the context and controls; none until the daemon said.
    pub footer: Option<AgentFooter>,
    /// The controls the agent offers, in draw order (`footer::sorted_config`).
    pub config: Vec<ConfigOption>,
    /// The commands the agent's session says it accepts (`agent_commands`); none until it has.
    pub commands: Option<Vec<String>>,
    next_id: usize,
    tool_rows: HashMap<String, usize>,
    plan_row: Option<usize>,
    /// The turns of the queued rows at the end of `rows`, in the same order.
    queued_turns: Vec<u64>,
    /// The turn that has started and produced nothing yet, so its message can still be taken back.
    unanswered_turn: Option<u64>,
    /// The first row whose content or place changed since the view last took it, so the view
    /// lays out again only the rows from there on.
    changed: Option<usize>,
    /// The rows were replaced whole (an attach) since the view last took it.
    replaced: bool,
}

impl Transcript {
    /// Take one server message. Returns whether it was about `agent`.
    pub fn on_message(&mut self, message: &ServerMsg, agent: &str) -> bool {
        match message {
            ServerMsg::AgentAttached { agent: a, state, events, config, context, .. } if a == agent => {
                self.context = *context;
                self.config = crate::footer::sorted_config(config);
                self.rows.clear();
                self.touch(0);
                self.replaced = true;
                self.tool_rows.clear();
                self.plan_row = None;
                self.queued_turns.clear();
                self.unanswered_turn = None;
                self.state = state_name(state).to_owned();
                for event in events {
                    self.event(&event.event);
                }
                true
            }
            ServerMsg::Agent { agent: a, event } if a == agent => {
                self.event(&event.event);
                true
            }
            ServerMsg::AgentStateChanged { agent: a, state } if a == agent => {
                self.state = state_name(state).to_owned();
                true
            }
            ServerMsg::AgentContext { agent: a, context } if a == agent => {
                self.context = *context;
                true
            }
            ServerMsg::AgentFooter { agent: a, footer } if a == agent => {
                self.footer = Some(footer.clone());
                true
            }
            ServerMsg::AgentConfig { agent: a, options } if a == agent => {
                self.config = crate::footer::sorted_config(options);
                true
            }
            ServerMsg::AgentCommands { agent: a, commands } if a == agent => {
                self.commands = Some(commands.clone());
                true
            }
            _ => false,
        }
    }

    pub fn busy(&self) -> bool {
        self.state == "busy"
    }

    /// The turns of the messages waiting behind the running turn, in the order they will run.
    pub fn queued_turns(&self) -> &[u64] {
        &self.queued_turns
    }

    /// The turn that started and has said nothing yet, so its message can still be taken back.
    pub fn unanswered_turn(&self) -> Option<u64> {
        self.unanswered_turn
    }

    /// The text of the person's message sent as `turn`.
    pub fn user_text(&self, turn: u64) -> Option<&str> {
        self.rows.iter().rev().find(|r| r.turn == Some(turn)).and_then(|r| match &r.kind {
            Kind::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
    }

    /// Rows above the queued messages.
    fn settled(&self) -> usize {
        self.rows.len() - self.queued_turns.len()
    }

    fn last_settled(&self) -> Option<usize> {
        self.settled().checked_sub(1)
    }

    fn append(&mut self, kind: Kind) -> usize {
        let at = self.settled();
        self.rows.insert(at, Row { id: self.next_id, kind, turn: None, open: false });
        self.next_id += 1;
        self.touch(at);
        at
    }

    /// Note that row `at` and every row after it may have changed.
    fn touch(&mut self, at: usize) {
        self.changed = Some(self.changed.map_or(at, |c| c.min(at)));
    }

    /// The first row that changed since the last call, if any; the next call starts afresh.
    pub fn take_changed(&mut self) -> Option<usize> {
        self.changed.take()
    }

    /// Whether the rows were replaced whole since the last call; the next call starts afresh.
    pub fn take_replaced(&mut self) -> bool {
        std::mem::take(&mut self.replaced)
    }

    fn close_all(&mut self) {
        for row in &mut self.rows {
            row.open = false;
        }
    }

    pub fn event(&mut self, event: &AgentEvent) {
        // A thought ends when its turn moves on to something else: a reply, a tool call, a plan,
        // or the turn's end. An update to an earlier call, or a later message being queued, is not
        // the turn moving on.
        if matches!(event, AgentEvent::Text { .. } | AgentEvent::ToolCall { .. } | AgentEvent::Plan { .. } | AgentEvent::Mode { .. } | AgentEvent::TurnEnd { .. } | AgentEvent::Error { .. })
            && let Some(last) = self.last_settled()
            && matches!(self.rows[last].kind, Kind::Thought(_))
        {
            self.rows[last].open = false;
        }
        if !matches!(event, AgentEvent::User { .. } | AgentEvent::TurnStart { .. } | AgentEvent::Retract { .. } | AgentEvent::TextRevised { .. } | AgentEvent::ThoughtRevised { .. }) {
            self.unanswered_turn = None;
        }
        match event {
            AgentEvent::User { turn, text } => {
                self.touch(self.rows.len());
                self.rows.push(Row { id: self.next_id, kind: Kind::User { text: text.clone(), queued: true }, turn: Some(*turn), open: false });
                self.next_id += 1;
                self.queued_turns.push(*turn);
            }
            AgentEvent::TurnStart { turn } => {
                let Some(position) = self.queued_turns.iter().position(|t| t == turn) else { return };
                let settled = self.settled();
                let mut row = self.rows.remove(settled + position);
                self.queued_turns.remove(position);
                if let Kind::User { queued, .. } = &mut row.kind {
                    *queued = false;
                }
                self.rows.insert(settled, row);
                self.touch(settled);
                self.unanswered_turn = Some(*turn);
            }
            AgentEvent::Retract { turn } => {
                if let Some(position) = self.queued_turns.iter().position(|t| t == turn) {
                    let settled = self.settled();
                    self.rows.remove(settled + position);
                    self.touch(settled + position);
                    self.queued_turns.remove(position);
                } else if self.unanswered_turn == Some(*turn) {
                    let settled = self.settled();
                    if let Some(index) = self.rows[..settled].iter().rposition(|r| r.turn == Some(*turn)) {
                        self.rows.remove(index);
                        self.touch(index);
                    }
                }
                if self.unanswered_turn == Some(*turn) {
                    self.unanswered_turn = None;
                }
            }
            AgentEvent::Text { text } => match self.last_settled().map(|i| (i, &mut self.rows[i].kind)) {
                Some((i, Kind::Reply(t))) => {
                    t.push_str(text);
                    self.touch(i);
                }
                _ => {
                    self.append(Kind::Reply(text.clone()));
                }
            },
            AgentEvent::Thought { text } => match self.last_settled().map(|i| (i, &mut self.rows[i])) {
                Some((i, Row { kind: Kind::Thought(t), open: true, .. })) => {
                    t.push_str(text);
                    self.touch(i);
                }
                _ => {
                    let at = self.append(Kind::Thought(text.clone()));
                    self.rows[at].open = true;
                }
            },
            AgentEvent::TextRevised { streamed, text } => {
                if let Some((i, Kind::Reply(t))) = self.last_settled().map(|i| (i, &mut self.rows[i].kind))
                    && let Some(kept) = replacing_end(t, streamed, text)
                {
                    *t = kept;
                    self.touch(i);
                }
            }
            AgentEvent::ThoughtRevised { streamed, text } => {
                if let Some((i, Kind::Thought(t))) = self.last_settled().map(|i| (i, &mut self.rows[i].kind))
                    && let Some(kept) = replacing_end(t, streamed, text)
                {
                    *t = kept;
                    self.touch(i);
                }
            }
            AgentEvent::ToolCall { id, title, tool_kind, status, content, detail } => {
                // A second call with the same id takes over the id; the earlier row gets no more updates.
                if let Some(&earlier) = self.tool_rows.get(id) {
                    self.rows[earlier].open = false;
                }
                let at = self.append(Kind::Tool { id: id.clone(), title: title.clone(), kind: tool_kind.clone(), status: status.clone(), content: content.clone(), detail: detail.clone() });
                self.rows[at].open = !is_final(status);
                self.tool_rows.insert(id.clone(), at);
            }
            AgentEvent::ToolUpdate { id, title: new_title, status: new_status, content: new_content } => {
                let Some(&at) = self.tool_rows.get(id) else { return };
                let Row { kind: Kind::Tool { title, status, content, .. }, open, .. } = &mut self.rows[at] else { return };
                if let Some(t) = new_title {
                    *title = t.clone();
                }
                if let Some(s) = new_status {
                    *status = s.clone();
                    if is_final(s) {
                        *open = false;
                    }
                }
                if let Some(c) = new_content {
                    *content = c.clone();
                }
                self.touch(at);
            }
            AgentEvent::Plan { entries } => match self.plan_row {
                Some(at) => {
                    self.rows[at].kind = Kind::Plan(entries.clone());
                    self.touch(at);
                }
                None => self.plan_row = Some(self.append(Kind::Plan(entries.clone()))),
            },
            AgentEvent::Mode { mode } => {
                self.append(Kind::Mode(mode.clone()));
            }
            AgentEvent::TurnEnd { stop_reason, .. } => {
                self.close_all();
                if let Some(words) = stop_reason_words(stop_reason) {
                    self.append(Kind::Notice(format!("Turn ended: {words}")));
                }
            }
            AgentEvent::Error { message, .. } => {
                // A failed turn, or an agent that stopped: nothing it started is still running.
                self.close_all();
                self.append(Kind::Error(message.clone()));
            }
        }
    }
}

/// `text` with its trailing `end` swapped for `replacement`; none when `text` does not end with `end`.
fn replacing_end(text: &str, end: &str, replacement: &str) -> Option<String> {
    let kept = text.strip_suffix(end)?;
    Some(format!("{kept}{replacement}"))
}

/// The words for an agent's stop reason, or none for a normal end, which needs no words
/// (WheelKit's `StopReason.words`).
pub fn stop_reason_words(reason: &str) -> Option<String> {
    Some(match reason {
        "end_turn" => return None,
        "max_tokens" => "Stopped at its length limit".to_owned(),
        "max_turn_requests" => "Stopped at its request limit".to_owned(),
        "refusal" => "Declined to continue".to_owned(),
        "cancelled" => "Cancelled".to_owned(),
        other => other
            .split('_')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    })
}

/// The state's wire phase name (the `phase` tag of `AgentState`).
fn state_name(state: &AgentState) -> &'static str {
    match state {
        AgentState::Starting => "starting",
        AgentState::Ready => "ready",
        AgentState::Busy { .. } => "busy",
        AgentState::Failed { .. } => "failed",
        AgentState::Switching => "switching",
        AgentState::GaveUp { .. } => "gave_up",
    }
}

/// What a tool row shows (WheelKit's `ToolRowInfo`): a glyph for the kind of action, a sentence,
/// the lines its diffs added and removed, and for a failed call the reason.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRow {
    pub glyph: &'static str,
    pub label: String,
    pub added: Option<usize>,
    pub removed: Option<usize>,
    pub reason: Option<String>,
}

impl ToolRow {
    pub fn new(title: &str, kind: &str, status: &str, content: &[ToolContent], detail: Option<&str>) -> Self {
        let diffs: Vec<(&str, Option<&str>, &str)> = content
            .iter()
            .filter_map(|c| match c {
                ToolContent::Diff { path, old_text, new_text } => Some((path.as_str(), old_text.as_deref(), new_text.as_str())),
                _ => None,
            })
            .collect();
        let creates = diffs.first().is_some_and(|d| d.1.is_none());
        let (added, removed) = if diffs.is_empty() || status == "failed" {
            (None, None)
        } else {
            let (mut a, mut r) = (0, 0);
            for (_, old, new) in &diffs {
                let (x, y) = line_counts(*old, new);
                a += x;
                r += y;
            }
            (Some(a), Some(r))
        };
        let reason = (status == "failed").then(|| failure_reason(content)).flatten();
        Self { glyph: glyph(kind, creates), label: label(title, kind, creates, detail.is_some()), added, removed, reason }
    }

    /// The paths of the row's diffs, in order, each once.
    pub fn diff_paths(content: &[ToolContent]) -> Vec<String> {
        let mut paths: Vec<String> = vec![];
        for c in content {
            if let ToolContent::Diff { path, .. } = c
                && !paths.contains(path)
            {
                paths.push(path.clone());
            }
        }
        paths
    }
}

/// A short text mark per kind of action; GPUI has no SF Symbols.
fn glyph(kind: &str, creates: bool) -> &'static str {
    match kind {
        "read" => "▤",
        "search" => "⌕",
        "edit" if creates => "✚",
        "edit" => "✎",
        "delete" => "✕",
        "move" => "⇒",
        "execute" => "›_",
        "skill" => "✦",
        "subagent" => "⚇",
        "fetch" => "⊕",
        "mcp" => "⧉",
        "think" => "◌",
        _ => "⚒",
    }
}

fn label(title: &str, kind: &str, creates: bool, described: bool) -> String {
    let rest = |prefix: &str| title.strip_prefix(prefix).map(str::to_owned);
    let name = |path: &str| path.rsplit('/').find(|p| !p.is_empty()).unwrap_or(path).to_owned();
    match kind {
        "execute" => if described { title.to_owned() } else { format!("Ran {title}") },
        "read" => rest("Read ").map(|p| format!("Read {}", name(&p))).unwrap_or_else(|| title.to_owned()),
        "edit" => {
            if let Some(path) = rest("Edit ") {
                return format!("Edited {}", name(&path));
            }
            if let Some(path) = rest("Write ") {
                return format!("{} {}", if creates { "Created" } else { "Edited" }, name(&path));
            }
            title.to_owned()
        }
        "delete" => rest("Delete ").map(|p| format!("Deleted {}", name(&p))).unwrap_or_else(|| title.to_owned()),
        "search" => {
            if let Some(pattern) = rest("grep ") {
                // `grep <pattern> in <path>`: the path is named by its last component.
                return match pattern.rfind(" in ") {
                    Some(at) => format!("Searched for {} in {}", &pattern[..at], name(&pattern[at + 4..])),
                    None => format!("Searched for {pattern}"),
                };
            }
            if let Some(pattern) = rest("Find ") {
                return format!("Searched for files matching {pattern}");
            }
            title.to_owned()
        }
        "fetch" => {
            if let Some(url) = rest("Fetch ") {
                return format!("Fetched {url}");
            }
            if let Some(query) = rest("Search ") {
                return format!("Searched the web for {query}");
            }
            title.to_owned()
        }
        "skill" => rest("Skill ").map(|s| format!("Used the {s} skill")).unwrap_or_else(|| title.to_owned()),
        "mcp" => {
            // `mcp__server__tool` reads `server: tool`.
            let parts: Vec<&str> = title.split("__").collect();
            if parts.len() >= 3 && parts[0] == "mcp" { format!("{}: {}", parts[1], parts[2..].join("__")) } else { title.to_owned() }
        }
        _ => title.to_owned(),
    }
}

/// The first non-empty line of a failed call's output, at most 80 characters.
fn failure_reason(content: &[ToolContent]) -> Option<String> {
    for c in content {
        if let ToolContent::Text { text } = c
            && let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty())
        {
            let chars: Vec<char> = line.chars().collect();
            return Some(if chars.len() > 80 { chars[..79].iter().collect::<String>() + "…" } else { line.to_owned() });
        }
    }
    None
}

/// The largest LCS table built; past it every differing line counts as changed (WheelKit's
/// `LineDiff.tableLimit`).
const TABLE_LIMIT: usize = 4_000_000;

/// The lines added and removed going from `old` to `new`: the common prefix and suffix are
/// dropped, then the longest common subsequence of what differs (what WheelKit's `LineDiff`
/// counts).
pub fn line_counts(old: Option<&str>, new: &str) -> (usize, usize) {
    let a: Vec<&str> = old.map(|o| o.lines().collect()).unwrap_or_default();
    let b: Vec<&str> = new.lines().collect();
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let (mut end_a, mut end_b) = (a.len(), b.len());
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let (x, y) = (&a[start..end_a], &b[start..end_b]);
    if x.len() * y.len() > TABLE_LIMIT {
        return (y.len(), x.len());
    }
    let mut table = vec![vec![0usize; y.len() + 1]; x.len() + 1];
    for i in (0..x.len()).rev() {
        for j in (0..y.len()).rev() {
            table[i][j] = if x[i] == y[j] { table[i + 1][j + 1] + 1 } else { table[i + 1][j].max(table[i][j + 1]) };
        }
    }
    let common = table[0][0];
    (y.len() - common, x.len() - common)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(wire: serde_json::Value) -> ServerMsg {
        serde_json::from_value(wire).unwrap()
    }
    fn ev(t: &mut Transcript, event: serde_json::Value) {
        t.on_message(&msg(json!({"ev":"agent","agent":"a1","event":event})), "a1");
    }
    fn kinds(t: &Transcript) -> Vec<&Kind> {
        t.rows.iter().map(|r| &r.kind).collect()
    }
    fn user(text: &str, queued: bool) -> Kind {
        Kind::User { text: text.into(), queued }
    }

    #[test]
    fn attach_replaces_and_chunks_merge() {
        let mut t = Transcript::default();
        ev(&mut t, json!({"kind":"text","text":"stale"}));
        assert!(t.on_message(&msg(json!({"ev":"agent_attached","agent":"a1","state":{"phase":"ready"},"now":0,"events":[
            {"kind":"user","turn":1,"text":"hi"},{"kind":"turn_start","turn":1},{"kind":"text","text":"hel"},{"kind":"text","text":"lo"},
            {"kind":"tool_call","id":"c1","title":"ls","tool_kind":"execute","status":"running"}]})), "a1"));
        ev(&mut t, json!({"kind":"tool_update","id":"c1","status":"completed"}));
        assert_eq!(t.state, "ready");
        assert_eq!(kinds(&t), vec![&user("hi", false), &Kind::Reply("hello".into()),
            &Kind::Tool { id: "c1".into(), title: "ls".into(), kind: "execute".into(), status: "completed".into(), content: vec![], detail: None }]);
        assert!(!t.rows[2].open);
    }

    #[test]
    fn a_message_waits_at_the_bottom_until_its_turn_starts() {
        let mut t = Transcript::default();
        ev(&mut t, json!({"kind":"user","turn":1,"text":"first"}));
        ev(&mut t, json!({"kind":"turn_start","turn":1}));
        ev(&mut t, json!({"kind":"user","turn":2,"text":"second"}));
        ev(&mut t, json!({"kind":"user","turn":3,"text":"third"}));
        ev(&mut t, json!({"kind":"text","text":"reply one"}));
        assert_eq!(kinds(&t), vec![&user("first", false), &Kind::Reply("reply one".into()), &user("second", true), &user("third", true)]);
        ev(&mut t, json!({"kind":"turn_end","turn":1,"stop_reason":"end_turn"}));
        ev(&mut t, json!({"kind":"turn_start","turn":2}));
        assert_eq!(kinds(&t), vec![&user("first", false), &Kind::Reply("reply one".into()), &user("second", false), &user("third", true)]);
    }

    #[test]
    fn a_retract_removes_a_queued_or_unanswered_message() {
        let mut t = Transcript::default();
        ev(&mut t, json!({"kind":"user","turn":1,"text":"go"}));
        ev(&mut t, json!({"kind":"user","turn":2,"text":"queued"}));
        ev(&mut t, json!({"kind":"turn_start","turn":1}));
        ev(&mut t, json!({"kind":"retract","turn":2}));
        assert_eq!(kinds(&t), vec![&user("go", false)]);
        ev(&mut t, json!({"kind":"retract","turn":1}));
        assert!(t.rows.is_empty());
    }

    #[test]
    fn tool_content_and_a_failed_update_land_on_the_rows_call() {
        let mut t = Transcript::default();
        ev(&mut t, json!({"kind":"tool_call","id":"e1","title":"Edit src/a.rs","tool_kind":"edit","status":"running",
            "content":[{"type":"diff","path":"src/a.rs","old_text":"one\n","new_text":"two\n"}]}));
        ev(&mut t, json!({"kind":"tool_call","id":"b1","title":"make build","tool_kind":"execute","status":"running"}));
        ev(&mut t, json!({"kind":"tool_update","id":"b1","status":"failed","content":[{"type":"text","text":"\nmake: *** [build] Error 2\nsecond"}]}));
        ev(&mut t, json!({"kind":"tool_update","id":"e1","status":"completed"}));
        let Kind::Tool { title, kind, status, content, detail, .. } = &t.rows[0].kind else { panic!() };
        let edit = ToolRow::new(title, kind, status, content, detail.as_deref());
        assert_eq!((edit.glyph, edit.label.as_str(), edit.added, edit.removed), ("✎", "Edited a.rs", Some(1), Some(1)));
        assert_eq!(ToolRow::diff_paths(content), vec!["src/a.rs"]);
        let Kind::Tool { title, kind, status, content, detail, .. } = &t.rows[1].kind else { panic!() };
        let build = ToolRow::new(title, kind, status, content, detail.as_deref());
        assert_eq!((build.label.as_str(), build.reason.as_deref()), ("Ran make build", Some("make: *** [build] Error 2")));
        assert!(t.rows.iter().all(|r| !r.open));
    }

    #[test]
    fn tool_labels_read_as_sentences() {
        let l = |title: &str, kind: &str| ToolRow::new(title, kind, "completed", &[], None).label;
        assert_eq!(l("Read src/model.go", "read"), "Read model.go");
        assert_eq!(l("Read src/dir/", "read"), "Read dir");
        assert_eq!(line_counts(Some("a\nb\nc\nd\n"), "a\nx\nc\nd\ne\n"), (2, 1));
        assert_eq!(l("grep addSuggestions in src/model_test.go", "search"), "Searched for addSuggestions in model_test.go");
        assert_eq!(l("Find **/*.go", "search"), "Searched for files matching **/*.go");
        assert_eq!(l("Skill implement", "skill"), "Used the implement skill");
        assert_eq!(l("mcp__github__list_issues", "mcp"), "github: list_issues");
        assert_eq!(ToolRow::new("cargo test", "execute", "completed", &[], Some("Run the tests")).label, "cargo test");
        let created = [ToolContent::Diff { path: "src/new.go".into(), old_text: None, new_text: "a\nb\n".into() }];
        let w = ToolRow::new("Write src/new.go", "edit", "completed", &created, None);
        assert_eq!((w.glyph, w.label.as_str(), w.added, w.removed), ("✚", "Created new.go", Some(2), Some(0)));
    }

    #[test]
    fn a_thought_closes_when_the_turn_moves_on_and_a_turn_end_adds_its_notice() {
        let mut t = Transcript::default();
        ev(&mut t, json!({"kind":"thought","text":"Planning "}));
        ev(&mut t, json!({"kind":"thought","text":"the change."}));
        assert!(t.rows[0].open);
        ev(&mut t, json!({"kind":"text","text":"Done."}));
        assert!(!t.rows[0].open);
        ev(&mut t, json!({"kind":"plan","entries":[{"content":"a","status":"pending"}]}));
        ev(&mut t, json!({"kind":"plan","entries":[{"content":"a","status":"completed"}]}));
        ev(&mut t, json!({"kind":"mode","mode":"plan"}));
        ev(&mut t, json!({"kind":"turn_end","turn":1,"stop_reason":"max_tokens"}));
        ev(&mut t, json!({"kind":"error","message":"boom"}));
        assert_eq!(kinds(&t), vec![&Kind::Thought("Planning the change.".into()), &Kind::Reply("Done.".into()),
            &Kind::Plan(vec![PlanItem { content: "a".into(), status: "completed".into() }]), &Kind::Mode("plan".into()),
            &Kind::Notice("Turn ended: Stopped at its length limit".into()), &Kind::Error("boom".into())]);
        assert_eq!(stop_reason_words("some_odd_reason").as_deref(), Some("Some Odd Reason"));
        assert_eq!(stop_reason_words("end_turn"), None);
    }

    #[test]
    fn state_names_follow_the_wire_phase() {
        let mut t = Transcript::default();
        t.on_message(&msg(json!({"ev":"agent_state_changed","agent":"a1","state":{"phase":"gave_up","tries":3,"message":"x"}})), "a1");
        assert_eq!(t.state, "gave_up");
        t.on_message(&msg(json!({"ev":"agent_state_changed","agent":"a1","state":{"phase":"busy","queued":1}})), "a1");
        assert!(t.busy());
    }

    #[test]
    fn another_agents_events_are_ignored() {
        let mut t = Transcript::default();
        assert!(!t.on_message(&msg(json!({"ev":"agent","agent":"a2","event":{"kind":"text","text":"x"}})), "a1"));
        assert!(t.rows.is_empty());
    }

    #[test]
    fn the_first_changed_row_is_taken_once() {
        let mut t = Transcript::default();
        ev(&mut t, json!({"kind":"text","text":"a"}));
        ev(&mut t, json!({"kind":"tool_call","id":"c1","title":"Read x","tool_kind":"read","status":"pending"}));
        ev(&mut t, json!({"kind":"text","text":"b"}));
        assert_eq!(t.take_changed(), Some(0));
        assert_eq!(t.take_changed(), None);
        ev(&mut t, json!({"kind":"text","text":"c"}));
        assert_eq!(t.take_changed(), Some(2));
        ev(&mut t, json!({"kind":"tool_update","id":"c1","status":"completed"}));
        ev(&mut t, json!({"kind":"text","text":"d"}));
        assert_eq!(t.take_changed(), Some(1));
        ev(&mut t, json!({"kind":"user","turn":4,"text":"later"}));
        ev(&mut t, json!({"kind":"retract","turn":4}));
        assert_eq!(t.take_changed(), Some(3));
    }

    #[test]
    fn a_revised_reply_replaces_its_streamed_chunks() {
        let mut t = Transcript::default();
        ev(&mut t, json!({"kind":"text","text":"draft"}));
        ev(&mut t, json!({"kind":"text_revised","streamed":"draft","text":"final"}));
        assert_eq!(kinds(&t), vec![&Kind::Reply("final".into())]);
    }
}
