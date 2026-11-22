//! The Agents mode's body (wheel-0but.3): every agent of the session as a card on one surface the
//! client pans and zooms. A card at 50% zoom or more is the agent pane the grid draws (its chat,
//! tool rows, permission card and composer), at the card's frame; text keeps its size, so a
//! smaller card shows less of the chat. Below 50% a card is its state dot and its name. A card can
//! also be one line. Frames, closed cards and one-line cards are this client's (ADR-0022).
use super::*;
use crate::cards::{self, Camera, Prefs};
use crate::popped::Frames;
use serde_json::json;

use super::settings::{ACCENT, DIM, LINE, PANEL as CARD_BG, TEXT};
/// The "+ Agent" button and the zoom readout: their size and their inset from the area's corners.
const CONTROL: (f32, f32) = (104., 30.);
const READOUT_W: f32 = 56.;
const INSET: f32 = 12.;
const MENU_W: f32 = 280.;
const MENU_ROW_H: f32 = 24.;
/// The hit area of a dot card, centred on its frame: at most this wide, and never wider than the
/// gap between two default slots at the zoom drawn, so neighbouring dots never overlap.
const DOT_AREA: (f32, f32) = (180., 64.);

/// The surface's state in one window: the camera, the drag on it, and the "+ Agent" menu. Frames
/// and prefs are shared by every window of the process.
pub struct Surface {
    pub camera: Camera,
    frames: Rc<RefCell<Frames>>,
    prefs: Rc<RefCell<Prefs>>,
    /// A card being moved or resized, with the pointer's travel in window points.
    drag: Option<CardDrag>,
    /// The surface being panned: where the pointer went down and the pan then.
    pan: Option<((f32, f32), (f32, f32))>,
    pub menu_open: bool,
    /// The splits sent for "Start a new agent", by request id, to the agent each starts, and the
    /// starts sent, whose refusal wheeld's layout-error notice shows.
    starts: HashMap<u64, String>,
    started: HashSet<u64>,
    /// Each card's hit area as the last frame drew it, for the surface's presses and scrolls.
    hits: Vec<Rect>,
}
impl Surface {
    pub fn new(hidden: bool, ws: &str) -> Self {
        let (frames, prefs) = if hidden { (None, None) } else { let (f, p) = cards::default_files(ws); (Some(f), Some(p)) };
        Self { camera: Camera::default(), frames: Frames::shared(frames), prefs: Prefs::shared(prefs, false), drag: None, pan: None, menu_open: false, starts: HashMap::new(), started: HashSet::new(), hits: vec![] }
    }
}

/// One agent's card as drawn.
#[derive(Clone, Debug)]
pub struct AgentCard {
    pub leaf: Leaf,
    pub space: String,
    pub space_name: String,
    /// Where it sits on the surface (zoom 1), a drag included, and where that draws in the window.
    pub world: Rect,
    pub rect: Rect,
    pub summary: bool,
    /// Its frame was stored by a move or a resize; otherwise it takes its default slot.
    pub stored: bool,
}

/// One row of the "+ Agent" menu: a heading, a start (`start:<space>:<agent>`) or a show (`show:<pane>`).
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceRow {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub checked: bool,
    pub heading: bool,
}

