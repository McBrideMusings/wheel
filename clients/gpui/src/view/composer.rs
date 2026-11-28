//! The composer's command list and the queue's actions (`Composer.swift`'s `SkillList`, and
//! `RowView.swift`'s `UserMessage`): the agent's commands listed when the draft starts with its
//! prefix, and Send now, Edit, Remove and Clear queue on the messages waiting behind a turn.
//! The model is `crate::composer`.
use super::*;
use crate::composer::{self, Command, Picker, PlusAction, Tray};
use serde_json::{Value, json};
use gpui::{ExternalPaths, Focusable as _, img};
use std::path::PathBuf;

/// The row of chips above the composer's text, while it holds any.
const CHIPS: f32 = 44.;

/// The room the chips take above `tray`'s composer: none while it holds nothing.
pub(super) fn chips_height(tray: Option<&Tray>) -> f32 {
    if tray.is_some_and(|t| !t.is_empty()) { CHIPS } else { 0. }
}

/// The lines the composer grows to before a longer draft scrolls inside it.
const COMPOSER_LINES: usize = 10;
/// The composer box around its lines: `py_1` above and below, and the 1 px border.
const COMPOSER_INSET: f32 = 10.;
/// The + at the start of the button row, and the microphone and the round button at its end, each this wide and tall.
pub(super) const ROW_BUTTON: f32 = 18.;
/// The room across the composer box that is neither draft nor button: the pane's 1 px border and
/// `p_2`, the box's border and `px_2`.
const COMPOSER_CHROME: f32 = 2. + 16. + 2. + 16.;
/// The button row's room that its extras cannot take: the chrome, then the +, the microphone and
/// the round button, each with its gap.
pub(super) const BUTTON_ROW_CHROME: f32 = COMPOSER_CHROME + 3. * (ROW_BUTTON + 8.);
/// The prompt above the composer that names the model before dictation downloads it: five lines
/// of `text_xs` and its padding, so the source and the folder show in a split pane.
const DICTATION_PROMPT: f32 = 84.;
/// The gap between the draft's last line and the button row under it.
pub(super) const BUTTON_ROW_GAP: f32 = 4.;
/// The draft's text size, `text_sm`'s 0.875 rem.
pub(super) const COMPOSER_TEXT_REMS: f32 = 0.875;
/// The widest a refusal under the composer's text gets, and the gap before it.
const ERROR_W: f32 = 260.;
const ERROR_GAP: f32 = 8.;

/// How the composer of an agent pane is laid out for its draft: the text's width (it wraps there),
/// the refusal's width beside it, the visible lines' height, and the box's height, the button row
/// under the lines included.
#[derive(Clone, Copy)]
pub(super) struct ComposerFit {
    pub text_w: f32,
    pub error_w: f32,
    pub lines_h: f32,
    pub height: f32,
}

