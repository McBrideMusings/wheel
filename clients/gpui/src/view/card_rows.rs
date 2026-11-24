//! Cards in an agent's transcript (`wheel agent card`, wheel-0but.6): a row that draws a pane or an
//! artifact live where the agent put it. A pane card shows the pane's name, kind and status, and an
//! agent pane's latest reply as it streams; an artifact card holds the artifact's page in a native
//! web view the bridge opens under the card's page id, which wheeld serves as it serves a web pane.
use super::*;
use wheel_proto::{AgentEvent, ArtifactScope, CardRef};

/// The height of an artifact card's page in the column, in points.
pub const PAGE_H: f32 = 240.;
/// The most lines of an agent's latest reply a pane card shows.
const REPLY_LINES: usize = 4;

/// Where an artifact card's page slot was painted, and the clip the list painted it inside.
#[derive(Clone, Debug, PartialEq)]
pub struct CardSlot {
    pub id: String,
    pub name: String,
    pub scope: ArtifactScope,
    pub space: Option<String>,
    pub bounds: Bounds<Pixels>,
    pub clip: Bounds<Pixels>,
}

/// Each open card view's last placement: its rect, whether it is hidden, and its holes.
pub type CardPlacement = ([f64; 4], bool, Vec<[f64; 4]>);

