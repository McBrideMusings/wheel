//! What the composer adds to typing: the agent's command list opened by its prefix
//! (`Composer.swift` with WheelKit's `SkillSearch`), and which message Esc or Up takes back
//! (`AgentChat.takeBack`).
use wheel_proto::SkillInfo;

/// One command the list offers: its name without the prefix and what it does, when known.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub name: String,
    pub description: String,
}

/// The character that starts a command in a harness's composer: `/` for Claude, `$` for Codex.
/// None for a harness with no command syntax Wheel knows.
pub fn prefix(harness: &str) -> Option<char> {
    match harness {
        "claude" => Some('/'),
        "codex" => Some('$'),
        _ => None,
    }
}

/// The query when `draft` is a command being typed: it starts with `prefix` and the rest holds no
/// whitespace yet. None once the name is finished or the draft is something else.
pub fn query(draft: &str, prefix: char) -> Option<&str> {
    let rest = draft.strip_prefix(prefix)?;
    (!rest.chars().any(char::is_whitespace)).then_some(rest)
}

/// The commands the list offers for `harness`. Claude's are the ones its session says it accepts
/// (`agent_commands`), described from the skills in its home; none until the session has said.
/// Every other harness offers the skills in its home (`AgentChat.accepted`).
pub fn offered(harness: &str, skills: &[SkillInfo], commands: Option<&[String]>) -> Vec<Command> {
    if harness != "claude" {
        return skills.iter().map(Command::from).collect();
    }
    let mut names: Vec<&String> = commands.unwrap_or_default().iter().collect();
    names.sort();
    names.dedup();
    names.into_iter().map(|n| skills.iter().find(|s| &s.name == n).map(Command::from).unwrap_or_else(|| Command { name: n.clone(), description: String::new() })).collect()
}

impl From<&SkillInfo> for Command {
    fn from(s: &SkillInfo) -> Self {
        Command { name: s.name.clone(), description: s.description.clone() }
    }
}

/// The commands `query` matches, best first. Every query letter must appear in a name in order:
/// a name that starts with the query, then one that contains it, then a looser in-order match
/// with fewer letters between; a description containing the query matches last. An empty query
/// lists every command by name.
pub fn rank(query: &str, commands: &[Command]) -> Vec<Command> {
    let q = query.to_lowercase();
    let mut scored: Vec<(usize, &Command)> = commands.iter().filter_map(|c| {
        let name = c.name.to_lowercase();
        let score = if q.is_empty() || name.starts_with(&q) {
            0
        } else if name.contains(&q) {
            1
        } else if let Some(gaps) = gaps(&q, &name) {
            2 + gaps
        } else if c.description.to_lowercase().contains(&q) {
            1000
        } else {
            return None;
        };
        Some((score, c))
    }).collect();
    scored.sort_by(|a, b| (a.0, &a.1.name).cmp(&(b.0, &b.1.name)));
    scored.into_iter().map(|(_, c)| c.clone()).collect()
}

/// How many letters of `name` sit between the first and last matched letter beyond the query's
/// own, when every letter of `q` appears in order; none when one does not.
fn gaps(q: &str, name: &str) -> Option<usize> {
    let chars: Vec<char> = name.chars().collect();
    let (mut at, mut first, mut last) = (0, None, 0);
    for ch in q.chars() {
        let hit = at + chars[at..].iter().position(|c| *c == ch)?;
        first.get_or_insert(hit);
        last = hit;
        at = hit + 1;
    }
    Some(first.map_or(0, |f| last + 1 - f - q.chars().count()))
}

/// The list's state for one composer. Any change to the draft starts the highlight and the
/// arrows' choice over (`edited`), so Return sends a built-in such as `/clear` as typed until the
/// arrows have chosen; Tab always accepts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picker {
    highlighted: usize,
    chosen: bool,
    /// A draft the person shut the list on with Esc; the list stays shut until it changes.
    dismissed: Option<String>,
}

impl Picker {
    /// The rows the list shows for `draft`: empty when it is shut or nothing matches.
    pub fn matches(&self, draft: &str, prefix: Option<char>, commands: &[Command]) -> Vec<Command> {
        match prefix.and_then(|p| query(draft, p)) {
            Some(q) if self.dismissed.as_deref() != Some(draft) => rank(q, commands),
            _ => vec![],
        }
    }

    /// The highlighted row among `count`.
    pub fn highlighted(&self, count: usize) -> usize {
        self.highlighted.min(count.saturating_sub(1))
    }

    /// Whether the arrows chose the highlighted row since the draft last changed.
    pub fn chosen(&self) -> bool {
        self.chosen
    }

    /// The draft changed: the first row is highlighted and nothing is chosen.
    pub fn edited(&mut self) {
        self.highlighted = 0;
        self.chosen = false;
    }

    /// Down or Up over `count` rows, wrapping at the ends.
    pub fn step(&mut self, count: usize, down: bool) {
        if count == 0 {
            return;
        }
        let at = self.highlighted(count);
        self.highlighted = if down { (at + 1) % count } else { (at + count - 1) % count };
        self.chosen = true;
    }