impl Client {
    /// Every agent pane of the session shown, with its space: the grid's, the popped and the tray's.
    pub fn session_agents(&self) -> Vec<(Leaf, String, String)> {
        self.session_spaces().into_iter().flat_map(|s| s.all().filter(|l| l.kind == "agent").map(|l| (l.clone(), s.id.clone(), s.name.clone())).collect::<Vec<_>>()).collect()
    }
    fn card_hidden(&self, leaf: &Leaf) -> bool {
        self.surface.prefs.borrow().hidden.contains(&hide_key(leaf))
    }
    /// The cards on the surface, in the order they draw (the focused card and a dragged one last,
    /// so they are on top).
    pub fn agent_cards(&self) -> Vec<AgentCard> {
        let area = self.grid_area();
        let mut agents: Vec<(Leaf, String, String)> = self.session_agents().into_iter().filter(|(l, _, _)| !self.card_hidden(l)).collect();
        // Every card takes the slot of its place in the order, stored or not, so a card moved
        // or a zoom leaves the others where they are.
        agents.sort_by(|(a, _, _), (b, _, _)| cards::order_key(a.status.as_ref(), &a.id).cmp(&cards::order_key(b.status.as_ref(), &b.id)));
        let frames = self.surface.frames.borrow();
        let prefs = self.surface.prefs.borrow();
        let camera = self.surface.camera;
        let mut out: Vec<AgentCard> = agents.into_iter().enumerate().map(|(slot, (leaf, space, space_name))| {
            let stored = frames.get(&leaf.id);
            let mut world = stored.unwrap_or_else(|| cards::default_frame(slot, area.w - 2. * cards::GAP));
            let summary = prefs.summary.contains(&leaf.id);
            if summary {
                world.h = cards::SUMMARY_H;
            }
            if let Some(d) = self.surface.drag.as_ref().filter(|d| d.pane == leaf.id) {
                let delta = (d.delta.0 / camera.zoom, d.delta.1 / camera.zoom);
                world = cards::dragged(world, d.resize && !summary, delta);
                if summary {
                    world.h = cards::SUMMARY_H;
                }
            }
            AgentCard { rect: camera.to_screen(area, world), leaf, space, space_name, world, summary, stored: stored.is_some() }
        }).collect();
        let focus = self.focus.current.clone();
        let dragged = self.surface.drag.as_ref().map(|d| d.pane.clone());
        out.sort_by_key(|c| (Some(&c.leaf.id) == dragged.as_ref(), Some(&c.leaf.id) == focus.as_ref()));
        out
    }
    /// What a card's hit area is in the window: its frame, or the area round its dot.
    fn card_hit(&self, c: &AgentCard) -> Rect {
        if self.surface.camera.dots() {
            let (cx_, cy_) = (c.rect.x + c.rect.w / 2., c.rect.y + c.rect.h / 2.);
            let w = DOT_AREA.0.min((cards::CARD.0 + cards::GAP) * self.surface.camera.zoom - 4.).max(1.);
            Rect { x: cx_ - w / 2., y: cy_ - DOT_AREA.1 / 2., w, h: DOT_AREA.1 }
        } else {
            c.rect
        }
    }
    /// Whether window point (`x`, `y`) is over a card as the last frame drew it.
    fn over_card(&self, x: f32, y: f32) -> bool {
        self.surface.hits.iter().any(|r| crate::sidebar::contains(*r, x, y))
    }
    /// Whether it is over "+ Agent", its menu or the readout.
    fn over_controls(&self, x: f32, y: f32) -> bool {
        [Some(self.agents_control_rect()), Some(self.zoom_readout_rect()), self.surface_menu_placed().map(|(r, _)| r)].into_iter().flatten().any(|r| crate::sidebar::contains(r, x, y))
    }
    /// The "+ Agent" button, bottom left of the grid area; the same in every state.
    pub fn agents_control_rect(&self) -> Rect {
        let a = self.grid_area();
        Rect { x: a.x + INSET, y: a.y + a.h - INSET - CONTROL.1, w: CONTROL.0, h: CONTROL.1 }
    }
    /// The zoom readout, bottom right of the grid area; the same in every state.
    pub fn zoom_readout_rect(&self) -> Rect {
        let a = self.grid_area();
        Rect { x: a.x + a.w - INSET - READOUT_W, y: a.y + a.h - INSET - CONTROL.1, w: READOUT_W, h: CONTROL.1 }
    }
    /// The "+ Agent" menu's rows: one start per agent found on this machine and space of the
    /// session (the pane picker's choices), then every agent of the session to show.
    pub fn surface_menu_rows(&self) -> Vec<SurfaceRow> {
        let heading = |t: &str| SurfaceRow { id: String::new(), title: t.into(), detail: String::new(), checked: false, heading: true };
        let mut rows = vec![heading("Start a new agent")];
        for space in self.session_spaces() {
            for agent in self.agents.iter().filter(|a| a.found) {
                rows.push(SurfaceRow { id: format!("start:{}:{}", space.id, agent.name), title: commands::capitalized(&agent.name), detail: space.name.clone(), checked: false, heading: false });
            }
        }
        let agents = self.session_agents();
        if !agents.is_empty() {
            rows.push(heading("Show an agent"));
        }
        for (leaf, _, space_name) in agents {
            rows.push(SurfaceRow { id: format!("show:{}", leaf.id), title: self.pane_label(&leaf), detail: space_name, checked: !self.card_hidden(&leaf), heading: false });
        }
        rows
    }
    /// The open menu's rows at their rectangles, above the "+ Agent" button.
    pub fn surface_menu_placed(&self) -> Option<(Rect, Vec<(SurfaceRow, Rect)>)> {
        if !self.surface.menu_open {
            return None;
        }
        let rows = self.surface_menu_rows();
        let b = self.agents_control_rect();
        let h = rows.len() as f32 * MENU_ROW_H + 8.;
        let top = (b.y - 6. - h).max(self.grid_area().y);
        let placed = rows.into_iter().enumerate().map(|(i, r)| (r, Rect { x: b.x + 4., y: top + 4. + i as f32 * MENU_ROW_H, w: MENU_W - 8., h: MENU_ROW_H })).collect();
        Some((Rect { x: b.x, y: top, w: MENU_W, h }, placed))
    }

