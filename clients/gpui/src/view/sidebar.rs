//! The spaces sidebar as drawn and driven (SpacesSidebar.swift, SpaceMenu.swift, SpaceDrag.swift,
//! SpacePreview.swift): a row per space of the session shown with its pane stack, its agents'
//! marks, its name and its waiting count, a close button; a press shows the space and a press
//! that moves drags it; a right press opens its menu; a pane's header dragged onto a row moves
//! the pane there; a pointer held over a row opens a preview of its grid.
use super::*;
use super::settings::now_ms;
use crate::sidebar::{self, DropLayer, DropPopover, DropWindow, MenuItem, PaneAway, PaneDrag, PaneTarget, Rename, Row, SpaceDrag, SpaceDropTarget};
use serde_json::json;
use std::path::Path;

/// What a space drag is over, as every window draws it: the window it is over draws the bar and
/// the outline, whichever window the drag started in (SpaceDrag.swift's state). The chip of a
/// measured pointer drag is a window of its own (float.rs); a drag the pointer does not drive
/// draws it in the window the chip names.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceHover {
    pub space: String,
    /// The window the drag started in, which owns this.
    pub source: String,
    pub target: Option<SpaceDropTarget>,
    /// The window the pointer is in and where in it, for the chip.
    pub chip: Option<(String, (f32, f32))>,
    /// The chip and the card follow the pointer in windows of their own.
    pub floating: bool,
}

thread_local! {
    static HOVER: RefCell<Option<SpaceHover>> = const { RefCell::new(None) };
    /// Where a pane drag in any window aims, and the window it started in.
    static PANE_HOVER: RefCell<Option<(String, PaneAway)>> = const { RefCell::new(None) };
}

/// Where the pane drag in progress in any window aims.
pub fn pane_hover() -> Option<PaneAway> {
    PANE_HOVER.with(|h| h.borrow().as_ref().map(|(_, a)| a.clone()))
}

/// A pane drag's state outside its own grid (PaneMove.swift's paneHold*, paneDragShown,
/// pendingNewSpace).
#[derive(Default)]
pub struct PaneOut {
    /// Every window and the popover, measured when the pointer drag started and again after a
    /// hold switched a window.
    pub world: Option<DragWorld>,
    /// The tab or row the pointer is over (window, space), and the one whose timer runs.
    want: Option<(String, String)>,
    hold: Option<(String, String)>,
    hold_gen: u64,
    /// Each window a hold switched and the space it showed before.
    switched: Vec<(String, String)>,
    /// A cancel waits for the next turn to put the windows back.
    restore: bool,
    pending: Option<PendingNewSpace>,
}

/// A new space `pane_move` is about to make and where it goes once wheeld reports it: the first
/// space other than `home` that holds `pane`, within `PENDING_MS`.
#[derive(Clone, Debug)]
struct PendingNewSpace {
    pane: String,
    home: Option<String>,
    /// The window whose session takes it.
    window: String,
    /// It opens in a little window, grown from a card centred on this screen point when there is one.
    little: Option<Option<(f32, f32)>>,
    at: u64,
}
const PENDING_MS: u64 = 10_000;

/// The space drag in progress in any window, if one is.
pub fn space_hover() -> Option<SpaceHover> {
    HOVER.with(|h| h.borrow().clone())
}

/// The windows and the popover as a space drag meets them, measured when it starts (SpaceDrag's
/// frozen frames): screen points from the main screen's top left.
#[derive(Clone, Debug, PartialEq)]
pub struct DragWorld {
    /// This window's content origin on screen: a pointer at (x, y) here is at origin + (x, y).
    pub origin: (f32, f32),
    /// The main windows front to back.
    pub windows: Vec<DropWindow>,
    pub popover: Option<DropPopover>,
}

/// The insertion bar's colour, in the sidebar and among the tabs.
pub(super) const ACCENT: u32 = 0x7fb89a;
const ROW_BG_ACTIVE: u32 = 0x31483e;
const CARD_BG: u32 = 0x17201c;
const CARD_BORDER: u32 = 0x2c3d34;
const DIM: u32 = 0x8fa89b;
const RED: u32 = 0xe07070;

impl Client {
    /// The sidebar's rectangle: under the title row, down the left edge, over the bottom row.
    pub fn sidebar_rect(&self) -> Rect {
        Rect { x: header::MARGIN, y: HEADER, w: sidebar::WIDTH, h: (self.viewport.1 - HEADER - BOTTOM).max(0.) }
    }
    /// The rows of the session shown, as drawn.
    pub fn sidebar_rows(&self) -> Vec<Row> {
        sidebar::rows(&self.session_ids(), self.sidebar_rect())
    }
    /// Where a space's row or chip is, for a check that presses it.
    pub fn space_hit_rect(&self, space: &str) -> Option<Rect> {
        if self.sidebar_open {
            self.sidebar_rows().into_iter().find(|r| r.id == space).map(|r| r.rect)
        } else {
            self.header_chips().into_iter().find(|c| c.hit == Hit::Space(space.to_owned())).map(|c| c.rect)
        }
    }
    /// The area the grid is placed in: right of the sidebar while it is open.
    pub fn grid_area(&self) -> Rect {
        if self.is_little() {
            return self.little_grid_area();
        }
        let left = if self.sidebar_open { header::MARGIN + sidebar::WIDTH + sidebar::GAP } else { 0. };
        Rect { x: left, y: HEADER, w: (self.viewport.0 - left).max(0.), h: (self.viewport.1 - HEADER - BOTTOM).max(0.) }
    }
    pub fn set_sidebar(&mut self, open: bool) {
        if self.sidebar_open != open {
            self.sidebar_open = open;
            self.log(format!("sidebar: {}", if open { "shown" } else { "hidden" }));
            self.cancel_space_drag(false);
            self.cancel_pane_drag();
            self.close_space_menu();
            self.close_preview();
        }
    }
    pub fn toggle_sidebar(&mut self) {
        self.set_sidebar(!self.sidebar_open);
    }
    pub fn space_name(&self, id: &str) -> Option<String> {
        self.spaces.iter().find(|s| s.id == id).map(|s| s.name.clone())
    }
    fn session_ids(&self) -> Vec<String> {
        self.session_spaces().iter().map(|s| s.id.clone()).collect()
    }

    // MARK: Presses and drags