/// Lay out `draft` in a composer `pane_w` wide: the draft has the box's first row to itself (design
/// §8.1) and wraps at the room a refusal leaves, and the box grows a line at a time from its
/// one-line `COMPOSER` height up to `COMPOSER_LINES`, past which the text scrolls.
pub(super) fn composer_fit(window: &Window, draft: &str, error: Option<&str>, pane_w: f32) -> ComposerFit {
    let error_w = error.map_or(0., |e| (title::text_width(window, e, 12.).ceil()).min(ERROR_W));
    let text_w = (pane_w - COMPOSER_CHROME - if error.is_some() { error_w + ERROR_GAP } else { 0. }).max(20.);
    // At the size the text div sets (`text_sm`), not the window's default.
    let mut style = window.text_style();
    style.font_size = gpui::rems(COMPOSER_TEXT_REMS).into();
    let size = style.font_size.to_pixels(window.rem_size());
    let line_h = f32::from(style.line_height_in_pixels(window.rem_size()));
    let run = gpui::TextRun { len: draft.len(), font: style.font(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    let lines = window.text_system().shape_text(draft.to_owned().into(), size, &[run], Some(px(text_w)), None)
        .map_or(1, |shaped| shaped.iter().map(|l| l.wrap_boundaries().len() + 1).sum::<usize>());
    let lines_h = lines.clamp(1, COMPOSER_LINES) as f32 * line_h;
    ComposerFit { text_w, error_w, lines_h, height: (lines_h + COMPOSER_INSET + BUTTON_ROW_GAP + ROW_BUTTON).max(super::COMPOSER) }
}

/// One row of the command list, and how many rows it shows at most before the rest wait for
/// the arrows (`SkillList`'s 150 pt).
const COMMAND_ROW: f32 = 20.;
const COMMAND_ROWS: usize = 7;

/// The list's height for `count` matching rows, zero while it is shut.
pub(super) fn command_list_height(count: usize) -> f32 {
    if count == 0 { 0. } else { count.min(COMMAND_ROWS) as f32 * COMMAND_ROW + 4. }
}

/// What a queued or unanswered message's line offers, for the row it sits under.
#[derive(Clone, Default, PartialEq)]
pub(super) struct QueueView {
    pub queued: Vec<u64>,
    pub unanswered: Option<u64>,
    pub sending_now: HashSet<u64>,
}
impl QueueView {
    /// Every turn a line is drawn for.
    pub fn turns(&self) -> impl Iterator<Item = u64> + '_ {
        self.queued.iter().copied().chain(self.unanswered).chain(self.sending_now.iter().copied())
    }
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
    pub(super) fn focused_agent(&self) -> Option<(String, String)> {
        let pane = self.focus.current.as_ref()?;
        let leaf = self.spaces.iter().flat_map(|s| s.shown()).find(|l| &l.id == pane && l.kind == "agent")?;
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

    /// `agent`'s draft changed: the highlight and the arrows' choice start over.
    pub(super) fn draft_edited(&mut self, agent: &str) {
        if let Some(picker) = self.pickers.get_mut(agent) {
            picker.edited();
        }
    }

    /// The client changes `agent`'s draft (a picked command, a restored message, a send, a debug
    /// request): `drafts` takes it now and the input takes it, caret at the end, when its
    /// composer is next drawn (`composer_input`).
    pub fn set_draft(&mut self, agent: &str, text: String) {
        self.drafts.insert(agent.to_owned(), text);
        self.stale_inputs.insert(agent.to_owned());
        self.draft_edited(agent);
    }

    /// `agent`'s composer input, made on first use holding its draft, and given any text the
    /// client set since it was drawn last. An edit in it copies into `drafts`.
    pub(super) fn composer_input(&mut self, agent: &str, window: &mut Window, cx: &mut Context<Self>) -> gpui::Entity<InputState> {
        let draft = self.drafts.get(agent).cloned().unwrap_or_default();
        let input = match self.inputs.get(agent) {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).multi_line(true));
                let name = agent.to_owned();
                cx.subscribe_in(&input, window, move |this: &mut Self, input, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        let value = input.read(cx).value().to_string();
                        if this.drafts.get(&name).map(String::as_str).unwrap_or("") != value {
                            this.drafts.insert(name.clone(), value);
                            this.draft_edited(&name);
                            cx.notify();
                        }
                    }
                })
                .detach();
                self.inputs.insert(agent.to_owned(), input.clone());
                self.stale_inputs.insert(agent.to_owned());
                // Load the speech model while the person reads, not after the first press.
                crate::speech::send(crate::speech::Cmd::DictationPrepare);
                input
            }
        };
        if self.stale_inputs.remove(agent) && input.read(cx).value() != draft.as_str() {
            input.update(cx, |state, cx| {
                // Text goes in by `insert`, which leaves the caret after it.
                state.set_value("", window, cx);
                state.insert(draft, window, cx);
            });
        }
        input
    }

    /// The input that holds the keyboard, a composer's or a field's, if one does.
    pub fn focused_input(&self, window: &Window, cx: &App) -> Option<gpui::Entity<InputState>> {
        self.inputs.values().chain(self.fields.values().map(|f| &f.input)).find(|i| i.read(cx).focus_handle(cx).is_focused(window)).cloned()
    }

    /// The input the keyboard belongs in now: an open field's, else the composer's that
    /// `composer_agent` names.
    pub fn wanted_input(&self) -> Option<gpui::Entity<InputState>> {
        self.keyboard_field().or_else(|| self.composer_agent().and_then(|a| self.inputs.get(&a).cloned()))
    }

    /// The agent whose composer takes typed keys: the focused pane's, while nothing else that
    /// reads typed keys is open over it.
    pub fn composer_agent(&self) -> Option<String> {
        let typing_elsewhere = self.name_edit.is_some() || self.rename.is_some() || self.command_palette.is_some() || self.settings_open
            || self.procs.open.is_some() || self.conversations.open.is_some() || self.confirmation.is_some() || self.shortcuts_open;
        if typing_elsewhere {
            return None;
        }
        self.focused_agent().map(|(agent, _)| agent)
    }

    /// Put the keyboard where `wanted_input` says, else on the root.
    pub(super) fn sync_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.wanted_input() {
            Some(input) => {
                let handle = input.read(cx).focus_handle(cx);
                if !handle.is_focused(window) {
                    handle.focus(window, cx);
                }
            }
            None if !self.handle.is_focused(window) => window.focus(&self.handle, cx),
            None => {}
        }
    }

    /// `composer_fit` for the composer of `agent` in `pane`, `pane_w` wide.
    pub(super) fn composer_fit_for(&self, window: &Window, agent: Option<&str>, pane_w: f32) -> ComposerFit {
        let draft = agent.and_then(|a| self.drafts.get(a)).map(String::as_str).unwrap_or("");
        // The refusal, else dictation's note: whichever the row draws beside the text.
        let error = agent.and_then(|a| self.composer_errors.get(a).cloned().or_else(|| self.dictation_note(a)));
        composer_fit(window, draft, error.as_deref(), pane_w)
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
                self.set_draft(&agent, composer::accepted(prefix, &rows[at]));
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
            self.set_draft(agent, composer::accepted(prefix, command));
        }
    }

    /// Stop `agent`'s running turn; the draft stays in the box.
    pub(super) fn stop_turn(&mut self, agent: &str) {
        self.press(format!("stop {agent}"));
        self.send(&ClientMsg::AgentCancel { req: None, agent: agent.to_owned() });
    }

    /// A key in the focused composer that is about the turn and its queue, after a waiting
    /// request had its chance: Esc or Up with nothing typed takes back the unanswered message,
    /// Esc otherwise stops a running turn, as Control-C does, and Control-Return while a turn
    /// runs sends the draft to go first.
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
                self.stop_turn(&agent);
                true
            }
            "c" if k.modifiers.control && !k.modifiers.shift && !e.is_held && busy => {
                self.stop_turn(&agent);
                true
            }
            "enter" if k.modifiers.control && busy => {
                if !empty || self.holds_attachments(&agent) {
                    self.send_draft(&agent, true);
                }
                true
            }
            _ => false,
        }
    }

    /// Send `agent`'s draft and empty it. `now` asks wheeld to run it once it is queued, ahead of
    /// the running turn (`AppModel.send(_:now:)`). With wheeld out of reach the draft stays.
    /// The attached files' paths go after the text in backticks (`composer::message`), so a
    /// message of attachments alone may go; a chip that failed holds it back until removed.
    pub(super) fn send_draft(&mut self, agent: &str, now: bool) {
        let tray = self.attachments.get(agent);
        let paths = tray.map(Tray::paths).unwrap_or_default();
        let text = composer::message(self.drafts.get(agent).map(String::as_str).unwrap_or(""), &paths);
        if text.is_empty() {
            return;
        }
        if !tray.is_none_or(Tray::ready) {
            self.composer_errors.insert(agent.to_owned(), "A file could not be attached; remove it to send.".into());
            return;
        }
        if self.link != Link::Up {
            self.composer_errors.insert(agent.to_owned(), "Not connected to wheeld; your message is still in the box.".into());
            return;
        }
        self.set_draft(agent, String::new());
        if let Some(tray) = self.attachments.get_mut(agent) {
            tray.sent();
        }
        self.log(format!("send to {agent}, {} chars, {} attachments", text.chars().count(), paths.len()));
        // Sending is going back to the conversation's end: the person wants to see what they sent
        // and what comes back, wherever they had scrolled.
        self.follow_end(agent);
        let req = self.agent_req(agent, now, None);
        self.send(&ClientMsg::AgentSend { req: Some(req), agent: agent.to_owned(), text });
    }

    /// A request id for an op on `agent`'s messages, kept until wheeld answers it.
    pub(super) fn agent_req(&mut self, agent: &str, now: bool, editing: Option<u64>) -> u64 {
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
        let restored = composer::restored(self.drafts.get(agent).map(String::as_str).unwrap_or(""), &text);
        self.set_draft(agent, restored);
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
            "attachments": self.attachments.get(agent).map(|t| t.items().iter().map(|i| json!({"id": i.id, "name": i.name, "detail": i.detail, "path": i.path, "image": i.thumbnail.is_some(), "failed": i.failed})).collect::<Vec<_>>()).unwrap_or_default(),
        })
    }

    /// Whether `agent`'s composer holds a file for its next message.
    pub(super) fn holds_attachments(&self, agent: &str) -> bool {
        self.attachments.get(agent).is_some_and(|t| !t.is_empty())
    }

    /// Attach files and folders to `agent`'s next message, where they are.
    pub fn attach_paths(&mut self, agent: &str, paths: &[PathBuf]) {
        let tray = self.attachments.entry(agent.to_owned()).or_default();
        for path in paths {
            tray.add_file(path);
        }
        self.composer_errors.remove(agent);
        self.log(format!("attach {} to {agent}", paths.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy()).collect::<Vec<_>>().join(", ")));
    }

    fn remove_attachment(&mut self, agent: &str, id: u64) {
        self.press(format!("remove attachment {agent} {id}"));
        if let Some(tray) = self.attachments.get_mut(agent) {
            tray.remove(id);
        }
        self.composer_errors.remove(agent);
    }

    /// ⌘V in `agent`'s composer (`AppModel.pasteAttachments`): files on the pasteboard become
    /// chips and an image with no text becomes a PNG chip. True when it attached; text is
    /// the input's to paste.
    pub(super) fn attach_pasted(&mut self, agent: &str, cx: &mut App) -> bool {
        let Some(item) = cx.read_from_clipboard() else { return false };
        let mut paths = vec![];
        let mut image = None;
        for entry in item.entries() {
            match entry {
                gpui::ClipboardEntry::ExternalPaths(p) => paths.extend(p.paths().iter().cloned()),
                gpui::ClipboardEntry::Image(i) if image.is_none() => image = Some(i.bytes.clone()),
                _ => {}
            }
        }
        if !paths.is_empty() {
            self.attach_paths(agent, &paths);
            true
        } else if item.text().is_some() {
            false
        } else if let Some(bytes) = image {
            let tray = self.attachments.entry(agent.to_owned()).or_default();
            tray.add_image(&bytes, &composer::attachments_folder(), &composer::stamp_now());
            self.composer_errors.remove(agent);
            self.log(format!("attach a pasted image to {agent}: {} bytes", bytes.len()));
            true
        } else {
            false
        }
    }

    /// The + menu's Add files…: the Mac's file picker, directories and several files allowed;
    /// what it returns attaches to `agent`'s next message.
    pub(super) fn pick_files(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.press(format!("attach {agent}"));
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: true, multiple: true, prompt: None });
        let agent = agent.to_owned();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                let _ = this.update(cx, |c, cx| {
                    c.attach_paths(&agent, &paths);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// What `agent`'s + menu offers: Clear chat only where its harness starts over on `/clear`.
    pub(super) fn plus_items(&self, agent: &str) -> Vec<PlusAction> {
        let harness = self.spaces.iter().flat_map(|s| s.all()).find(|l| l.agent.as_deref() == Some(agent)).and_then(|l| l.harness.clone());
        composer::plus_items(composer::can_clear(harness.as_deref()))
    }

    /// Run one + menu item for `agent`, closing the menu.
    pub(super) fn run_plus(&mut self, agent: &str, action: PlusAction, cx: &mut Context<Self>) {
        self.menu = None;
        self.press(format!("plus {agent} {}", action.id()));
        match action {
            PlusAction::AddFiles => self.pick_files(agent, cx),
            // Asked first, as the phone's AttachMenu asks: the conversation cannot be taken back here.
            PlusAction::Clear => self.confirm("Clear this chat?", "The agent starts a new conversation.".into(), "Clear Chat".into(), Pending::ClearChat(agent.to_owned())),
        }
    }

    /// Start `agent`'s conversation over (the phone's `clearChat`): `/clear` goes out as a
    /// message, leaving the draft and its attachments in the box.
    pub(super) fn clear_chat(&mut self, agent: &str) {
        if self.link != Link::Up {
            self.composer_errors.insert(agent.to_owned(), "Not connected to wheeld; the chat was not cleared.".into());
            return;
        }
        self.log(format!("clear chat {agent}"));
        self.follow_end(agent);
        let req = self.agent_req(agent, false, None);
        self.send(&ClientMsg::AgentSend { req: Some(req), agent: agent.to_owned(), text: "/clear".into() });
    }
}

/// The id `Client::menu` holds while an agent's + menu is open, beside the config options' ids.
pub(super) const PLUS_MENU: &str = "+";

/// The composer and its chips as one drop target: files dropped there attach to `agent`'s next
/// message, and the box is outlined (`over`) while they are held over it. GPUI's own `on_drop`
/// and `drag_over` go by hover, which GPUI turns off after a key press, and a drag from Finder
/// sends no mouse move: the drop is found by where the pointer is instead. The drag's moves name
/// the files (`Client::file_drag`), and the release over the box's hitbox attaches them.
pub(super) fn drop_target(agent: Option<&str>, over: bool, el: gpui::Div, cx: &mut Context<Client>) -> gpui::Div {
    let Some(agent) = agent.map(str::to_owned) else { return el };
    let (moved, released, weak) = (agent.clone(), agent, cx.entity().downgrade());
    el.relative()
        .on_drag_move::<ExternalPaths>(cx.listener(move |this, e: &gpui::DragMoveEvent<ExternalPaths>, _, cx| {
            let held = e.bounds.contains(&e.event.position).then(|| (moved.clone(), e.drag(cx).paths().to_vec()));
            let was_here = this.file_drag.as_ref().is_some_and(|(a, _)| *a == moved);
            if (held.is_some() || was_here) && this.file_drag != held {
                this.file_drag = held;
                cx.notify();
            }
        }))
        .child(canvas(|bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal), move |_, hitbox, window, _| {
            let (agent, weak) = (released.clone(), weak.clone());
            window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                if phase != gpui::DispatchPhase::Bubble || !cx.has_active_drag() || !hitbox.should_handle_scroll(window) {
                    return;
                }
                let Some(client) = weak.upgrade() else { return };
                let dropped = client.update(cx, |c, cx| {
                    let Some((_, paths)) = c.file_drag.take_if(|(a, _)| *a == agent) else { return false };
                    c.attach_paths(&agent, &paths);
                    cx.notify();
                    true
                });
                if dropped {
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                }
            });
        }).absolute().top_0().left_0().size_full())
        // Over everything else and never part of the layout, so the composer stays where it is.
        .when(over, |d| d.child(div().absolute().top_0().left_0().size_full().border_1().border_color(rgb(0x7fb89a)).rounded_sm()))
}