    // MARK: Pointer

    /// A press on a card's header (or its dot, or its one line) moves it; on its corner resizes it.
    pub(super) fn surface_card_press(&mut self, pane: &str, resize: bool, x: f32, y: f32) {
        self.surface.menu_open = false;
        self.surface.drag = Some(CardDrag { pane: pane.to_owned(), resize, start: (x, y), delta: (0., 0.) });
        self.focus_pane(pane);
    }
    /// The pointer moved while the surface holds it.
    pub(super) fn surface_move(&mut self, x: f32, y: f32) {
        if let Some(d) = &mut self.surface.drag {
            d.delta = (x - d.start.0, y - d.start.1);
        }
        if let Some(((x0, y0), (px0, py0))) = self.surface.pan {
            self.surface.camera.pan = (px0 + x - x0, py0 + y - y0);
        }
    }
    pub(super) fn surface_holds_pointer(&self) -> bool {
        self.surface.drag.is_some() || self.surface.pan.is_some()
    }
    /// The button came up: a moved or resized card keeps its frame.
    pub(super) fn surface_release(&mut self) {
        if let Some(d) = self.surface.drag.clone()
            && d.delta != (0., 0.)
            && let Some(card) = self.agent_cards().into_iter().find(|c| c.leaf.id == d.pane)
        {
            let mut frame = card.world;
            if card.summary {
                // A one-line card keeps the height its full chat comes back at.
                frame.h = self.surface.frames.borrow().get(&d.pane).map_or(cards::CARD.1, |f| f.h);
            }
            self.surface.frames.borrow_mut().put(&d.pane, frame);
            self.log(format!("agent card {} {} to [{:.0}, {:.0}, {:.0}, {:.0}]", d.pane, if d.resize { "resized" } else { "moved" }, frame.x, frame.y, frame.w, frame.h));
        }
        self.surface.drag = None;
        if self.surface.pan.take().is_some() {
            let (x, y) = self.surface.camera.pan;
            self.log(format!("agents surface panned to [{x:.0}, {y:.0}]"));
        }
    }
    /// Zoom by `factor` about window point `at`.
    pub fn surface_zoom(&mut self, at: (f32, f32), factor: f32) {
        // A drag measured at one zoom ends before the zoom changes.
        self.surface_release();
        let was = self.surface.camera.dots();
        self.surface.camera.zoom_about(self.grid_area(), at, factor);
        if was != self.surface.camera.dots() {
            self.log(format!("agents surface zoom {}: cards are {}", self.surface.camera.readout(), if self.surface.camera.dots() { "dots" } else { "full" }));
        }
    }
    /// The readout's press: back to 100% about the area's middle.
    fn surface_zoom_reset(&mut self) {
        let a = self.grid_area();
        let factor = 1. / self.surface.camera.zoom;
        self.surface_zoom((a.x + a.w / 2., a.y + a.h / 2.), factor);
        self.log("agents surface zoom 100%".into());
    }

    // MARK: Cards