    /// Esc on an open list: shut it for this draft.
    pub fn dismiss(&mut self, draft: &str) {
        self.dismissed = Some(draft.to_owned());
    }
}

/// The draft a command picked from the list leaves: the prefix, the name and a space.
pub fn accepted(prefix: char, command: &Command) -> String {
    format!("{prefix}{} ", command.name)
}

/// The message Esc or Up in an empty composer takes back: the running turn's while it has said
/// nothing, else, while no turn runs, the last queued one (its `turn_start` is on its way). None
/// while a turn runs and has answered.
pub fn take_back_target(unanswered: Option<u64>, busy: bool, queued: &[u64]) -> Option<u64> {
    unanswered.or_else(|| if busy { None } else { queued.last().copied() })
}

/// The draft after a taken-back message's text comes back: alone in an empty draft, else after
/// what is there on a line of its own.
pub fn restored(draft: &str, text: &str) -> String {
    if draft.is_empty() { text.to_owned() } else { format!("{draft}\n{text}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(names: &[&str]) -> Vec<Command> {
        names.iter().map(|n| Command { name: (*n).into(), description: String::new() }).collect()
    }

    fn names(list: &[Command]) -> Vec<&str> {
        list.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn the_prefix_opens_the_list_until_a_space_ends_the_name() {
        assert_eq!(query("/", '/'), Some(""));
        assert_eq!(query("/co", '/'), Some("co"));
        assert_eq!(query("/compact now", '/'), None);
        assert_eq!(query("hello", '/'), None);
        assert_eq!(prefix("claude"), Some('/'));
        assert_eq!(prefix("codex"), Some('$'));
        assert_eq!(prefix("pi"), None);
    }

    #[test]
    fn ranking_puts_a_prefix_first_then_containment_then_in_order_letters_then_descriptions() {
        let mut list = commands(&["review", "compact", "clear", "recompile", "cxt"]);
        list.push(Command { name: "zz".into(), description: "compact the context".into() });
        assert_eq!(names(&rank("", &list)), ["clear", "compact", "cxt", "recompile", "review", "zz"]);
        assert_eq!(names(&rank("comp", &list)), ["compact", "recompile", "zz"]);
        assert_eq!(names(&rank("ct", &list)), ["compact", "cxt", "zz"]);
        assert_eq!(names(&rank("CL", &list)), ["clear", "recompile"], "case is ignored, and c…l in order matches");
        assert!(rank("q", &list).is_empty());
    }

    #[test]
    fn claude_offers_the_commands_its_session_accepts_described_from_its_skills() {
        let skills = vec![SkillInfo { name: "review".into(), description: "Review the diff".into() }, SkillInfo { name: "deploy".into(), description: "ship".into() }];
        let said = vec!["review".to_owned(), "clear".to_owned(), "compact".to_owned()];
        let offered_claude = offered("claude", &skills, Some(&said));
        assert_eq!(names(&offered_claude), ["clear", "compact", "review"]);
        assert_eq!(offered_claude[2].description, "Review the diff");
        assert!(offered("claude", &skills, None).is_empty(), "nothing until the session says what it accepts");
        assert_eq!(names(&offered("codex", &skills, None)), ["review", "deploy"]);
    }

    #[test]
    fn arrows_choose_and_any_edit_starts_over() {
        let list = commands(&["clear", "compact", "review"]);
        let mut p = Picker::default();
        assert_eq!(p.matches("/", Some('/'), &list).len(), 3);
        assert!(!p.chosen());
        p.step(3, true);
        assert_eq!((p.highlighted(3), p.chosen()), (1, true));
        p.step(3, false);
        p.step(3, false);
        assert_eq!(p.highlighted(3), 2, "Up wraps to the last row");
        assert_eq!(p.highlighted(1), 0, "a shorter list keeps the highlight on a row");
        p.edited();
        assert_eq!((p.highlighted(3), p.chosen()), (0, false), "a typed letter, even one erased again, starts the highlight over");
        p.dismiss("/c");
        assert!(p.matches("/c", Some('/'), &list).is_empty());
        assert_eq!(p.matches("/co", Some('/'), &list).len(), 1, "a changed draft opens the list again");
        assert!(p.matches("/", None, &list).is_empty(), "a harness with no prefix opens no list");
        assert_eq!(accepted('/', &list[1]), "/compact ");
    }

    #[test]
    fn take_back_picks_the_unanswered_turn_or_the_last_queued_one_while_idle() {
        assert_eq!(take_back_target(Some(4), true, &[5, 6]), Some(4));
        assert_eq!(take_back_target(None, true, &[5, 6]), None, "a running turn that answered keeps its queue");
        assert_eq!(take_back_target(None, false, &[5, 6]), Some(6));
        assert_eq!(take_back_target(None, false, &[]), None);
        assert_eq!(restored("", "fix it"), "fix it");
        assert_eq!(restored("also", "fix it"), "also\nfix it");
    }
}
