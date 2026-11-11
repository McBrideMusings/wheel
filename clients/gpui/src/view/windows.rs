//! This window as `wheel window` and the window menus see it (AppModel+Windows.swift,
//! ContentView.swift's WindowCloseSheet, TitleRow.swift's WindowTitle, SessionMenu.swift): the
//! record it writes to the app's list of windows, its name and the title field that edits it,
//! the question closing the last window on a session asks, Rename Session…, and placing a
//! space in this window for `wheel window space-move`.
use super::*;
use super::settings::{ACCENT, DIM, LINE, TEXT};
use crate::focus::{Typed, type_into};
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
}
#[derive(Clone, Debug, PartialEq)]
pub struct NameEdit {
    pub target: NameTarget,
    pub draft: String,
}

/// The close question's answers (AppModel.WindowCloseAnswer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAnswer {
    /// Background Session and Close: the session keeps running with no window.
    Background,
    /// Close Session and All Processes.
    Stop,
    Cancel,
}

impl Client {
    // MARK: The record

    /// What this window says about itself to the app's list of windows.
    pub fn record(&self) -> Record {
        Record { id: self.window_id.clone(), name: self.window_name.clone(), session: self.session_id.clone(), active: self.space_id.clone(), sidebar: self.sidebar_open }
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
        }
        self.sync_overlay();
    }
    /// Return commits, Escape cancels, the rest edits the draft; false when no field is up.
    pub fn name_key(&mut self, key: &str, text: Option<&str>, command: bool) -> bool {
        let Some(edit) = self.name_edit.as_mut() else { return false };
        match key {
            "escape" => self.cancel_name_edit(),
            "enter" => self.commit_name_edit(),
            _ => {
                if let Typed::Nothing = type_into(&mut edit.draft, key, text, command) {
                    return !command;
                }
            }
        }
        true
    }
    pub fn renaming_window(&self) -> bool {
        self.name_edit.as_ref().is_some_and(|e| e.target == NameTarget::Window)
    }

    // MARK: Closing

    /// Close Window, the red light and `wheel window close`: the last window showing a session
    /// that holds spaces asks what to do with the session; true when the window may close now.
    pub fn ask_close_window(&mut self) -> bool {
        // A little window's red light returns its space to its tab, and asks nothing.
        if self.is_little() {
            self.log(format!("little window {} closed by the person", self.window_id));
            return true;
        }
        self.publish();
        if !windows::with(|r| r.close_needs_answer(&self.window_id, &self.sessions)) {
            return true;
        }
        if !self.closing_window {
            self.closing_window = true;
            let summary = self.close_summary();
            self.log(format!("close window {} asked: {summary}", self.short_id()));
            self.sync_overlay();
        }
        false
    }
    /// Close Window and the debug route: close now, or when the question is answered.
    pub fn request_close_window(&mut self) {
        if self.ask_close_window() {
            self.close_window_requested = true;
        }
    }
    pub fn close_summary(&self) -> String {
        let record = self.record();
        windows::with(|r| windows::close_summary(r, &record, &self.sessions, &self.spaces))
    }
    /// The question's answer: close the window, close its session and then the window, or neither.
    pub fn answer_window_close(&mut self, answer: CloseAnswer) {
        if !std::mem::take(&mut self.closing_window) {
            return;
        }
        self.sync_overlay();
        self.log(format!("close window {} answered: {answer:?}", self.short_id()));
        match answer {
            CloseAnswer::Background => self.close_window_requested = true,
            CloseAnswer::Stop => {
                if let Some(session) = self.session_id.clone() {
                    self.send_layout(|req| ClientMsg::SessionClose { req: Some(req), session });
                }
                self.close_window_requested = true;
            }
            CloseAnswer::Cancel => {}
        }
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
            let placeholder = self.space().map(|s| s.name.clone()).unwrap_or_else(|| "Window name".into());
            let shown = if edit.draft.is_empty() { div().text_color(rgb(DIM)).child(format!("▏{placeholder}")) } else { div().child(format!("{}▏", edit.draft)) };
            return div().w(px(220.)).h(px(22.)).px(px(6.)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(ACCENT)).overflow_hidden().whitespace_nowrap().text_sm().flex().items_center().child(shown);
        }
        match &self.window_name {
            Some(name) => div().flex().items_center().gap(px(6.)).overflow_hidden().whitespace_nowrap()
                .child(div().font_weight(FontWeight::BOLD).text_color(rgb(TEXT)).child(name.clone()))
                .children(self.space().map(|s| div().text_color(rgb(DIM)).child(s.name.clone()))),
            None => div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)).child(label.to_owned()),
        }
    }
    /// The close question over the window (WindowCloseSheet).
    pub(super) fn window_close_overlay(&self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let answer = |label: &'static str, answer: CloseAnswer, cx: &mut Context<Self>| {
            let (bg, fg) = match answer { CloseAnswer::Background => (ACCENT, TEXT), CloseAnswer::Stop => (0x22302a, 0xff453a), CloseAnswer::Cancel => (0x22302a, TEXT) };
            div().w_full().py_1().rounded_sm().flex().justify_center().cursor_pointer().bg(rgb(bg)).text_color(rgb(fg)).child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.answer_window_close(answer);
                    cx.stop_propagation();
                    cx.notify();
                }))
        };
        let card = div().w(px(300f32.min(width - 32.))).flex().flex_col().items_center().gap(px(6.)).p(px(20.)).rounded_md().bg(rgb(0x17201c)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_center().child(format!("Close the “{}” window?", self.window_title())))
            .child(div().text_xs().text_center().text_color(rgb(DIM)).child(self.close_summary()))
            .child(div().w_full().flex().flex_col().gap_2().mt(px(12.))
                .child(answer("Background Session and Close", CloseAnswer::Background, cx))
                .child(answer("Close Session and All Processes", CloseAnswer::Stop, cx))
                .child(answer("Cancel", CloseAnswer::Cancel, cx)));
        Self::scrim(width, height, card, cx)
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
            .child(div().h(px(24.)).px(px(6.)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(ACCENT)).overflow_hidden().whitespace_nowrap().flex().items_center().child(format!("{}▏", edit.draft)))
            .child(div().flex().justify_end().gap_2().mt_2().child(button("Cancel", false, cx)).child(button("Rename", true, cx)));
        Some(Self::scrim(width, height, card, cx))
    }

    // MARK: State for checks

    /// `/state` `window_info`: this window's record, its title, the close question and the name field.
    pub fn window_json(&self) -> Value {
        json!({
            "id": self.window_id,
            "name": self.window_name,
            "title": self.window_title(),
            "closing": self.closing_window.then(|| self.close_summary()),
            "name_edit": self.name_edit.as_ref().map(|e| json!({"target": match &e.target { NameTarget::Window => "window".to_owned(), NameTarget::Session(s) => format!("session:{s}") }, "draft": e.draft})),
            "windows": windows::with(|r| windows::ls(r, &self.sessions, &self.spaces)),
            "little": self.little_json(),
        })
    }
}
