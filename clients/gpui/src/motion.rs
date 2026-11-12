//! The grid's layout animation (GridMotion.swift): a split, close, swap, move or zoom slides each
//! pane from where it was drawn to where the new layout puts it, over 0.22 s, and a zoomed pane
//! grows over the grid while the others slide out past the edge they are nearest to.
use crate::layout::{Rect, Tree};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a grid layout change lasts at normal speed, in seconds.
pub const LAYOUT: f32 = 0.22;
/// The scale a pane the layout adds starts at, around its centre (SwiftUI's `.scale(0.96)` transition).
pub const ENTER_SCALE: f32 = 0.96;

/// `WHEEL_SLOW_ANIMATIONS=<n>` multiplies every grid animation's length by `n`; unset, empty, zero
/// or not a number means 1.
pub fn slowdown(raw: Option<&str>) -> f32 {
    raw.and_then(|r| r.trim().parse::<f32>().ok()).filter(|n| n.is_finite() && *n > 0.).unwrap_or(1.)
}

/// The length a grid animation runs after `WHEEL_SLOW_ANIMATIONS`.
pub fn duration() -> Duration {
    length(slowdown(std::env::var("WHEEL_SLOW_ANIMATIONS").ok().as_deref()))
}

/// `LAYOUT` times `slowdown`, to the microsecond (0.22 s as an f32 is a hair under 220 ms).
pub fn length(slowdown: f32) -> Duration {
    Duration::from_micros((LAYOUT * slowdown * 1e6).round() as u64)
}

/// How far along a change is after `t` of `total` seconds: SwiftUI's `.smooth(duration:)`, a
/// spring with no bounce, as a critically damped spring whose period is the duration. It reaches
/// 1 exactly at the end, where the spring alone stands at 0.986.
pub fn progress(t: f32, total: f32) -> f32 {
    if total <= 0. || t >= total {
        return 1.;
    }
    if t <= 0. {
        return 0.;
    }
    let w = std::f32::consts::TAU / total;
    1. - (1. + w * t) * (-w * t).exp()
}

pub fn lerp(a: Rect, b: Rect, p: f32) -> Rect {
    let mix = |x: f32, y: f32| x + (y - x) * p;
    Rect { x: mix(a.x, b.x), y: mix(a.y, b.y), w: mix(a.w, b.w), h: mix(a.h, b.h) }
}

/// `r` scaled by `s` around its centre.
pub fn scaled(r: Rect, s: f32) -> Rect {
    let (w, h) = (r.w * s, r.h * s);
    Rect { x: r.x + (r.w - w) / 2., y: r.y + (r.h - h) / 2., w, h }
}

/// Where a pane at `frame` sits while another in `area` is zoomed: pushed past the edge of the
/// grid it is nearest to, so the zoomed pane appears to grow over it (SpaceGrid.slidOut).
pub fn slid_out(frame: Rect, area: Rect) -> Rect {
    let (mid_x, mid_y) = (frame.x + frame.w / 2., frame.y + frame.h / 2.);
    let to_left = mid_x - area.x;
    let to_right = area.x + area.w - mid_x;
    let to_top = mid_y - area.y;
    let to_bottom = area.y + area.h - mid_y;
    let nearest = to_left.min(to_right).min(to_top).min(to_bottom);
    let mut out = frame;
    if nearest == to_left {
        out.x = area.x - frame.w;
    } else if nearest == to_right {
        out.x = area.x + area.w;
    } else if nearest == to_top {
        out.y = area.y - frame.h;
    } else {
        out.y = area.y + area.h;
    }
    out
}

/// Whether `a` and `b` lie fully apart.
pub fn apart(a: Rect, b: Rect) -> bool {
    a.x >= b.x + b.w || b.x >= a.x + a.w || a.y >= b.y + b.h || b.y >= a.y + a.h
}

/// The parts of a window `width` by `height` outside `area`: what is cut out of a native view that
/// slides past the grid's edge, so it never draws over the sidebar, the title row or the bottom row.
pub fn outside(area: Rect, width: f32, height: f32) -> [Rect; 4] {
    let (right, bottom) = (area.x + area.w, area.y + area.h);
    [
        Rect { x: 0., y: 0., w: area.x.max(0.), h: height },
        Rect { x: right, y: 0., w: (width - right).max(0.), h: height },
        Rect { x: area.x, y: 0., w: area.w, h: area.y.max(0.) },
        Rect { x: area.x, y: bottom, w: area.w, h: (height - bottom).max(0.) },
    ]
}

/// Whether two trees place the same panes in the same splits at the same ratios (Node.sameShape):
/// a layout that changes only a pane's kind, label or status never animates.
pub fn same_shape(a: &Tree, b: &Tree) -> bool {
    match (a, b) {
        (Tree::Pane(x), Tree::Pane(y)) => x.id == y.id,
        (Tree::Split { id: i, horizontal: h, ratio: r, a: aa, b: ab }, Tree::Split { id: j, horizontal: g, ratio: s, a: ba, b: bb }) => {
            i == j && h == g && r == s && same_shape(aa, ba) && same_shape(ab, bb)
        }
        _ => false,
    }
}

/// `tree` with each split in `ratios` at that ratio.
pub fn set_ratios(tree: &mut Tree, ratios: &HashMap<String, f64>) {
    if let Tree::Split { id, ratio, a, b, .. } = tree {
        if let Some(r) = ratios.get(id) {
            *ratio = *r;
        }
        set_ratios(a, ratios);
        set_ratios(b, ratios);
    }
}

/// One running grid animation: where each pane was drawn when it began, in one space.
#[derive(Clone, Debug)]
pub struct Motion {
    pub space: String,
    pub from: HashMap<String, Rect>,
    pub started: Instant,
    pub length: Duration,
}