    /// A press on a space's row: the space shows at once, as a sidebar selects, and a press that
    /// then moves `DRAG_DISTANCE` drags it.
    pub fn space_press_down(&mut self, space: &str, x: f32, y: f32) {
        if self.space_drag.is_some() {
            return;
        }
        self.press(format!("space row {space} at ({x:.0},{y:.0})"));
        self.close_space_menu();
        if self.rename.as_ref().is_some_and(|r| r.space != space) {
            self.cancel_rename();
        }
        self.select_space(space);
        self.space_press = Some((space.to_owned(), x, y));
    }
    /// A press on a pane's header, which may become a pane drag.
    pub fn pane_press_down(&mut self, pane: &str, label: &str, x: f32, y: f32, grab: (f32, f32)) {
        if self.pane_drag.is_none() {
            self.pane_press = Some((pane.to_owned(), label.to_owned(), x, y, grab));
        }
    }
    /// The pointer moved to (`x`, `y`): a held press becomes a drag, a drag finds its target, and
    /// the row under the pointer is the one the preview follows.
    pub fn pointer_moved(&mut self, x: f32, y: f32, pressed: bool) {
        // A release GPUI never saw (a native view took it) leaves no press armed.
        if !pressed {
            self.space_press = None;
            self.pane_press = None;
        }
        if let Some((space, px0, py0)) = self.space_press.clone()
            && self.space_drag.is_none()
            && ((x - px0).powi(2) + (y - py0).powi(2)).sqrt() >= sidebar::DRAG_DISTANCE
        {
            self.begin_space_drag(&space, true, (x, y));
        }
        if let Some((pane, label, px0, py0, grab)) = self.pane_press.clone()
            && self.pane_drag.is_none()
            && ((x - px0).powi(2) + (y - py0).powi(2)).sqrt() >= sidebar::DRAG_DISTANCE
        {
            self.begin_pane_drag(&pane, &label, (x, y), grab, true);
        }
        if self.space_drag.is_some() {
            self.move_space_drag(x, y);
        }
        if self.pane_drag.is_some() {
            self.move_pane_drag(x, y);
        }
        let over = if self.sidebar_open && self.space_drag.is_none() && self.pane_drag.is_none() && self.space_menu.is_none() && self.rename.is_none() {
            sidebar::row_at(&self.sidebar_rows(), x, y).map(|r| r.id.clone())
        } else {
            None
        };
        self.row_hover(over);
    }
    /// The button came up: a drag drops, a press that never moved is over.
    pub fn pointer_up(&mut self, cx: &mut App) {
        self.space_press = None;
        self.pane_press = None;
        if self.space_drag.as_ref().is_some_and(|d| d.by_pointer) {
            self.end_space_drag(cx);
        }
        if self.pane_drag.is_some() {
            self.end_pane_drag(cx);
        }
    }
    pub fn begin_space_drag(&mut self, space: &str, by_pointer: bool, at: (f32, f32)) {
        if self.space_drag.is_some() || !self.spaces.iter().any(|s| s.id == space) {
            return;
        }
        self.close_preview();
        self.space_drag = Some(SpaceDrag { space: space.to_owned(), by_pointer, target: None, at, screen: None, chip: Some((self.window_id.clone(), at)) });
        self.space_drag_world = None;
        self.log(format!("space drag {space}: started"));
        self.publish_space_hover();
    }
    /// Measure every main window and the popover for the drag that just started: this window,
    /// each other main window front to back (the bridge's order; windows off screen after them,
    /// newest first), and the popover's cards. Little windows are never targets.
    pub(super) fn measure_space_drag(&mut self, cx: &App) {
        if self.space_drag.is_none() || self.space_drag_world.is_some() {
            return;
        }
        let world = self.measure_world(cx);
        self.log(format!("space drag: {} windows, popover {}, this window at {:.0},{:.0}", world.windows.iter().filter(|w| !w.little).count(), if world.popover.is_some() { "open" } else { "closed" }, world.origin.0, world.origin.1));
        self.space_drag_world = Some(world);
    }
    /// Every window front to back, little ones included, and the popover, as a drag meets them.
    fn measure_world(&self, cx: &App) -> DragWorld {
        let me = self.window_id.clone();
        let frame_of = |native: &Native| -> Option<((f32, f32), i64, bool)> {
            let answer = native.call(json!({"op": "frame"}));
            let f: Vec<f32> = answer["frame"].as_array()?.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect();
            Some(((*f.first()?, *f.get(1)?), answer["order"].as_i64().unwrap_or(-1), answer["visible"] == true))
        };
        let drop_window = |c: &Client, origin: (f32, f32)| {
            let mut w = c.drop_window(origin);
            w.frame = Rect { x: origin.0, y: origin.1, w: c.viewport.0, h: c.viewport.1 };
            w
        };
        let newest = |id: &str| id.rsplit('-').next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
        // A hidden app's windows are never on screen; the check's windows count all the same.
        let counts = |visible: bool| visible || self.hidden;
        let (origin, order, _) = frame_of(&self.native).unwrap_or(((0., 0.), -1, true));
        let mut found: Vec<(i64, u64, DropWindow)> = vec![(order, newest(&me), drop_window(self, origin))];
        for (id, handle) in crate::window_handles() {
            if id == me {
                continue;
            }
            let Ok(c) = handle.read(cx) else { continue };
            if let Some((at, order, visible)) = frame_of(&c.native)
                && counts(visible)
            {
                found.push((order, newest(&id), drop_window(c, at)));
            }
        }
        found.sort_by_key(|(order, n, _)| if *order >= 0 { (0, *order as u64) } else { (1, u64::MAX - n) });
        let popover = crate::view::menu_bar().and_then(|m| m.read(cx).drop_popover(cx));
        DragWorld { origin, windows: found.into_iter().map(|(_, _, w)| w).collect(), popover }
    }
    /// This window as a drop meets it, its content's top left at `origin` on screen: its sidebar
    /// and rows while it shows them, else its tab strip and every tab, and its grid's panes
    /// (DropWindow). A little window is a frame only.
    fn drop_window(&self, origin: (f32, f32)) -> DropWindow {
        let at = |r: Rect| Rect { x: r.x + origin.0, y: r.y + origin.1, w: r.w, h: r.h };
        if self.is_little() {
            return DropWindow { id: self.window_id.clone(), frame: Rect { x: origin.0, y: origin.1, w: f32::MAX, h: f32::MAX }, sidebar: None, rows: vec![], tab_strip: None, tabs: vec![], little: true, spaces: vec![], shown: self.space_id.clone(), layers: vec![] };
        }
        let row = (!self.sidebar_open).then(|| self.title_row());
        let layers = self.layers().into_iter().map(|l| DropLayer { card: l.card.map(|c| at(c.frame)), panes: l.leaves.iter().map(|p| (p.leaf.id.clone(), at(p.rect))).collect() }).collect();
        DropWindow {
            little: false,
            spaces: self.session_ids(),
            shown: self.space_id.clone().filter(|_| !self.space_away()),
            layers,
            id: self.window_id.clone(),
            frame: Rect { x: origin.0, y: origin.1, w: f32::MAX, h: f32::MAX },
            sidebar: self.sidebar_open.then(|| at(self.sidebar_rect())),
            rows: if self.sidebar_open { self.sidebar_rows().into_iter().map(|r| at(r.rect)).collect() } else { vec![] },
            tab_strip: row.as_ref().and_then(|r| r.track).map(at),
            tabs: row.map(|r| r.tabs.into_iter().map(at).collect()).unwrap_or_default(),
        }
    }
    pub fn move_space_drag(&mut self, x: f32, y: f32) {
        let me = self.window_id.clone();
        let (target, screen, chip) = match &self.space_drag_world {
            Some(world) => {
                let (gx, gy) = (world.origin.0 + x, world.origin.1 + y);
                let under = world.windows.iter().find(|w| sidebar::contains(w.frame, gx, gy)).map(|w| (w.id.clone(), (gx - w.frame.x, gy - w.frame.y)));
                (sidebar::space_target(gx, gy, &world.windows, world.popover.as_ref()), Some((gx, gy)), under)
            }
            // Measured nowhere yet (the debug route): this window is the only one.
            None => {
                let mine = self.drop_window((0., 0.));
                (sidebar::space_target(x, y, &[mine], None), None, Some((me, (x, y))))
            }
        };
        if let Some(drag) = &mut self.space_drag {
            drag.at = (x, y);
            drag.screen = screen;
            drag.chip = chip;
        }
        self.hover_space_drag(Some(target));
    }
    /// Name the target directly, as the debug route does without a pointer.
    pub fn hover_space_drag(&mut self, target: Option<SpaceDropTarget>) {
        let Some(drag) = &mut self.space_drag else { return };
        if drag.target != target {
            drag.target = target;
            let (space, words) = (drag.space.clone(), drag.target.as_ref().map(describe_target).unwrap_or_else(|| "nothing".into()));
            self.log(format!("space drag {space}: over {words}"));
        }
        self.publish_space_hover();
    }
    /// Write this window's drag where every window reads it; the next draw redraws the others.
    fn publish_space_hover(&mut self) {
        let me = self.window_id.clone();
        let next = self.space_drag.as_ref().map(|d| SpaceHover { space: d.space.clone(), source: me.clone(), target: d.target.clone(), chip: d.chip.clone(), floating: d.by_pointer && d.screen.is_some() });
        let changed = HOVER.with(|h| {
            let mut h = h.borrow_mut();
            // Another window's drag is that window's to end.
            if next.is_none() && h.as_ref().is_some_and(|held| held.source != me) {
                return false;
            }
            let changed = *h != next;
            *h = next;
            changed
        });
        self.space_hover_changed |= changed;
    }
    /// The other windows redraw after this one's drag moved over or off them.
    pub(super) fn refresh_for_space_hover(&mut self, cx: &mut App) {
        self.settle_pane_out(cx);
        if std::mem::take(&mut self.space_hover_changed) {
            crate::refresh_other_windows(cx, &self.window_id);
        }
        let want = self.float_want();
        super::float::set(&self.window_id, want, &self.bridge, self.hidden, cx);
    }
    /// What this window's drag draws outside its windows: over the desktop the space's preview
    /// card, elsewhere the chip, once the pointer drives it and the windows are measured.
    fn float_want(&self) -> super::float::Want {
        use super::float::Want;
        if let Some(d) = self.pane_drag.as_ref().filter(|d| d.by_pointer)
            && let Some(at) = d.screen
        {
            let name = if d.away == Some(PaneAway::Desktop) { format!("{} → new little window", d.label) } else { d.label.clone() };
            return Want::Chip { name, at };
        }
        let Some(drag) = self.space_drag.as_ref().filter(|d| d.by_pointer) else { return Want::Nothing };
        let Some(at) = drag.screen else { return Want::Nothing };
        let name = self.space_name(&drag.space).unwrap_or_else(|| drag.space.clone());
        if drag.target == Some(SpaceDropTarget::Desktop) {
            let (mut leaves, mut dividers) = (vec![], vec![]);
            if let Some(info) = self.spaces.iter().find(|s| s.id == drag.space) {
                place(&info.root, super::float::card_grid(), &HashMap::new(), &mut leaves, &mut dividers);
            }
            Want::Card { name, panes: leaves.into_iter().map(|p| p.rect).collect(), at }
        } else {
            Want::Chip { name, at }
        }
    }
    /// Release: put the space where the pointer is. The layout changes once wheeld answers.
    pub fn end_space_drag(&mut self, cx: &mut App) {
        let Some(drag) = self.space_drag.take() else { return };
        self.space_press = None;
        self.space_drag_world = None;
        self.publish_space_hover();
        self.refresh_for_space_hover(cx);
        let Some(target) = drag.target else {
            self.log(format!("space drag {}: released over nothing", drag.space));
            return;
        };
        self.log(format!("space drag {}: dropped on {}", drag.space, describe_target(&target)));
        let space = drag.space;
        match target {
            SpaceDropTarget::Sidebar { window, index } | SpaceDropTarget::Tabs { window, index } => self.place_dragged(&space, &window, Some(index), cx),
            SpaceDropTarget::Window(window) => self.place_dragged(&space, &window, None, cx),
            SpaceDropTarget::Card { session, index } => {
                let held = self.sessions.iter().find(|s| s.id == session).map(|s| s.spaces.clone()).unwrap_or_default();
                if let Some(index) = sidebar::drop_move(Some(index), &held, &space) {
                    self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space, session, index });
                }
            }
            SpaceDropTarget::Desktop => self.drop_on_desktop(&space, drag.screen, cx),
        }
    }
    /// A drop on `window`'s sidebar at `index`, or on its body (the end): the space moves into the
    /// session that window shows, and shows there once wheeld's layout holds it (AppModel.place(_:in:at:)).
    fn place_dragged(&mut self, space: &str, window: &str, index: Option<usize>, cx: &mut App) {
        let other = if window == self.window_id {
            None
        } else {
            // A window that closed since the drag measured it takes nothing.
            let Some(handle) = crate::window_handle(window) else {
                self.log(format!("space drag {space}: window {window} closed during the drag"));
                return;
            };
            Some(handle)
        };
        let session = match other {
            Some(handle) => handle.read(cx).ok().and_then(|c| c.session_id.clone()),
            None => self.session_id.clone(),
        };
        let Some(session) = session else {
            self.log(format!("space drag {space}: window {window} shows no session"));
            return;
        };
        let held = self.sessions.iter().find(|s| s.id == session).map(|s| s.spaces.clone()).unwrap_or_default();
        if let Some(index) = sidebar::drop_move(index, &held, space) {
            let space = space.to_owned();
            self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space, session, index });
        }
        if let Some(handle) = other {
            let space = space.to_owned();
            let _ = handle.update(cx, |c, _, cx| {
                c.pending_show = Some((space, settings::now_ms()));
                c.show_placed();
                cx.notify();
            });
            crate::bring_front(cx, handle);
        }
    }
    /// A drop on no window: the space opens in a little window centred on the drop, or moves its
    /// open one there; a space no session holds opens in a window of its own (endSpaceDrag's `.desktop`).
    fn drop_on_desktop(&mut self, space: &str, at: Option<(f32, f32)>, cx: &mut App) {
        let Some(id) = self.open_little(space, cx) else {
            let holder = crate::windows::session_holding(&self.sessions, space).map(|s| s.id.clone());
            match holder {
                Some(session) => match crate::new_window(cx, &self.bridge, self.endpoint.clone(), self.hidden, Some(session), Some(space.to_owned()), None) {
                    Ok(_) => self.log(format!("space drag {space}: torn off into a window of its own")),
                    Err(e) => self.log(format!("space drag {space}: no window opened: {e:#}")),
                },
                None => self.log(format!("space drag {space}: no session holds it; nothing opened")),
            }
            return;
        };
        if let Some(at) = at {
            self.grow_little(&id, at, cx);
        }
    }
    /// Little window `id` grows from the preview card centred on (`x`, `y`) on screen, to its
    /// place centred on the card (openLittle(for:from:)).
    fn grow_little(&mut self, id: &str, (x, y): (f32, f32), cx: &mut App) {
        let Some(handle) = crate::window_handle(id) else { return };
        let card = sidebar::card_frame(x, y);
        let (ox, oy) = sidebar::little_origin(card.x + card.w / 2., card.y + card.h / 2., super::little::SIZE);
        let moved = handle.read(cx).map(|c| c.native.call(json!({"op": "grow_window", "from": [card.x, card.y, card.w, card.h], "point": [ox, oy]}))).unwrap_or_default();
        self.log(format!("little window {id} grown from the card at {:.0},{:.0} to {ox:.0},{oy:.0} for the drop at {x:.0},{y:.0}: {moved}", card.x, card.y));
    }
    pub fn cancel_space_drag(&mut self, escape: bool) {
        self.space_press = None;
        self.space_drag_world = None;
        if let Some(drag) = self.space_drag.take() {
            self.log(format!("space drag {}: cancelled{}", drag.space, if escape { " by escape" } else { "" }));
        }
        self.publish_space_hover();
    }
    /// The insertion bar's place in this window's sidebar while a drag in any window aims at it:
    /// none beside the dragged row itself.
    pub fn space_drag_insertion(&self) -> Option<usize> {
        let hover = space_hover()?;
        match hover.target? {
            SpaceDropTarget::Sidebar { window, index } if window == self.window_id => sidebar::bar_index(index, &self.session_ids(), &hover.space),
            _ => None,
        }
    }
    /// The insertion bar's place among this window's tabs while a drag in any window aims at
    /// them: none beside the dragged tab itself.
    pub fn space_drag_tab_bar(&self) -> Option<usize> {
        let hover = space_hover()?;
        match hover.target? {
            SpaceDropTarget::Tabs { window, index } if window == self.window_id => sidebar::bar_index(index, &self.session_ids(), &hover.space),
            _ => None,
        }
    }
    /// A drag in any window aims at this window's body: the window is outlined.
    pub fn space_drag_outlined(&self) -> bool {
        space_hover().and_then(|h| h.target).is_some_and(|t| t == SpaceDropTarget::Window(self.window_id.clone()))
    }
    pub fn space_drag_hint(&self) -> Option<String> {
        let drag = self.space_drag.as_ref()?;
        let name = self.space_name(&drag.space).unwrap_or_else(|| drag.space.clone());
        let session_name = |id: &str| self.sessions.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_else(|| id.to_owned());
        let window_title = |id: &str| crate::windows::with(|r| r.get(id).map(|w| crate::windows::title(w, &self.spaces))).unwrap_or_else(|| "Wheel".into());
        Some(sidebar::space_hint(drag.target.as_ref()?, &name, session_name, window_title))
    }
    /// While a space is held, every pointer move and the release reach this window wherever the
    /// pointer is, outside the window too, and nothing under the pointer hears them meanwhile.
    pub(super) fn space_drag_capture(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        (self.space_drag.as_ref().is_some_and(|d| d.by_pointer) || self.pane_drag.as_ref().is_some_and(|d| d.by_pointer)).then_some(())?;
        let (moves, ups) = (cx.entity().downgrade(), cx.entity().downgrade());
        Some(canvas(|_, _, _| {}, move |_, _, window, _| {
            let moves = moves.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Capture {
                    let _ = moves.update(cx, |this, cx| this.window_mouse_move(e, cx));
                    cx.stop_propagation();
                }
            });
            let ups = ups.clone();
            window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Capture && e.button == MouseButton::Left {
                    let _ = ups.update(cx, |this, cx| this.window_mouse_up(e, cx));
                    cx.stop_propagation();
                }
            });
        }).absolute().size_0())
    }

    pub fn begin_pane_drag(&mut self, pane: &str, label: &str, at: (f32, f32), grab: (f32, f32), by_pointer: bool) {
        if self.pane_drag.is_some() {
            return;
        }
        self.close_preview();
        self.pane_drag = Some(PaneDrag { pane: pane.to_owned(), label: label.to_owned(), away: None, drop: None, at, grab, by_pointer, screen: None });
        self.pane_out.world = None;
        // A new drag supersedes a new space an earlier drop still waits for.
        self.pane_out.pending = None;
        self.log(format!("pane drag {pane}: started"));
    }
    /// Measure every window for the pointer drag that just started, or again after a hold
    /// switched one.
    pub(super) fn measure_pane_drag(&mut self, cx: &App) {
        if !self.pane_drag.as_ref().is_some_and(|d| d.by_pointer) {
            return;
        }
        let world = self.measure_world(cx);
        self.log(format!("pane drag: {} windows, popover {}, this window at {:.0},{:.0}", world.windows.iter().filter(|w| !w.little).count(), if world.popover.is_some() { "open" } else { "closed" }, world.origin.0, world.origin.1));
        self.pane_out.world = Some(world);
    }
    /// The pointer at (`x`, `y`) in this window: what is under it in any window, or the desktop
    /// (AppModel.movePaneDrag). Over this window's grid showing the pane's space, its drop zones.
    pub fn move_pane_drag(&mut self, x: f32, y: f32) {
        let Some(pane) = self.pane_drag.as_ref().map(|d| d.pane.clone()) else { return };
        let home = self.space_holding(&pane).map(|s| s.id.clone());
        let (target, screen) = match &self.pane_out.world {
            Some(world) => {
                let (gx, gy) = (world.origin.0 + x, world.origin.1 + y);
                (sidebar::pane_target(gx, gy, &world.windows, world.popover.as_ref(), &pane, home.as_deref()), Some((gx, gy)))
            }
            // Measured nowhere yet: this window is the only one, and past it is nothing.
            None => match sidebar::pane_target(x, y, &[self.drop_window((0., 0.))], None, &pane, home.as_deref()) {
                PaneTarget::Away(PaneAway::Desktop, _) => (PaneTarget::Nothing, None),
                t => (t, None),
            },
        };
        if let Some(drag) = &mut self.pane_drag {
            drag.at = (x, y);
            drag.screen = screen;
        }
        match target {
            PaneTarget::Away(away, hold) => {
                self.hover_grid_drop(None);
                self.hover_pane_drag(Some(away));
                self.pane_out.want = hold;
            }
            PaneTarget::Home(window) if window == self.window_id => {
                let drop = self.grid_drop_at(x, y, &pane);
                self.hover_pane_drag(None);
                self.hover_grid_drop(drop);
                self.pane_out.want = None;
            }
            _ => {
                self.hover_pane_drag(None);
                self.hover_grid_drop(None);
                self.pane_out.want = None;
            }
        }
    }
    /// Name where the pane lands outside its space directly, as the debug route does: over a
    /// space's end the window whose session holds it is the one a hold switches.
    pub fn name_pane_away(&mut self, away: Option<PaneAway>) {
        self.pane_out.want = match &away {
            Some(PaneAway::End(space)) => crate::windows::with(|r| r.window_holding(space, &self.sessions).map(|w| (w.id.clone(), space.clone()))),
            _ => None,
        };
        self.hover_pane_drag(away);
    }
    pub fn hover_pane_drag(&mut self, away: Option<PaneAway>) {
        let Some(drag) = &mut self.pane_drag else { return };
        if drag.away != away {
            if away.is_some() {
                drag.drop = None;
            }
            drag.away = away;
            let (pane, words) = (drag.pane.clone(), drag.away.as_ref().map(describe_away).unwrap_or_else(|| "nothing".into()));
            self.log(format!("pane drag {pane}: over {words}"));
        }
        self.publish_pane_hover();
    }
    /// Write where this window's pane drag aims where every window reads it.
    pub(super) fn publish_pane_hover(&mut self) {
        let me = self.window_id.clone();
        let next = self.pane_drag.as_ref().and_then(|d| d.away.clone()).map(|a| (me.clone(), a));
        let changed = PANE_HOVER.with(|h| {
            let mut h = h.borrow_mut();
            if next.is_none() && h.as_ref().is_some_and(|(source, _)| *source != me) {
                return false;
            }
            let changed = *h != next;
            *h = next;
            changed
        });
        self.space_hover_changed |= changed;
    }
    /// The tab or row under the pointer changed: holding over it `HOLD_SECONDS` shows its space in
    /// its window, unless that window shows it already (AppModel.armHold).
    pub(super) fn arm_pane_hold(&mut self, cx: &mut Context<Self>) {
        if self.pane_out.want == self.pane_out.hold {
            return;
        }
        self.pane_out.hold = self.pane_out.want.clone();
        self.pane_out.hold_gen += 1;
        let Some((window, space)) = self.pane_out.hold.clone() else { return };
        if self.shown_in(&window, cx).as_deref() == Some(space.as_str()) {
            return;
        }
        let generation = self.pane_out.hold_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs_f64(sidebar::HOLD_SECONDS)).await;
            let _ = this.update(cx, |this, cx| this.show_held(generation, cx));
        }).detach();
    }
    /// The space `window` shows.
    fn shown_in(&self, window: &str, cx: &App) -> Option<String> {
        if window == self.window_id {
            return self.space_id.clone();
        }
        crate::window_handle(window).and_then(|h| h.read(cx).ok().and_then(|c| c.space_id().map(str::to_owned)))
    }
    /// `window` shows `space`.
    fn show_in(&mut self, window: &str, space: &str, cx: &mut App) {
        if window == self.window_id {
            self.select_space(space);
        } else if let Some(handle) = crate::window_handle(window) {
            let _ = handle.update(cx, |c, _, cx| {
                c.select_space(space);
                cx.notify();
            });
        }
    }
    /// The hold ran its time with the pane still over the tab: the window shows that space
    /// (AppModel.showHeld), and the windows are measured again.
    fn show_held(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.pane_out.hold_gen != generation {
            return;
        }
        let Some((window, space)) = self.pane_out.hold.clone() else { return };
        if !matches!(self.pane_drag.as_ref().and_then(|d| d.away.as_ref()), Some(PaneAway::End(s)) if *s == space) {
            return;
        }
        let Some(was) = self.shown_in(&window, cx).filter(|w| *w != space) else { return };
        if !self.pane_out.switched.iter().any(|(w, _)| *w == window) {
            self.pane_out.switched.push((window.clone(), was));
        }
        self.log(format!("pane drag: held over space {space} for {} s; window {window} shows it", sidebar::HOLD_SECONDS));
        self.show_in(&window, &space, cx);
        if self.pane_out.world.is_some() {
            self.measure_pane_drag(cx);
        }
        cx.notify();
    }
    /// The drag is over: stop waiting and, unless a window shows the space the pane landed in,
    /// put back what each window a hold switched showed (AppModel.endAway).
    fn end_away(&mut self, landed: Option<&str>, cx: &mut App) {
        self.pane_out.want = None;
        self.pane_out.hold = None;
        self.pane_out.hold_gen += 1;
        self.pane_out.world = None;
        self.pane_out.restore = false;
        for (window, was) in std::mem::take(&mut self.pane_out.switched) {
            let holds = if window == self.window_id {
                self.session_spaces().iter().any(|s| s.id == was)
            } else {
                crate::window_handle(&window).and_then(|h| h.read(cx).ok().map(|c| c.session_spaces().iter().any(|s| s.id == was))).unwrap_or(false)
            };
            if holds && self.shown_in(&window, cx).as_deref() != landed {
                self.log(format!("pane drag: window {window} shows {was} again"));
                self.show_in(&window, &was, cx);
            }
        }
    }
    /// What a pane drag leaves for a turn with the app at hand: a cancel puts the windows back,
    /// and a new space the drop asked for goes where the drop was once wheeld reports it.
    fn settle_pane_out(&mut self, cx: &mut App) {
        let me = self.window_id.clone();
        if self.pane_drag.is_none() && self.pane_out.restore {
            self.pane_out.restore = false;
            cx.defer(move |cx| if let Some(h) = crate::window_handle(&me) {
                let _ = h.update(cx, |c, _, cx| {
                    c.end_away(None, cx);
                    cx.notify();
                });
            });
            return;
        }
        let Some(pending) = self.pane_out.pending.clone() else { return };
        if settings::now_ms().saturating_sub(pending.at) > PENDING_MS {
            self.log(format!("pane drag {}: wheeld reported no new space for it", pending.pane));
            self.pane_out.pending = None;
            return;
        }
        let Some(space) = self.space_holding(&pending.pane).map(|s| s.id.clone()).filter(|s| Some(s) != pending.home.as_ref()) else { return };
        self.pane_out.pending = None;
        cx.defer(move |cx| if let Some(h) = crate::window_handle(&me) {
            let _ = h.update(cx, |c, _, cx| {
                c.place_new_space(&space, &pending, cx);
                cx.notify();
            });
        });
    }
    /// The new space a drop made shows in its window, moved into that window's session first when
    /// another session holds it, and opens in a little window too for a drop on the desktop
    /// (AppModel.reconcileWindows).
    fn place_new_space(&mut self, space: &str, pending: &PendingNewSpace, cx: &mut App) {
        self.log(format!("pane drag {}: new space {space} for window {}{}", pending.pane, pending.window, if pending.little.is_some() { ", in a little window" } else { "" }));
        if pending.window == self.window_id {
            self.select_space(space);
        } else {
            self.place_dragged(space, &pending.window, None, cx);
        }
        if let Some(at) = pending.little {
            if let Some(id) = self.open_little(space, cx)
                && let Some(at) = at
            {
                self.grow_little(&id, at, cx);
            }
        }
    }
    /// Release: dock beside a pane, move to a space's end or into a new space (AppModel.dropAway).
    pub fn end_pane_drag(&mut self, cx: &mut App) {
        let Some(drag) = self.pane_drag.take() else { return };
        self.pane_press = None;
        self.publish_pane_hover();
        self.sync_sidebar_overlay();
        let pane = drag.pane;
        if let Some(drop) = drag.drop {
            let here = self.space_id.clone();
            self.end_away(here.as_deref(), cx);
            return self.send_grid_drop(pane, drop);
        }
        let Some(away) = drag.away else {
            self.end_away(None, cx);
            self.log(format!("pane drag {pane}: released over nothing"));
            return;
        };
        self.log(format!("pane drag {pane}: dropped on {}", describe_away(&away)));
        let to = match away {
            PaneAway::Beside { pane: target, dir, space } => {
                self.end_away(Some(&space), cx);
                wheel_proto::MoveTarget::Beside { pane: target, dir }
            }
            PaneAway::End(space) => {
                self.end_away(Some(&space), cx);
                wheel_proto::MoveTarget::End { space: space.clone() }
            }
            PaneAway::NewSpace(window) => {
                self.end_away(None, cx);
                self.expect_new_space(&pane, window, None);
                wheel_proto::MoveTarget::NewSpace { name: None, focus: false }
            }
            PaneAway::Desktop => {
                self.end_away(None, cx);
                self.expect_new_space(&pane, self.window_id.clone(), Some(drag.screen));
                wheel_proto::MoveTarget::NewSpace { name: None, focus: false }
            }
        };
        self.send_layout(|req| ClientMsg::PaneMove { req: Some(req), pane, to });
    }
    fn expect_new_space(&mut self, pane: &str, window: String, little: Option<Option<(f32, f32)>>) {
        let home = self.space_holding(pane).map(|s| s.id.clone());
        self.pane_out.pending = Some(PendingNewSpace { pane: pane.to_owned(), home, window, little, at: settings::now_ms() });
    }
    pub fn cancel_pane_drag(&mut self) {
        self.pane_press = None;
        if let Some(drag) = self.pane_drag.take() {
            self.log(format!("pane drag {}: cancelled", drag.pane));
            self.publish_pane_hover();
            // No timer fires after a cancel; the windows a hold switched go back on the next turn.
            self.pane_out.want = None;
            self.pane_out.hold = None;
            self.pane_out.hold_gen += 1;
            self.pane_out.world = None;
            self.pane_out.restore = true;
            self.sync_sidebar_overlay();
        }
    }
    pub fn pane_drag_hint(&self) -> Option<String> {
        let drag = self.pane_drag.as_ref()?;
        if let Some(drop) = &drag.drop {
            return Some(crate::grid_drop::hint(drop, &drag.pane));
        }
        let window_title = |id: &str| crate::windows::with(|r| r.get(id).map(|w| crate::windows::title(w, &self.spaces))).unwrap_or_else(|| "Wheel".into());
        Some(sidebar::pane_hint(drag.away.as_ref()?, &drag.label, |id| self.space_name(id), window_title))
    }
    /// Whether a pane dragged in any window is over `space`'s row or tab, which draws outlined.
    pub(super) fn pane_is_over(&self, space: &str) -> bool {
        matches!(pane_hover(), Some(PaneAway::End(s)) if s == space)
    }
    /// Whether a pane dragged in any window is over this window's sidebar room or tab strip.
    pub(super) fn pane_is_over_new_space(&self) -> bool {
        matches!(pane_hover(), Some(PaneAway::NewSpace(w)) if w == self.window_id)
    }
    /// A pane's name for the chip that follows the pointer (AppModel.paneLabel).
    pub fn pane_label(&self, leaf: &Leaf) -> String {
        let capitalized = |s: &str| { let mut c = s.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() };
        match leaf.kind.as_str() {
            "terminal" => "Terminal".into(),
            "agent" => capitalized(leaf.harness.as_deref().unwrap_or("agent")),
            "web" => leaf.label.clone(),
            "browser" => "Browser".into(),
            "empty" => "Empty pane".into(),
            _ => leaf.id.clone(),
        }
    }

    // MARK: The menu

    pub fn space_menu_items(&self, space: &str) -> Vec<MenuItem> {
        let home = crate::windows::session_holding(&self.sessions, space);
        let sessions: Vec<(String, String)> = self.sessions.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
        sidebar::menu(space, home.map(|s| s.id.as_str()), &sessions, home.map(|s| s.spaces.as_slice()).unwrap_or(&[]), self.little_showing(space).is_some())
    }
    pub fn open_space_menu(&mut self, space: &str, x: f32, y: f32) {
        if !self.spaces.iter().any(|s| s.id == space) {
            return;
        }
        self.close_preview();
        // The menu stays inside the window: it is moved up when it would run off the bottom.
        let h = self.menu_height(space);
        let y = y.min(self.viewport.1 - h - 4.).max(0.);
        self.pane_menu = None;
        self.message_menu = None;
        self.space_menu = Some((space.to_owned(), x, y));
        self.sync_sidebar_overlay();
    }
    /// The menu's height as drawn: its rows, a line per section break, the padding.
    fn menu_height(&self, space: &str) -> f32 {
        let items = self.space_menu_items(space);
        let dividers = items.windows(2).filter(|w| w[0].section != w[1].section).count() as f32;
        items.len() as f32 * sidebar::MENU_ROW_H + dividers * 7. + 8.
    }
    pub fn close_space_menu(&mut self) {
        if self.space_menu.take().is_some() {
            self.sync_sidebar_overlay();
        }
    }
    /// Run a space menu item, as the menu and the debug route do (AppModel.runSpaceMenu).
    pub fn run_space_menu(&mut self, id: &str, space: &str, cx: &mut App) {
        self.close_space_menu();
        let Some(info) = self.spaces.iter().find(|s| s.id == space).cloned() else { return };
        if !self.space_menu_items(space).iter().any(|i| i.id == id && i.enabled) {
            self.log(format!("space menu {space}: {id} not available"));
            return;
        }
        self.log(format!("space menu {space}: {id}"));
        let home = crate::windows::session_holding(&self.sessions, space).cloned();
        let space_id = space.to_owned();
        match id {
            "new-session" => {
                let taken: Vec<&str> = self.sessions.iter().map(|s| s.name.as_str()).collect();
                let name = header::session_name(&taken);
                let req = self.req();
                self.layout_reqs.insert(req);
                self.space_for_new_session = Some((req, space_id));
                self.send(&ClientMsg::SessionNew { req: Some(req), name });
            }
            "left" | "right" => {
                let Some(home) = home else { return };
                let Some(i) = home.spaces.iter().position(|s| s == space) else { return };
                let index = if id == "left" { i - 1 } else { i + 1 };
                self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space: space_id, session: home.id, index: Some(index) });
            }
            "little" => {
                self.open_little(space, cx);
            }
            "rename" => self.begin_rename(space),
            "copy-path" => cx.write_to_clipboard(ClipboardItem::new_string(info.cwd.clone())),
            "reveal" => cx.reveal_path(Path::new(&info.cwd)),
            "new-pane" => {
                // The space's focused pane while it is shown, else its first.
                let first = leaves_of(&info.root).first().map(|l| l.id.clone());
                let pane = self.focus.current.clone().filter(|p| self.space_id.as_deref() == Some(space) && leaves_of(&info.root).iter().any(|l| &l.id == p)).or(first);
                if let Some(pane) = pane {
                    self.send_layout(|req| ClientMsg::PaneSplit { req: Some(req), pane, dir: None, focus: false, site: None });
                }
            }
            "close-others" => self.ask_close_other_spaces(space),
            "close" => self.ask_close_space(space),
            other => {
                if let Some(session) = other.strip_prefix("session:") {
                    let session = session.to_owned();
                    self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space: space_id, session, index: None });
                }
            }
        }
    }
    /// Close every space the window holds but `space`, after asking (askCloseOtherSpaces).
    fn ask_close_other_spaces(&mut self, space: &str) {
        let others: Vec<String> = self.session_ids().into_iter().filter(|id| id != space).collect();
        if others.is_empty() {
            return;
        }
        let names: Vec<String> = others.iter().filter_map(|id| self.space_name(id)).map(|n| format!("“{n}”")).collect();
        let count = if others.len() == 1 { "1 other space".to_owned() } else { format!("{} other spaces", others.len()) };
        let button = if others.len() == 1 { "Close Other Space" } else { "Close Other Spaces" };
        self.confirm(&format!("Close {count} in this window?"), format!("Every terminal and agent in {} stops.", names.join(", ")), button.into(), Pending::CloseSpaces(others));
    }

    // MARK: Renaming

    pub fn begin_rename(&mut self, space: &str) {
        let Some(name) = self.space_name(space) else { return };
        self.close_space_menu();
        // One name field at a time: the window's or a session's goes.
        self.cancel_name_edit();
        self.rename = Some(Rename { space: space.to_owned(), draft: name, error: None, req: None });
    }
    pub fn cancel_rename(&mut self) {
        self.rename = None;
    }
    /// Return commits, Escape cancels, the rest edits the draft; false when no rename is up.
    pub fn rename_key(&mut self, key: &str, text: Option<&str>, command: bool) -> bool {
        if self.rename.is_none() {
            return false;
        }
        match key {
            "escape" => self.cancel_rename(),
            "enter" => self.commit_rename(),
            _ => {
                let draft = &mut self.rename.as_mut().unwrap().draft;
                if let Typed::Nothing = type_into(draft, key, text, command) {
                    return !command;
                }
            }
        }
        true
    }
    /// Send the draft as the space's name, or show why it cannot be sent (AppModel.renameSpace).
    pub fn commit_rename(&mut self) {
        let Some(rename) = self.rename.clone() else { return };
        // One request at a time: a second Return waits for the first answer.
        if rename.req.is_some() {
            return;
        }
        match sidebar::rename_check(&rename.draft, self.link == Link::Up) {
            Err(why) => self.rename.as_mut().unwrap().error = Some(why),
            Ok(name) => {
                let space = rename.space.clone();
                if self.space_name(&space).as_deref() == Some(name.as_str()) {
                    self.rename = None;
                    return;
                }
                let req = self.req();
                let rename = self.rename.as_mut().unwrap();
                rename.error = None;
                rename.req = Some(req);
                self.log(format!("rename space {space} to {name}"));
                self.send(&ClientMsg::SpaceRename { req: Some(req), space, name });
            }
        }
    }
    /// The answers the sidebar waits on: a rename's, and the session made for Move to New Session.
    pub fn on_sidebar_reply(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::Ok { req } if self.rename.as_ref().and_then(|r| r.req) == Some(*req) => {
                self.rename = None;
                true
            }
            ServerMsg::Error { req: Some(req), message, .. } if self.rename.as_ref().and_then(|r| r.req) == Some(*req) => {
                let rename = self.rename.as_mut().unwrap();
                rename.req = None;
                rename.error = Some(message.clone());
                true
            }
            ServerMsg::Created { req, id, .. } if self.space_for_new_session.as_ref().map(|(r, _)| r) == Some(req) => {
                self.layout_reqs.remove(req);
                let (_, space) = self.space_for_new_session.take().unwrap();
                let session = id.clone();
                self.send_layout(|req| ClientMsg::SessionMove { req: Some(req), space, session, index: None });
                true
            }
            _ => false,
        }
    }

    // MARK: The hover preview

    /// The row under the pointer changed: the preview opens after `PREVIEW_OPEN_MS` over a row
    /// (at once when another row's is open), and closes `PREVIEW_CLOSE_MS` after leaving.
    pub fn row_hover(&mut self, space: Option<String>) {
        let now = now_ms();
        match space {
            Some(space) => {
                if self.hover.as_ref().map(|(s, _)| s) != Some(&space) {
                    self.hover = Some((space, now));
                }
                self.preview_leave = None;
            }
            None => {
                if self.hover.take().is_some() && self.preview.is_some() {
                    self.preview_leave = Some(now);
                }
            }
        }
    }
    /// Advance the preview's timers; called each render and from the redraw timer.
    pub fn tick_preview(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        // A previewed space that closed or left the session takes its card and its hole with it.
        if self.preview.as_ref().is_some_and(|p| !self.session_ids().contains(p)) {
            self.close_preview();
        }
        if let Some((space, since)) = self.hover.clone() {
            let delay = if self.preview.is_some() { 0 } else { sidebar::PREVIEW_OPEN_MS };
            if now >= since + delay {
                if self.preview.as_deref() != Some(space.as_str()) {
                    self.open_preview(&space);
                }
            } else {
                self.redraw_in(since + delay - now, cx);
            }
        }
        if let Some(left) = self.preview_leave {
            if now >= left + sidebar::PREVIEW_CLOSE_MS {
                self.preview_leave = None;
                self.close_preview();
            } else {
                self.redraw_in(left + sidebar::PREVIEW_CLOSE_MS - now, cx);
            }
        }
        if let Some(space) = self.preview.clone()
            && now >= self.preview_asked + 2000
        {
            self.preview_asked = now;
            self.request_procs(&space);
            self.redraw_in(2000, cx);
        }
    }
    pub fn open_preview(&mut self, space: &str) {
        if !self.spaces.iter().any(|s| s.id == space) {
            return;
        }
        self.preview = Some(space.to_owned());
        self.preview_asked = now_ms();
        self.request_procs(space);
        self.log(format!("preview: opened {space}"));
        self.sync_sidebar_overlay();
    }
    pub fn close_preview(&mut self) {
        self.hover = None;
        self.preview_leave = None;
        if let Some(was) = self.preview.take() {
            self.log(format!("preview: closed (was {was})"));
            self.sync_sidebar_overlay();
        }
    }
    /// Where the preview card is drawn: right of the sidebar, level with its row, kept inside the window.
    pub fn preview_rect(&self) -> Option<Rect> {
        let space = self.preview.as_ref()?;
        let row = self.sidebar_rows().into_iter().find(|r| &r.id == space)?;
        let procs = self.procs.by_space.get(space).map(Vec::len).unwrap_or(0);
        let lines = procs.clamp(1, 4) + usize::from(procs > 4);
        let h = 28. + sidebar::PREVIEW_GRID_H + 20. + lines as f32 * 18. + 30.;
        let y = row.rect.y.min(self.viewport.1 - BOTTOM - h).max(HEADER);
        Some(Rect { x: header::MARGIN + sidebar::WIDTH + sidebar::GAP, y, w: sidebar::PREVIEW_W, h })
    }
    /// The last thing a pane shows, as one line (AppModel.lastLine): an agent's last reply, message
    /// or tool call, the last line of a terminal, a browser's address, a web pane's artifact.
    pub fn last_line(&self, leaf: &Leaf) -> Option<String> {
        fn last(text: &str) -> Option<String> {
            text.lines().map(str::trim).filter(|l| !l.is_empty()).next_back().map(str::to_owned)
        }
        match leaf.kind.as_str() {
            "agent" => {
                let t = self.transcripts.get(leaf.agent.as_deref()?)?;
                t.rows.iter().rev().find_map(|row| match &row.kind {
                    Kind::Reply(text) => last(text),
                    Kind::User { text, .. } => last(text).map(|l| format!("> {l}")),
                    Kind::Tool { title, .. } => Some(title.clone()),
                    _ => None,
                })
            }
            "terminal" => self.native.status()["panes"][leaf.id.as_str()]["viewport"].as_str().and_then(last),
            "browser" => leaf.url.clone(),
            "web" => Some(leaf.label.clone()),
            _ => None,
        }
    }
    /// What the preview shows, for the state query: (pane, kind, status rank, last line).
    pub fn preview_panes(&self, space: &str) -> Vec<(String, String, Option<&'static str>, Option<String>)> {
        self.spaces.iter().find(|s| s.id == space).map(|s| leaves_of(&s.root).into_iter()
            .map(|l| (l.id.clone(), l.kind.clone(), l.status.as_ref().map(status::rank), self.last_line(l))).collect()).unwrap_or_default()
    }
    /// The bridge's hole for the menu or the preview, which sit over the grid's native views.
    pub(super) fn sync_sidebar_overlay(&mut self) {
        if self.command_palette.is_some() || self.shortcuts_open || self.settings_open || self.confirmation.is_some() || self.pairing.open || self.procs.open.is_some() {
            return;
        }
        // GPUI draws over the native views only inside one rect: the open menu, else the lit drop
        // zone, else the preview card.
        let rect = if let Some((space, x, y)) = self.space_menu.clone() {
            Some(Rect { x, y, w: sidebar::MENU_W, h: self.menu_height(&space) })
        } else if let Some((pane, x, y, level)) = self.pane_menu.clone() {
            Some(Rect { x, y, w: super::panes::MENU_W, h: self.pane_menu_height(&pane, level) })
        } else if let Some(m) = self.message_menu.as_ref() {
            Some(Rect { x: m.x, y: m.y, w: super::messages::MENU_W, h: Self::message_menu_height() })
        } else if let Some(h) = self.grid_drop_highlight() {
            Some(h)
        } else {
            self.preview_rect()
        };
        let op = match rect {
            Some(r) => serde_json::json!({"op": "overlay", "open": true, "rect": [f64::from(r.x), f64::from(r.y), f64::from(r.w), f64::from(r.h)]}),
            None => serde_json::json!({"op": "overlay", "open": false}),
        };
        self.native.call(op);
    }

    /// The sidebar for `/state`.
    pub fn sidebar_json(&self) -> serde_json::Value {
        use serde_json::json;
        let rows: Vec<_> = self.sidebar_rows().into_iter().map(|r| {
            let space = self.spaces.iter().find(|s| s.id == r.id);
            let (marks, more) = space.map(|s| status::agent_statuses(s, &self.status_dots, status::AGENT_DOT_LIMIT)).unwrap_or_default();
            json!({"id": r.id, "name": space.map(|s| s.name.clone()), "active": self.space_id.as_deref() == Some(r.id.as_str()),
                "frame": [r.rect.x, r.rect.y, r.rect.w, r.rect.h], "close": [r.close.x, r.close.y, r.close.w, r.close.h],
                "dots": marks.iter().map(status::rank).collect::<Vec<_>>(), "more": more,
                "caption": space.and_then(|s| s.status.as_ref()).filter(|s| self.status_dots.contains(status::rank(s))).and_then(status::caption),
                "panes": space.map(|s| leaves_of(&s.root).len()).unwrap_or(0), "pane_over": self.pane_is_over(&r.id)})
        }).collect();
        let r = self.sidebar_rect();
        json!({
            "open": self.sidebar_open, "frame": [r.x, r.y, r.w, r.h], "rows": rows,
            "drag": self.space_drag.as_ref().map(|d| json!({"space": d.space, "target": d.target.as_ref().map(describe_target), "insertion": self.space_drag_insertion(), "tab_bar": self.space_drag_tab_bar(), "hint": self.space_drag_hint(),
                "screen": d.screen.map(|(x, y)| [x, y]), "measured": self.space_drag_world.as_ref().map(|w| json!({"origin": [w.origin.0, w.origin.1], "windows": w.windows.iter().filter(|d| !d.little).map(|d| d.id.clone()).collect::<Vec<_>>(), "popover": w.popover.as_ref().map(|p| p.cards.len())}))})),
            // A drag from any window, as this window draws it.
            "hovered": space_hover().map(|h| json!({"space": h.space, "source": h.source, "target": h.target.as_ref().map(describe_target), "insertion": self.space_drag_insertion(), "tab_bar": self.space_drag_tab_bar(), "outlined": self.space_drag_outlined(), "floating": h.floating,
                "chip": h.chip.filter(|(w, _)| *w == self.window_id).map(|(_, (x, y))| [x, y])})),
            "pane_drag": self.pane_drag.as_ref().map(|d| json!({"pane": d.pane, "label": d.label, "away": d.away.as_ref().map(describe_away), "drop": d.drop.as_ref().map(|g| json!({"target": g.target, "zone": g.zone.label()})), "hint": self.pane_drag_hint(), "new_space": self.pane_is_over_new_space(),
                "screen": d.screen.map(|(x, y)| [x, y]), "measured": self.pane_out.world.as_ref().map(|w| w.windows.iter().filter(|d| !d.little).map(|d| d.id.clone()).collect::<Vec<_>>()),
                "hold": self.pane_out.hold.as_ref().map(|(w, s)| json!([w, s])), "switched": self.pane_out.switched.iter().map(|(w, s)| json!([w, s])).collect::<Vec<_>>()})),
            "pane_hover": pane_hover().as_ref().map(describe_away), "pane_new_space": self.pane_is_over_new_space(),
            "renaming": self.rename.as_ref().map(|r| json!({"space": r.space, "draft": r.draft, "error": r.error})),
            "menu": self.space_menu.as_ref().map(|(space, _, _)| json!({"space": space, "items": self.space_menu_items(space).iter().map(|i| json!({"id": i.id, "title": i.title, "enabled": i.enabled, "section": i.section, "destructive": i.destructive})).collect::<Vec<_>>()})),
            "preview": self.preview.as_ref().map(|space| json!({"space": space, "frame": self.preview_rect().map(|r| [r.x, r.y, r.w, r.h]),
                "panes": self.preview_panes(space).iter().map(|(pane, kind, status, last)| json!({"pane": pane, "kind": kind, "status": status, "last": last})).collect::<Vec<_>>(),
                "procs": self.procs.by_space.get(space).map(|p| p.len()).unwrap_or(0)})),
        })
    }

    // MARK: Drawing

    /// The sidebar: its rows, the insertion bar of a space drag, the outline of a pane drag.
    pub fn sidebar_element(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let r = self.sidebar_rect();
        let rows = self.sidebar_rows();
        let insertion = self.space_drag_insertion();
        let dragged = self.space_drag.as_ref().map(|d| d.space.clone());
        let over_new = self.pane_is_over_new_space();
        let mut bar = div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).rounded_lg().bg(rgb(CARD_BG))
            .border_2().border_color(rgb(if over_new { ACCENT } else { CARD_BORDER })).overflow_hidden();
        for row in &rows {
            let Some(space) = self.spaces.iter().find(|s| s.id == row.id) else { continue };
            let active = self.space_id.as_deref() == Some(row.id.as_str());
            let (id, rr, close) = (row.id.clone(), row.rect, row.close);
            // A space in a little window dims, and its close button is the back icon (BackFromLittleButton).
            let away = self.little_showing(&id).is_some();
            let dimmed = dragged.as_deref() == Some(id.as_str());
            let caption = space.status.as_ref().filter(|s| self.status_dots.contains(status::rank(s))).and_then(status::caption);
            let renaming = self.rename.as_ref().filter(|rn| rn.space == id).cloned();
            let (press_id, menu_id, close_id) = (id.clone(), id.clone(), id.clone());
            let mut el = div().absolute().left(px(rr.x - r.x)).top(px(rr.y - r.y)).w(px(rr.w)).h(px(rr.h)).px(px(6.)).rounded_md().flex().items_center().gap(px(4.))
                .bg(rgb(if active { ROW_BG_ACTIVE } else { CARD_BG })).cursor_pointer()
                .when(dimmed, |d| d.opacity(0.4))
                .when(away && !dimmed, |d| d.opacity(super::little::AWAY_OPACITY))
                .when(self.pane_is_over(&id), |d| d.border_2().border_color(rgb(ACCENT)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    if this.rename.as_ref().is_some_and(|rn| rn.space == press_id) {
                        cx.stop_propagation();
                        return;
                    }
                    // A control-click asks for the menu, as a right press does.
                    if e.modifiers.control {
                        cx.stop_propagation();
                        this.open_space_menu(&press_id, f32::from(e.position.x), f32::from(e.position.y));
                        cx.notify();
                        return;
                    }
                    this.space_press_down(&press_id, f32::from(e.position.x), f32::from(e.position.y));
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_space_menu(&menu_id, f32::from(e.position.x), f32::from(e.position.y));
                    cx.notify();
                }))
                .child(pane_stack(leaves_of(&space.root).len(), active))
                .child(div().w(px(12.)).h(px(12.)).flex_none().flex().items_center().children(status::space_dots(space, &self.status_dots)));
            match renaming {
                Some(rn) => {
                    el = el.child(div().flex_1().min_w(px(0.)).flex().flex_col().child(
                        div().h(px(20.)).px(px(4.)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(ACCENT)).overflow_hidden().whitespace_nowrap().text_xs().flex().items_center()
                            .child(format!("{}▏", rn.draft))));
                }
                None => {
                    el = el.child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_sm()
                        .when(active, |d| d.font_weight(FontWeight::SEMIBOLD)).child(space.name.clone()));
                    if let Some(caption) = caption {
                        el = el.child(div().text_xs().text_color(rgb(DIM)).whitespace_nowrap().overflow_hidden().max_w(px(100.)).child(caption));
                    }
                }
            }
            el = el.child(div().w(px(close.w)).h(px(close.h)).flex_none().rounded_sm().flex().items_center().justify_center().text_xs().text_color(rgb(DIM)).cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if away {
                        this.press(format!("back space {close_id}"));
                        this.bring_back(&close_id, cx);
                    } else {
                        this.press(format!("close space {close_id}"));
                        this.ask_close_space(&close_id);
                    }
                    cx.notify();
                }))
                .child(if away { "↩" } else { "×" }));
            bar = bar.child(el);
            if let Some(err) = self.rename.as_ref().filter(|rn| rn.space == id).and_then(|rn| rn.error.clone()) {
                bar = bar.child(div().absolute().left(px(rr.x - r.x + 6.)).top(px(rr.y - r.y + rr.h)).w(px(rr.w - 12.)).text_xs().text_color(rgb(RED)).child(err));
            }
        }
        if let Some(bar_rect) = insertion.and_then(|i| sidebar::bar_rect(&rows, i)) {
            bar = bar.child(div().absolute().left(px(bar_rect.x - r.x)).top(px(bar_rect.y - r.y)).w(px(bar_rect.w)).h(px(bar_rect.h)).rounded_full().bg(rgb(ACCENT)));
        }
        bar
    }
    /// The outline round this window while a space drag aims at its body.
    pub fn space_drop_outline(&self) -> Option<gpui::Div> {
        self.space_drag_outlined().then(|| div().absolute().left_0().top_0().size_full().rounded_lg().border_2().border_color(rgb(ACCENT)))
    }
    /// The chip under the pointer during a space or pane drag, naming what is held.
    pub fn drag_chip(&self) -> Option<gpui::Div> {
        let hover = space_hover();
        let (text, at) = if let Some(h) = &hover {
            // A pointer drag's chip is a window of its own; else the window the pointer is in draws it.
            if h.floating {
                return None;
            }
            let (window, at) = h.chip.clone()?;
            if window != self.window_id {
                return None;
            }
            (self.space_name(&h.space).unwrap_or_else(|| h.space.clone()), at)
        } else if let Some(d) = &self.pane_drag {
            // A measured pointer drag's chip is a window of its own.
            if d.by_pointer && d.screen.is_some() {
                return None;
            }
            (if matches!(d.away, Some(PaneAway::NewSpace(_))) { format!("{} → new space", d.label) } else { d.label.clone() }, d.at)
        } else {
            return None;
        };
        Some(div().absolute().left(px(at.0 + 12.)).top(px(at.1 + 12.)).px(px(10.)).py(px(4.)).rounded_md().bg(rgb(0x22302a)).border_1().border_color(rgb(CARD_BORDER)).text_sm().whitespace_nowrap().child(text))
    }
    /// The space's context menu, anchored at the press.
    pub fn space_menu_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let (space, x, y) = self.space_menu.clone()?;
        let items = self.space_menu_items(&space);
        let mut list = div().absolute().left(px(x)).top(px(y)).w(px(sidebar::MENU_W)).py(px(4.)).rounded_md().bg(rgb(0x22302a)).border_1().border_color(rgb(CARD_BORDER)).flex().flex_col().text_sm();
        let mut last_section = None;
        for item in items {
            if last_section.is_some_and(|s| s != item.section) {
                list = list.child(div().h(px(1.)).my(px(3.)).bg(rgb(CARD_BORDER)));
            }
            last_section = Some(item.section);
            let (id, sp) = (item.id.clone(), space.clone());
            let colour = if !item.enabled { 0x5a6e64 } else if item.destructive { RED } else { 0xd9e8e1 };
            let mut row = div().h(px(sidebar::MENU_ROW_H)).px(px(10.)).flex().items_center().text_color(rgb(colour)).child(item.title.clone());
            if item.enabled {
                row = row.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.run_space_menu(&id, &sp, cx);
                    cx.notify();
                }));
            }
            list = list.child(row);
        }
        Some(list)
    }
    /// The hover preview: the space's grid as the panes look, each with its kind, its mark and
    /// its last line, then the processes its panes started (SpacePreviewView).
    pub fn preview_card(&self) -> Option<gpui::Div> {
        let space = self.preview.clone()?;
        let r = self.preview_rect()?;
        let info = self.spaces.iter().find(|s| s.id == space)?;
        let mut grid = div().relative().w(px(r.w - 20.)).h(px(sidebar::PREVIEW_GRID_H));
        let (mut leaves, mut dividers) = (vec![], vec![]);
        place(&info.root, Rect { x: 0., y: 0., w: r.w - 20., h: sidebar::PREVIEW_GRID_H }, &HashMap::new(), &mut leaves, &mut dividers);
        for p in leaves {
            let title = match p.leaf.kind.as_str() {
                "agent" => p.leaf.harness.clone().unwrap_or_else(|| "agent".into()),
                "terminal" => "terminal".into(),
                other => other.to_owned(),
            };
            let focused = self.focus.current.as_deref() == Some(p.leaf.id.as_str());
            let mut pane = div().absolute().left(px(p.rect.x)).top(px(p.rect.y)).w(px(p.rect.w)).h(px(p.rect.h)).p(px(6.)).rounded_sm().bg(rgb(0x22302a)).overflow_hidden()
                .border_1().border_color(rgb(if focused { ACCENT } else { CARD_BORDER })).flex().flex_col().gap(px(4.))
                .child(div().flex().items_center().gap(px(5.)).text_xs().font_weight(FontWeight::SEMIBOLD).child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().child(title))
                    .child(div().w(px(12.)).h(px(12.)).flex_none().children(p.leaf.status.as_ref().and_then(|s| status::dot(s, &self.status_dots, gpui::ElementId::Name(format!("preview-dot-{}", p.leaf.id).into()))))));
            if let Some(last) = self.last_line(&p.leaf) {
                pane = pane.child(div().text_xs().text_color(rgb(DIM)).overflow_hidden().child(last));
            }
            grid = grid.child(pane);
        }
        let procs = self.procs.by_space.get(&space).cloned().unwrap_or_default();
        let mut list = div().flex().flex_col().gap(px(2.)).text_xs();
        if procs.is_empty() {
            list = list.child(div().text_color(rgb(DIM)).child("No processes running"));
        } else {
            for p in procs.iter().take(4) {
                list = list.child(div().flex().gap(px(6.)).child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().child(p.name.clone())).child(div().text_color(rgb(DIM)).child(format!("{:.0}%", p.cpu))));
            }
            if procs.len() > 4 {
                list = list.child(div().text_color(rgb(DIM)).child(format!("+{} more", procs.len() - 4)));
            }
        }
        Some(div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).p(px(10.)).rounded_lg().bg(rgb(CARD_BG)).border_1().border_color(rgb(CARD_BORDER)).flex().flex_col().gap(px(8.))
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().child(info.name.clone()))
            .child(grid)
            .child(div().text_xs().text_color(rgb(DIM)).whitespace_nowrap().overflow_hidden().child(info.cwd.clone()))
            .child(list))
    }
}

