//! The composer's command list and the queue's actions (`Composer.swift`'s `SkillList`, and
//! `RowView.swift`'s `UserMessage`): the agent's commands listed when the draft starts with its
//! prefix, and Send now, Edit, Remove and Clear queue on the messages waiting behind a turn.
//! The model is `crate::composer`.
use super::*;
use crate::composer::{self, Command, Picker, Tray};
use serde_json::{Value, json};
use gpui::{ExternalPaths, img};
use std::path::PathBuf;

/// The row of chips above the composer's text, while it holds any.
const CHIPS: f32 = 44.;

/// The room the chips take above `tray`'s composer: none while it holds nothing.
pub(super) fn chips_height(tray: Option<&Tray>) -> f32 {
    if tray.is_some_and(|t| !t.is_empty()) { CHIPS } else { 0. }
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
    fn focused_agent(&self) -> Option<(String, String)> {
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
        self.drafts.insert(agent.to_owned(), String::new());
        if let Some(tray) = self.attachments.get_mut(agent) {
            tray.sent();
        }
        self.log(format!("send to {agent}, {} chars, {} attachments", text.chars().count(), paths.len()));
        self.draft_edited(agent);
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
    /// chips, an image with no text becomes a PNG chip, and text goes into the draft.
    pub(super) fn paste_into_composer(&mut self, agent: &str, cx: &mut App) {
        let Some(item) = cx.read_from_clipboard() else { return };
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
        } else if let Some(text) = item.text() {
            // As typed text is: no control characters but the line breaks and tabs a text box keeps.
            let text: String = text.replace("\r\n", "\n").chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect();
            self.drafts.entry(agent.to_owned()).or_default().push_str(&text);
            self.draft_edited(agent);
        } else if let Some(bytes) = image {
            let tray = self.attachments.entry(agent.to_owned()).or_default();
            tray.add_image(&bytes, &composer::attachments_folder(), &composer::stamp_now());
            self.composer_errors.remove(agent);
            self.log(format!("attach a pasted image to {agent}: {} bytes", bytes.len()));
        }
    }

    /// The paperclip: the Mac's file picker, directories and several files allowed; what it
    /// returns attaches to `agent`'s next message.
    fn pick_files(&mut self, agent: &str, cx: &mut Context<Self>) {
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
}

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

/// The paperclip at the start of the composer, `icon` the SF Symbol the bridge drew ("+" until
/// it has).
pub(super) fn paperclip(agent: &str, icon: Option<gpui::AnyElement>, controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> gpui::Stateful<gpui::Div> {
    let a = agent.to_owned();
    div().id(gpui::ElementId::Name(format!("attach-{agent}").into())).tooltip(crate::tooltip::tip("Attach files"))
        .relative().flex_none().w(px(18.)).h(px(18.)).flex().items_center().justify_center().cursor_pointer().text_color(rgb(0x8fa89b))
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
            this.pick_files(&a, cx);
            cx.stop_propagation();
        }))
        .child(icon.unwrap_or_else(|| div().child("+").into_any_element()))
        .child(recorded(agent, "attach", controls))
}

/// The chips above the composer's text (`AttachmentChips`): a thumbnail for an image or the
/// `doc` symbol `doc` draws, the name, its size, Folder or why it failed (red), and × to take it out.
pub(super) fn attachment_chips(agent: &str, tray: &Tray, doc: impl Fn() -> Option<gpui::AnyElement>, controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> gpui::Div {
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