impl Motion {
    pub fn new(space: &str, from: HashMap<String, Rect>, started: Instant, length: Duration) -> Self {
        Motion { space: space.to_owned(), from, started, length }
    }
    /// How far along it is at `now`, from 0 to 1.
    pub fn progress(&self, now: Instant) -> f32 {
        progress(now.saturating_duration_since(self.started).as_secs_f32(), self.length.as_secs_f32())
    }
    pub fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.length
    }
    /// Where pane `id`, bound for `to`, draws at `now`: from where it was, or grown from
    /// `ENTER_SCALE` at its new place when the layout added it.
    pub fn frame(&self, id: &str, to: Rect, now: Instant) -> Rect {
        let p = self.progress(now);
        let from = self.from.get(id).copied().unwrap_or_else(|| scaled(to, ENTER_SCALE));
        lerp(from, to, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Leaf;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    fn leaf(id: &str) -> Tree {
        Tree::Pane(Leaf { id: id.into(), kind: "terminal".into(), label: String::new(), term: None, url: None, scope: None, agent: None, harness: None, home: None, status: None })
    }

    #[test]
    fn slowdown_reads_a_positive_number_only() {
        assert_eq!(slowdown(None), 1.);
        assert_eq!(slowdown(Some("")), 1.);
        assert_eq!(slowdown(Some("0")), 1.);
        assert_eq!(slowdown(Some("-2")), 1.);
        assert_eq!(slowdown(Some("slow")), 1.);
        assert_eq!(slowdown(Some("10")), 10.);
        assert_eq!(slowdown(Some(" 2.5 ")), 2.5);
    }

    #[test]
    fn length_is_whole_milliseconds() {
        assert_eq!(length(1.).as_millis(), 220);
        assert_eq!(length(10.).as_millis(), 2200);
    }

    #[test]
    fn progress_runs_from_zero_to_one_and_only_rises() {
        assert_eq!(progress(0., LAYOUT), 0.);
        assert_eq!(progress(LAYOUT, LAYOUT), 1.);
        assert_eq!(progress(1., LAYOUT), 1.);
        let mut last = 0.;
        for i in 1..22 {
            let p = progress(i as f32 / 100., LAYOUT);
            assert!(p > last && p < 1., "step {i}: {p}");
            last = p;
        }
        // Half way through, the spring has covered most of the way.
        let half = progress(LAYOUT / 2., LAYOUT);
        assert!((0.8..0.9).contains(&half), "{half}");
    }

    #[test]
    fn a_pane_slides_out_past_its_nearest_edge() {
        let area = r(0., 0., 1000., 600.);
        assert_eq!(slid_out(r(0., 0., 300., 600.), area), r(-300., 0., 300., 600.));
        assert_eq!(slid_out(r(700., 0., 300., 600.), area), r(1000., 0., 300., 600.));
        assert_eq!(slid_out(r(300., 0., 400., 200.), area), r(300., -200., 400., 200.));
        assert_eq!(slid_out(r(300., 400., 400., 200.), area), r(300., 600., 400., 200.));
        // A grid offset by the sidebar and the title row.
        let area = r(200., 40., 800., 500.);
        assert_eq!(slid_out(r(200., 40., 300., 500.), area), r(-100., 40., 300., 500.));
        assert!(apart(slid_out(r(600., 40., 400., 500.), area), area));
    }

    #[test]
    fn outside_covers_the_window_around_the_grid() {
        let parts = outside(r(200., 40., 800., 500.), 1000., 580.);
        assert_eq!(parts, [r(0., 0., 200., 580.), r(1000., 0., 0., 580.), r(200., 0., 800., 40.), r(200., 540., 800., 40.)]);
    }

    #[test]
    fn motion_moves_known_panes_and_grows_new_ones() {
        let start = Instant::now();
        let from = HashMap::from([("a".to_owned(), r(0., 0., 1000., 600.))]);
        let m = Motion::new("s", from, start, Duration::from_millis(220));
        let a_to = r(0., 0., 497., 600.);
        assert_eq!(m.frame("a", a_to, start), r(0., 0., 1000., 600.));
        assert_eq!(m.frame("a", a_to, start + Duration::from_millis(220)), a_to);
        let mid = m.frame("a", a_to, start + Duration::from_millis(60));
        assert!(mid.w < 1000. && mid.w > 497., "{mid:?}");
        let b_to = r(503., 0., 497., 600.);
        assert_eq!(m.frame("b", b_to, start), scaled(b_to, ENTER_SCALE));
        assert_eq!(m.frame("b", b_to, start + Duration::from_secs(1)), b_to);
        assert!(!m.done(start + Duration::from_millis(219)) && m.done(start + Duration::from_millis(220)));
    }

    #[test]
    fn shape_ignores_what_a_pane_shows() {
        let split = |ratio: f64, a: Tree| Tree::Split { id: "s".into(), horizontal: true, ratio, a: Box::new(a), b: Box::new(leaf("b")) };
        let mut relabelled = leaf("a");
        if let Tree::Pane(l) = &mut relabelled {
            l.kind = "web".into();
            l.label = "page".into();
        }
        assert!(same_shape(&split(0.5, leaf("a")), &split(0.5, relabelled)));
        assert!(!same_shape(&split(0.5, leaf("a")), &split(0.6, leaf("a"))));
        assert!(!same_shape(&split(0.5, leaf("a")), &leaf("a")));
        assert!(!same_shape(&leaf("a"), &leaf("c")));
        // A divider drag's ratio, confirmed by wheeld, is the same shape.
        let mut dragged = split(0.5, leaf("a"));
        set_ratios(&mut dragged, &HashMap::from([("s".to_owned(), 0.6)]));
        assert!(same_shape(&dragged, &split(0.6, leaf("a"))));
    }
}