/// The + at the start of the composer row (the phone's `AttachMenu`, design §8.6 K1), `icon`
/// the SF Symbol the bridge drew ("+" until it has). A press opens its menu above it, `items` in
/// order with the row `at` lit for the arrow keys.
#[allow(clippy::too_many_arguments)]
pub(super) fn plus_button(agent: &str, open: bool, at: usize, items: &[PlusAction], icon: Option<gpui::AnyElement>, controls: &Rc<crate::view::scoped::Scoped<Control>>, cx: &mut Context<Client>) -> gpui::Stateful<gpui::Div> {
    let a = agent.to_owned();
    let mut button = div().id(gpui::ElementId::Name(format!("plus-{agent}").into())).tooltip(crate::tooltip::tip("Add files and more"))
        .relative().flex_none().w(px(ROW_BUTTON)).h(px(ROW_BUTTON)).flex().items_center().justify_center().cursor_pointer().text_color(rgb(0x8fa89b))
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            // The pane under the button closes any menu on a press; this press is the button's.
            cx.stop_propagation();
            this.press(format!("plus {a}"));
            this.toggle_menu(&a, PLUS_MENU);
            cx.notify();
        }))
        .child(icon.unwrap_or_else(|| div().child("+").into_any_element()))
        .child(recorded(agent, "plus", controls));
    if open {
        // Recorded as `menu:+`, so the bridge's overlay lets it over a card's page view, as a config menu is.
        let mut list = div().relative().flex().flex_col().min_w(px(140.)).rounded_sm().bg(rgb(0x22302a)).border_1().border_color(rgb(0x3d5247)).py_1().text_xs().text_color(rgb(0xd9e8e1))
            .child(recorded(agent, &format!("menu:{PLUS_MENU}"), controls));
        for (row, action) in items.iter().copied().enumerate() {
            let a = agent.to_owned();
            let mut entry = div().relative().px_2().py_1().cursor_pointer().flex().gap_2().whitespace_nowrap();
            if row == at {
                entry = entry.bg(rgb(0x31483e));
            }
            list = list.child(entry
                .child(action.title())
                .child(recorded(agent, &format!("plus:{}", action.id()), controls))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.run_plus(&a, action, cx);
                    cx.notify();
                })));
        }
        // Out of the button's flex layout, so the menu's bottom-left corner sits at the button's top-left.
        button = button.child(div().absolute().top_0().left_0().child(deferred(anchored().anchor(Anchor::BottomLeft).snap_to_window().child(list))));
    }
    button
}

