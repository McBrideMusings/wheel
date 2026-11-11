//! A pane's or space's status as the daemon computes it (`PaneStatus`), in the words and marks
//! the Swift client uses (`apple/WheelKit/Sources/WheelKit/PaneStatus.swift`,
//! `apple/WheelUI/Sources/WheelUI/StatusDot.swift`): a 9 px dot (orange needs you, a hollow red
//! ring stopped, blue done, green idle) or, while a turn runs, a 12 px blue arc turning once a
//! second. The `status.dots` setting names the ranks that draw a mark.
use crate::layout::{Space, leaves_of};
use gpui::{Animation, AnimationExt as _, AnyElement, Bounds, PathBuilder, Pixels, canvas, div, point, prelude::*, px, rgb};
use std::{collections::HashSet, f32::consts::PI, time::Duration};
use wheel_proto::PaneStatus;

/// The five ranks, highest first, as `status.dots` names them.
pub const RANKS: [&str; 5] = ["needs_you", "stopped", "working", "done", "idle"];

/// The most agent marks a space draws before it says how many more there are.
pub const AGENT_DOT_LIMIT: usize = 4;

/// The rank's name, as `status.dots` lists it.
pub fn rank(status: &PaneStatus) -> &'static str {
    match status {
        PaneStatus::NeedsYou { .. } => "needs_you",
        PaneStatus::Stopped => "stopped",
        PaneStatus::Working => "working",
        PaneStatus::Done => "done",
        PaneStatus::Idle => "idle",
    }
}

/// The rank's place in `RANKS`: 0 is the most urgent.
fn order(status: &PaneStatus) -> usize {
    RANKS.iter().position(|r| *r == rank(status)).unwrap_or(RANKS.len())
}

/// The mark's tooltip and what a screen reader reads: "Needs you, 2 waiting", "Needs you: Bash:
/// make test", "Stopped", "Working", "Done", "Idle".
pub fn label(status: &PaneStatus) -> String {
    match status {
        PaneStatus::NeedsYou { reason: Some(reason), .. } => format!("Needs you: {reason}"),
        PaneStatus::NeedsYou { waiting, reason: None } => format!("Needs you, {waiting} waiting"),
        PaneStatus::Stopped => "Stopped".into(),
        PaneStatus::Working => "Working".into(),
        PaneStatus::Done => "Done".into(),
        PaneStatus::Idle => "Idle".into(),
    }
}

/// The words a row carries beside its name, for the two ranks that have any.
pub fn caption(status: &PaneStatus) -> Option<String> {
    match status {
        PaneStatus::NeedsYou { reason: Some(reason), .. } => Some(reason.clone()),
        PaneStatus::NeedsYou { waiting, reason: None } => Some(format!("{waiting} waiting")),
        PaneStatus::Stopped => Some("stopped".into()),
        PaneStatus::Working | PaneStatus::Done | PaneStatus::Idle => None,
    }
}

/// The ranks a `status.dots` value lists; a name that is not a rank is ignored, an empty value
/// lists none.
pub fn listed(value: &str) -> HashSet<&'static str> {
    value.split(',').filter_map(|name| RANKS.iter().copied().find(|r| *r == name.trim())).collect()
}

/// Every rank, which is what the dots show until the daemon says otherwise.
pub fn all_ranks() -> HashSet<&'static str> {
    RANKS.into_iter().collect()
}

/// One status per agent pane of `space`, most urgent first (needs you, stopped, working, done,
/// idle), panes of the same rank in layout order, leaving out the ranks `shown` leaves out. The
/// marks hold at most `limit` of them and the count is the rest.
pub fn agent_statuses(space: &Space, shown: &HashSet<&'static str>, limit: usize) -> (Vec<PaneStatus>, usize) {
    let mut all: Vec<(usize, usize, PaneStatus)> = leaves_of(&space.root).iter().enumerate()
        .filter_map(|(at, leaf)| leaf.status.clone().filter(|s| shown.contains(rank(s))).map(|s| (order(&s), at, s)))
        .collect();
    all.sort_by_key(|(rank, at, _)| (*rank, *at));
    let more = all.len().saturating_sub(limit);
    all.truncate(limit);
    (all.into_iter().map(|(_, _, s)| s).collect(), more)
}

const ORANGE: u32 = 0xff9f0a;
const RED: u32 = 0xff453a;
const BLUE: u32 = 0x0a84ff;
const GREEN: u32 = 0x30d158;

/// The mark for `status`, or none for a rank `shown` leaves out. `id` keeps the arc's turn going
/// from frame to frame; a working mark takes the arc's 12 px slot, every other a 9 px dot
/// centred in the same slot so a mark never moves its neighbours when its rank changes.
pub fn dot(status: &PaneStatus, shown: &HashSet<&'static str>, id: impl Into<gpui::ElementId>) -> Option<AnyElement> {
    if !shown.contains(rank(status)) {
        return None;
    }
    let slot = div().w(px(12.)).h(px(12.)).flex().items_center().justify_center().flex_none();
    let mark = match status {
        PaneStatus::Working => return Some(slot.child(arc(id)).into_any_element()),
        PaneStatus::NeedsYou { .. } => div().w(px(9.)).h(px(9.)).rounded_full().bg(rgb(ORANGE)),
        PaneStatus::Stopped => div().w(px(9.)).h(px(9.)).rounded_full().border(px(1.8)).border_color(rgb(RED)),
        PaneStatus::Done => div().w(px(9.)).h(px(9.)).rounded_full().bg(rgb(BLUE)),
        PaneStatus::Idle => div().w(px(9.)).h(px(9.)).rounded_full().bg(rgb(GREEN)),
    };
    Some(slot.child(mark).into_any_element())
}

