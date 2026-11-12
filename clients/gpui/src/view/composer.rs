//! The composer's command list and the queue's actions (`Composer.swift`'s `SkillList`, and
//! `RowView.swift`'s `UserMessage`): the agent's commands listed when the draft starts with its
//! prefix, and Send now, Edit, Remove and Clear queue on the messages waiting behind a turn.
//! The model is `crate::composer`.
use super::*;
use crate::composer::{self, Command, Picker};
use serde_json::{Value, json};

/// One row of the command list, and how many rows it shows at most before the rest wait for
/// the arrows (`SkillList`'s 150 pt).
const COMMAND_ROW: f32 = 20.;
const COMMAND_ROWS: usize = 7;

/// The list's height for `count` matching rows, zero while it is shut.
pub(super) fn command_list_height(count: usize) -> f32 {
    if count == 0 { 0. } else { count.min(COMMAND_ROWS) as f32 * COMMAND_ROW + 4. }
}

/// What a queued or unanswered message's line offers, for `agent_pane`.
#[derive(Default)]
pub(super) struct QueueView {
    pub queued: Vec<u64>,
    pub unanswered: Option<u64>,
    pub sending_now: HashSet<u64>,
}

/// A request about an agent's messages waiting for wheeld's answer (`AppModel.agentReqs`): a
/// refusal shows under the composer, and a message being edited stays where it was.
pub(super) struct AgentReq {
    agent: String,
    /// A send that should run ahead of the turn once it is queued (Control-Return).
    now: bool,
    /// The turn an Edit or take-back asked wheeld to take back.
    editing: Option<u64>,
}

impl Client {
    /// The focused pane's agent and harness, when it is an agent pane.
    fn focused_agent(&self) -> Option<(String, String)> {
        let pane = self.focus.current.as_ref()?;
        let leaf = self.spaces.iter().flat_map(|s| leaves_of(&s.root)).find(|l| &l.id == pane && l.kind == "agent")?;
        Some((leaf.agent.clone()?, leaf.harness.clone().unwrap_or_default()))
    }

    /// The commands `agent`'s composer offers (`composer::offered`).
    fn commands_for(&self, agent: &str, harness: &str) -> Vec<Command> {
        let skills = self.agents.iter().find(|a| a.name == harness).map(|a| a.skills.as_slice()).unwrap_or_default();
        composer::offered(harness, skills, self.transcripts.get(agent).and_then(|t| t.commands.as_deref()))
    }

    /// The list `agent`'s draft opens: its rows (empty while shut), the highlighted one, and the
    /// harness's prefix.
    pub(super) fn command_matches(&self, agent: &str, harness: &str) -> (Vec<Command>, usize, Option<char>) {
        let draft = self.drafts.get(agent).map(String::as_str).unwrap_or("");
        let prefix = composer::prefix(harness);
        let none = Picker::default();
        let picker = self.pickers.get(agent).unwrap_or(&none);
        let rows = picker.matches(draft, prefix, &self.commands_for(agent, harness));
        let at = picker.highlighted(rows.len());
        (rows, at, prefix)
    }

    /// `agent`'s draft changed by typing: the highlight and the arrows' choice start over.
    pub(super) fn draft_edited(&mut self, agent: &str) {
        if let Some(picker) = self.pickers.get_mut(agent) {
            picker.edited();
        }
    }

    /// A key while the focused composer's list is open: Up and Down move the highlight, Tab takes
    /// it, Return (Control-Return too) takes it once the arrows chose, Esc shuts the list. False
    /// leaves the key to the rest, so a draft such as `/clear` typed out goes as typed.
    pub(super) fn picker_key(&mut self, e: &KeyDownEvent) -> bool {
        let k = &e.keystroke;
        if k.modifiers.platform || k.modifiers.alt || (k.modifiers.control && k.key != "enter") {
            return false;
        }
        let Some((agent, harness)) = self.focused_agent() else { return false };
        let (rows, at, Some(prefix)) = self.command_matches(&agent, &harness) else { return false };
        if rows.is_empty() {
            return false;
        }
        let draft = self.drafts.get(&agent).cloned().unwrap_or_default();
        let picker = self.pickers.entry(agent.clone()).or_default();
        match k.key.as_str() {
            "down" | "up" => picker.step(rows.len(), k.key == "down"),
            "escape" => picker.dismiss(&draft),
            "tab" | "enter" if k.key == "tab" || (!k.modifiers.shift && picker.chosen()) => {
                picker.edited();
                self.drafts.insert(agent, composer::accepted(prefix, &rows[at]));
            }
            _ => return false,
        }
        true
    }