fn describe_target(target: &SpaceDropTarget) -> String {
    match target {
        SpaceDropTarget::Sidebar { window, index } => format!("sidebar of {window} at {index}"),
        SpaceDropTarget::Tabs { window, index } => format!("tabs of {window} at {index}"),
        SpaceDropTarget::Window(window) => format!("window {window}"),
        SpaceDropTarget::Card { session, index } => format!("card of session {session} at {index}"),
        SpaceDropTarget::Desktop => "the desktop".into(),
    }
}
fn describe_away(away: &PaneAway) -> String {
    match away {
        PaneAway::Beside { pane, dir, space } => format!("{pane} {} in space {space}", crate::grid_drop::Zone::Edge(*dir).label()),
        PaneAway::End(space) => format!("the end of space {space}"),
        PaneAway::NewSpace(window) => format!("a new space in window {window}"),
        PaneAway::Desktop => "the desktop".into(),
    }
}

/// A space's panes as a stack of small squares, one per pane up to four, each offset to the
/// right of the one in front (PaneStack.swift).
fn pane_stack(panes: usize, active: bool) -> gpui::Div {
    let layers = sidebar::stack_layers(panes);
    let (size, step) = (12., 3.);
    let mut stack = div().relative().flex_none().w(px(size + (layers - 1) as f32 * step)).h(px(size));
    for i in (0..layers).rev() {
        stack = stack.child(div().absolute().left(px(i as f32 * step)).top(px(0.)).w(px(size)).h(px(size)).rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(if active { 0xd9e8e1 } else { DIM })));
    }
    stack
}

