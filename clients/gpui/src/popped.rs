//! A space's popped-out trees drawn as cards over its grid (SpacesView.swift `PoppedWindow`,
//! AppModel.swift `poppedFrame`, `poppedSize`, `poppedWindows`). Where each card sits is this
//! client's own (ADR-0022): wheeld never hears it, and a second client keeps its own.
use crate::layout::{Divider, Placed, Rect, Tree, leaves_of};
use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

/// The smallest a card is drawn, as the Swift client's.
pub const LEAST: (f32, f32) = (280., 180.);
/// The square at a card's bottom right corner that resizes it.
pub const GRIP: f32 = 16.;

/// Where card `index` sits in a grid `w` by `h` (grid coordinates, top left origin): where one of
/// its panes was left, else a cascade step from the grid's top left; always inside the grid.
pub fn frame(remembered: Option<Rect>, index: usize, w: f32, h: f32) -> Rect {
    let step = index as f32 * 36.;
    let f = remembered.unwrap_or(Rect { x: w * 0.4 + step, y: h * 0.1 + step, w: (w * 0.5).max(320.), h: (h * 0.5).max(220.) });
    let (fw, fh) = (f.w.min(w.max(1.)), f.h.min(h.max(1.)));
    Rect { x: f.x.min((w - fw).max(0.)).max(0.), y: f.y.min((h - fh).max(0.)).max(0.), w: fw, h: fh }
}

/// The size a card draws at from its placed frame and a resize in progress: never below `LEAST`,
/// never past the grid's right or bottom edge.
pub fn size(base: Rect, grow: (f32, f32), w: f32, h: f32) -> (f32, f32) {
    ((base.w + grow.0).max(LEAST.0).min((w - base.x).max(LEAST.0)), (base.h + grow.1).max(LEAST.1).min((h - base.y).max(LEAST.1)))
}

/// How near the grid's edge a card's pointer is to send the card's panes to the tray.
pub const EDGE_REACH: f32 = 14.;
/// How thick the bar lit along that edge is.
pub const EDGE_BAR: f32 = 8.;

/// The edge of a grid at `area` that a card dragged with the pointer at (`x`, `y`) is dropped on
/// (PoppedWindow's edge(of:)): within `EDGE_REACH` of it, left and right before up and down.
pub fn hot_edge(x: f32, y: f32, area: Rect) -> Option<wheel_proto::Dir> {
    use wheel_proto::Dir;
    let (x, y) = (x - area.x, y - area.y);
    if x < EDGE_REACH {
        Some(Dir::Left)
    } else if x > area.w - EDGE_REACH {
        Some(Dir::Right)
    } else if y < EDGE_REACH {
        Some(Dir::Up)
    } else if y > area.h - EDGE_REACH {
        Some(Dir::Down)
    } else {
        None
    }
}

/// The bar lit along `edge` of a grid at `area` (EdgeBar).
pub fn edge_bar(edge: wheel_proto::Dir, area: Rect) -> Rect {
    use wheel_proto::Dir;
    match edge {
        Dir::Left => Rect { w: EDGE_BAR, ..area },
        Dir::Right => Rect { x: area.x + area.w - EDGE_BAR, w: EDGE_BAR, ..area },
        Dir::Up => Rect { h: EDGE_BAR, ..area },
        Dir::Down => Rect { y: area.y + area.h - EDGE_BAR, h: EDGE_BAR, ..area },
    }
}

/// The order the cards draw in, bottom first: the one holding the focus last, so it is on top,
/// and the others in the order wheeld lists them.
pub fn order(trees: &[Tree], focus: Option<&str>) -> Vec<usize> {
    let holds = |t: &Tree| focus.is_some_and(|f| t.holds(f));
    let (top, rest): (Vec<usize>, Vec<usize>) = (0..trees.len()).partition(|&i| holds(&trees[i]));
    rest.into_iter().chain(top).collect()
}

/// One popped tree as drawn: its panes, its frame and its resize corner, in window coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub panes: Vec<String>,
    pub frame: Rect,
    pub grip: Rect,
}

/// One layer of what the grid draws: the space's grid (no card), or one card and its panes.
pub struct Layer {
    pub card: Option<Card>,
    pub leaves: Vec<Placed>,
    pub dividers: Vec<Divider>,
}

/// A drag on a card: its header moves it, its corner resizes it. `pane` is any pane of the card.
#[derive(Clone, Debug, PartialEq)]
pub struct CardDrag {
    pub pane: String,
    pub resize: bool,
    pub start: (f32, f32),
    pub delta: (f32, f32),
}

/// The frames cards were left at, by pane id, in grid coordinates; saved to `file` when there is one.
#[derive(Default)]
pub struct Frames {
    by_pane: HashMap<String, Rect>,
    file: Option<PathBuf>,
}