    /// A click on a row of `agent`'s list puts that command in the draft.
    fn pick_command(&mut self, agent: &str, harness: &str, name: &str) {
        let (rows, _, prefix) = self.command_matches(agent, harness);
        if let (Some(command), Some(prefix)) = (rows.iter().find(|c| c.name == name), prefix) {
            self.press(format!("command {name}"));
            self.drafts.insert(agent.to_owned(), composer::accepted(prefix, command));
            self.draft_edited(agent);
        }
    }

    /// A key in the focused composer that is about the turn and its queue, after a waiting
    /// request had its chance: Esc or Up with nothing typed takes back the unanswered message,
    /// Esc otherwise stops a running turn, and Control-Return while a turn runs sends the draft
    /// to go first.
    pub(super) fn queue_key(&mut self, e: &KeyDownEvent) -> bool {
        let k = &e.keystroke;
        if k.modifiers.platform || k.modifiers.alt || self.link != Link::Up {
            return false;
        }
        let Some((agent, _)) = self.focused_agent() else { return false };
        let empty = self.drafts.get(&agent).is_none_or(|d| d.trim().is_empty());
        let busy = self.transcripts.get(&agent).is_some_and(Transcript::busy);
        let plain = !k.modifiers.control && !k.modifiers.shift && !e.is_held;
        match k.key.as_str() {
            "escape" | "up" if plain && empty && self.take_back(&agent) => true,
            "escape" if plain && busy => {
                self.press(format!("stop {agent}"));
                self.send(&ClientMsg::AgentCancel { req: None, agent });
                true
            }
            "enter" if k.modifiers.control && busy => {
                if !empty {
                    self.send_draft(&agent, true);
                }
                true
            }
            _ => false,
        }
    }

    /// Send `agent`'s draft and empty it. `now` asks wheeld to run it once it is queued, ahead of
    /// the running turn (`AppModel.send(_:now:)`). With wheeld out of reach the draft stays.
    pub(super) fn send_draft(&mut self, agent: &str, now: bool) {
        if self.drafts.get(agent).is_none_or(|d| d.trim().is_empty()) {
            return;
        }
        if self.link != Link::Up {
            self.composer_errors.insert(agent.to_owned(), "Not connected to wheeld; your message is still in the box.".into());
            return;
        }
        let text = self.drafts.insert(agent.to_owned(), String::new()).unwrap_or_default();
        self.draft_edited(agent);
        let req = self.agent_req(agent, now, None);
        self.send(&ClientMsg::AgentSend { req: Some(req), agent: agent.to_owned(), text });
    }

    /// A request id for an op on `agent`'s messages, kept until wheeld answers it.
    fn agent_req(&mut self, agent: &str, now: bool, editing: Option<u64>) -> u64 {
        self.composer_errors.remove(agent);
        let req = self.req();
        self.agent_reqs.insert(req, AgentReq { agent: agent.to_owned(), now, editing });
        req
    }

    /// wheeld answered `req`: an `ok`, `agent_queued`, or a refusal (`refused`, shown under the
    /// composer; a message being edited stays in place).
    pub(super) fn on_agent_answer(&mut self, req: u64, refused: Option<&str>) {
        let Some(r) = self.agent_reqs.remove(&req) else { return };
        if let Some(why) = refused {
            if let Some(turn) = r.editing {
                self.editing.remove(&(r.agent.clone(), turn));
            }
            self.composer_errors.insert(r.agent, why.to_owned());
        }
    }

