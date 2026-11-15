//! The Agents mode's surface (wheel-0but.3): every agent of the session as a card the client
//! pans and zooms. What is pure lives here: the camera, the default order and frames, and which
//! cards are closed or drawn as one line. Card frames are kept by `popped::Frames` in their own
//! file; all of it is this client's (ADR-0022) and never reaches wheeld.
use crate::layout::Rect;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::{BTreeSet, HashMap}, path::PathBuf, rc::Rc};
use wheel_proto::PaneStatus;

/// A card's size when it has no stored frame, and the gap between cards laid by default.
pub const CARD: (f32, f32) = (340., 320.);
pub const GAP: f32 = 16.;
/// The smallest a card is resized to, in surface points.
pub const LEAST: (f32, f32) = crate::popped::LEAST;
/// Below this zoom a card is its state dot and its name.
pub const DOT_BELOW: f32 = 0.5;
pub const ZOOM_MIN: f32 = 0.2;
pub const ZOOM_MAX: f32 = 2.0;
/// The height of a one-line card, in surface points.
pub const SUMMARY_H: f32 = 34.;

/// Where the surface's origin sits in the grid area and how large it draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub pan: (f32, f32),
    pub zoom: f32,
}
impl Default for Camera {
    fn default() -> Self {
        Self { pan: (GAP, GAP), zoom: 1. }
    }
}
impl Camera {
    /// `r` on the surface as drawn in a grid area at `area` (window coordinates).
    pub fn to_screen(&self, area: Rect, r: Rect) -> Rect {
        Rect { x: area.x + self.pan.0 + r.x * self.zoom, y: area.y + self.pan.1 + r.y * self.zoom, w: r.w * self.zoom, h: r.h * self.zoom }
    }
    /// The surface point under window point (`x`, `y`).
    pub fn to_world(&self, area: Rect, (x, y): (f32, f32)) -> (f32, f32) {
        ((x - area.x - self.pan.0) / self.zoom, (y - area.y - self.pan.1) / self.zoom)
    }
    /// Zoom by `factor`, keeping the surface point under (`x`, `y`) where it is.
    pub fn zoom_about(&mut self, area: Rect, at: (f32, f32), factor: f32) {
        let (wx, wy) = self.to_world(area, at);
        self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.pan = (at.0 - area.x - wx * self.zoom, at.1 - area.y - wy * self.zoom);
    }
    /// Put the middle of `r` in the middle of the grid area.
    pub fn centre_on(&mut self, area: Rect, r: Rect) {
        self.pan = (area.w / 2. - (r.x + r.w / 2.) * self.zoom, area.h / 2. - (r.y + r.h / 2.) * self.zoom);
    }
    pub fn dots(&self) -> bool {
        self.zoom < DOT_BELOW
    }
    /// The readout at the surface's bottom right: "100%".
    pub fn readout(&self) -> String {
        format!("{}%", (self.zoom * 100.).round())
    }
}

/// Where a status sits in the default order: needs you, stopped, working, done, idle; a pane with
/// no status yet counts as idle.
pub fn rank(status: Option<&PaneStatus>) -> usize {
    let name = status.map_or("idle", crate::status::rank);
    crate::status::RANKS.iter().position(|r| *r == name).unwrap_or(crate::status::RANKS.len())
}

/// What the default order sorts by: the status's rank, then the pane id, so cards of one rank keep
/// one order from frame to frame.
pub fn order_key<'a>(status: Option<&PaneStatus>, pane: &'a str) -> (usize, &'a str) {
    (rank(status), pane)
}

/// The frame of the card in default slot `slot` on a surface `width` wide at zoom 1: rows of
/// cards, left to right, as many across as fit (at least one).
pub fn default_frame(slot: usize, width: f32) -> Rect {
    let across = (((width - GAP) / (CARD.0 + GAP)).floor() as usize).max(1);
    let (col, row) = (slot % across, slot / across);
    Rect { x: col as f32 * (CARD.0 + GAP), y: row as f32 * (CARD.1 + GAP), w: CARD.0, h: CARD.1 }
}