/// The microphone before the round button: it starts dictating into `agent`'s draft and, while
/// that records, stops and transcribes. Dimmed while the model downloads or the recording is
/// transcribed. `icon` is the SF Symbol drawn for the state.
pub(super) fn mic_button(agent: &str, d: &crate::dictation::Dictation, icon: Option<gpui::AnyElement>, controls: &Rc<crate::view::scoped::Scoped<Control>>, cx: &mut Context<Client>) -> gpui::Stateful<gpui::Div> {
    let recording = d.recording_for(agent);
    let busy = d.busy_for(agent);
    let a = agent.to_owned();
    let tip = if recording { "Stop dictating and put the text in the message" } else { "Dictate: speak, then press again" };
    div().id(gpui::ElementId::Name(format!("dictate-{agent}").into())).tooltip(crate::tooltip::tip(tip))
        .relative().flex_none().w(px(ROW_BUTTON)).h(px(ROW_BUTTON)).flex().items_center().justify_center().rounded_sm()
        .when(!busy, |d| d.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            this.toggle_dictation(&a);
            cx.stop_propagation();
            cx.notify();
        })))
        .when(busy, |d| d.opacity(0.4))
        .child(icon.unwrap_or_else(|| div().text_xs().child(if recording { "■" } else { "mic" }).into_any_element()))
        .child(recorded(agent, "dictate", controls))
}

