//! wheeld's split tree (`Node` in proto/src/lib.rs) and its placement into rectangles.
use std::collections::HashMap;
use wheel_proto::{Node, PaneKind, PaneStatus, ServerMsg, SessionInfo, SpaceInfo};

/// Width or height of a divider between two panes, in points.
pub const GAP: f32 = 6.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub id: String,
    /// The kind's wire name (`empty`, `terminal`, `web`, `browser`, `agent`, or one this build
    /// does not know).
    pub kind: String,
    pub label: String,
    pub term: Option<String>,
    /// A browser pane's page.
    pub url: Option<String>,
    /// A web pane's artifact scope (`project` or `machine`); its name is the label.
    pub scope: Option<String>,
    /// An agent pane's agent, the harness it runs (`claude`, `codex`, `pi`) and the home the
    /// pane saved for it.
    pub agent: Option<String>,
    pub harness: Option<String>,
    pub home: Option<String>,
    /// What the pane asks of the person, as the daemon last said: in the `status` listing, then
    /// `status_changed`; a `spaces` broadcast carries none and the client keeps what it holds.
    pub status: Option<PaneStatus>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Tree {
    Pane(Leaf),
    Split { id: String, horizontal: bool, ratio: f64, a: Box<Tree>, b: Box<Tree> },
}

impl Tree {
    /// Whether `pane` is one of this tree's panes.
    pub fn holds(&self, pane: &str) -> bool {
        leaves_of(self).iter().any(|l| l.id == pane)
    }
    /// The splits from the root down to `pane`: each split's id, whether the pane is in its first
    /// side, and its ratio. None when the pane is not in this tree.
    pub fn splits_to(&self, pane: &str) -> Option<Vec<(String, bool, f64)>> {
        match self {
            Tree::Pane(leaf) => (leaf.id == pane).then(Vec::new),
            Tree::Split { id, ratio, a, b, .. } => {
                let (first, rest) = match a.splits_to(pane) {
                    Some(rest) => (true, rest),
                    None => (false, b.splits_to(pane)?),
                };
                let mut out = vec![(id.clone(), first, *ratio)];
                out.extend(rest);
                Some(out)
            }
        }
    }
}

/// One space as the daemon lists it.
#[derive(Clone, Debug)]
pub struct Space {
    pub id: String,
    pub name: String,
    /// The space's directory: a project artifact's page store is keyed by it.
    pub cwd: String,
    pub root: Tree,
    /// The highest status among its panes, as the daemon computed it.
    pub status: Option<PaneStatus>,
    /// The trees popped out of the grid, each drawn as a card over it (`crate::popped`), and the
    /// panes in its tray, which nothing draws.
    pub popped: Vec<Tree>,
    pub backgrounded: Vec<Tree>,
}

impl Space {
    /// Every pane the space holds: the grid's, the popped trees' and the tray's.
    pub fn all(&self) -> impl Iterator<Item = &Leaf> {
        std::iter::once(&self.root).chain(&self.popped).chain(&self.backgrounded).flat_map(leaves_of)
    }
    /// Every pane this space draws: the grid's, then each popped tree's.
    pub fn shown(&self) -> impl Iterator<Item = &Leaf> {
        std::iter::once(&self.root).chain(&self.popped).flat_map(leaves_of)
    }
    /// Where `pane` is in this space: `grid`, `popped` or `backgrounded`; none when it is not here.
    pub fn site_of(&self, pane: &str) -> Option<wheel_proto::PaneSite> {
        let holds = |t: &Tree| t.holds(pane);
        if holds(&self.root) {
            Some(wheel_proto::PaneSite::Grid)
        } else if self.popped.iter().any(holds) {
            Some(wheel_proto::PaneSite::Popped)
        } else if self.backgrounded.iter().any(holds) {
            Some(wheel_proto::PaneSite::Backgrounded)
        } else {
            None
        }
    }
}

