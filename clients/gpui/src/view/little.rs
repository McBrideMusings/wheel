//! Little windows (LittleWindow.swift, WindowManager.openLittle, AppModel+Windows.swift's
//! openLittle, goToLittle, closeLittle, bringBack and littleClosedByPerson): a window showing one
//! space's grid under a 30 pt strip, with no tabs, no sidebar and no bottom row. It belongs to the
//! main window that holds the space, whose tab and row dim with a back icon and whose grid shows
//! a placeholder while the space is away. A little window is its own client of wheeld, as every
//! window is (ADR-0022); the app's list of windows holds its record beside the main windows'.
use super::*;
use super::settings::{ACCENT, DIM, LINE, TEXT};
use crate::windows::{self, Little};

/// The strip's height: the traffic lights sit in it.
const STRIP: f32 = 30.0;
/// The gap the grid keeps from the window's edges and the strip.
const INSET: f32 = 8.0;
/// The width the traffic lights take at the strip's left end.
const LIGHTS: f32 = 72.0;
/// How a tab or row of a space in a little window dims (AppModel.isDimmed).
pub const AWAY_OPACITY: f32 = 0.5;
/// A little window's size when it opens.
pub const SIZE: (f32, f32) = (480.0, 330.0);
pub const MIN_SIZE: (f32, f32) = (320.0, 200.0);

/// Open `space` in a little window owned by the main window that holds it (none for a space
/// whose session no window shows), or bring its open one front (AppModel.openLittle): what a
/// window's menu and the menu bar's popover both run. The window's id, none when no session
/// holds the space, and the line that says what happened.
pub fn open_little(cx: &mut App, bridge: &Rc<Bridge>, endpoint: Endpoint, hidden: bool, sessions: &[Session], space: &str) -> (Option<String>, String) {
    if let Some(open) = windows::with(|r| r.little_showing(space).map(|l| l.id.clone())) {
        if let Some(handle) = crate::window_handle(&open) {
            crate::bring_front(cx, handle);
        }
        return (Some(open.clone()), format!("little window {open} brought front"));
    }
    let Some(session) = crate::windows::session_holding(sessions, space).map(|s| s.id.clone()) else {
        return (None, format!("little window for {space}: the layout places it nowhere"));
    };
    let owner = windows::with(|r| r.window_holding(space, sessions).map(|w| w.id.clone()));
    let handle = match crate::open_window_as(cx, bridge, endpoint, Some(session), Some(space.to_owned()), hidden, crate::Opening::Little, None) {
        Ok(handle) => handle,
        Err(e) => return (None, format!("little window for {space} did not open: {e:#}")),
    };
    let Ok(id) = handle.read(cx).map(|c| c.window_id().to_owned()) else { return (None, format!("little window for {space} closed as it opened")) };
    crate::bring_front(cx, handle);
    crate::refresh_windows(cx);
    let line = format!("little window {id} opened for space {space}, owner {}", owner.as_deref().unwrap_or("none"));
    (Some(id), line)
}

impl Client {
    // MARK: This window as a little one

    pub fn is_little(&self) -> bool {
        self.little
    }
    /// The main window this little window belongs to: the one holding its space now, so a
    /// space that moved session or an owner that closed re-points it (WindowLayout.normalize).
    fn little_owner(&self) -> Option<String> {
        let space = self.space_id.as_deref()?;
        windows::with(|r| r.window_holding(space, &self.sessions).map(|w| w.id.clone()))
    }
    /// This little window's record for the app's list of windows.
    pub(super) fn little_record(&self) -> Option<Little> {
        if !self.little {
            return None;
        }
        Some(Little { id: self.window_id.clone(), space: self.space_id.clone().unwrap_or_default(), owner: self.little_owner(), pinned: self.pinned })
    }
    /// The grid of a little window: under the strip, inset from the edges.
    pub(super) fn little_grid_area(&self) -> Rect {
        let (w, h) = self.viewport;
        Rect { x: INSET, y: STRIP + INSET, w: (w - 2. * INSET).max(0.), h: (h - STRIP - 2. * INSET).max(0.) }
    }
    /// A layout arrived in a little window: it shows its space in whichever session holds it,
    /// and closes once no session does (the space closed). True when it is closing: the space
    /// it showed stays its own until it goes, so no other space is reported or dimmed.
    pub(super) fn follow_little_space(&mut self) -> bool {
        if !self.little {
            return false;
        }
        let Some(space) = self.space_id.clone() else { return false };
        match crate::windows::session_holding(&self.sessions, &space) {
            Some(holder) => self.session_id = Some(holder.id.clone()),
            None if !self.sessions.is_empty() => {
                self.log(format!("little window {}: space {space} is gone; closing", self.window_id));
                self.close_window_requested = true;
                return true;
            }
            None => {}
        }
        false
    }
    /// The back icon in the strip: the window closes and its space returns to its tab.
    pub fn little_back(&mut self) {
        if self.is_little() {
            self.log(format!("little window {} back", self.window_id));
            self.close_window_requested = true;
        }
    }

