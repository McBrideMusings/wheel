//! A pane's header (SpacesView.swift's PaneHeader): its buttons, its menu, and the drag that
//! swaps it with another pane or docks it beside one (PaneDrop.swift, AppModel.dragPane and
//! endPaneDrag); and wheeld's `space_focused`, which shows a space and focuses a pane
//! (AppModel.showRequested).
use super::*;
use super::commands::capitalized;
use serde_json::Value;
use crate::grid_drop::{self, GridDrop, Zone};
use crate::pane_header::{self, Control, Placement};
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
/// The side of one header button and the gap between two.
const BUTTON: f32 = 18.0;
const BUTTON_GAP: f32 = 2.0;
/// The header's padding at each end, and the status dot with its gap after the buttons.
const HEADER_PAD: f32 = 8.0;
const DOT: f32 = 14.0;
/// The title's ideal width: a header that would squeeze it below this drops controls instead.
const TITLE_MIN: f32 = 64.0;

/// The width `n` header buttons take.
fn buttons_width(n: usize) -> f32 {
    n as f32 * BUTTON + n.saturating_sub(1) as f32 * BUTTON_GAP
}

/// One button the header draws: a control, or the pane menu (`menu`).
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderButton {
    pub id: &'static str,
    pub symbol: &'static str,
    pub label: &'static str,
    pub enabled: bool,
    pub on: bool,
}

/// How long a focus request or a split's new pane waits for the layout to list it.
const PENDING_FOCUS_MS: u64 = 10_000;

/// The window that answers a `space_focused` for `space` (AppModel.focus(space:)): the first
/// window whose session holds it, else the one key last. None when no window is open.
pub fn focus_target(windows: &crate::windows::Registry, sessions: &[Session], space: &str) -> Option<String> {
    let home = crate::windows::session_holding(sessions, space).map(|s| s.id.as_str());
    windows.windows.iter().find(|w| home.is_some() && w.session.as_deref() == home)
        .or_else(|| windows.focused())
        .map(|w| w.id.clone())
}

impl Client {
    // MARK: The drag inside the space

