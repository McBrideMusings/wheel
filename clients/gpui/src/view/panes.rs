//! A pane's header (SpacesView.swift's PaneHeader): its buttons, its menu, and the drag that
//! swaps it with another pane or docks it beside one (PaneDrop.swift, AppModel.dragPane and
//! endPaneDrag); and wheeld's `space_focused`, which shows a space and focuses a pane
//! (AppModel.showRequested).
use super::*;
use super::commands::capitalized;
use serde_json::Value;
use crate::grid_drop::{self, GridDrop, Zone};
use wheel_proto::{MoveTarget, PaneSite};

/// One entry of a pane header's menu (AppModel.HeaderMenuItem).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderMenuItem {
    pub id: String,
    pub title: String,
    pub enabled: bool,
}

/// The width and row height the header menu draws with.
pub const MENU_W: f32 = 300.0;
pub const MENU_ROW_H: f32 = 22.0;
/// The side of one header button.
const BUTTON: f32 = 18.0;

/// The header's buttons, in the Swift order (Split Right, Split Down, Pop Out, Background, the
/// pane menu, Close): (id, SF Symbol, label).
pub const HEADER_BUTTONS: [(&str, &str, &str); 6] = [
    ("split-right", "square.split.2x1", "Split Right"),
    ("split-down", "square.split.1x2", "Split Down"),
    ("pop", "rectangle.on.rectangle", "Pop out"),
    ("background", "tray.and.arrow.down", "Background"),
    ("menu", "ellipsis", "Pane menu"),
    ("close", "xmark", "Close Pane"),
];

/// How long a focus request or a split's new pane waits for the layout to list it.
const PENDING_FOCUS_MS: u64 = 10_000;

/// The window that answers a `space_focused` for `space` (AppModel.focus(space:)): the first
/// window whose session holds it, else the one key last. None when no window is open.
pub fn focus_target(windows: &crate::windows::Registry, sessions: &[Session], space: &str) -> Option<String> {
    let home = sessions.iter().find(|s| s.spaces.iter().any(|id| id == space)).map(|s| s.id.as_str());
    windows.windows.iter().find(|w| home.is_some() && w.session.as_deref() == home)
        .or_else(|| windows.focused())
        .map(|w| w.id.clone())
}

impl Client {
    // MARK: The drag inside the space

    /// The drop the pointer at (`x`, `y`) is over in this window's grid.
    pub fn grid_drop_at(&self, x: f32, y: f32, pane: &str) -> Option<GridDrop> {
        let layers = self.layers();
        let (grid, cards) = layers.split_first()?;
        if cards.iter().any(|l| l.card.as_ref().is_some_and(|c| crate::sidebar::contains(c.frame, x, y))) {
            return None;
        }
        grid_drop::drop_at(x, y, &grid.leaves, pane)
    }
    /// Name where the dragged pane docks in its space, as the pointer or the debug route says.
    pub fn hover_grid_drop(&mut self, drop: Option<GridDrop>) {
        let Some(drag) = &mut self.pane_drag else { return };
        if drag.drop == drop {
            return;
        }
        if drop.is_some() {
            drag.away = None;
        }
        drag.drop = drop;
        let words = drag.drop.as_ref().map(|d| format!("{} {}", d.target, d.zone.label())).unwrap_or_else(|| "nothing".into());
        let pane = drag.pane.clone();
        self.log(format!("pane drag {pane}: over {words}"));
        self.sync_sidebar_overlay();
    }
    /// The part of the grid lit while a dragged pane is over a drop zone.
    pub fn grid_drop_highlight(&self) -> Option<Rect> {
        let drop = self.pane_drag.as_ref()?.drop.as_ref()?;
        let (leaves, _) = self.scene();
        let target = leaves.iter().find(|p| p.leaf.id == drop.target)?;
        Some(grid_drop::highlight(drop.zone, target.rect))
    }
    /// Release over a drop zone: swap the two panes, or dock the dragged one beside the target.
    pub(super) fn send_grid_drop(&mut self, pane: String, drop: GridDrop) {
        self.log(format!("pane drag {pane}: dropped on {} {}", drop.target, drop.zone.label()));
        match drop.zone {
            Zone::Center => self.send_layout(|req| ClientMsg::PaneSwap { req: Some(req), pane, with: drop.target }),
            Zone::Edge(dir) => self.send_layout(|req| ClientMsg::PaneMove { req: Some(req), pane, to: MoveTarget::Beside { pane: drop.target, dir } }),
        }
    }

    // MARK: Sites