/// The symbol and colour the microphone draws for `agent`: a red stop while it records.
pub(super) fn mic_symbol(d: &crate::dictation::Dictation, agent: &str) -> (&'static str, u32) {
    if d.recording_for(agent) { ("stop.circle.fill", 0xe07070) } else { ("mic", 0x8fa89b) }
}

/// The prompt's height above `agent`'s composer: there while it asks or downloads.
pub(super) fn dictation_prompt_height(d: &crate::dictation::Dictation, agent: Option<&str>) -> f32 {
    // With the `mt_1` above it, as `command_list_height` counts its own.
    if agent.is_some_and(|a| d.prompt_for(a)) { DICTATION_PROMPT + 4. } else { 0. }
}

/// What the model download fetches, how big, where from and where it is kept, with Download and
/// Cancel (ADR-0013): shown above the composer the first time the microphone is pressed with no
/// model on the Mac. While it downloads, its progress and Cancel.
pub(super) fn dictation_prompt(agent: &str, d: &crate::dictation::Dictation, model: &crate::speech_models::Model, controls: &Rc<crate::view::scoped::Scoped<Control>>, cx: &mut Context<Client>) -> gpui::Div {
    let button = |label: &'static str, id: &'static str, run: fn(&mut Client), cx: &mut Context<Client>| {
        div().id(gpui::ElementId::Name(format!("dictation-{id}-{agent}").into())).relative().px_2().rounded_sm().cursor_pointer().bg(rgb(0x31483e)).text_color(rgb(0xd9e8e1))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                run(this);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(label)
            .child(recorded(agent, &format!("dictation {id}"), controls))
    };
    let model = *model;
    let text = match d.phase {
        crate::dictation::Phase::Downloading(p) => format!("Downloading {} ({}), {}%", model.name, model.size(), (p * 100.).floor() as u32),
        _ => crate::dictation::prompt(&model, &crate::speech_models::root()),
    };
    let asking = d.phase == crate::dictation::Phase::Asking;
    div().flex_none().h(px(DICTATION_PROMPT)).mt_1().px_2().py_1().rounded_sm().bg(rgb(0x1f2b26)).border_1().border_color(rgb(0x2c3d34))
        .flex().gap_2().text_xs().text_color(rgb(0xd9e8e1)).overflow_hidden()
        // The text wraps beside the buttons, so however long the folder is it never pushes them out of the box.
        .child(div().flex_1().min_w_0().h_full().overflow_hidden().child(text))
        .child(div().flex_none().flex().flex_col().gap_1()
            .when(asking, |b| b.child(button("Download", "download", Client::download_dictation_model, cx)))
            .child(button("Cancel", "cancel", Client::cancel_dictation, cx)))
}