/// A frame being moved by (`dx`, `dy`) or resized by it, in surface points; a resize stops at `LEAST`.
pub fn dragged(base: Rect, resize: bool, (dx, dy): (f32, f32)) -> Rect {
    if resize {
        Rect { w: (base.w + dx).max(LEAST.0), h: (base.h + dy).max(LEAST.1), ..base }
    } else {
        Rect { x: base.x + dx, y: base.y + dy, ..base }
    }
}

/// The closed cards (by agent name, so a new agent in the same pane shows) and the cards drawn as
/// one line (by pane), saved to `file` when there is one.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub hidden: BTreeSet<String>,
    #[serde(default)]
    pub summary: BTreeSet<String>,
    #[serde(skip)]
    file: Option<PathBuf>,
}
impl Prefs {
    /// The prefs saved in `file`; a missing or unreadable file reads as none closed.
    pub fn load(file: Option<PathBuf>) -> Self {
        let read = file.as_ref().and_then(|f| std::fs::read(f).ok());
        let mut prefs: Prefs = match read.map(|bytes| serde_json::from_slice::<Prefs>(&bytes)) {
            Some(Ok(p)) => p,
            Some(Err(error)) => {
                eprintln!("wheel-gpui: the agent card settings in {} were not read: {error}", file.as_ref().map(|f| f.display().to_string()).unwrap_or_default());
                Prefs::default()
            }
            None => Prefs::default(),
        };
        prefs.file = file;
        prefs
    }
    /// Close or show `agent`'s card.
    pub fn set_hidden(&mut self, agent: &str, hidden: bool) {
        if hidden { self.hidden.insert(agent.to_owned()) } else { self.hidden.remove(agent) };
        self.save();
    }
    /// Draw `pane`'s card as one line, or as the full chat.
    pub fn set_summary(&mut self, pane: &str, summary: bool) {
        if summary { self.summary.insert(pane.to_owned()) } else { self.summary.remove(pane) };
        self.save();
    }
    fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Err(error) = crate::popped::write_json(file, self) {
            eprintln!("wheel-gpui: the agent card settings were not saved to {}: {error}", file.display());
        }
    }
    /// The one map every window of this process reads and writes for `file`; `fresh` reads it
    /// from the disk again, as a launch does.
    pub fn shared(file: Option<PathBuf>, fresh: bool) -> Rc<RefCell<Prefs>> {
        thread_local! {
            static SHARED: RefCell<HashMap<Option<PathBuf>, Rc<RefCell<Prefs>>>> = RefCell::default();
        }
        SHARED.with(|s| {
            let mut s = s.borrow_mut();
            if fresh {
                s.remove(&file);
            }
            s.entry(file.clone()).or_insert_with(|| Rc::new(RefCell::new(Prefs::load(file)))).clone()
        })
    }
}