/// The 3/4 arc, 1.8 px wide with round ends, turning clockwise once a second.
fn arc(id: impl Into<gpui::ElementId>) -> AnyElement {
    div().w(px(12.)).h(px(12.))
        .with_animation(id, Animation::new(Duration::from_secs(1)).repeat(), |slot, delta| {
            slot.child(canvas(|_, _, _| (), move |bounds: Bounds<Pixels>, _, window, _| paint_arc(bounds, delta * 2. * PI, window)).w(px(12.)).h(px(12.)))
        })
        .into_any_element()
}

fn paint_arc(bounds: Bounds<Pixels>, angle: f32, window: &mut gpui::Window) {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (cx, cy, r) = (w / 2., h / 2., (w.min(h) - 1.8) / 2.);
    let mut path = PathBuilder::stroke(px(1.8));
    let start = angle;
    let end = angle + 1.5 * PI;
    path.move_to(point(px(cx + r * start.cos()), px(cy + r * start.sin())));
    path.arc_to(point(px(r), px(r)), px(0.), true, true, point(px(cx + r * end.cos()), px(cy + r * end.sin())));
    path.translate(bounds.origin);
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(BLUE));
    }
}

/// A space's agents as marks: one per agent pane, most urgent first, the 12 px slots overlapping
/// by 2 px, at most `AGENT_DOT_LIMIT` and then `+N`; the space's own status when no pane sent
/// one. None when nothing draws a mark.
pub fn space_dots(space: &Space, shown: &HashSet<&'static str>) -> Option<gpui::Div> {
    if leaves_of(&space.root).iter().all(|l| l.status.is_none()) {
        // A daemon that predates per-pane statuses sends the space's alone.
        return space.status.as_ref().and_then(|s| dot(s, shown, gpui::ElementId::Name(format!("space-dot-{}", space.id).into()))).map(|mark| div().flex().items_center().flex_none().child(mark));
    }
    let (marks, more) = agent_statuses(space, shown, AGENT_DOT_LIMIT);
    if marks.is_empty() {
        return None;
    }
    let mut row = div().flex().items_center().gap(px(3.)).flex_none();
    let mut dots = div().flex().items_center().flex_none();
    for (i, status) in marks.iter().enumerate() {
        if let Some(mark) = dot(status, shown, gpui::ElementId::Name(format!("space-dot-{}-{i}", space.id).into())) {
            dots = dots.child(if i > 0 { div().ml(px(-2.)).child(mark) } else { div().child(mark) });
        }
    }
    row = row.child(dots);
    if more > 0 {
        row = row.child(div().text_xs().text_color(rgb(0x8fa89b)).child(format!("+{more}")));
    }
    Some(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Leaf, Tree};

    fn leaf(id: &str, status: Option<PaneStatus>) -> Tree {
        Tree::Pane(Leaf { id: id.into(), kind: "agent".into(), label: String::new(), term: None, url: None, scope: None, agent: Some(id.into()), harness: None, home: None, status })
    }
    fn split(a: Tree, b: Tree) -> Tree {
        Tree::Split { id: "x".into(), horizontal: true, ratio: 0.5, a: Box::new(a), b: Box::new(b) }
    }

    #[test]
    fn labels_and_captions_read_as_the_swift_client_writes_them() {
        assert_eq!(label(&PaneStatus::NeedsYou { waiting: 2, reason: None }), "Needs you, 2 waiting");
        assert_eq!(label(&PaneStatus::NeedsYou { waiting: 1, reason: Some("Bash: make test".into()) }), "Needs you: Bash: make test");
        assert_eq!(label(&PaneStatus::Working), "Working");
        assert_eq!(caption(&PaneStatus::Stopped).as_deref(), Some("stopped"));
        assert_eq!(caption(&PaneStatus::NeedsYou { waiting: 3, reason: None }).as_deref(), Some("3 waiting"));
        assert_eq!(caption(&PaneStatus::Idle), None);
    }

    #[test]
    fn the_setting_lists_ranks_and_ignores_what_is_not_one() {
        assert_eq!(listed("needs_you, working,bogus"), HashSet::from(["needs_you", "working"]));
        assert!(listed("").is_empty());
        assert_eq!(all_ranks().len(), 5);
    }

    #[test]
    fn a_space_orders_its_agents_by_urgency_then_layout_and_counts_the_rest() {
        let root = split(
            split(leaf("a", Some(PaneStatus::Idle)), leaf("b", Some(PaneStatus::Working))),
            split(leaf("c", Some(PaneStatus::NeedsYou { waiting: 1, reason: None })), split(leaf("d", Some(PaneStatus::Working)), leaf("e", Some(PaneStatus::Done)))),
        );
        let space = Space { id: "s".into(), name: "s".into(), cwd: "/w".into(), root, status: None, popped: vec![], backgrounded: vec![] };
        let (marks, more) = agent_statuses(&space, &all_ranks(), AGENT_DOT_LIMIT);
        assert_eq!(marks, vec![PaneStatus::NeedsYou { waiting: 1, reason: None }, PaneStatus::Working, PaneStatus::Working, PaneStatus::Done]);
        assert_eq!(more, 1);
        let (hidden, more) = agent_statuses(&space, &listed("needs_you"), AGENT_DOT_LIMIT);
        assert_eq!((hidden.len(), more), (1, 0));
    }
}