    fn space_holding(&self, pane: &str) -> Option<&Space> {
        self.spaces.iter().find(|s| s.site_of(pane).is_some())
    }
    pub(super) fn site_of(&self, pane: &str) -> Option<PaneSite> {
        self.spaces.iter().find_map(|s| s.site_of(pane))
    }
    /// Pop Out and Background: put `pane` at `site`, or back in the grid when it is there already
    /// (AppModel.toggleSite).
    pub fn toggle_site(&mut self, pane: &str, site: PaneSite) {
        let Some(now) = self.site_of(pane) else { return };
        let to = if now == site { PaneSite::Grid } else { site };
        self.log(format!("pane {pane}: {} -> {}", now.name(), to.name()));
        let pane = pane.to_owned();
        self.send_layout(|req| ClientMsg::PaneSetSite { req: Some(req), pane, site: to });
    }

    // MARK: The header's menu

    /// What the pane menu offers (AppModel.headerMenu): the four splits, every space it can move
    /// to under its session's name, the kinds it can change to, a new space, a new little window
    /// (none until wheel-ynjo.7.5, so disabled), and Close.
    pub fn header_menu(&self, pane: &str) -> Vec<HeaderMenuItem> {
        let item = |id: String, title: String, enabled: bool| HeaderMenuItem { id, title, enabled };
        let Some(space) = self.space_holding(pane) else { return vec![] };
        let home = Some(space.id.clone());
        let Some(leaf) = std::iter::once(&space.root).chain(&space.popped).chain(&space.backgrounded).flat_map(leaves_of).find(|l| l.id == pane).cloned() else { return vec![] };
        let known = matches!(leaf.kind.as_str(), "empty" | "terminal" | "web" | "browser" | "agent");
        let mut items = vec![];
        if known {
            for word in ["right", "left", "down", "up"] {
                items.push(item(format!("split-{word}"), format!("Split {}", capitalized(word)), true));
            }
        }
        for session in &self.sessions {
            for id in &session.spaces {
                let Some(space) = self.spaces.iter().find(|s| &s.id == id) else { continue };
                let here = home.as_deref() == Some(id.as_str());
                items.push(item(format!("move-space:{id}"), format!("Move Pane to Space ▸ {} ▸ {}{}", session.name, space.name, if here { " (here)" } else { "" }), !here));
            }
        }
        if known {
            items.push(item("kind:shell".into(), "Change Type ▸ Shell".into(), leaf.kind != "terminal" || leaf.harness.is_some()));
            for agent in self.agents.iter().filter(|a| a.found) {
                let current = leaf.kind == "agent" && leaf.harness.as_deref() == Some(agent.name.as_str());
                items.push(item(format!("kind:agent:{}", agent.name), format!("Change Type ▸ {}", capitalized(&agent.name)), !current));
            }
            items.push(item("kind:browser".into(), "Change Type ▸ Browser".into(), leaf.kind != "browser"));
            items.push(item("kind:artifact".into(), "Change Type ▸ Artifact…".into(), true));
            items.push(item("kind:empty".into(), "Change Type ▸ Empty".into(), leaf.kind != "empty"));
        }
        if leaf.kind == "agent" && leaf.agent.is_some() {
            items.push(item("conversations".into(), "Conversations…".into(), true));
        }
        items.push(item("move-new-space".into(), "Move Pane to New Space".into(), true));
        items.push(item("move-new-little".into(), "Move Pane to New Little Window".into(), false));
        items.push(item("close".into(), "Close Pane".into(), true));
        items
    }
    /// Run a pane menu item, as the menu, the header's buttons and the debug route do.
    pub fn run_header_menu(&mut self, id: &str, pane: &str) {
        self.close_pane_menu();
        let pane = pane.to_owned();
        match id {
            split if split.starts_with("split-") => {
                let Some(dir) = super::commands::dir_named(&split[6..]) else { return };
                let req = self.send_layout_req(|req| ClientMsg::PaneSplit { req: Some(req), pane, dir: Some(dir), focus: false, site: None });
                self.split_reqs.insert(req);
            }
            "pop" => self.toggle_site(&pane, PaneSite::Popped),
            "background" => self.toggle_site(&pane, PaneSite::Backgrounded),
            "close" => self.ask_close_pane(&pane),
            "kind:shell" => self.ask_set_kind(&pane, NewPaneKind::Terminal),
            "kind:browser" => self.ask_set_kind(&pane, NewPaneKind::Browser),
            "kind:empty" => self.ask_set_kind(&pane, NewPaneKind::Empty),
            "kind:artifact" => self.open_palette(&pane),
            "conversations" => {
                self.open_conversations(&pane);
            }
            "move-new-space" => self.send_layout(|req| ClientMsg::PaneMove { req: Some(req), pane, to: MoveTarget::NewSpace { name: None, focus: false } }),
            other => {
                if let Some(agent) = other.strip_prefix("kind:agent:") {
                    self.ask_start_agent(&pane, agent);
                } else if let Some(space) = other.strip_prefix("move-space:") {
                    let space = space.to_owned();
                    self.log(format!("pane {pane}: move to the end of space {space}"));
                    self.send_layout(|req| ClientMsg::PaneMove { req: Some(req), pane, to: MoveTarget::End { space } });
                }
            }
        }
    }
    /// Run `id` when the menu offers it enabled; false when it does not.
    pub fn choose_header_menu(&mut self, id: &str, pane: &str) -> bool {
        let offered = matches!(id, "pop" | "background") || self.header_menu(pane).iter().any(|i| i.id == id && i.enabled);
        if !offered {
            self.log(format!("pane menu {pane}: {id} not available"));
            return false;
        }
        self.log(format!("pane menu {pane}: {id}"));
        self.run_header_menu(id, pane);
        true
    }
    pub fn open_pane_menu(&mut self, pane: &str, x: f32, y: f32) {
        self.close_space_menu();
        let h = self.pane_menu_height(pane);
        let x = x.min(self.viewport.0 - MENU_W - 4.).max(0.);
        let y = y.min(self.viewport.1 - h - 4.).max(0.);
        self.pane_menu = Some((pane.to_owned(), x, y));
        self.sync_sidebar_overlay();
    }
    pub fn close_pane_menu(&mut self) {
        if self.pane_menu.take().is_some() {
            self.sync_sidebar_overlay();
        }
    }
    pub(super) fn pane_menu_height(&self, pane: &str) -> f32 {
        self.header_menu(pane).len() as f32 * MENU_ROW_H + 8.
    }
    /// The pane menu, anchored at the press that opened it.
    pub fn pane_menu_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let (pane, x, y) = self.pane_menu.clone()?;
        let mut list = div().absolute().left(px(x)).top(px(y)).w(px(MENU_W)).py(px(4.)).rounded_md().bg(rgb(0x22302a)).border_1().border_color(rgb(0x3d5247)).flex().flex_col().text_xs()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for item in self.header_menu(&pane) {
            let colour = if !item.enabled { 0x5a6e64 } else if item.id == "close" { 0xe07a6a } else { 0xd9e8e1 };
            let mut row = div().h(px(MENU_ROW_H)).px(px(10.)).flex().items_center().whitespace_nowrap().overflow_hidden().text_color(rgb(colour)).child(item.title.clone());
            if item.enabled {
                let (id, pane) = (item.id.clone(), pane.clone());
                row = row.cursor_pointer().hover(|s| s.bg(rgb(0x31483e))).on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.choose_header_menu(&id, &pane);
                    cx.notify();
                }));
            }
            list = list.child(row);
        }
        Some(list)
    }

    // MARK: The header's buttons

    /// The buttons on `pane`'s header, right of its title; each press stops before the header's
    /// own press, so a click never starts a drag.
    pub(super) fn header_buttons(&self, pane: &str, cx: &mut Context<Self>) -> gpui::Div {
        let popped = self.site_of(pane) == Some(PaneSite::Popped);
        let mut row = div().flex().items_center().gap(px(2.)).flex_none();
        for (id, symbol, _label) in HEADER_BUTTONS {
            let colour = if id == "pop" && popped { 0x7fb89a } else { 0xb7cbc1 };
            let icon = self.symbol(symbol, 11., colour).unwrap_or_else(|| div().text_xs().child(symbol).into_any_element());
            let pane = pane.to_owned();
            row = row.child(div().id(gpui::ElementId::Name(format!("hdr-{id}-{pane}").into())).w(px(BUTTON)).h(px(BUTTON)).rounded_sm().flex().items_center().justify_center().cursor_pointer()
                .hover(|s| s.bg(rgb(0x31483e))).child(icon)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.focus_pane(&pane);
                    if id == "menu" {
                        this.open_pane_menu(&pane, f32::from(e.position.x) - MENU_W + BUTTON, f32::from(e.position.y) + 10.);
                    } else {
                        this.choose_header_menu(id, &pane);
                    }
                    cx.notify();
                })));
        }
        row
    }
    /// Each header button's frame in window points, for `/state`.
    pub fn header_button_frames(&self, pane: &str) -> Option<Value> {
        let (leaves, _) = self.scene();
        let r = leaves.iter().find(|p| p.leaf.id == pane)?.rect;
        // The buttons end 8 pt (the header's padding) before the pane's right edge, the status
        // dot's 12 pt and a 2 pt gap after them.
        let n = HEADER_BUTTONS.len() as f32;
        let right = r.x + r.w - 8. - 12. - 2.;
        let left = right - n * BUTTON - (n - 1.) * 2.;
        Some(Value::Object(HEADER_BUTTONS.iter().enumerate().map(|(i, (id, _, label))| {
            let x = left + i as f32 * (BUTTON + 2.);
            ((*id).to_owned(), serde_json::json!({"label": label, "frame": [x, r.y + (TITLE - BUTTON) / 2., BUTTON, BUTTON]}))
        }).collect()))
    }

    // MARK: Focus requests

    /// wheeld asked every client to show `space` and focus `pane` (`wheel space focus`, `wheel pane
    /// focus`, a `--focus` split): this window shows the session holding it, then the space, then
    /// focuses the pane.
    /// A space this window does not list yet waits for its layout (AppModel's `pendingShow`).
    /// True when the space showed now.
    pub fn show_requested(&mut self, space: &str, pane: Option<&str>) -> bool {
        if !self.spaces.iter().any(|s| s.id == space) {
            self.pending_focus = Some((Some(space.to_owned()), pane.map(str::to_owned), super::settings::now_ms()));
            return false;
        }
        if let Some(session) = self.sessions.iter().find(|s| s.spaces.iter().any(|id| id == space)).map(|s| s.id.clone()) {
            self.select_session(&session);
        }
        self.select_space(space);
        self.log(format!("space {space} focused by a request{}", pane.map(|p| format!(", pane {p}")).unwrap_or_default()));
        if let Some(pane) = pane.filter(|p| self.space_holding(p).is_some_and(|s| s.id == space)) {
            self.focus_pane(pane);
        }
        true
    }
    /// A layout arrived: show what was waiting for it, once listed; give up after 10 s.
    pub(super) fn retry_pending_focus(&mut self) {
        let Some((space, pane, at)) = self.pending_focus.take() else { return };
        if super::settings::now_ms().saturating_sub(at) > PENDING_FOCUS_MS {
            return;
        }
        let holder = pane.as_deref().and_then(|p| self.space_holding(p)).map(|s| s.id.clone());
        match (space, holder) {
            (Some(space), _) if self.spaces.iter().any(|s| s.id == space) => {
                self.show_requested(&space, pane.as_deref());
            }
            // A split's new pane: focused where it is, when this window shows its space.
            (None, Some(home)) => {
                if self.space_id.as_deref() == Some(home.as_str()) {
                    self.focus_pane(pane.as_deref().unwrap_or_default());
                }
            }
            (space, _) => self.pending_focus = Some((space, pane, at)),
        }
    }
    /// wheeld's answers to what the header sent: a split's new pane takes the focus.
    pub fn on_pane_reply(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::Created { req, id, .. } if self.split_reqs.remove(req) => {
                self.pending_focus = Some((None, Some(id.clone()), super::settings::now_ms()));
                self.retry_pending_focus();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::{Record, Registry};

    fn session(id: &str, spaces: &[&str]) -> Session {
        Session { id: id.into(), name: id.into(), spaces: spaces.iter().map(|s| (*s).into()).collect() }
    }
    fn window(id: &str, session: &str) -> Record {
        Record { id: id.into(), session: Some(session.into()), ..Default::default() }
    }

    #[test]
    fn the_window_showing_the_session_takes_the_focus_request_else_the_key_one() {
        let sessions = [session("a", &["s1"]), session("b", &["s2"]), session("c", &["s3"])];
        let mut r = Registry { windows: vec![window("w1", "a"), window("w2", "b")], focused: Some("w1".into()), ..Default::default() };
        assert_eq!(focus_target(&r, &sessions, "s2").as_deref(), Some("w2"));
        assert_eq!(focus_target(&r, &sessions, "s1").as_deref(), Some("w1"));
        // No window shows session c: the key window shows it.
        assert_eq!(focus_target(&r, &sessions, "s3").as_deref(), Some("w1"));
        r.focused = None;
        assert_eq!(focus_target(&r, &sessions, "s3").as_deref(), Some("w2"));
        r.windows.clear();
        assert_eq!(focus_target(&r, &sessions, "s3"), None);
    }
}