/// The round button at the end of the composer row (wheel-eg4e.19.31): Stop while the agent
/// works (as Esc and Control-C do), Send with a draft or files, voice mode with nothing typed, a
/// dimmed Send when wheeld lacks voice mode's ops. Same size and place in every state. `icon` is
/// the SF Symbol drawn for the state. Recorded as `stop`, `send` or `voice`.
pub(super) fn right_button(agent: &str, pane: &str, state: crate::composer::RightButton, icon: Option<gpui::AnyElement>, controls: &Rc<crate::view::scoped::Scoped<Control>>, cx: &mut Context<Client>) -> gpui::Stateful<gpui::Div> {
    use crate::composer::RightButton::*;
    let (name, tip, fallback, bg) = match state {
        Stop => ("stop", "Stop (Esc or ⌃C)", "■", 0x31483e),
        Send => ("send", "Send (Return)", "↑", 0x7fb89a),
        Voice => ("voice", "Voice mode: talk to the agent and hear it", "◉", 0xd9e8e1),
        Idle => ("send", "Type a message to send", "↑", 0x31483e),
    };
    let (a, p) = (agent.to_owned(), pane.to_owned());
    div().id(gpui::ElementId::Name(format!("right-{agent}").into())).tooltip(crate::tooltip::tip(tip))
        .relative().flex_none().w(px(ROW_BUTTON)).h(px(ROW_BUTTON)).flex().items_center().justify_center().rounded_full().bg(rgb(bg))
        .when(state == Idle, |d| d.opacity(0.5))
        .when(state != Idle, |d| d.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            match state {
                Stop => this.stop_turn(&a),
                Send => this.send_draft(&a, false),
                Voice => this.open_voice(&a, &p),
                Idle => {}
            }
            cx.notify();
        })))
        .child(icon.unwrap_or_else(|| div().text_xs().child(fallback).into_any_element()))
        .child(recorded(agent, name, controls))
}