    /// Close `pane`'s card: it leaves the surface, its agent keeps running.
    pub fn close_card(&mut self, pane: &str) {
        let Some(leaf) = self.leaf(pane) else { return };
        let key = hide_key(&leaf);
        self.surface.prefs.borrow_mut().set_hidden(&key, true);
        self.log(format!("agent card {pane} closed; agent {key} still runs"));
    }
    /// Draw `pane`'s card as one line, or back as its full chat.
    pub fn set_card_summary(&mut self, pane: &str, summary: bool) {
        self.surface.prefs.borrow_mut().set_summary(pane, summary);
        self.log(format!("agent card {pane} drawn as {}", if summary { "one line" } else { "the full chat" }));
    }
    /// Put `pane`'s card back on the surface if it was closed, and centre the camera on it.
    pub fn show_card(&mut self, pane: &str) {
        if let Some(leaf) = self.leaf(pane) {
            self.surface.prefs.borrow_mut().set_hidden(&hide_key(&leaf), false);
        }
        if let Some(card) = self.agent_cards().into_iter().find(|c| c.leaf.id == pane) {
            self.surface.camera.centre_on(self.grid_area(), card.world);
        }
        self.focus_pane(pane);
        self.log(format!("agent card {pane} shown"));
    }
    /// Start `agent` in a new pane of `space`: the space's empty pane when it has one, else a pane
    /// split from its first one, as the pane picker does in an empty pane.
    pub fn start_agent_in(&mut self, space: &str, agent: &str) {
        let Some(s) = self.spaces.iter().find(|s| s.id == space) else { return };
        let leaves = leaves_of(&s.root);
        if let Some(empty) = leaves.iter().find(|l| l.kind == "empty").map(|l| l.id.clone()) {
            self.log(format!("agents surface: starting {agent} in empty pane {empty} of {space}"));
            self.send_start(&empty, agent);
            return;
        }
        let Some(first) = leaves.first().map(|l| l.id.clone()) else { return };
        let req = self.send_layout_req(|req| ClientMsg::PaneSplit { req: Some(req), pane: first.clone(), dir: None, focus: false, site: None });
        self.surface.starts.insert(req, agent.to_owned());
        self.log(format!("agents surface: starting {agent} in a pane split from {first} of {space}"));
    }
    /// wheeld made the pane a start asked for: the agent starts in it.
    pub fn on_surface_reply(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::Created { req, id, .. } if self.surface.starts.contains_key(req) => {
                let agent = self.surface.starts.remove(req).unwrap_or_default();
                self.layout_reqs.remove(req);
                self.log(format!("agents surface: pane {id} made; starting {agent}"));
                self.send_start(&id.clone(), &agent);
                true
            }
            ServerMsg::Started { req, pane, agent, .. } if self.surface.started.remove(req) => {
                self.layout_reqs.remove(req);
                self.log(format!("agents surface: {} started in {pane}", agent.as_deref().unwrap_or("agent")));
                true
            }
            // A refusal goes on to the layout-error notice, which shows it.
            ServerMsg::Error { req: Some(req), message, .. } if self.surface.starts.remove(req).is_some() || self.surface.started.remove(req) => {
                self.log(format!("agents surface: the start was refused: {message}"));
                false
            }
            _ => false,
        }
    }
    /// Start `agent` in `pane`; wheeld's refusal shows as the layout-error notice.
    fn send_start(&mut self, pane: &str, agent: &str) {
        let req = self.send_layout_req(|req| ClientMsg::PaneStart { req: Some(req), pane: pane.to_owned(), kind: wheel_proto::StartKind::Agent, message: None, agent: Some(agent.to_owned()), model: None, permission_mode: None, sandbox: None, dir: None, from: None });
        self.surface.started.insert(req);
    }
    pub fn run_surface_row(&mut self, id: &str) {
        self.surface.menu_open = false;
        if let Some(pane) = id.strip_prefix("show:") {
            self.show_card(pane);
        } else if let Some((space, agent)) = id.strip_prefix("start:").and_then(|r| r.split_once(':')) {
            self.start_agent_in(space, agent);
        }
    }
    /// Read the card files again from `frames` and `prefs`, as a launch does; the check names its
    /// own files, since the hidden mode keeps none.
    pub fn reload_card_files(&mut self, frames: std::path::PathBuf, prefs: std::path::PathBuf) {
        self.surface.frames = Frames::reload(frames);
        self.surface.prefs = Prefs::shared(Some(prefs), true);
    }

    // MARK: Drawing

    /// The surface: its cards, the "+ Agent" button, its menu and the zoom readout, in window
    /// coordinates inside a layer clipped to the grid area.
    pub(super) fn surface_element(&mut self, cards: &[AgentCard], window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let a = self.grid_area();
        let (w, h) = self.viewport;
        let mut layer = div().absolute().left(px(-a.x)).top(px(-a.y)).w(px(w)).h(px(h));
        // The empty surface: a press pans it, a two-finger scroll off a card pans it, ⌘-scroll and a pinch zoom it.
        layer = layer.child(div().absolute().left(px(a.x)).top(px(a.y)).w(px(a.w)).h(px(a.h))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
                this.surface.menu_open = false;
                if !this.over_card(x, y) {
                    this.surface.pan = Some(((x, y), this.surface.camera.pan));
                }
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, window, cx| {
                let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
                let d = e.delta.pixel_delta(window.line_height());
                let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                if e.modifiers.platform {
                    this.surface_zoom((x, y), (1. + dy * 0.01).clamp(0.5, 2.));
                } else if !this.over_card(x, y) && !this.over_controls(x, y) {
                    this.surface.camera.pan = (this.surface.camera.pan.0 + dx, this.surface.camera.pan.1 + dy);
                } else {
                    return;
                }
                cx.notify();
            }))
            .on_pinch(cx.listener(|this, e: &gpui::PinchEvent, _, cx| {
                this.surface_zoom((f32::from(e.position.x), f32::from(e.position.y)), 1. + e.delta);
                cx.notify();
            })));
        let dots = self.surface.camera.dots();
        self.surface.hits = cards.iter().map(|c| self.card_hit(c)).collect();
        let meets = |r: Rect| r.x < a.x + a.w && r.x + r.w > a.x && r.y < a.y + a.h && r.y + r.h > a.y;
        for card in cards {
            // A card wholly outside the grid area builds nothing.
            if !meets(self.card_hit(card)) {
                continue;
            }
            layer = layer.child(if dots { self.dot_card(card, cx) } else if card.summary { self.summary_card(card, cx) } else { self.full_card(card, window, cx) });
        }
        let mut root = div().absolute().left(px(a.x)).top(px(a.y)).w(px(a.w)).h(px(a.h)).overflow_hidden().child(layer);
        if cards.is_empty() {
            root = root.child(div().absolute().left_0().top_0().size_full().flex().items_center().justify_center().text_color(rgb(DIM))
                .child(if self.session_agents().is_empty() { "No agents in this session. Start one with + Agent." } else { "Every card is closed. Show one with + Agent." }));
        }
        let mut out = div().absolute().left_0().top_0().size_full().child(root);
        out = out.child(self.surface_controls(cx));
        out
    }
    /// A card at 50% zoom or more: the agent pane the grid draws, at the card's frame, and its corner.
    fn full_card(&mut self, card: &AgentCard, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let r = card.rect;
        let pane = self.pane_element(&Placed { leaf: card.leaf.clone(), rect: r }, window, cx).overflow_hidden().rounded_md().shadow_lg();
        let id = card.leaf.id.clone();
        // The corner straddles the card's edge, over its padding, so no composer control lies under it.
        let grip = div().absolute().left(px(r.x + r.w - GRIP / 2.)).top(px(r.y + r.h - GRIP / 2.)).w(px(GRIP)).h(px(GRIP)).cursor_crosshair()
            .child(div().absolute().left(px(1.)).top(px(1.)).w(px(7.)).h(px(7.)).border_r_2().border_b_2().border_color(rgb(0x5d7a6b)))
            .child(recorded(&id, "card-grip", &self.controls))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.surface_card_press(&id, true, f32::from(e.position.x), f32::from(e.position.y));
                cx.notify();
            }));
        div().absolute().left_0().top_0().size_full().child(pane).child(grip)
    }
    /// The buttons a card's header carries in place of the pane's: one line or full, and close.
    pub(super) fn card_header_buttons(&self, pane: &str, cx: &mut Context<Self>) -> gpui::Div {
        let summary = self.surface.prefs.borrow().summary.contains(pane);
        let button = |name: &'static str, glyph: &'static str, pane: String, cx: &mut Context<Self>, run: Box<dyn Fn(&mut Client, &str)>| {
            div().id(gpui::ElementId::Name(format!("card-{name}-{pane}").into())).relative().w(px(18.)).h(px(18.)).flex().items_center().justify_center().rounded_sm().cursor_pointer().hover(|s| s.bg(rgb(ACCENT)))
                .child(glyph).child(recorded(&pane, name, &self.controls))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    run(this, &pane);
                    cx.notify();
                }))
        };
        div().flex().items_center().gap(px(2.)).ml_2().flex_none()
            .child(button("card-summary", if summary { "▢" } else { "–" }, pane.to_owned(), cx, Box::new(move |c, p| c.set_card_summary(p, !summary))))
            .child(button("card-close", "×", pane.to_owned(), cx, Box::new(|c, p| c.close_card(p))))
    }
    /// A one-line card: its dot, its name and space, and its current tool or last text.
    fn summary_card(&mut self, card: &AgentCard, cx: &mut Context<Self>) -> gpui::Div {
        let r = card.rect;
        let id = card.leaf.id.clone();
        let line = card.leaf.agent.as_deref().and_then(|a| self.transcripts.get(a)).map(last_line).unwrap_or_default();
        let colour = card.leaf.status.as_ref().map_or(0x5a7266, status::colour);
        div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).px_2().flex().items_center().gap_2().rounded_md().bg(rgb(CARD_BG)).border_1().shadow_lg()
            .border_color(rgb(if self.focus.current.as_ref() == Some(&id) { 0x7fb89a } else { 0x2c3d34 })).overflow_hidden().text_xs()
            .child(div().w(px(9.)).h(px(9.)).flex_none().rounded_full().bg(rgb(colour)))
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)).child(self.pane_label(&card.leaf)))
            .child(div().flex_none().text_color(rgb(DIM)).child(card.space_name.clone()))
            .child(div().flex_1().min_w(px(0.)).whitespace_nowrap().overflow_hidden().text_ellipsis().text_color(rgb(DIM)).child(line))
            .child(self.card_header_buttons(&id, cx))
            .child(recorded(&id, "card-summary-line", &self.controls))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.surface_card_press(&id, false, f32::from(e.position.x), f32::from(e.position.y));
                cx.notify();
            }))
    }
    /// A card below 50% zoom: its state dot, its harness and space, and its name. A press moves
    /// it; a double press zooms back to 100% on it.
    fn dot_card(&mut self, card: &AgentCard, cx: &mut Context<Self>) -> gpui::Div {
        let hit = self.card_hit(card);
        let id = card.leaf.id.clone();
        let colour = card.leaf.status.as_ref().map_or(0x5a7266, status::colour);
        let needs = matches!(card.leaf.status, Some(PaneStatus::NeedsYou { .. }));
        let harness = card.leaf.harness.clone().unwrap_or_else(|| "agent".into());
        div().absolute().left(px(hit.x)).top(px(hit.y)).w(px(hit.w)).h(px(hit.h)).flex().flex_col().items_center().justify_center().gap(px(3.)).cursor_pointer()
            .child(div().w(px(14.)).h(px(14.)).rounded_full().bg(rgb(colour)))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(if needs { 0xff9f0a } else { TEXT })).max_w_full().whitespace_nowrap().overflow_hidden().text_ellipsis().child(format!("{harness} · {}", card.space_name)))
            .child(div().text_xs().text_color(rgb(DIM)).max_w_full().whitespace_nowrap().overflow_hidden().text_ellipsis().child(self.pane_label(&card.leaf)))
            .child(recorded(&id, "card-dot", &self.controls))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
                if e.click_count == 2 {
                    this.surface_zoom_reset();
                    this.show_card(&id);
                } else {
                    this.surface_card_press(&id, false, x, y);
                }
                cx.notify();
            }))
    }
    /// "+ Agent" at the bottom left, its menu over the surface, and the zoom readout at the bottom right.
    fn surface_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let b = self.agents_control_rect();
        let z = self.zoom_readout_rect();
        let mut out = div().absolute().left_0().top_0().size_full()
            .child(div().absolute().left(px(b.x)).top(px(b.y)).w(px(b.w)).h(px(b.h)).px_3().flex().items_center().justify_between().rounded_md().cursor_pointer()
                .bg(rgb(if self.surface.menu_open { ACCENT } else { 0x22302a })).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT)).hover(|s| s.bg(rgb(ACCENT)))
                .child("+  Agent").child(div().text_xs().text_color(rgb(DIM)).child("⌄")).child(recorded("surface", "agents-control", &self.controls))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.surface.menu_open = !this.surface.menu_open;
                    cx.notify();
                })))
            .child(div().absolute().left(px(z.x)).top(px(z.y)).w(px(z.w)).h(px(z.h)).flex().items_center().justify_end().cursor_pointer().text_xs().text_color(rgb(DIM))
                .child(self.surface.camera.readout()).child(recorded("surface", "zoom-readout", &self.controls))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.surface_zoom_reset();
                    cx.notify();
                })));
        if let Some((card, rows)) = self.surface_menu_placed() {
            let mut menu = div().absolute().left(px(card.x)).top(px(card.y)).w(px(card.w)).h(px(card.h)).rounded_md().bg(rgb(CARD_BG)).border_1().border_color(rgb(LINE)).shadow_lg()
                .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()));
            for (row, r) in rows {
                let (x, y) = (r.x - card.x, r.y - card.y);
                if row.heading {
                    menu = menu.child(div().absolute().left(px(x + 6.)).top(px(y)).h(px(r.h)).flex().items_end().pb_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(DIM)).child(row.title));
                    continue;
                }
                let id = row.id.clone();
                menu = menu.child(div().absolute().left(px(x)).top(px(y)).w(px(r.w)).h(px(r.h)).px_2().rounded_sm().flex().items_center().gap_2().cursor_pointer().hover(|s| s.bg(rgb(ACCENT)))
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_color(rgb(TEXT)).child(row.title))
                    .child(div().flex_none().text_xs().text_color(rgb(DIM)).child(row.detail))
                    .child(div().w(px(12.)).flex_none().text_xs().text_color(rgb(DIM)).child(if row.id.starts_with("show:") { if row.checked { "✓" } else { "○" } } else { "" }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.run_surface_row(&id);
                        cx.notify();
                    })));
            }
            out = out.child(menu);
        }
        out
    }

    /// The surface as a check reads it (`state_json()["agents"]`): the camera, every card with
    /// its world frame and window rect, the closed ones, the controls and the open menu.
    pub fn agents_json(&self) -> serde_json::Value {
        let rect = |r: Rect| json!([r.x, r.y, r.w, r.h]);
        let cards: Vec<serde_json::Value> = self.agent_cards().iter().map(|c| json!({
            "pane": c.leaf.id, "agent": c.leaf.agent, "harness": c.leaf.harness, "space": c.space, "label": self.pane_label(&c.leaf),
            "status": c.leaf.status.as_ref().map(status::rank), "colour": c.leaf.status.as_ref().map(|s| format!("#{:06x}", status::colour(s))),
            "world": rect(c.world), "rect": rect(c.rect), "hit": rect(self.card_hit(c)), "stored": c.stored, "summary": c.summary,
            "dot": self.surface.camera.dots(), "rank": cards::rank(c.leaf.status.as_ref()),
        })).collect();
        let closed: Vec<String> = self.session_agents().into_iter().filter(|(l, _, _)| self.card_hidden(l)).map(|(l, _, _)| l.id).collect();
        json!({
            "camera": {"pan": [self.surface.camera.pan.0, self.surface.camera.pan.1], "zoom": self.surface.camera.zoom, "readout": self.surface.camera.readout(), "dots": self.surface.camera.dots()},
            "cards": cards, "closed": closed, "control": rect(self.agents_control_rect()), "readout": rect(self.zoom_readout_rect()),
            "menu": self.surface_menu_placed().map(|(card, rows)| json!({"frame": rect(card), "rows": rows.iter().map(|(r, at)| json!({"id": r.id, "title": r.title, "detail": r.detail, "checked": r.checked, "heading": r.heading, "frame": rect(*at)})).collect::<Vec<_>>()})),
            "dragging": self.surface.drag.as_ref().map(|d| d.pane.clone()), "panning": self.surface.pan.is_some(),
        })
    }
}

/// What a closed card is kept by: its agent's name, so a new agent in the same pane shows; the
/// pane for one whose agent has not started.
fn hide_key(leaf: &Leaf) -> String {
    leaf.agent.clone().unwrap_or_else(|| leaf.id.clone())
}

/// What a one-line card says: the running tool's line, else the last reply's last line.
fn last_line(t: &Transcript) -> String {
    for row in t.rows.iter().rev() {
        match &row.kind {
            Kind::Tool { title, kind, status, content, detail, .. } => {
                let tool = ToolRow::new(title, kind, status, content, detail.as_deref());
                return format!("{} {}", tool.glyph, tool.label);
            }
            Kind::Reply(text) => return text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().to_owned(),
            Kind::Notice(text) | Kind::Error(text) => return text.clone(),
            _ => {}
        }
    }
    String::new()
}