/// The two files the surface keeps for the daemon at `ws`: card frames and prefs.
pub fn default_files(ws: &str) -> (PathBuf, PathBuf) {
    (crate::popped::client_file("agent-card-frames", ws), crate::popped::client_file("agent-card-prefs", ws))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 200., y: 50., w: 1000., h: 600. };

    #[test]
    fn the_camera_maps_both_ways_and_zooms_about_the_pointer() {
        let mut c = Camera::default();
        let r = Rect { x: 100., y: 40., w: 300., h: 200. };
        assert_eq!(c.to_screen(AREA, r), Rect { x: 316., y: 106., w: 300., h: 200. });
        assert_eq!(c.to_world(AREA, (316., 106.)), (100., 40.));
        let before = c.to_world(AREA, (500., 300.));
        c.zoom_about(AREA, (500., 300.), 0.5);
        assert_eq!(c.zoom, 0.5);
        let after = c.to_world(AREA, (500., 300.));
        assert!((before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01, "{before:?} {after:?}");
        assert!(!c.dots() && c.readout() == "50%");
        c.zoom_about(AREA, (500., 300.), 0.5);
        assert!(c.dots() && c.readout() == "25%");
        c.zoom_about(AREA, (500., 300.), 0.1);
        assert_eq!(c.zoom, ZOOM_MIN);
        c.zoom_about(AREA, (500., 300.), 100.);
        assert_eq!(c.zoom, ZOOM_MAX);
    }

    #[test]
    fn centring_puts_a_card_in_the_middle() {
        let mut c = Camera { pan: (0., 0.), zoom: 0.5 };
        let r = Rect { x: 1000., y: 1000., w: 200., h: 100. };
        c.centre_on(AREA, r);
        let s = c.to_screen(AREA, r);
        assert_eq!((s.x + s.w / 2., s.y + s.h / 2.), (AREA.x + AREA.w / 2., AREA.y + AREA.h / 2.));
    }

    #[test]
    fn the_default_order_puts_needs_you_first_and_idle_last() {
        let needs = PaneStatus::NeedsYou { waiting: 1, reason: None };
        let order: Vec<usize> = [Some(&needs), Some(&PaneStatus::Stopped), Some(&PaneStatus::Working), Some(&PaneStatus::Done), Some(&PaneStatus::Idle), None].iter().map(|s| rank(*s)).collect();
        assert_eq!(order, vec![0, 1, 2, 3, 4, 4]);
    }

    #[test]
    fn the_order_is_by_rank_then_by_pane() {
        let needs = PaneStatus::NeedsYou { waiting: 1, reason: None };
        let mut cards = vec![("p9", None), ("p3", Some(&PaneStatus::Done)), ("p1", None), ("p7", Some(&needs)), ("p5", Some(&PaneStatus::Stopped)), ("p2", Some(&PaneStatus::Working))];
        cards.sort_by(|a, b| order_key(a.1, a.0).cmp(&order_key(b.1, b.0)));
        assert_eq!(cards.iter().map(|c| c.0).collect::<Vec<_>>(), ["p7", "p5", "p2", "p3", "p1", "p9"]);
    }

    #[test]
    fn default_frames_fill_rows_as_wide_as_the_surface() {
        assert_eq!(default_frame(0, 1000.), Rect { x: 0., y: 0., w: CARD.0, h: CARD.1 });
        assert_eq!(default_frame(1, 1000.), Rect { x: CARD.0 + GAP, y: 0., w: CARD.0, h: CARD.1 });
        assert_eq!(default_frame(2, 1000.), Rect { x: 0., y: CARD.1 + GAP, w: CARD.0, h: CARD.1 });
        // A surface narrower than one card still lays one per row.
        assert_eq!(default_frame(1, 100.), Rect { x: 0., y: CARD.1 + GAP, w: CARD.0, h: CARD.1 });
    }

    #[test]
    fn a_resize_stops_at_the_least_size() {
        let base = Rect { x: 10., y: 10., w: 300., h: 200. };
        assert_eq!(dragged(base, false, (5., -5.)), Rect { x: 15., y: 5., w: 300., h: 200. });
        assert_eq!(dragged(base, true, (-500., 40.)), Rect { x: 10., y: 10., w: LEAST.0, h: 240. });
    }

    #[test]
    fn prefs_are_saved_and_read_back() {
        let file = std::env::temp_dir().join(format!("wheel-gpui-card-prefs-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let mut prefs = Prefs::load(Some(file.clone()));
        prefs.set_hidden("w1", true);
        prefs.set_summary("p2", true);
        let read = Prefs::load(Some(file.clone()));
        assert!(read.hidden.contains("w1") && read.summary.contains("p2"));
        prefs.set_hidden("w1", false);
        assert!(Prefs::load(Some(file.clone())).hidden.is_empty());
        std::fs::write(&file, b"not json").unwrap();
        assert_eq!(Prefs::load(Some(file.clone())).hidden.len(), 0);
        assert!(default_files("ws://127.0.0.1:7422/ws").0.ends_with("agent-card-frames-7422.json"));
        let _ = std::fs::remove_file(&file);
    }
}