    /// wheeld queued a message as `turn`: one sent with Control-Return goes first now.
    pub(super) fn on_queued(&mut self, req: u64, agent: &str, turn: u64) {
        if self.agent_reqs.get(&req).is_some_and(|r| r.now) {
            self.send_now(agent, turn);
        }
        self.on_agent_answer(req, None);
    }

    /// Esc or Up in an empty composer: take back the message the agent has not answered
    /// (`composer::take_back_target`). False when there is none, so the key does what it did.
    fn take_back(&mut self, agent: &str) -> bool {
        let Some(t) = self.transcripts.get(agent) else { return false };
        let Some(turn) = composer::take_back_target(t.unanswered_turn(), t.busy(), t.queued_turns()) else { return false };
        if self.editing.contains_key(&(agent.to_owned(), turn)) || t.user_text(turn).is_none() {
            return false;
        }
        self.edit_message(agent, turn);
        true
    }

    /// Take a message back to rewrite it: its text goes to the draft on the `retract` event, so a
    /// message the agent answered first is never doubled there.
    fn edit_message(&mut self, agent: &str, turn: u64) {
        let Some(text) = self.transcripts.get(agent).and_then(|t| t.user_text(turn)).map(str::to_owned) else { return };
        self.press(format!("edit {agent} {turn}"));
        self.editing.insert((agent.to_owned(), turn), text);
        let req = self.agent_req(agent, false, Some(turn));
        self.send(&ClientMsg::AgentUnsend { req: Some(req), agent: agent.to_owned(), turn });
    }

    /// A `retract` for `turn` of `agent`: a message being edited comes back into the draft, and
    /// the keyboard goes to the pane showing that agent.
    pub(super) fn on_retract(&mut self, agent: &str, turn: u64) {
        self.sending_now.remove(&(agent.to_owned(), turn));
        let Some(text) = self.editing.remove(&(agent.to_owned(), turn)) else { return };
        let draft = self.drafts.entry(agent.to_owned()).or_default();
        *draft = composer::restored(draft, &text);
        self.draft_edited(agent);
        let pane = self.scene().0.into_iter().find(|p| p.leaf.kind == "agent" && p.leaf.agent.as_deref() == Some(agent)).map(|p| p.leaf.id);
        if let Some(pane) = pane {
            self.focus_pane(&pane);
        }
    }

    fn remove_queued(&mut self, agent: &str, turn: u64) {
        self.press(format!("remove {agent} {turn}"));
        let req = self.agent_req(agent, false, None);
        self.send(&ClientMsg::AgentUnsend { req: Some(req), agent: agent.to_owned(), turn });
    }

    /// Run a queued message now: the running turn is interrupted and it goes first.
    fn send_now(&mut self, agent: &str, turn: u64) {
        self.press(format!("send now {agent} {turn}"));
        self.sending_now.insert((agent.to_owned(), turn));
        let req = self.agent_req(agent, false, None);
        self.send(&ClientMsg::AgentSendNow { req: Some(req), agent: agent.to_owned(), turn });
    }

    fn clear_queue(&mut self, agent: &str) {
        self.press(format!("clear queue {agent}"));
        let req = self.agent_req(agent, false, None);
        self.send(&ClientMsg::AgentClearQueue { req: Some(req), agent: agent.to_owned() });
    }

    /// The queued turns of `agent` asked to run now, in turn order.
    fn sending_now_for(&self, agent: &str) -> Vec<u64> {
        let mut turns: Vec<u64> = self.sending_now.iter().filter(|(a, _)| a == agent).map(|(_, turn)| *turn).collect();
        turns.sort_unstable();
        turns
    }

    /// What `agent_pane` needs to draw the queue's lines for `agent`. A turn marked sending now
    /// stays marked only while it is still queued.
    pub(super) fn queue_view(&mut self, agent: &str) -> QueueView {
        let Some(t) = self.transcripts.get(agent) else { return QueueView::default() };
        let queued = t.queued_turns().to_vec();
        self.sending_now.retain(|(a, turn)| a != agent || queued.contains(turn));
        QueueView { sending_now: self.sending_now_for(agent).into_iter().collect(), unanswered: t.unanswered_turn(), queued }
    }

