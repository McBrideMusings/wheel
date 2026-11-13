//! Where a pane dragged by its header lands in its own space (PaneDrop.swift's PaneDragging):
//! the middle of another pane swaps the two, the quarter nearest an edge docks the dragged pane
//! on that side. The client only previews it; wheeld's `pane_swap` and `pane_move` change the
//! tree.
use crate::layout::{Placed, Rect};
use wheel_proto::Dir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Center,
    Edge(Dir),
}

impl Zone {
    /// `center`, or the edge's side: `left`, `right`, `up` or `down`.
    pub fn label(self) -> &'static str {
        match self {
            Zone::Center => "center",
            Zone::Edge(Dir::Left) => "left",
            Zone::Edge(Dir::Right) => "right",
            Zone::Edge(Dir::Up) => "up",
            Zone::Edge(Dir::Down) => "down",
        }
    }
    pub fn parse(text: &str) -> Option<Zone> {
        Some(match text {
            "center" => Zone::Center,
            "left" => Zone::Edge(Dir::Left),
            "right" => Zone::Edge(Dir::Right),
            "up" => Zone::Edge(Dir::Up),
            "down" => Zone::Edge(Dir::Down),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridDrop {
    pub target: String,
    pub zone: Zone,
}

/// How much of a pane, measured in from each edge, counts as that edge's drop zone.
pub const EDGE_SHARE: f32 = 0.25;

/// The edge of `frame` nearest (`x`, `y`) and how far in it is, as a share of the frame's width
/// or height (PaneDragging.nearestEdge); right for an empty frame.
fn nearest(x: f32, y: f32, frame: Rect) -> (f32, Dir) {
    if frame.w <= 0. || frame.h <= 0. {
        return (f32::INFINITY, Dir::Right);
    }
    let fx = (x - frame.x) / frame.w;
    let fy = (y - frame.y) / frame.h;
    let near = [(fx, Dir::Left), (1. - fx, Dir::Right), (fy, Dir::Up), (1. - fy, Dir::Down)];
    near.iter().copied().fold(near[0], |a, b| if b.0 < a.0 { b } else { a })
}

/// The side of a pane at `frame` nearest (`x`, `y`): a pane arriving from another space always
/// takes a side, never swaps.
pub fn nearest_side(x: f32, y: f32, frame: Rect) -> Dir {
    nearest(x, y, frame).1
}

/// The zone of a pane at `frame` that (`x`, `y`) is in; the nearest edge when it is in two.
pub fn zone(x: f32, y: f32, frame: Rect) -> Zone {
    match nearest(x, y, frame) {
        (share, dir) if share < EDGE_SHARE => Zone::Edge(dir),
        _ => Zone::Center,
    }
}

/// The drop under (`x`, `y`): the pane holding it, other than the one dragged. None outside every
/// pane and over the dragged pane itself, where releasing cancels.
pub fn drop_at(x: f32, y: f32, leaves: &[Placed], dragging: &str) -> Option<GridDrop> {
    let placed = leaves.iter().find(|p| crate::sidebar::contains(p.rect, x, y))?;
    if placed.leaf.id == dragging {
        return None;
    }
    Some(GridDrop { target: placed.leaf.id.clone(), zone: zone(x, y, placed.rect) })
}

/// The drop under (`x`, `y`) on a popped card holding `leaves` (AppModel.poppedDrop): the card's
/// pane there, docked on its nearest side, since a card's tree and the grid's never swap. None
/// over the dragged pane itself or a gap between the card's panes.
pub fn card_drop_at(x: f32, y: f32, leaves: &[Placed], dragging: &str) -> Option<GridDrop> {
    let placed = leaves.iter().find(|p| crate::sidebar::contains(p.rect, x, y))?;
    if placed.leaf.id == dragging {
        return None;
    }
    Some(GridDrop { target: placed.leaf.id.clone(), zone: Zone::Edge(nearest(x, y, placed.rect).1) })
}

/// The part of the target's frame that lights up for `zone`: all of it for a swap, the half on
/// that side for a dock.
pub fn highlight(zone: Zone, f: Rect) -> Rect {
    let (w, h) = (f.w * 0.5, f.h * 0.5);
    match zone {
        Zone::Center => f,
        Zone::Edge(Dir::Left) => Rect { w, ..f },
        Zone::Edge(Dir::Right) => Rect { x: f.x + f.w - w, w, ..f },
        Zone::Edge(Dir::Up) => Rect { h, ..f },
        Zone::Edge(Dir::Down) => Rect { y: f.y + f.h - h, h, ..f },
    }
}

/// What releasing does, in words, for the debug state.
pub fn hint(drop: &GridDrop, pane: &str) -> String {
    match drop.zone {
        Zone::Center => format!("Swap {pane} with {}", drop.target),
        Zone::Edge(_) => format!("Dock {pane} on the {} of {}", drop.zone.label(), drop.target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Leaf;

    fn placed(id: &str, x: f32, w: f32) -> Placed {
        Placed { leaf: Leaf { id: id.into(), kind: "terminal".into(), label: String::new(), term: None, url: None, scope: None, agent: None, harness: None, home: None, status: None }, rect: Rect { x, y: 0., w, h: 100. } }
    }

    #[test]
    fn the_middle_swaps_and_an_edge_quarter_docks() {
        let f = Rect { x: 0., y: 0., w: 200., h: 100. };
        assert_eq!(zone(100., 50., f), Zone::Center);
        assert_eq!(zone(10., 50., f), Zone::Edge(Dir::Left));
        assert_eq!(zone(190., 50., f), Zone::Edge(Dir::Right));
        assert_eq!(zone(100., 5., f), Zone::Edge(Dir::Up));
        assert_eq!(zone(100., 95., f), Zone::Edge(Dir::Down));
        // In two zones at once the nearer edge wins: 4% from the left beats 10% from the top.
        assert_eq!(zone(8., 10., f), Zone::Edge(Dir::Left));
    }

    #[test]
    fn the_dragged_pane_and_the_gaps_drop_nothing() {
        let leaves = [placed("p1", 0., 100.), placed("p2", 110., 100.)];
        assert_eq!(drop_at(50., 50., &leaves, "p1"), None);
        assert_eq!(drop_at(105., 50., &leaves, "p1"), None);
        assert_eq!(drop_at(160., 50., &leaves, "p1"), Some(GridDrop { target: "p2".into(), zone: Zone::Center }));
        assert_eq!(drop_at(115., 50., &leaves, "p1"), Some(GridDrop { target: "p2".into(), zone: Zone::Edge(Dir::Left) }));
    }

    #[test]
    fn a_card_pane_takes_its_nearest_side_even_in_its_middle() {
        let leaves = [placed("p3", 100., 200.)];
        assert_eq!(card_drop_at(200., 50., &leaves, "p1").map(|d| d.zone), Some(Zone::Edge(Dir::Left)));
        assert_eq!(card_drop_at(210., 50., &leaves, "p1").map(|d| d.zone), Some(Zone::Edge(Dir::Right)));
        assert_eq!(card_drop_at(200., 40., &leaves, "p1").map(|d| d.zone), Some(Zone::Edge(Dir::Up)));
        assert_eq!(card_drop_at(150., 50., &leaves, "p3"), None);
        assert_eq!(card_drop_at(50., 50., &leaves, "p1"), None);
    }

    #[test]
    fn a_dock_lights_the_half_on_its_side() {
        let f = Rect { x: 10., y: 20., w: 200., h: 100. };
        assert_eq!(highlight(Zone::Center, f), f);
        assert_eq!(highlight(Zone::Edge(Dir::Right), f), Rect { x: 110., y: 20., w: 100., h: 100. });
        assert_eq!(highlight(Zone::Edge(Dir::Down), f), Rect { x: 10., y: 70., w: 200., h: 50. });
        for z in ["center", "left", "right", "up", "down"] {
            assert_eq!(Zone::parse(z).unwrap().label(), z);
        }
    }
}