/// Put browser pane `pane` on `url` in `tree` (a `pane_url`), leaving every other leaf alone.
pub fn set_url(tree: &mut Tree, pane: &str, url: &str) {
    match tree {
        Tree::Pane(leaf) => {
            if leaf.id == pane {
                leaf.url = Some(url.to_owned());
                leaf.label = url.to_owned();
            }
        }
        Tree::Split { a, b, .. } => {
            set_url(a, pane, url);
            set_url(b, pane, url);
        }
    }
}

/// The daemon's split tree as the view draws it.
impl From<&Node> for Tree {
    fn from(node: &Node) -> Self {
        match node {
            Node::Split { id, axis, ratio, a, b } => Tree::Split {
                id: id.clone(),
                horizontal: *axis == wheel_proto::Axis::Horizontal,
                ratio: *ratio,
                a: Box::new(Tree::from(a.as_ref())),
                b: Box::new(Tree::from(b.as_ref())),
            },
            Node::Pane { id, kind, status, .. } => {
                let mut leaf = Leaf { id: id.clone(), kind: String::new(), label: String::new(), term: None, url: None, scope: None, agent: None, harness: None, home: None, status: status.clone() };
                match kind {
                    PaneKind::Empty { .. } => leaf.kind = "empty".into(),
                    PaneKind::Terminal { term, cwd, .. } => {
                        leaf.kind = "terminal".into();
                        leaf.term = Some(term.clone());
                        leaf.label = cwd.clone().unwrap_or_default();
                    }
                    PaneKind::Web { artifact, .. } => {
                        leaf.kind = "web".into();
                        leaf.label = artifact.name().to_owned();
                        leaf.scope = Some(scope_name(artifact.scope()));
                    }
                    PaneKind::Browser { url, .. } => {
                        leaf.kind = "browser".into();
                        leaf.label = url.clone();
                        leaf.url = Some(url.clone());
                    }
                    PaneKind::Agent { agent, harness, place, .. } => {
                        leaf.kind = "agent".into();
                        leaf.agent = Some(agent.clone());
                        leaf.harness = Some(harness.clone());
                        leaf.home = place.as_ref().and_then(|p| p.home.clone());
                    }
                    PaneKind::Unknown(pane) => leaf.kind = pane.kind().to_owned(),
                }
                Tree::Pane(leaf)
            }
        }
    }
}

/// An artifact scope's wire name (`project`, `machine`).
fn scope_name(scope: wheel_proto::ArtifactScope) -> String {
    match scope {
        wheel_proto::ArtifactScope::Project => "project".into(),
        wheel_proto::ArtifactScope::Machine => "machine".into(),
    }
}

/// A named, ordered group of spaces (ADR-0021).
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: String,
    pub name: String,
    /// Its spaces' ids, in order.
    pub spaces: Vec<String>,
}

/// What wheeld holds: the spaces and the sessions that group them.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub spaces: Vec<Space>,
    pub sessions: Vec<Session>,
}

impl Layout {
    fn new(spaces: &[SpaceInfo], sessions: &[SessionInfo]) -> Self {
        Self {
            spaces: spaces.iter().map(|s| Space { id: s.id.clone(), name: s.name.clone(), cwd: s.cwd.clone(), root: Tree::from(&s.root), status: s.status.clone(), popped: s.popped.iter().map(Tree::from).collect(), backgrounded: s.backgrounded.iter().map(Tree::from).collect() }).collect(),
            sessions: sessions.iter().map(|s| Session { id: s.id.clone(), name: s.name.clone(), spaces: s.spaces.clone() }).collect(),
        }
    }
}

impl Layout {
    /// A `spaces` broadcast carries no statuses (they change without the layout changing), so a
    /// space or pane still there keeps the status `old` held for it.
    pub fn keep_statuses(&mut self, old: &[Space]) {
        for space in &mut self.spaces {
            let Some(was) = old.iter().find(|o| o.id == space.id) else { continue };
            if space.status.is_none() {
                space.status = was.status.clone();
            }
            let held: HashMap<&str, &Option<PaneStatus>> = leaves_of(&was.root).into_iter().map(|l| (l.id.as_str(), &l.status)).collect();
            set_leaf_status(&mut space.root, &|leaf| {
                if leaf.status.is_none() && let Some(status) = held.get(leaf.id.as_str()) {
                    leaf.status = (*status).clone();
                }
            });
        }
    }
}