impl Client {
    /// Note the panes `events` put cards for, so their agents are attached and a card shows their
    /// latest reply. Read once per event as it arrives, never per frame.
    pub(super) fn note_cards<'a>(&mut self, events: impl IntoIterator<Item = &'a AgentEvent>) {
        let before = self.card_panes.len();
        for event in events {
            if let AgentEvent::Card { card: CardRef::Pane { pane } } = event {
                self.card_panes.insert(pane.clone());
            }
        }
        if self.card_panes.len() != before {
            self.sync_card_agents();
        }
    }

    /// Attach the agent of every agent pane a card names, as `sync_agents` does for a shown pane,
    /// and forget the panes that closed. Run when a card arrives and when the layout changes.
    pub(super) fn sync_card_agents(&mut self) {
        let spaces = &self.spaces;
        self.card_panes.retain(|pane| spaces.iter().flat_map(|s| s.all()).any(|l| &l.id == pane));
        if self.link != Link::Up {
            return;
        }
        let agents: Vec<String> = self.card_panes.iter()
            .filter_map(|pane| self.spaces.iter().flat_map(|s| s.all()).find(|l| &l.id == pane && l.kind == "agent").and_then(|l| l.agent.clone()))
            .collect();
        for agent in agents {
            if self.attached.insert(agent.clone()) {
                self.transcripts.entry(agent.clone()).or_default();
                self.send(&ClientMsg::AgentAttach { req: None, agent });
            }
        }
    }

    /// The row for `card` in `agent`'s transcript, row `ix`.
    pub(super) fn card_row(&self, agent: &str, ix: usize, card: &CardRef) -> gpui::Div {
        let frame = div().flex().flex_col().gap_1().p_2().rounded_md().border_1().border_color(rgb(0x2c3d34)).bg(rgb(0x1b2621));
        let head = |text: String| div().text_xs().text_color(rgb(0x8fa89b)).child(text);
        match card {
            CardRef::Pane { pane } => {
                let found = self.spaces.iter().find_map(|s| s.all().find(|l| &l.id == pane).map(|l| (s, l)));
                let Some((space, leaf)) = found else {
                    return frame.child(head(format!("pane {pane}"))).child(div().text_color(rgb(0x8fa89b)).child("This pane is closed."));
                };
                let mut line = div().flex().items_center().gap_2().text_color(rgb(0xd9e8e1));
                if let Some(status) = &leaf.status {
                    line = line.children(status::dot(status, &self.status_dots, gpui::ElementId::Name(format!("card-dot-{agent}-{ix}").into())));
                }
                let kind = match (leaf.kind.as_str(), leaf.harness.as_deref()) {
                    ("agent", Some(harness)) => harness.to_owned(),
                    (kind, _) => kind.to_owned(),
                };
                line = line.child(div().child(format!("{kind} · {}", leaf.label)))
                    .child(div().text_xs().text_color(rgb(0x6f8a7c)).child(format!("{} · {pane}", space.name)));
                let mut el = frame.child(line);
                if let Some(text) = leaf.agent.as_ref().and_then(|a| self.transcripts.get(a)).and_then(latest_reply) {
                    el = el.child(div().text_color(rgb(0xb9cbc2)).child(text));
                } else if let Some(status) = &leaf.status {
                    el = el.child(div().text_xs().text_color(rgb(0x8fa89b)).child(status::label(status)));
                }
                el
            }
            CardRef::Artifact { id, name, scope, space } => {
                let slot = CardSlot { id: id.clone(), name: name.clone(), scope: *scope, space: space.clone(), bounds: Bounds::default(), clip: Bounds::default() };
                let (slots, last, drew_in) = (self.card_slots.clone(), self.card_last.clone(), self.card_slots.scope());
                let page = canvas(
                    move |bounds, window, _| {
                        let clip = window.content_mask().bounds;
                        let slot = CardSlot { bounds, clip, ..slot };
                        // A slot that moved since the views were placed (a scroll) draws once more, so the
                        // view follows it.
                        if last.borrow().get(&slot.id) != Some(&(bounds, clip)) {
                            window.refresh();
                        }
                        slots.push(&drew_in, slot);
                    },
                    |_, _, _, _| (),
                ).w_full().h(px(PAGE_H));
                frame.child(head(format!("artifact · {name} ({})", scope.name()))).child(page)
            }
            CardRef::Unknown => frame.child(head("card".into())).child(div().text_color(rgb(0x8fa89b)).child("A card this version of Wheel does not draw.")),
        }
    }

    /// Open, place and close the artifact cards' views from the slots the last frame painted: a
    /// slot partly outside the column has that part cut out, one wholly outside is hidden, and a
    /// card no longer painted (scrolled away, or its pane gone) has its view closed.
    pub(super) fn sync_card_views(&mut self) {
        // A footer menu is laid out where it fits only once drawn, so the overlay that keeps card
        // views off it follows it each frame while it is open, and is cleared once after it closes.
        let menu = self.footer_menu_rect().is_some();
        if menu || self.footer_overlay {
            self.sync_sidebar_overlay();
            self.footer_overlay = menu;
        }
        // A replayed pane body keeps the slots it drew last; a body that drew again replaced its own.
        let slots: Vec<CardSlot> = self.card_slots.borrow().clone();
        self.card_slots.clear(super::scoped::SHELL);
        let over: Vec<Rect> = self.tray_holes().into_iter().chain(self.edge_bar()).chain(self.toast_frames.borrow().iter().copied()).collect();
        let at = |r: Rect| [f64::from(r.x), f64::from(r.y), f64::from(r.w), f64::from(r.h)];
        let mut seen = HashSet::new();
        for slot in &slots {
            if !seen.insert(slot.id.clone()) {
                continue;
            }
            if !self.card_views.contains_key(&slot.id) {
                let space = slot.space.clone().or_else(|| self.space_id.clone());
                let cwd = self.spaces.iter().find(|s| Some(&s.id) == space.as_ref()).map(|s| s.cwd.clone()).unwrap_or_default();
                self.native.open_artifact(&slot.id, &slot.name, slot.scope.name(), space.as_deref(), &cwd, &self.endpoint.ws);
                self.log(format!("card view opened: {} for artifact {} ({})", slot.id, slot.name, slot.scope.name()));
            }
            let b = rect_of(slot.bounds);
            let c = rect_of(slot.clip);
            let hidden = b.y >= c.y + c.h || b.y + b.h <= c.y || b.w <= 0. || b.h <= 0.;
            let mut holes: Vec<[f64; 4]> = over.iter().copied().map(at).collect();
            if b.y < c.y {
                holes.push(at(Rect { x: b.x, y: b.y, w: b.w, h: c.y - b.y }));
            }
            if b.y + b.h > c.y + c.h {
                holes.push(at(Rect { x: b.x, y: c.y + c.h, w: b.w, h: b.y + b.h - (c.y + c.h) }));
            }
            let placement = (at(b), hidden, holes);
            if self.card_views.get(&slot.id) != Some(&placement) {
                self.native.place(&slot.id, placement.0, placement.1, &placement.2, 1.);
                self.card_views.insert(slot.id.clone(), placement);
            }
        }
        let gone: Vec<String> = self.card_views.keys().filter(|id| !seen.contains(*id)).cloned().collect();
        for id in gone {
            self.native.close(&id);
            self.card_views.remove(&id);
        }
        *self.card_last.borrow_mut() = slots.into_iter().map(|s| (s.id, (s.bounds, s.clip))).collect();
    }

    /// The card views for `/state`: each open view's page id, rect, whether hidden, and its holes.
    pub(super) fn card_views_json(&self) -> serde_json::Value {
        let mut views: Vec<_> = self.card_views.iter().map(|(id, (rect, hidden, holes))| serde_json::json!({"id": id, "rect": rect, "hidden": hidden, "holes": holes})).collect();
        views.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        serde_json::json!(views)
    }
}

/// The last lines of the latest reply in `t`, at most [`REPLY_LINES`] of them.
fn latest_reply(t: &Transcript) -> Option<String> {
    let text = t.rows.iter().rev().find_map(|r| match &r.kind {
        Kind::Reply(x) if !x.trim().is_empty() => Some(x.trim()),
        _ => None,
    })?;
    // From the end, so a long reply streaming in costs only its last lines each frame.
    let start = text.rmatch_indices('\n').nth(REPLY_LINES - 1).map_or(0, |(i, _)| i + 1);
    Some(text[start..].to_owned())
}

fn rect_of(b: Bounds<Pixels>) -> Rect {
    Rect { x: f32::from(b.origin.x), y: f32::from(b.origin.y), w: f32::from(b.size.width), h: f32::from(b.size.height) }
}