/// The SF Symbol and its colour for the round button in `state`.
pub(super) fn right_symbol(state: crate::composer::RightButton) -> (&'static str, u32) {
    use crate::composer::RightButton::*;
    match state {
        Stop => ("stop.fill", 0xd9e8e1),
        Send | Idle => ("arrow.up", 0x0e1311),
        Voice => ("waveform", 0x0e1311),
    }
}

/// The chips above the composer's text (`AttachmentChips`): a thumbnail for an image or the
/// `doc` symbol `doc` draws, the name, its size, Folder or why it failed (red), and × to take it out.
pub(super) fn attachment_chips(agent: &str, tray: &Tray, doc: impl Fn() -> Option<gpui::AnyElement>, controls: &Rc<crate::view::scoped::Scoped<Control>>, cx: &mut Context<Client>) -> gpui::Div {
    let mut row = div().flex_none().h(px(CHIPS - 4.)).mt_1().flex().gap(px(6.)).overflow_hidden().child(recorded(agent, "chips", controls));
    for item in tray.items() {
        let (a, id) = (agent.to_owned(), item.id);
        let thumb = div().flex_none().w(px(32.)).h(px(32.)).rounded(px(6.)).overflow_hidden().bg(rgb(0x24302a)).flex().items_center().justify_center();
        let thumb = if let Some(png) = &item.thumbnail {
            thumb.child(img(std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png.clone()))).w(px(32.)).h(px(32.)).object_fit(gpui::ObjectFit::Cover))
        } else {
            thumb.children(doc())
        };
        let (detail, tint) = match &item.failed {
            Some(why) => (why.clone(), 0xe07070),
            None => (item.detail.clone(), 0x8fa89b),
        };
        row = row.child(div().relative().flex_none().flex().items_center().gap(px(6.)).pl(px(4.)).pr(px(6.)).py(px(4.)).rounded(px(8.))
            .bg(rgb(if item.failed.is_some() { 0x3a2424 } else { 0x24302a }))
            .child(thumb)
            .child(div().flex().flex_col().max_w(px(160.)).overflow_hidden().whitespace_nowrap().text_xs()
                .child(div().text_color(rgb(0xd9e8e1)).text_ellipsis().child(item.name.clone()))
                .child(div().text_color(rgb(tint)).text_ellipsis().child(detail)))
            .child(div().relative().flex_none().cursor_pointer().text_xs().text_color(rgb(0x8fa89b)).child("✕")
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.remove_attachment(&a, id);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(recorded(agent, &format!("attachment:remove:{}", item.name), controls)))
            .child(recorded(agent, &format!("attachment:{}", item.name), controls)));
    }
    row
}

/// The command list over the composer: up to `COMMAND_ROWS` rows around the highlight, each the
/// prefixed name and its description, the highlighted one shaded. A click picks a row.
pub(super) fn command_list(agent: &str, harness: &str, rows: &[Command], at: usize, prefix: char, controls: &Rc<crate::view::scoped::Scoped<Control>>, cx: &mut Context<Client>) -> gpui::Div {
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
/// for any other message. A queued reload has no text to edit, so its line has no Edit.
pub(super) fn queue_line(agent: &str, turn: u64, queued: bool, reload: bool, queue: &QueueView, controls: &Rc<crate::view::scoped::Scoped<Control>>, cx: &mut Context<Client>) -> Option<gpui::Div> {
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
        .when(!reload, |line| line.child(button("Edit", format!("queue:edit:{turn}"), Client::edit_message, cx)))
        .child(button("Remove", format!("queue:remove:{turn}"), Client::remove_queued, cx));
    if queue.queued.len() >= 2 && queue.queued.first() == Some(&turn) {
        line = line.child(button("Clear queue", "queue:clear".into(), |c, a, _| c.clear_queue(a), cx));
    }
    Some(line)
}
