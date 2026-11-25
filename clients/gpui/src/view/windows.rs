//! This window as `wheel window` and the window menus see it (AppModel+Windows.swift,
//! TitleRow.swift's WindowTitle, SessionMenu.swift): the record it writes to the app's list of
//! windows, its name and the title field that edits it, closing it, Rename Session…, and placing
//! a space in this window for `wheel window space-move`.
use super::*;
use super::settings::{ACCENT, DIM, LINE, TEXT};
use crate::windows::{self, Record};
use serde_json::{Value, json};
use wheel_proto::WindowCommand;

/// How long a space moved into this window's session waits to be shown, in ms (`pendingActive`).
const PLACE_WAIT_MS: u64 = 10_000;

/// What a name field edits.
#[derive(Clone, Debug, PartialEq)]
pub enum NameTarget {
    /// This window's name, in the title row.
    Window,
    /// A session's name, in the Rename Session card.
    Session(String),
    /// The pairing code, in the Add Server card: whether it is pairing, and why the last try failed.
    Server { busy: bool, error: Option<String> },
}
#[derive(Clone, Debug, PartialEq)]
pub struct NameEdit {
    pub target: NameTarget,
    pub draft: String,
}

impl Client {
    // MARK: The record

    /// What this window says about itself to the app's list of windows.
    pub fn record(&self) -> Record {
        Record { id: self.window_id.clone(), server: self.endpoint.server().to_owned(), name: self.window_name.clone(), session: self.session_id.clone(), active: self.space_id.clone(), sidebar: self.sidebar_open, pinned: self.pinned }
    }
    /// Measure where this window's content is on screen and write it to the app's list, which
    /// saves it (saved.rs); a window off every screen reports no size and keeps its last frame.
    pub fn measure_frame(&self) {
        let Some(placed) = crate::appkit::window::frame(self.native.ns_view()) else { return };
        let frame = placed.frame.map(|v| v as f32);
        if frame[2] > 0. && frame[3] > 0. {
            windows::with(|r| r.set_frame(&self.window_id, [frame[0], frame[1], frame[2], frame[3]]));
        }
    }
    /// Write this window's record to the app's list.
    pub fn publish(&self) {
        match self.little_record() {
            Some(little) => windows::with(|r| r.put_little(little)),
            None => windows::with(|r| r.put(self.record())),
        }
    }
    /// What opening a window like this one needs: the bridge, the endpoint and the test mode.
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }
    pub fn opener(&self) -> (Rc<Bridge>, Endpoint, bool) {
        (self.bridge.clone(), self.endpoint.clone(), self.hidden)
    }
    /// The wheeld this window shows, as the app keys its connection (`Endpoint::server`).
    pub fn server(&self) -> &str {
        self.endpoint.server()
    }
    pub fn layout_lists(&self) -> (Vec<Session>, Vec<Space>) {
        (self.sessions.clone(), self.spaces.clone())
    }
    /// The window's title: its name, else the space shown, else "Wheel".
    pub fn window_title(&self) -> String {
        windows::title(&self.record(), &self.spaces)
    }

    // MARK: Naming

    /// Name the window; an empty name clears it, so the title shows the space again.
    pub fn set_window_name(&mut self, name: Option<String>) {
        let name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
        if self.window_name != name {
            self.log(format!("window {} named {}", self.short_id(), name.as_deref().unwrap_or("(none)")));
            self.window_name = name;
        }
        self.publish();
    }
    fn short_id(&self) -> &str {
        &self.window_id[..self.window_id.len().min(12)]
    }
    /// Rename Window…: the title row shows a field holding the name.
    pub fn begin_window_rename(&mut self) {
        self.close_title_menu();
        self.rename = None;
        self.name_edit = Some(NameEdit { target: NameTarget::Window, draft: self.window_name.clone().unwrap_or_default() });
        self.sync_overlay();
    }
    /// Rename Session…: a card with the session's name in a field.
    pub fn begin_session_rename(&mut self, session: &str) {
        let Some(name) = self.sessions.iter().find(|s| s.id == session).map(|s| s.name.clone()) else { return };
        self.close_title_menu();
        self.rename = None;
        self.name_edit = Some(NameEdit { target: NameTarget::Session(session.to_owned()), draft: name });
        self.sync_overlay();
    }
    pub fn cancel_name_edit(&mut self) {
        if self.name_edit.take().is_some() {
            self.sync_overlay();
        }
    }
    /// Return in a name field: the window takes the name (none when empty); a session's name is
    /// sent to wheeld, and an empty one is not.
    pub fn commit_name_edit(&mut self) {
        let Some(edit) = self.name_edit.take() else { return };
        match edit.target {
            NameTarget::Window => self.set_window_name(Some(edit.draft)),
            NameTarget::Session(ref session) => {
                let name = edit.draft.trim().to_owned();
                if name.is_empty() {
                    self.name_edit = Some(edit);
                    return;
                }
                let session = session.clone();
                if self.sessions.iter().any(|s| s.id == session && s.name != name) {
                    self.log(format!("rename session {session} to {name}"));
                    self.send_layout(|req| ClientMsg::SessionRename { req: Some(req), session, name });
                }
            }
            NameTarget::Server { busy: true, .. } => {
                self.name_edit = Some(edit);
                return;
            }
            NameTarget::Server { .. } => {
                let code = edit.draft.trim().to_owned();
                if code.is_empty() {
                    self.name_edit = Some(NameEdit { target: NameTarget::Server { busy: false, error: Some("Paste a pairing code first".into()) }, draft: edit.draft });
                    return;
                }
                self.log("add server: pairing".into());
                self.name_edit = Some(NameEdit { target: NameTarget::Server { busy: true, error: None }, draft: edit.draft });
                crate::directory::ask(crate::directory::Ask::Pair { window: self.window_id.clone(), code });
                return;
            }
        }
        self.sync_overlay();
    }
    /// Return commits, Escape cancels, the field's input takes the edits; false when no field is up.
    pub fn name_key(&mut self, key: &str, command: bool) -> bool {
        if self.name_edit.is_none() {
            return false;
        }
        match key {
            "escape" => self.cancel_name_edit(),
            "enter" => self.commit_name_edit(),
            _ => return !command,
        }
        true
    }
    pub fn renaming_window(&self) -> bool {
        self.name_edit.as_ref().is_some_and(|e| e.target == NameTarget::Window)
    }

    // MARK: Closing

    /// Close Window, the red light and `wheel window close` close the window at once, and each
    /// logs it here first: a session no other window shows keeps running in the background, and a
    /// little window's space goes back to its tab.
    pub fn log_close(&mut self) {
        let what = if self.is_little() { "little window" } else { "window" };
        self.log(format!("{what} {} closed", self.short_id()));
    }
    /// Close Window and the palette's action: the window goes on its next frame.
    pub fn request_close_window(&mut self) {
        self.log_close();
        self.close_window_requested = true;
    }

    // MARK: Placing a space

    /// Move `space` into the session this window shows and show it here once wheeld's layout
    /// holds it there (AppModel.place(_:in:)): at the end of its spaces, even when it is one of
    /// them already. False when the window shows no session or the space is not listed.
    pub fn place(&mut self, space: &str) -> bool {
        let Some(session) = self.session_id.clone() else { return false };
        if !self.spaces.iter().any(|s| s.id == space) {
            return false;
        }
        self.pending_show = Some((space.to_owned(), settings::now_ms()));
        let space = space.to_owned();
        self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space, session, index: None });
        true
    }
    /// A layout arrived: a space placed here shows once this window's session holds it.
    pub(super) fn show_placed(&mut self) {
        let Some((space, at)) = self.pending_show.clone() else { return };
        if settings::now_ms().saturating_sub(at) > PLACE_WAIT_MS {
            self.pending_show = None;
        } else if self.session_spaces().iter().any(|s| s.id == space) {
            self.pending_show = None;
            self.space_id = Some(space);
        }
    }

    // MARK: Answering wheeld

    /// Send what a window command came to back to wheeld, and log it.
    pub fn window_answer(&mut self, id: u64, command: &WindowCommand, result: Result<Value, String>) {
        let message = match result {
            Ok(data) => {
                self.log(format!("window command {command:?} answered: {data}"));
                ClientMsg::WindowAnswer { id, data: Some(data), error: None }
            }
            Err(error) => {
                self.log(format!("window command {command:?} refused: {error}"));
                ClientMsg::WindowAnswer { id, data: None, error: Some(error) }
            }
        };
        self.send(&message);
    }

    // MARK: Drawing

    /// The title text while the sidebar is open: the name in bold with the space after it, or
    /// the space alone; the name field while Rename Window is up.
    pub(super) fn window_title_element(&self, label: &str) -> gpui::Div {
        if let Some(edit) = self.name_edit.as_ref().filter(|e| e.target == NameTarget::Window) {
            let shown = self.field_element(&super::fields::FieldId::WindowName, &edit.draft);
            return div().w(px(220.)).h(px(22.)).px(px(6.)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(ACCENT)).overflow_hidden().whitespace_nowrap().text_sm().flex().items_center().child(shown);
        }
        match &self.window_name {
            Some(name) => div().flex().items_center().gap(px(6.)).overflow_hidden().whitespace_nowrap()
                .child(div().font_weight(FontWeight::BOLD).text_color(rgb(TEXT)).child(name.clone()))
                .children(self.space().map(|s| div().text_color(rgb(DIM)).child(s.name.clone()))),
            None => div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)).child(label.to_owned()),
        }
    }
    /// The dimmed backdrop a card sits on; it takes every press under it.
    fn scrim(width: f32, height: f32, card: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x0000008c)).flex().justify_center().items_center()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(card)
    }
    /// The Rename Session card: the field, Cancel and Rename (SessionMenu.askName).
    pub(super) fn session_name_overlay(&self, width: f32, height: f32, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let edit = self.name_edit.as_ref().filter(|e| matches!(e.target, NameTarget::Session(_)))?;
        let button = |label: &'static str, yes: bool, cx: &mut Context<Self>| {
            div().px_3().py_1().rounded_sm().cursor_pointer().bg(rgb(if yes { 0x31483e } else { 0x22302a })).child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    if yes { this.commit_name_edit() } else { this.cancel_name_edit() }
                    cx.stop_propagation();
                    cx.notify();
                }))
        };
        let card = div().w(px(320f32.min(width - 32.))).flex().flex_col().gap_2().p_4().rounded_md().bg(rgb(0x17201c)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Rename session"))
            .child(div().h(px(24.)).px(px(6.)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(ACCENT)).overflow_hidden().whitespace_nowrap().flex().items_center().child(self.field_element(&super::fields::FieldId::WindowName, &edit.draft)))
            .child(div().flex().justify_end().gap_2().mt_2().child(button("Cancel", false, cx)).child(button("Rename", true, cx)));
        Some(Self::scrim(width, height, card, cx))
    }

    // MARK: State for checks

    /// `/state` `window_info`: this window's record, its title and the name field.
    pub fn window_json(&self) -> Value {
        json!({
            "id": self.window_id,
            "name": self.window_name,
            "title": self.window_title(),
            "name_edit": self.name_edit.as_ref().map(|e| json!({"target": match &e.target { NameTarget::Window => "window".to_owned(), NameTarget::Session(s) => format!("session:{s}"), NameTarget::Server { .. } => "server".to_owned() }, "error": match &e.target { NameTarget::Server { error, .. } => error.clone(), _ => None }, "draft": e.draft})),
            "windows": windows::with(|r| windows::ls(r, self.endpoint.server(), &self.sessions, &self.spaces)),
            "little": self.little_json(),
        })
    }
}