impl Frames {
    /// The frames saved in `file`, which later changes are written to; none read or written without one.
    pub fn load(file: Option<PathBuf>) -> Self {
        let by_pane = file.as_ref().and_then(|f| std::fs::read(f).ok())
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, [f32; 4]>>(&bytes).ok())
            .map(|raw| raw.into_iter().map(|(id, [x, y, w, h])| (id, Rect { x, y, w, h })).collect())
            .unwrap_or_default();
        Self { by_pane, file }
    }
    /// The file this client keeps them in for the daemon at `ws`: pane ids are a daemon's own, so
    /// each daemon (by port) gets its file.
    pub fn default_file(ws: &str) -> PathBuf {
        let host = ws.split("://").nth(1).unwrap_or(ws).split(['/', '?']).next().unwrap_or_default();
        let port = host.rsplit_once(':').map_or("default", |(_, p)| p);
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(format!("Library/Application Support/wheel-gpui/popped-frames-{port}.json"))
    }
    /// The one map every window of this process reads and writes for `file` (none: kept in memory
    /// only), as the Swift app keeps one in its model.
    pub fn shared(file: Option<PathBuf>) -> Rc<RefCell<Frames>> {
        thread_local! {
            static SHARED: RefCell<HashMap<Option<PathBuf>, Rc<RefCell<Frames>>>> = RefCell::default();
        }
        SHARED.with(|s| s.borrow_mut().entry(file.clone()).or_insert_with(|| Rc::new(RefCell::new(Frames::load(file)))).clone())
    }
    /// Where one of `tree`'s panes was left.
    pub fn remembered(&self, tree: &Tree) -> Option<Rect> {
        leaves_of(tree).iter().find_map(|l| self.by_pane.get(&l.id).copied())
    }
    /// `tree`'s card was left at `frame`: every pane in it remembers it.
    pub fn set(&mut self, tree: &Tree, frame: Rect) {
        for leaf in leaves_of(tree) {
            self.by_pane.insert(leaf.id.clone(), frame);
        }
        let Some(file) = &self.file else { return };
        let raw: HashMap<&String, [f32; 4]> = self.by_pane.iter().map(|(id, r)| (id, [r.x, r.y, r.w, r.h])).collect();
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec(&raw)
            && let Err(error) = std::fs::write(file, bytes)
        {
            eprintln!("wheel-gpui: the popped frames were not saved to {}: {error}", file.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Leaf;

    fn tree(id: &str) -> Tree {
        Tree::Pane(Leaf { id: id.into(), kind: "empty".into(), label: String::new(), term: None, url: None, scope: None, agent: None, harness: None, home: None, status: None })
    }

    #[test]
    fn a_card_near_the_grid_edge_lights_that_edge() {
        use wheel_proto::Dir;
        let area = Rect { x: 100., y: 50., w: 400., h: 300. };
        assert_eq!(hot_edge(110., 200., area), Some(Dir::Left));
        assert_eq!(hot_edge(490., 200., area), Some(Dir::Right));
        assert_eq!(hot_edge(300., 60., area), Some(Dir::Up));
        assert_eq!(hot_edge(300., 340., area), Some(Dir::Down));
        assert_eq!(hot_edge(300., 200., area), None);
        // A corner counts as its side edge, as the Swift client's does.
        assert_eq!(hot_edge(105., 55., area), Some(Dir::Left));
        assert_eq!(edge_bar(Dir::Left, area), Rect { x: 100., y: 50., w: EDGE_BAR, h: 300. });
        assert_eq!(edge_bar(Dir::Down, area), Rect { x: 100., y: 350. - EDGE_BAR, w: 400., h: EDGE_BAR });
    }

    #[test]
    fn a_card_cascades_from_the_top_left_until_it_is_moved() {
        assert_eq!(frame(None, 0, 1000., 600.), Rect { x: 400., y: 60., w: 500., h: 300. });
        assert_eq!(frame(None, 1, 1000., 600.), Rect { x: 436., y: 96., w: 500., h: 300. });
        // A small grid: the least fallback size, pulled back inside.
        assert_eq!(frame(None, 0, 500., 300.), Rect { x: 180., y: 30., w: 320., h: 220. });
    }

    #[test]
    fn a_remembered_frame_is_kept_inside_the_grid() {
        let left = Rect { x: 900., y: -40., w: 400., h: 300. };
        assert_eq!(frame(Some(left), 0, 1000., 600.), Rect { x: 600., y: 0., w: 400., h: 300. });
        let huge = Rect { x: 10., y: 10., w: 2000., h: 900. };
        assert_eq!(frame(Some(huge), 0, 1000., 600.), Rect { x: 0., y: 0., w: 1000., h: 600. });
    }

    #[test]
    fn a_resize_stops_at_the_least_size_and_the_grid_edge() {
        let base = Rect { x: 600., y: 100., w: 300., h: 200. };
        assert_eq!(size(base, (-200., -200.), 1000., 600.), LEAST);
        assert_eq!(size(base, (500., 500.), 1000., 600.), (400., 500.));
        assert_eq!(size(base, (20., 30.), 1000., 600.), (320., 230.));
    }

    #[test]
    fn the_focused_card_draws_last() {
        let trees = [tree("a"), tree("b"), tree("c")];
        assert_eq!(order(&trees, None), vec![0, 1, 2]);
        assert_eq!(order(&trees, Some("a")), vec![1, 2, 0]);
        assert_eq!(order(&trees, Some("zz")), vec![0, 1, 2]);
    }

    #[test]
    fn frames_are_saved_and_read_back_by_pane() {
        let file = std::env::temp_dir().join(format!("wheel-gpui-popped-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let mut frames = Frames::load(Some(file.clone()));
        let r = Rect { x: 1., y: 2., w: 300., h: 200. };
        frames.set(&tree("p7"), r);
        assert_eq!(Frames::load(Some(file.clone())).remembered(&tree("p7")), Some(r));
        assert_eq!(Frames::load(None).remembered(&tree("p7")), None);
        assert!(Frames::default_file("ws://127.0.0.1:7422/ws?token=x").ends_with("popped-frames-7422.json"));
        // Two windows share one map.
        Frames::shared(None).borrow_mut().set(&tree("p9"), r);
        assert_eq!(Frames::shared(None).borrow().remembered(&tree("p9")), Some(r));
        let _ = std::fs::remove_file(&file);
    }
}
