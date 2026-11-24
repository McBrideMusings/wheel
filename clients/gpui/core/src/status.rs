//! A pane's or space's status as the daemon computes it (`PaneStatus`), in the words and marks
//! the Swift client uses (`apple/WheelKit/Sources/WheelKit/PaneStatus.swift`,
//! `apple/WheelUI/Sources/WheelUI/StatusDot.swift`): a 9 px dot (orange needs you, a hollow red
//! ring stopped, blue done, green idle) or, while a turn runs, a 12 px blue arc turning once a
//! second. The `status.dots` setting names the ranks that draw a mark.
use crate::layout::{Space, leaves_of};
use std::collections::HashSet;
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

pub const ORANGE: u32 = 0xff9f0a;
pub const RED: u32 = 0xff453a;
pub const BLUE: u32 = 0x0a84ff;
pub const GREEN: u32 = 0x30d158;

/// The colour of `status`'s mark: needs you orange, stopped red, working and done blue, idle green.
pub fn colour(status: &PaneStatus) -> u32 {
    match status {
        PaneStatus::NeedsYou { .. } => ORANGE,
        PaneStatus::Stopped => RED,
        PaneStatus::Working | PaneStatus::Done => BLUE,
        PaneStatus::Idle => GREEN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Leaf, Tree};

    fn leaf(id: &str, status: Option<PaneStatus>) -> Tree {
        Tree::Pane(Leaf { id: id.into(), kind: "agent".into(), label: String::new(), term: None, url: None, scope: None, agent: Some(id.into()), harness: None, status, name: None, title: None, visited: vec![], mirror: None, quiet: false })
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
        let space = Space { id: "s".into(), name: "s".into(), cwd: "/w".into(), root, status: None, popped: vec![], backgrounded: vec![], frames: Default::default() };
        let (marks, more) = agent_statuses(&space, &all_ranks(), AGENT_DOT_LIMIT);
        assert_eq!(marks, vec![PaneStatus::NeedsYou { waiting: 1, reason: None }, PaneStatus::Working, PaneStatus::Working, PaneStatus::Done]);
        assert_eq!(more, 1);
        let (hidden, more) = agent_statuses(&space, &listed("needs_you"), AGENT_DOT_LIMIT);
        assert_eq!((hidden.len(), more), (1, 0));
    }
}