/// Apply a `status_changed` to `spaces`: `pane` is none when only the space's status changed.
pub fn apply_status(spaces: &mut [Space], space: &str, pane: Option<&str>, status: Option<PaneStatus>, space_status: Option<PaneStatus>) {
    let Some(s) = spaces.iter_mut().find(|s| s.id == space) else { return };
    s.status = space_status;
    if let Some(pane) = pane {
        set_leaf_status(&mut s.root, &|leaf| {
            if leaf.id == pane {
                leaf.status = status.clone();
            }
        });
    }
}

fn set_leaf_status(tree: &mut Tree, f: &dyn Fn(&mut Leaf)) {
    match tree {
        Tree::Pane(leaf) => f(leaf),
        Tree::Split { a, b, .. } => {
            set_leaf_status(a, f);
            set_leaf_status(b, f);
        }
    }
}

/// The layout in a `status` reply or a `spaces` broadcast.
pub fn parse_layout(message: &ServerMsg) -> Option<Layout> {
    match message {
        ServerMsg::Status { spaces, sessions, .. } | ServerMsg::Spaces { spaces, sessions } => Some(Layout::new(spaces, sessions)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
pub struct Divider {
    pub split: String,
    pub horizontal: bool,
    pub rect: Rect,
    /// The area of the split this divider belongs to; a drag measures its ratio against it.
    pub parent: Rect,
}
#[derive(Clone)]
pub struct Placed {
    pub leaf: Leaf,
    pub rect: Rect,
}

/// Binary split tree to rectangles. `ratio` is the share of the area less one gap that `a`
/// takes; `overrides` holds ratios being dragged and not yet confirmed by the daemon.
pub fn place(tree: &Tree, rect: Rect, overrides: &HashMap<String, f64>, leaves: &mut Vec<Placed>, dividers: &mut Vec<Divider>) {
    match tree {
        Tree::Pane(leaf) => leaves.push(Placed { leaf: leaf.clone(), rect }),
        Tree::Split { id, horizontal, ratio, a, b } => {
            let ratio = overrides.get(id).copied().unwrap_or(*ratio) as f32;
            if *horizontal {
                let aw = ((rect.w - GAP) * ratio).round();
                place(a, Rect { w: aw, ..rect }, overrides, leaves, dividers);
                dividers.push(Divider { split: id.clone(), horizontal: true, rect: Rect { x: rect.x + aw, w: GAP, ..rect }, parent: rect });
                place(b, Rect { x: rect.x + aw + GAP, w: rect.w - aw - GAP, ..rect }, overrides, leaves, dividers);
            } else {
                let ah = ((rect.h - GAP) * ratio).round();
                place(a, Rect { h: ah, ..rect }, overrides, leaves, dividers);
                dividers.push(Divider { split: id.clone(), horizontal: false, rect: Rect { y: rect.y + ah, h: GAP, ..rect }, parent: rect });
                place(b, Rect { y: rect.y + ah + GAP, h: rect.h - ah - GAP, ..rect }, overrides, leaves, dividers);
            }
        }
    }
}

/// Every pane of a tree, in layout order.
pub fn leaves_of(tree: &Tree) -> Vec<&Leaf> {
    match tree {
        Tree::Pane(leaf) => vec![leaf],
        Tree::Split { a, b, .. } => leaves_of(a).into_iter().chain(leaves_of(b)).collect(),
    }
}

/// The ratio a pointer at `(x, y)` asks of a split, within the daemon's 0.1 to 0.9 range.
pub fn drag_ratio(horizontal: bool, parent: Rect, x: f32, y: f32) -> f64 {
    let ratio = if horizontal { (x - parent.x) / (parent.w - GAP) } else { (y - parent.y) / (parent.h - GAP) };
    f64::from(ratio).clamp(0.1, 0.9)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A tree as wheeld sends it, read through the proto crate.
    fn node(wire: serde_json::Value) -> Tree {
        Tree::from(&serde_json::from_value::<Node>(wire).unwrap())
    }
    fn tree() -> Tree {
        node(json!({"node":"split","id":"x1","axis":"horizontal","ratio":0.5,
            "a":{"node":"pane","id":"p1","kind":"web","scope":"project","name":"page"},
            "b":{"node":"split","id":"x2","axis":"vertical","ratio":0.25,
                "a":{"node":"pane","id":"p2","kind":"terminal","term":"t1","cwd":"/tmp"},
                "b":{"node":"pane","id":"p3","kind":"browser","url":"about:blank"}}}))
    }
    fn scene(overrides: &HashMap<String, f64>) -> (Vec<Placed>, Vec<Divider>) {
        let (mut leaves, mut dividers) = (vec![], vec![]);
        place(&tree(), Rect { x: 0., y: 10., w: 1006., h: 506. }, overrides, &mut leaves, &mut dividers);
        (leaves, dividers)
    }

    #[test]
    fn places_every_leaf_and_divider() {
        let (leaves, dividers) = scene(&HashMap::new());
        let rects: Vec<_> = leaves.iter().map(|p| (p.leaf.id.as_str(), p.rect)).collect();
        assert_eq!(rects[0], ("p1", Rect { x: 0., y: 10., w: 500., h: 506. }));
        assert_eq!(rects[1], ("p2", Rect { x: 506., y: 10., w: 500., h: 125. }));
        assert_eq!(rects[2], ("p3", Rect { x: 506., y: 141., w: 500., h: 375. }));
        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[0].rect, Rect { x: 500., y: 10., w: GAP, h: 506. });
        assert_eq!(dividers[1].rect, Rect { x: 506., y: 135., w: 500., h: GAP });
    }

    #[test]
    fn every_kind_keeps_what_the_view_draws() {
        let leaves: Vec<_> = leaves_of(&tree()).into_iter().cloned().collect();
        assert_eq!(leaves[0], Leaf { id: "p1".into(), kind: "web".into(), label: "page".into(), term: None, url: None, scope: Some("project".into()), agent: None, harness: None, home: None, status: None });
        assert_eq!(leaves[1], Leaf { id: "p2".into(), kind: "terminal".into(), label: "/tmp".into(), term: Some("t1".into()), url: None, scope: None, agent: None, harness: None, home: None, status: None });
        assert_eq!(leaves[2], Leaf { id: "p3".into(), kind: "browser".into(), label: "about:blank".into(), term: None, url: Some("about:blank".into()), scope: None, agent: None, harness: None, home: None, status: None });
        let agent = node(json!({"node":"pane","id":"p4","kind":"agent","agent":"w1","harness":"claude"}));
        assert_eq!(agent, Tree::Pane(Leaf { id: "p4".into(), kind: "agent".into(), label: String::new(), term: None, url: None, scope: None, agent: Some("w1".into()), harness: Some("claude".into()), home: None, status: None }));
    }

    #[test]
    fn leaves_and_gaps_tile_the_area_without_overlap() {
        let (leaves, _) = scene(&HashMap::new());
        let (a, b) = (leaves[0].rect, leaves[1].rect);
        assert_eq!(a.x + a.w + GAP, b.x);
    }

    #[test]
    fn an_override_replaces_the_daemon_ratio() {
        let (leaves, _) = scene(&HashMap::from([("x1".to_owned(), 0.8)]));
        assert_eq!(leaves[0].rect.w, 800.);
        assert_eq!(leaves[1].rect.x, 806.);
    }

    #[test]
    fn a_drag_stays_inside_the_daemons_range() {
        let parent = Rect { x: 0., y: 0., w: 1006., h: 506. };
        assert_eq!(drag_ratio(true, parent, -50., 0.), 0.1);
        assert_eq!(drag_ratio(true, parent, 5000., 0.), 0.9);
        assert_eq!(drag_ratio(true, parent, 500., 0.), 0.5);
        assert_eq!(drag_ratio(false, parent, 0., 250.), 0.5);
    }

    #[test]
    fn unknown_kinds_survive_parsing() {
        let leaf = node(json!({"node":"pane","id":"p9","kind":"hologram"}));
        assert_eq!(leaf, Tree::Pane(Leaf { id: "p9".into(), kind: "hologram".into(), label: String::new(), term: None, url: None, scope: None, agent: None, harness: None, home: None, status: None }));
    }

    #[test]
    fn a_spaces_broadcast_round_trips_and_gives_the_layout() {
        let wire = json!({"ev":"spaces","spaces":[{"id":"s1","name":"wheel","cwd":"/w","root":{"node":"pane","id":"p1","kind":"empty"}}],
            "sessions":[{"id":"n1","name":"Main","spaces":["s1"]}]});
        let message: ServerMsg = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), wire);
        let layout = parse_layout(&message).unwrap();
        assert_eq!(layout.spaces[0].name, "wheel");
        assert_eq!(layout.sessions, [Session { id: "n1".into(), name: "Main".into(), spaces: vec!["s1".into()] }]);
    }

    #[test]
    fn the_layout_comes_from_status_and_broadcast_only() {
        let status = crate::daemon::tests::status_frame(wheel_proto::PROTOCOL_VERSION);
        let message: ServerMsg = serde_json::from_str(&status).unwrap();
        assert!(matches!(&message, ServerMsg::Status { protocol, .. } if *protocol == wheel_proto::PROTOCOL_VERSION));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&serde_json::to_string(&message).unwrap()).unwrap()["spaces"], serde_json::from_str::<serde_json::Value>(&status).unwrap()["spaces"]);
        assert_eq!(parse_layout(&message).unwrap().spaces[0].id, "s1");
        assert!(parse_layout(&ServerMsg::SpaceFocused { space: "s1".into(), pane: None }).is_none());
    }

    #[test]
    fn statuses_come_from_the_listing_survive_a_broadcast_and_follow_status_changed() {
        let agent = |status: serde_json::Value| json!({"node":"pane","id":"p4","kind":"agent","agent":"w1","harness":"claude","status":status});
        let listing = |status: serde_json::Value| serde_json::from_value::<Vec<SpaceInfo>>(json!([{"id":"s1","name":"w","cwd":"/w","root":agent(status),"status":{"kind":"working"}}])).unwrap();
        let mut layout = Layout::new(&listing(json!({"kind":"working"})), &[]);
        assert_eq!(leaves_of(&layout.spaces[0].root)[0].status, Some(PaneStatus::Working));
        assert_eq!(layout.spaces[0].status, Some(PaneStatus::Working));
        // A broadcast carries none: the client keeps what it held.
        let mut broadcast = Layout::new(&serde_json::from_value::<Vec<SpaceInfo>>(json!([{"id":"s1","name":"w","cwd":"/w","root":{"node":"pane","id":"p4","kind":"agent","agent":"w1","harness":"claude"}}])).unwrap(), &[]);
        broadcast.keep_statuses(&layout.spaces);
        assert_eq!(leaves_of(&broadcast.spaces[0].root)[0].status, Some(PaneStatus::Working));
        assert_eq!(broadcast.spaces[0].status, Some(PaneStatus::Working));
        apply_status(&mut layout.spaces, "s1", Some("p4"), Some(PaneStatus::NeedsYou { waiting: 1, reason: Some("Bash: rm".into()) }), Some(PaneStatus::NeedsYou { waiting: 1, reason: Some("Bash: rm".into()) }));
        assert_eq!(leaves_of(&layout.spaces[0].root)[0].status, Some(PaneStatus::NeedsYou { waiting: 1, reason: Some("Bash: rm".into()) }));
        apply_status(&mut layout.spaces, "s1", None, None, None);
        assert_eq!(layout.spaces[0].status, None);
        assert!(leaves_of(&layout.spaces[0].root)[0].status.is_some());
    }
}
