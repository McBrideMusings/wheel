//! Grid motion and zoom (GridMotion.swift, AppModel.toggleZoom, SpaceGrid): a layout change
//! animates the grid's panes from where they were drawn, and a zoomed pane fills its space while
//! the others slide out past the grid's edge, or, under Reduce Motion, stay in place and fade.
use super::*;
use crate::motion::{self, Motion};
use std::time::Instant;

impl Client {
    /// The pane `space` shows alone: one still in its grid and holding this window's focus, so
    /// keys never go to a hidden pane (AppModel.zoomedPane).
    pub fn zoomed_pane(&self, space: &Space) -> Option<&str> {
        let pane = self.zoomed.get(&space.id)?;
        (self.focus.current.as_ref() == Some(pane) && space.root.holds(pane)).then_some(pane.as_str())
    }
    /// The shown space's zoomed pane.
    pub(super) fn zoomed_here(&self) -> Option<&str> {
        self.space().and_then(|s| self.zoomed_pane(s))
    }
    /// Show `pane` alone in its space, or put the grid back when it already is.
    pub fn toggle_zoom(&mut self, pane: &str) {
        let Some(space) = self.space_holding(pane) else { return };
        let (space_id, in_grid) = (space.id.clone(), space.root.holds(pane));
        let zoom = self.zoomed_pane(space).is_none_or(|z| z != pane);
        if zoom && !in_grid {
            self.log(format!("pane {pane}: zoom refused, it is not in the grid"));
            return;
        }
        if zoom {
            self.focus_pane(pane);
            self.zoomed.insert(space_id.clone(), pane.to_owned());
        } else {
            self.zoomed.remove(&space_id);
        }
        // Under Reduce Motion the others fade with this motion: out as the pane grows, back in as it shrinks.
        if self.start_motion(&space_id, if zoom { "zoom" } else { "unzoom" }) {
            self.zoom_fade = true;
            self.unzoomed = (!zoom).then(|| pane.to_owned());
        }
        self.log(format!("pane {pane}: {}", if zoom { "zoomed" } else { "unzoomed" }));
    }
    /// The header's zoom button: (enabled, on). Disabled while the space's grid holds one pane
    /// and the pane is not zoomed (AppModel.zoomButton).
    pub fn zoom_button(&self, pane: &str) -> (bool, bool) {
        let on = self.space_holding(pane).is_some_and(|s| self.zoomed_pane(s) == Some(pane));
        let many = self.space_holding(pane).is_some_and(|s| s.root.holds(pane) && leaves_of(&s.root).len() > 1);
        (on || many, on)
    }
    /// `zoom-pane`: offered while the shown space has a zoomed pane or more than one in its grid.
    pub(super) fn zoom_enabled(&self) -> bool {
        self.zoomed_here().is_some() || self.space().is_some_and(|s| leaves_of(&s.root).len() > 1)
    }
    /// A split, a close or a site change ends the zoom of the pane's space: a new pane would sit
    /// hidden behind the zoomed one (AppModel.split, closePane, setSite).
    pub(super) fn unzoom_for(&mut self, message: &ClientMsg) {
        let pane = match message {
            ClientMsg::PaneSplit { pane, .. } | ClientMsg::PaneClose { pane, .. } | ClientMsg::PaneSetSite { pane, .. } => pane,
            _ => return,
        };
        if let Some(space) = self.space_holding(pane).map(|s| s.id.clone()) {
            self.zoomed.remove(&space);
        }
    }
    /// Animate the shown grid from where its panes were drawn last. Returns whether it started.
    pub(super) fn start_motion(&mut self, space: &str, why: &str) -> bool {
        if self.space_id.as_deref() != Some(space) || self.grid_drawn.is_empty() {
            return false;
        }
        self.unzoomed = None;
        self.zoom_fade = false;
        let length = motion::duration();
        self.log(format!("grid motion: {why}, {} pane(s) over {} ms", self.grid_drawn.len(), length.as_millis()));
        self.motion = Some(Motion::new(space, self.grid_drawn.clone(), Instant::now(), length));
        true
    }
    /// A new layout for the shown space that moves its panes animates (AppModel.apply's `animated`).
    /// The old tree is compared with the ratios a divider drag drew (`dragged`), so wheeld
    /// confirming a drag moves nothing and starts no motion.
    pub(super) fn animate_layout(&mut self, old: Option<Tree>, dragged: &HashMap<String, f64>) {
        let Some(mut old) = old else { return };
        let Some(space) = self.space() else { return };
        motion::set_ratios(&mut old, dragged);
        if !motion::same_shape(&old, &space.root) {
            let id = space.id.clone();
            self.start_motion(&id, "layout");
        }
    }
    /// The grid's panes as the zoom and a running animation draw them: the zoomed pane takes the
    /// whole grid and draws last, every other one slides out past its nearest edge (stays in place
    /// under Reduce Motion, faded by `grid_opacity`), and no divider shows while zoomed or moving.
    pub(super) fn move_grid(&self, space: &Space, area: Rect, leaves: &mut Vec<Placed>, dividers: &mut Vec<Divider>) {
        let zoomed = self.zoomed_pane(space).map(str::to_owned);
        if let Some(z) = &zoomed {
            for p in leaves.iter_mut() {
                p.rect = if &p.leaf.id == z {
                    area
                } else if self.reduce_motion {
                    p.rect
                } else {
                    motion::slid_out(p.rect, area)
                };
            }
            // Stable: the zoomed pane goes last and the others keep their order.
            leaves.sort_by_key(|p| &p.leaf.id == z);
        }
        let moving = self.motion.as_ref().filter(|m| m.space == space.id);
        if let Some(m) = moving {
            let now = self.motion_now();
            for p in leaves.iter_mut() {
                p.rect = m.frame(&p.leaf.id, p.rect, now);
            }
        }
        // The dividers come back where the layout ends.
        if zoomed.is_some() || moving.is_some() {
            dividers.clear();
        }
    }
    /// The time the grid's motion is drawn at: now, or the point `motion_hold` holds it at.
    fn motion_now(&self) -> Instant {
        match (self.motion_hold, &self.motion) {
            (Some(f), Some(m)) => m.started + m.length.mul_f32(f.clamp(0., 1.)),
            _ => Instant::now(),
        }
    }
    /// Once per frame: keep drawing while the grid moves, and end the motion when its time is up.
    /// Returns whether it still runs.
    pub(super) fn tick_motion(&mut self, window: &mut Window) -> bool {
        let now = self.motion_now();
        // A motion belongs to its space: showing another ends it.
        if self.motion.as_ref().is_some_and(|m| self.space_id.as_deref() != Some(m.space.as_str())) {
            self.motion = None;
        }
        match &self.motion {
            Some(m) if !m.done(now) => {
                window.request_animation_frame();
                true
            }
            Some(_) => {
                self.motion = None;
                self.unzoomed = None;
                self.zoom_fade = false;
                self.log("grid motion: done".to_owned());
                false
            }
            None => false,
        }
    }
    /// Whether a terminal's size is a step of a move whose end settles it (AppModel.settle): the
    /// grid animates or a divider is under the pointer. A window edge being dragged the bridge
    /// holds on its own, as AppKit posts the live resize.
    pub(super) fn resizes_wanted(&self) -> bool {
        self.motion.is_some() || self.drag.is_some()
    }
    /// Before the views are placed: a move that began holds the terminals' resizes.
    pub(super) fn hold_resizes(&mut self) {
        if self.resizes_wanted() && !self.resizes_held {
            self.native.hold_resizes(true);
            self.resizes_held = true;
        }
    }
    /// After the views are placed where every move ends: each terminal sends the size it ended at.
    pub(super) fn release_resizes(&mut self) {
        if !self.resizes_wanted() && self.resizes_held {
            self.native.hold_resizes(false);
            self.resizes_held = false;
        }
    }
    /// What a grid view has cut out while the grid moves or is zoomed (none otherwise): the
    /// window outside the grid at `area`, so a view sliding past its edge never draws over the
    /// sidebar or the title row, and, under another pane, the zoomed pane growing over it.
    pub(super) fn grid_cuts(&self, pane: &str, area: Rect, grid: &[Placed]) -> Vec<Rect> {
        let zoomed = self.zoomed_here();
        if zoomed.is_none() && self.motion.is_none() {
            return vec![];
        }
        let mut cuts: Vec<Rect> = motion::outside(area, self.viewport.0, self.viewport.1).into_iter().filter(|r| r.w > 0. && r.h > 0.).collect();
        if let Some(z) = zoomed.filter(|z| *z != pane)
            && let Some(top) = grid.iter().find(|p| p.leaf.id == z)
        {
            cuts.push(top.rect);
        }
        cuts
    }
    /// How opaque grid pane `pane` draws: under Reduce Motion the panes a zoom covers fade out
    /// and back in (`motion::faded`); otherwise every one draws whole.
    pub(super) fn grid_opacity(&self, pane: &str) -> f32 {
        if !self.reduce_motion {
            return 1.;
        }
        let progress = self.motion.as_ref().filter(|m| self.zoom_fade && self.space_id.as_deref() == Some(m.space.as_str())).map(|m| m.progress(self.motion_now()));
        motion::faded(pane, self.zoomed_here(), self.unzoomed.as_deref(), progress)
    }
    /// What `/state` reports of the grid's motion and zoom.
    pub(super) fn motion_json(&self) -> serde_json::Value {
        let now = self.motion_now();
        serde_json::json!({
            // Until the frame that ends it draws, so the views are placed where the layout ends.
            "moving": self.motion.is_some(),
            "progress": self.motion.as_ref().map(|m| m.progress(now)),
            "length_ms": motion::duration().as_millis() as u64,
            "resizes_held": self.resizes_held,
            "dragging_divider": self.drag.is_some(),
            "zoomed": self.zoomed_here(),
            "reduce_motion": self.reduce_motion,
        })
    }
}