    // MARK: A main window's spaces in little windows

    /// The little window showing `space`, if one is open.
    pub fn little_showing(&self, space: &str) -> Option<String> {
        windows::with(|r| r.little_showing(space).map(|l| l.id.clone()))
    }
    /// This main window's shown space is in a little window: the grid shows the placeholder.
    pub fn space_away(&self) -> bool {
        !self.is_little() && self.space_id.as_deref().is_some_and(|s| self.little_showing(s).is_some())
    }
    /// Open `space` in a little window, or bring its open one front (`open_little`).
    pub fn open_little(&mut self, space: &str, cx: &mut App) -> Option<String> {
        let (id, line) = open_little(cx, &self.bridge, self.endpoint.clone(), self.hidden, &self.sessions, space);
        self.log(line);
        id
    }
    /// Go to Little Window: bring the little window showing `space` front.
    pub fn go_to_little(&mut self, space: &str, cx: &mut App) {
        let Some(id) = self.little_showing(space) else { return };
        self.log(format!("little window {id} brought front"));
        if let Some(handle) = crate::window_handle(&id) {
            crate::bring_front(cx, handle);
        }
    }
    /// Bring Back and the back icon on a tab or row: the little window showing `space` closes,
    /// and the tab shows the grid again.
    pub fn bring_back(&mut self, space: &str, cx: &mut App) {
        let Some(id) = self.little_showing(space) else { return };
        self.log(format!("space {space} brought back from little window {id}"));
        if let Some(handle) = crate::window_handle(&id) {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
    }

    // MARK: Drawing

    /// The strip along a little window's top: the gap for the traffic lights, the space's name,
    /// its pane count and dots, then the pin and the back icon (LittleHeader).
    pub(super) fn little_strip(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (w, _) = self.viewport;
        let space = self.space();
        let pinned = self.pinned;
        let icon = |label: &'static str, on: bool| {
            div().w(px(22.)).h(px(22.)).rounded_sm().flex().items_center().justify_center().cursor_pointer()
                .text_color(rgb(if on { ACCENT } else { TEXT })).child(label)
        };
        let mut strip = div().absolute().left_0().top_0().w(px(w)).h(px(STRIP)).px(px(10.)).flex().items_center().gap(px(8.)).bg(rgb(0x0e1311)).border_b_1().border_color(rgb(LINE))
            .child(div().w(px(LIGHTS - 10.)).flex_none());
        if let Some(space) = space {
            strip = strip.child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)).overflow_hidden().whitespace_nowrap().child(space.name.clone()))
                .child(div().text_xs().text_color(rgb(DIM)).child(leaves_of(&space.root).len().to_string()))
                .children(status::space_dots(space, &self.status_dots));
        }
        strip.child(div().flex_1())
            .child(icon(if pinned { "◉" } else { "○" }, pinned)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.press("little pin".into());
                    this.set_pinned(!pinned);
                    cx.stop_propagation();
                    cx.notify();
                })))
            .child(icon("↩", false)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.press("little back".into());
                    this.little_back();
                    cx.stop_propagation();
                    cx.notify();
                })))
    }
    /// What a main window's grid shows while its space is in a little window (LittlePlaceholder).
    pub(super) fn little_placeholder(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let space = self.space().filter(|_| self.space_away())?;
        let r = self.grid_area();
        let id = space.id.clone();
        let button = |label: &'static str, primary: bool| {
            div().px_3().py_1().rounded_sm().cursor_pointer().bg(rgb(if primary { ACCENT } else { 0x22302a })).text_color(rgb(TEXT)).child(label)
        };
        let (go, back) = (id.clone(), id);
        Some(div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).p(px(4.))
            .child(div().size_full().rounded(px(10.)).bg(rgb(0x17201c)).border_1().border_color(rgb(LINE)).flex().flex_col().items_center().justify_center().gap(px(10.))
                .child(div().text_3xl().text_color(rgb(DIM)).child("⧉"))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)).child(space.name.clone()))
                .child(div().text_color(rgb(DIM)).child("This space is open in a little window."))
                .child(div().flex().gap_2()
                    .child(button("Go to Little Window", true).on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.press("placeholder go to little".into());
                        this.go_to_little(&go, cx);
                        cx.stop_propagation();
                    })))
                    .child(button("Bring Back", false).on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.press("placeholder bring back".into());
                        this.bring_back(&back, cx);
                        cx.stop_propagation();
                    }))))))
    }

    // MARK: State for checks

    /// `/state` `little`: this window as a little one, and whether this main window's space is away.
    pub fn little_json(&self) -> serde_json::Value {
        serde_json::json!({
            "mode": self.little.then(|| serde_json::json!({"owner": self.little_owner(), "space": self.space_id})),
            "space_away": self.space_away(),
            "littles": windows::with(|r| r.littles.iter().map(|l| serde_json::json!({"id": l.id, "space": l.space, "owner": l.owner, "pinned": l.pinned})).collect::<Vec<_>>()),
        })
    }
}