    /// The drop the pointer at (`x`, `y`) is over in this window's grid. A card floats over the
    /// grid, so over one the drop is on the topmost card's pane there, never the grid under it.
    pub fn grid_drop_at(&self, x: f32, y: f32, pane: &str) -> Option<GridDrop> {
        let layers = self.layers();
        let (grid, cards) = layers.split_first()?;
        if let Some(card) = cards.iter().rev().find(|l| l.card.as_ref().is_some_and(|c| crate::sidebar::contains(c.frame, x, y))) {
            return grid_drop::card_drop_at(x, y, &card.leaves, pane);
        }
        grid_drop::drop_at(x, y, &grid.leaves, pane)
    }
    /// Whether `target` is a pane of `pane`'s space in another of its trees (the grid, a card, the
    /// tray), where the two can dock but never swap.
    fn in_another_tree(&self, pane: &str, target: &str) -> bool {
        let Some(home) = self.space_holding(pane).filter(|s| s.site_of(target).is_some()) else { return false };
        std::iter::once(&home.root).chain(&home.popped).chain(&home.backgrounded).find(|t| t.holds(pane)).is_some_and(|t| !t.holds(target))
    }
    /// Name where the dragged pane docks in its space, as the pointer or the debug route says.
    pub fn hover_grid_drop(&mut self, drop: Option<GridDrop>) {
        let Some(dragged) = self.pane_drag.as_ref().map(|d| d.pane.clone()) else { return };
        // Panes in two trees of one space never swap: the middle docks on the right (AppModel.hoverPaneDrag).
        let drop = drop.map(|d| if d.zone == Zone::Center && self.in_another_tree(&dragged, &d.target) { GridDrop { zone: Zone::Edge(wheel_proto::Dir::Right), ..d } } else { d });
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

    pub(super) fn space_holding(&self, pane: &str) -> Option<&Space> {
        self.spaces.iter().find(|s| s.site_of(pane).is_some())
    }
    pub(super) fn site_of(&self, pane: &str) -> Option<PaneSite> {
        self.spaces.iter().find_map(|s| s.site_of(pane))
    }
    /// Pop Out and Background: put `pane` at `site`, or back in the grid when it is there already
    /// (AppModel.toggleSite).
    pub fn toggle_site(&mut self, pane: &str, site: PaneSite) {
        let Some(now) = self.site_of(pane) else { return };
        self.set_site(pane, if now == site { PaneSite::Grid } else { site });
    }
    /// Put `pane` at `site` outright (AppModel.setSite); nothing when it is there already.
    pub fn set_site(&mut self, pane: &str, site: PaneSite) {
        let Some(now) = self.site_of(pane).filter(|now| *now != site) else { return };
        self.log(format!("pane {pane}: {} -> {}", now.name(), site.name()));
        let pane = pane.to_owned();
        self.send_layout(|req| ClientMsg::PaneSetSite { req: Some(req), pane, site });
    }

    // MARK: The header's menu

    /// What the pane menu of a header at `level` offers (PaneMenuItems over AppModel.headerMenu):
    /// first each control the header dropped at that level or the person moved to the menu, then
    /// the four splits, every space it can move to under its session's name, the kinds it can
    /// change to, a new space, a new little window (none until wheel-ynjo.7.5, so disabled), and
    /// Close. A right-click asks at level 0; Split Right, Split Down and Close set to Off are left
    /// out.
    pub fn header_menu(&self, pane: &str, level: u8) -> Vec<HeaderMenuItem> {
        let item = |id: String, title: String, enabled: bool| HeaderMenuItem { id, title, enabled };
        let Some(space) = self.space_holding(pane) else { return vec![] };
        let home = Some(space.id.clone());
        let Some(leaf) = std::iter::once(&space.root).chain(&space.popped).chain(&space.backgrounded).flat_map(leaves_of).find(|l| l.id == pane).cloned() else { return vec![] };
        let kind = pane_header::kind_name(&leaf.kind);
        let known = kind != "unknown";
        let store = self.header_layout.borrow();
        let layout = &store.layout;
        let mut items = vec![];
        for control in [Control::Conversations, Control::Pop, Control::Background, Control::Zoom] {
            if !layout.in_menu(control, kind, level) {
                continue;
            }
            match control {
                Control::Conversations if leaf.agent.is_some() => items.push(item("conversations".into(), "Conversations…".into(), true)),
                Control::Pop => items.push(item("pop".into(), "Pop out".into(), true)),
                Control::Background => items.push(item("background".into(), "Background".into(), true)),
                Control::Zoom => {
                    let (enabled, zoomed) = self.zoom_button(pane);
                    items.push(item("zoom".into(), if zoomed { "Unzoom Pane (⇧⌘↩)" } else { "Zoom Pane (⇧⌘↩)" }.into(), enabled));
                }
                _ => {}
            }
        }
        if known {
            for word in ["right", "left", "down", "up"] {
                let off = match word {
                    "right" => layout.is_off(Control::SplitRight, kind),
                    "down" => layout.is_off(Control::SplitDown, kind),
                    _ => false,
                };
                if !off {
                    items.push(item(format!("split-{word}"), format!("Split {}", capitalized(word)), true));
                }
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
        items.push(item("move-new-space".into(), "Move Pane to New Space".into(), true));
        items.push(item("move-new-little".into(), "Move Pane to New Little Window".into(), false));
        if !layout.is_off(Control::Close, kind) {
            items.push(item("close".into(), "Close Pane".into(), true));
        }
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
            "zoom" => self.toggle_zoom(&pane),
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
    /// Run `id` when the pane's header offers it enabled, as a button or in the menu open for it
    /// (the right-click one when none is); false when it does not.
    pub fn choose_header_menu(&mut self, id: &str, pane: &str) -> bool {
        let level = self.pane_menu.as_ref().filter(|m| m.0 == pane).map_or(0, |m| m.3);
        let button = id != "menu" && self.header_buttons_at(pane, self.header_level(pane)).iter().any(|b| b.id == id && b.enabled);
        let offered = button || self.header_menu(pane, level).iter().any(|i| i.id == id && i.enabled);
        if !offered {
            self.log(format!("pane menu {pane}: {id} not available"));
            return false;
        }
        self.log(format!("pane menu {pane}: {id}"));
        self.run_header_menu(id, pane);
        true
    }
    /// Open the menu of a header at `level` (0 for a right-click, the header's own for ⋯).
    pub fn open_pane_menu(&mut self, pane: &str, x: f32, y: f32, level: u8) {
        self.close_space_menu();
        self.message_menu = None;
        let h = self.pane_menu_height(pane, level);
        let x = x.min(self.viewport.0 - MENU_W - 4.).max(0.);
        let y = y.min(self.viewport.1 - h - 4.).max(0.);
        self.pane_menu = Some((pane.to_owned(), x, y, level));
        self.sync_sidebar_overlay();
    }
    pub fn close_pane_menu(&mut self) {
        if self.pane_menu.take().is_some() {
            self.sync_sidebar_overlay();
        }
    }
    pub(super) fn pane_menu_height(&self, pane: &str, level: u8) -> f32 {
        self.header_menu(pane, level).len() as f32 * MENU_ROW_H + 8.
    }
    /// The pane menu, anchored at the press that opened it.
    pub fn pane_menu_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let (pane, x, y, level) = self.pane_menu.clone()?;
        let mut list = div().absolute().left(px(x)).top(px(y)).w(px(MENU_W)).py(px(4.)).rounded_md().bg(rgb(0x22302a)).border_1().border_color(rgb(0x3d5247)).flex().flex_col().text_xs()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for item in self.header_menu(&pane, level) {
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

    /// The level `pane`'s header falls to for its width (PaneHeader.levels): the first whose
    /// buttons leave the title its ideal width.
    pub fn header_level(&self, pane: &str) -> u8 {
        let (leaves, _) = self.scene();
        leaves.iter().find(|p| p.leaf.id == pane).map_or(0, |p| self.header_level_for(&p.leaf.kind, p.rect.w))
    }
    /// The level of a header `width` points wide over a pane of `kind`.
    pub(super) fn header_level_for(&self, kind: &str, width: f32) -> u8 {
        self.header_layout.borrow().layout.level(pane_header::kind_name(kind), width - 2. * HEADER_PAD - DOT, TITLE_MIN, buttons_width)
    }
    /// The buttons `pane`'s header draws at `level`, left to right: the controls its kind's layout
    /// puts in the bar, the pane menu, then Close.
    pub fn header_buttons_at(&self, pane: &str, level: u8) -> Vec<HeaderButton> {
        let Some((kind, has_agent)) = self.space_holding(pane).and_then(|s| s.shown().find(|l| l.id == pane)).map(|l| (pane_header::kind_name(&l.kind), l.agent.is_some())) else { return vec![] };
        let (zoom_enabled, zoomed) = self.zoom_button(pane);
        let popped = self.site_of(pane) == Some(PaneSite::Popped);
        let mut out = vec![];
        for control in self.header_layout.borrow().layout.bar(kind, level) {
            if control == Control::Close {
                out.push(HeaderButton { id: "menu", symbol: "ellipsis", label: "Pane menu", enabled: true, on: false });
            }
            out.push(match control {
                Control::Conversations => HeaderButton { id: control.id(), symbol: control.symbol(), label: "Conversations", enabled: has_agent, on: false },
                Control::SplitRight => HeaderButton { id: control.id(), symbol: control.symbol(), label: "Split Right", enabled: true, on: false },
                Control::SplitDown => HeaderButton { id: control.id(), symbol: control.symbol(), label: "Split Down", enabled: true, on: false },
                Control::Pop => HeaderButton { id: control.id(), symbol: control.symbol(), label: "Pop out", enabled: true, on: popped },
                Control::Background => HeaderButton { id: control.id(), symbol: control.symbol(), label: "Background", enabled: true, on: false },
                Control::Zoom if zoomed => HeaderButton { id: control.id(), symbol: "arrow.down.right.and.arrow.up.left", label: "Unzoom Pane (⇧⌘↩)", enabled: zoom_enabled, on: true },
                Control::Zoom => HeaderButton { id: control.id(), symbol: control.symbol(), label: "Zoom Pane (⇧⌘↩)", enabled: zoom_enabled, on: false },
                Control::Close => HeaderButton { id: control.id(), symbol: control.symbol(), label: "Close Pane", enabled: true, on: false },
            });
        }
        if !out.iter().any(|b| b.id == "menu") {
            out.push(HeaderButton { id: "menu", symbol: "ellipsis", label: "Pane menu", enabled: true, on: false });
        }
        out
    }
    /// The buttons on `pane`'s header, right of its title; each press stops before the header's
    /// own press, so a click never starts a drag.
    pub(super) fn header_buttons(&self, pane: &str, level: u8, cx: &mut Context<Self>) -> gpui::Div {
        let mut row = div().flex().items_center().gap(px(BUTTON_GAP)).flex_none();
        for button in self.header_buttons_at(pane, level) {
            let colour = if button.on { 0x7fb89a } else if !button.enabled { 0x55665d } else { 0xb7cbc1 };
            let icon = self.symbol(button.symbol, 11., colour).unwrap_or_else(|| div().text_xs().child(button.symbol).into_any_element());
            let (pane, id, enabled) = (pane.to_owned(), button.id, button.enabled);
            row = row.child(div().id(gpui::ElementId::Name(format!("hdr-{id}-{pane}").into())).w(px(BUTTON)).h(px(BUTTON)).rounded_sm().flex().items_center().justify_center().cursor_pointer()
                .hover(|s| s.bg(rgb(0x31483e))).child(icon)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.focus_pane(&pane);
                    if id == "menu" {
                        this.open_pane_menu(&pane, f32::from(e.position.x) - MENU_W + BUTTON, f32::from(e.position.y) + 10., level);
                    } else if enabled {
                        this.log(format!("pane header {pane}: {id}"));
                        this.run_header_menu(id, &pane);
                    }
                    cx.notify();
                })));
        }
        row
    }
    /// Each header button's frame in window points, for `/state`.
    pub fn header_button_frames(&self, pane: &str) -> Option<Value> {
        let (leaves, _) = self.scene();
        let placed = leaves.iter().find(|p| p.leaf.id == pane)?;
        let r = placed.rect;
        // The buttons end at the header's padding and the status dot, before the pane's right edge.
        let buttons = self.header_buttons_at(pane, self.header_level_for(&placed.leaf.kind, r.w));
        let right = r.x + r.w - HEADER_PAD - DOT;
        let left = right - buttons_width(buttons.len());
        Some(Value::Object(buttons.iter().enumerate().map(|(i, b)| {
            let x = left + i as f32 * (BUTTON + BUTTON_GAP);
            (b.id.to_owned(), serde_json::json!({"label": b.label, "frame": [x, r.y + (TITLE - BUTTON) / 2., BUTTON, BUTTON], "enabled": b.enabled, "on": b.on}))
        }).collect()))
    }
    /// Settings › General › Pane header, and its debug route: place `control` for `kind`.
    pub fn set_header_placement(&mut self, control: Control, kind: &str, placement: Placement) {
        self.header_layout.borrow_mut().set(control, kind, placement);
        self.log(format!("pane header: {kind} {} -> {}", control.name(), placement.name()));
        self.close_pane_menu();
    }
    pub fn reset_header_layout(&mut self, kind: &str) {
        self.header_layout.borrow_mut().reset(kind);
        self.log(format!("pane header: {kind} reset"));
        self.close_pane_menu();
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
        if let Some(session) = crate::windows::session_holding(&self.sessions, space).map(|s| s.id.clone()) {
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