// MARK: Servers

impl Client {
    pub fn set_pump(&mut self, task: gpui::Task<()>) {
        self.pump = Some(task);
    }
    /// Move this window to `session` of `endpoint`'s wheeld: it tells its old wheeld it shows
    /// nothing and leaves that subscription, forgets what only the old wheeld's answers would
    /// complete, and the `Up` its new subscription hears resets the rest as a reconnect does.
    pub fn rebind(&mut self, out: crate::daemon::Outbox, endpoint: Endpoint, session: String) {
        self.log(format!("window {}: switch to server {} session {session}", self.short_id(), endpoint.server()));
        let _ = self.out.send(crate::daemon::frame(&ClientMsg::View { req: None, window: self.window_id.clone(), session: None, space: None, pane: None }));
        self.out.leave();
        self.out = out;
        self.endpoint = endpoint;
        self.pump = None;
        self.close_title_menu();
        self.session_id = Some(session);
        self.space_id = None;
        self.session_to_show = None;
        self.session_show_req = None;
        self.orchestrator_dir = None;
        self.agent_reqs.clear();
        self.split_reqs.clear();
        // Not Connecting: the new subscription's `Up` runs the reset a reconnect runs.
        self.link = Link::Down("switching servers".into());
        self.publish();
    }
    /// Orchestrator mode shows the one orchestrator (ADR-0054): while this window's wheeld has no
    /// space on its orchestrator's folder and another server's does, the window moves there.
    pub fn follow_orchestrator(&mut self) {
        if self.little || self.mode != "orchestrator" || self.link != Link::Up || self.orchestrator_dir.is_none() || self.orchestrator_space().is_some() {
            return;
        }
        if let Some((server, session)) = crate::directory::orchestrator_elsewhere(self.server()) {
            self.log(format!("window {}: no orchestrator space on server {}; server {server} has one, switching", self.short_id(), self.server()));
            crate::directory::ask(crate::directory::Ask::Switch { window: self.window_id.clone(), server, session });
        }
    }
    /// Add Server…: a card with a field for the code `wheel remote pair` prints.
    pub fn begin_add_server(&mut self) {
        self.close_title_menu();
        self.rename = None;
        self.name_edit = Some(NameEdit { target: NameTarget::Server { busy: false, error: None }, draft: String::new() });
        self.sync_overlay();
    }
    /// The pairing Add Server… asked for ended: the card closes, or says why it failed.
    pub fn paired(&mut self, result: Result<String, String>) {
        match result {
            Ok(name) => {
                self.log(format!("add server: paired with {name}"));
                if self.name_edit.as_ref().is_some_and(|e| matches!(e.target, NameTarget::Server { .. })) {
                    self.name_edit = None;
                    self.sync_overlay();
                }
            }
            Err(error) => {
                self.log(format!("add server: pairing failed: {error}"));
                if let Some(edit) = self.name_edit.as_mut().filter(|e| matches!(e.target, NameTarget::Server { .. })) {
                    edit.target = NameTarget::Server { busy: false, error: Some(error) };
                }
            }
        }
    }
    /// Something the directory did for this window failed: say so in a notice.
    pub fn directory_error(&mut self, text: String) {
        self.log(text.clone());
        self.post(crate::toasts::Toast::posted("directory".into(), crate::toasts::Level::Error, text, None));
    }
    /// The Add Server card: the field, its error, Cancel and Pair.
    pub(super) fn add_server_overlay(&self, width: f32, height: f32, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let edit = self.name_edit.as_ref()?;
        let NameTarget::Server { busy, error } = &edit.target else { return None };
        let busy = *busy;
        // Fixed-width buttons and a reserved error line: Pairing… and an error never move Cancel or Pair.
        let button = |label: &'static str, yes: bool, cx: &mut Context<Self>| {
            let el = div().w(px(84.)).py_1().flex().justify_center().rounded_sm().bg(rgb(if yes { 0x31483e } else { 0x22302a })).child(label);
            if yes && busy {
                return el.text_color(rgb(DIM));
            }
            el.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                if yes { this.commit_name_edit() } else { this.cancel_name_edit() }
                cx.stop_propagation();
                cx.notify();
            }))
        };
        let card = div().w(px(360f32.min(width - 32.))).flex().flex_col().gap_2().p_4().rounded_md().bg(rgb(0x17201c)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Add Server"))
            .child(div().text_xs().text_color(rgb(DIM)).child("Paste the code `wheel remote pair` prints"))
            .child(div().h(px(24.)).px(px(6.)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(ACCENT)).overflow_hidden().whitespace_nowrap().flex().items_center().child(self.field_element(&super::fields::FieldId::WindowName, &edit.draft)))
            .child(div().h(px(16.)).text_xs().text_color(rgb(super::sidebar::RED)).overflow_hidden().text_ellipsis().whitespace_nowrap().children(error.clone()))
            .child(div().flex().justify_end().gap_2().child(button("Cancel", false, cx)).child(button(if busy { "Pairing…" } else { "Pair" }, true, cx)));
        Some(Self::scrim(width, height, card, cx))
    }
}