    /// What `/state` says about `agent`'s composer: the list's rows and highlight, the queue, and
    /// wheeld's last refusal.
    pub fn composer_json(&self, agent: &str, harness: &str) -> Value {
        let (rows, at, prefix) = self.command_matches(agent, harness);
        let t = self.transcripts.get(agent);
        json!({
            "prefix": prefix.map(String::from),
            "offered": self.commands_for(agent, harness).iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            "listed": rows.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            "highlighted": (!rows.is_empty()).then_some(at),
            "queued": t.map(|t| t.queued_turns().to_vec()).unwrap_or_default(),
            "unanswered": t.and_then(Transcript::unanswered_turn),
            "sending_now": self.sending_now_for(agent),
            "error": self.composer_errors.get(agent),
        })
    }
}

/// The command list over the composer: up to `COMMAND_ROWS` rows around the highlight, each the
/// prefixed name and its description, the highlighted one shaded. A click picks a row.
pub(super) fn command_list(agent: &str, harness: &str, rows: &[Command], at: usize, prefix: char, controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> gpui::Div {
    let start = (at + 1).saturating_sub(COMMAND_ROWS);
    let mut list = div().flex_none().flex().flex_col().mt_1().p(px(2.)).rounded_sm().bg(rgb(0x1d2823)).border_1().border_color(rgb(0x2c3d34))
        .child(recorded(agent, "command-list", controls));
    for (i, command) in rows.iter().enumerate().skip(start).take(COMMAND_ROWS) {
        let (a, h, name) = (agent.to_owned(), harness.to_owned(), command.name.clone());
        list = list.child(div().relative().h(px(COMMAND_ROW)).flex().items_center().gap_2().px_2().rounded_sm().cursor_pointer().overflow_hidden().whitespace_nowrap()
            .when(i == at, |d| d.bg(rgb(0x31483e)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.pick_command(&a, &h, &name);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(div().text_color(rgb(0xd9e8e1)).child(format!("{prefix}{}", command.name)))
            .child(div().text_xs().text_color(rgb(0x8fa89b)).child(command.description.clone()))
            .child(recorded(agent, &format!("command:{}", command.name), controls)));
    }
    list
}

/// The line under a queued message (Queued, then Send now, Edit, Remove, and Clear queue on the
/// first of two or more), or under the message of a turn that has said nothing yet (Edit). None
/// for any other message.
pub(super) fn queue_line(agent: &str, turn: u64, queued: bool, queue: &QueueView, controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> Option<gpui::Div> {
    let button = |label: &'static str, id: String, act: fn(&mut Client, &str, u64), cx: &mut Context<Client>| {
        let a = agent.to_owned();
        div().relative().cursor_pointer().text_color(rgb(0x7fb89a))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                act(this, &a, turn);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(label)
            .child(recorded(agent, &id, controls))
    };
    let line = div().flex().gap(px(10.)).pl(px(30.)).text_xs();
    if !queued {
        return (queue.unanswered == Some(turn)).then(|| line.child(button("Edit", format!("unsend:edit:{turn}"), Client::edit_message, cx)));
    }
    let mut line = line.child(div().text_color(rgb(0x8fa89b)).child("Queued"));
    if queue.sending_now.contains(&turn) {
        return Some(line.child(div().text_color(rgb(0x7fb89a)).child("Sending now…")));
    }
    line = line.child(button("Send now", format!("queue:now:{turn}"), Client::send_now, cx))
        .child(button("Edit", format!("queue:edit:{turn}"), Client::edit_message, cx))
        .child(button("Remove", format!("queue:remove:{turn}"), Client::remove_queued, cx));
    if queue.queued.len() >= 2 && queue.queued.first() == Some(&turn) {
        line = line.child(button("Clear queue", "queue:clear".into(), |c, a, _| c.clear_queue(a), cx));
    }
    Some(line)
}
