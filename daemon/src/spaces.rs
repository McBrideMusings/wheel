//! Spaces: one per project, each a split tree of panes (after Ghostty's SplitTree). The daemon
//! owns the layout (ADR-0001), saves it after every change, and broadcasts it to every client.
//! Every caller — a client op or the CLI — changes it through the `AppState`
//! methods here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wheel_proto::{
    AgentPermissions, ArtifactRef, ArtifactScope, Axis, ContextUse, Dir, Extra, MoveTarget,
    NewPaneKind, Node, PaneKind, PaneSite, ServerMsg, AgentState, SessionInfo, SpaceInfo, StartKind,
};

use crate::pty::Term;
use crate::resume::Harness;
use crate::server::{AgentStart, AppState, Owner};
use crate::settings::{page, HOMEPAGE, PLACEMENT, SESSIONS_CLOSE_EMPTY};

/// A pane's place in `AppState::agent_starts` while its agent starts; dropping it frees the pane.
struct StartClaim<'a> {
    starts: &'a Mutex<HashSet<String>>,
    pane: String,
}

impl<'a> StartClaim<'a> {
    fn take(
        starts: &'a Mutex<HashSet<String>>,
        pane: &str,
    ) -> Result<Self, String> {
        if !starts.lock().unwrap().insert(pane.to_string()) {
            return Err(format!("pane {pane} is already starting an agent"));
        }
        Ok(Self {
            starts,
            pane: pane.to_string(),
        })
    }
}

impl Drop for StartClaim<'_> {
    fn drop(&mut self) {
        self.starts.lock().unwrap().remove(&self.pane);
    }
}

/// Every space, in tab order, and which one is showing. Also the saved file's format.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Spaces {
    pub spaces: Vec<SpaceInfo>,
    /// The sessions in order, each holding its spaces' ids in order (ADR-0021). Every space is in
    /// exactly one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionInfo>,
}

impl Spaces {
    pub(crate) fn index(&self, id: &str) -> Result<usize, String> {
        self.spaces
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| format!("no space {id}"))
    }

    /// The index of the space holding `pane`.
    pub(crate) fn holding(&self, pane: &str) -> Result<usize, String> {
        self.spaces
            .iter()
            .position(|s| space_find(s, pane).is_some())
            .ok_or_else(|| format!("no pane {pane}"))
    }

    /// The index of the session `text` names: an id, else a name only one session has.
    pub(crate) fn session_index(&self, text: &str) -> Result<usize, String> {
        if let Some(i) = self.sessions.iter().position(|s| s.id == text) {
            return Ok(i);
        }
        let named: Vec<usize> = (0..self.sessions.len())
            .filter(|&i| self.sessions[i].name == text)
            .collect();
        match named.as_slice() {
            [one] => Ok(*one),
            [] => Err(format!("no session {text}")),
            _ => Err(format!("more than one session is named {text}; use its id")),
        }
    }

    /// The index of the session holding space `id`.
    pub(crate) fn session_holding(&self, id: &str) -> Option<usize> {
        self.sessions
            .iter()
            .position(|s| s.spaces.iter().any(|x| x == id))
    }

    /// Take space `id` out of whichever session holds it.
    fn detach(&mut self, id: &str) {
        for session in &mut self.sessions {
            session.spaces.retain(|x| x != id);
        }
    }

    /// Make the sessions agree with the spaces: a session names only spaces that exist, each at
    /// most once, and a space no session names joins the first session. With no session at all,
    /// `make_id` names one new session, "Main", to hold them. Returns whether anything changed.
    pub(crate) fn heal(&mut self, mut make_id: impl FnMut() -> String) -> bool {
        let before = self.sessions.clone();
        let mut seen = std::collections::HashSet::new();
        let known: std::collections::HashSet<&str> =
            self.spaces.iter().map(|s| s.id.as_str()).collect();
        for session in &mut self.sessions {
            session
                .spaces
                .retain(|id| known.contains(id.as_str()) && seen.insert(id.clone()));
        }
        let orphans: Vec<String> = self
            .spaces
            .iter()
            .filter(|s| !seen.contains(&s.id))
            .map(|s| s.id.clone())
            .collect();
        if !orphans.is_empty() {
            if self.sessions.is_empty() {
                self.sessions.push(SessionInfo {
                    id: make_id(),
                    name: "Main".into(),
                    spaces: Vec::new(),
                });
            }
            self.sessions[0].spaces.extend(orphans);
        }
        self.sessions != before
    }

    /// Remove a space from the list and from its session.
    fn remove(&mut self, index: usize) -> SpaceInfo {
        let space = self.spaces.remove(index);
        self.detach(&space.id);
        space
    }
}

/// How many directories the recent list keeps.
pub const RECENT_LIMIT: usize = 50;

/// The saved file: the layout, the id counter so a restarted daemon never hands out an id it gave
/// before — not a closed pane's, not a terminal's a client may still remember. A top-level key this build does not know — one a
/// newer build wrote — rides in `extra` and is written back, as an unknown pane kind is.
#[derive(Serialize, Deserialize)]
struct Saved {
    #[serde(flatten)]
    layout: Spaces,
    next_id: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recent: Vec<wheel_proto::RecentDir>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

pub fn find_pane<'a>(node: &'a Node, pane: &str) -> Option<&'a PaneKind> {
    match node {
        Node::Pane { id, kind, .. } => (id == pane).then_some(kind),
        Node::Split { a, b, .. } => find_pane(a, pane).or_else(|| find_pane(b, pane)),
    }
}

/// Whether `pane` is an agent pane in some space of `spaces`.
pub(crate) fn is_agent_pane(spaces: &Spaces, pane: &str) -> bool {
    spaces
        .spaces
        .iter()
        .find_map(|s| space_find(s, pane))
        .is_some_and(|kind| matches!(kind, PaneKind::Agent { .. }))
}

fn find_pane_mut<'a>(node: &'a mut Node, pane: &str) -> Option<&'a mut PaneKind> {
    match node {
        Node::Pane { id, kind, .. } => (id == pane).then_some(kind),
        Node::Split { a, b, .. } => match find_pane_mut(a, pane) {
            Some(kind) => Some(kind),
            None => find_pane_mut(b, pane),
        },
    }
}

/// The top-left pane.
pub fn first_pane(node: &Node) -> &str {
    match node {
        Node::Pane { id, .. } => id,
        Node::Split { a, .. } => first_pane(a),
    }
}

/// The bottom-right pane.
pub(crate) fn last_pane(node: &Node) -> &str {
    match node {
        Node::Pane { id, .. } => id,
        Node::Split { b, .. } => last_pane(b),
    }
}

/// Every pane with its kind, top-left first.
pub fn panes(node: &Node) -> Vec<(&str, &PaneKind)> {
    match node {
        Node::Pane { id, kind, .. } => vec![(id, kind)],
        Node::Split { a, b, .. } => {
            let mut all = panes(a);
            all.extend(panes(b));
            all
        }
    }
}

/// Where a split that names no side puts the new pane: `panes.placement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Along the pane's longer side.
    Dwindle,
    /// Always to the right.
    Off,
    /// The first pane keeps the left half; the rest share one column on the right.
    MainStack,
    /// Every pane in the squarest rows of equal sizes.
    Grid,
}

impl Placement {
    /// The strategy a `panes.placement` value names.
    pub fn parse(value: &str) -> Option<Placement> {
        match value {
            "dwindle" => Some(Placement::Dwindle),
            "off" => Some(Placement::Off),
            "main-stack" => Some(Placement::MainStack),
            "grid" => Some(Placement::Grid),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Placement::Dwindle => "dwindle",
            Placement::Off => "off",
            Placement::MainStack => "main-stack",
            Placement::Grid => "grid",
        }
    }
}

/// Split `pane` by `strategy`, the new empty pane `new_id` going where it puts it, and say which
/// side that was. `size` is the pane's terminal grid, columns by rows; a pane with none splits
/// right. A terminal cell is about twice as tall as wide, so a pane is wider than tall when it
/// has more than twice as many columns as rows. False when no pane matches.
pub fn place(
    node: &mut Node,
    pane: &str,
    strategy: Placement,
    size: Option<(u16, u16)>,
    split_id: &str,
    new_id: &str,
) -> Option<Dir> {
    if matches!(strategy, Placement::MainStack | Placement::Grid) {
        find_leaf(node, pane)?;
        return Some(rebuild(node, strategy, split_id, new_id));
    }
    let dir = match (strategy, size) {
        (Placement::Dwindle, Some((cols, rows))) if u32::from(cols) <= 2 * u32::from(rows) => {
            Dir::Down
        }
        _ => Dir::Right,
    };
    split(node, pane, dir, split_id, new_id).then_some(dir)
}

/// Rebuild the tree of a space from its panes in reading order plus the new empty pane `new_id`
/// last, arranged by `strategy`. Every leaf keeps its id and what it shows; only the splits and
/// their ratios change. The old splits' ids are reused, and `split_id` covers the one more split
/// the extra pane needs. Says which side of its neighbour the new pane ended up on.
fn rebuild(node: &mut Node, strategy: Placement, split_id: &str, new_id: &str) -> Dir {
    let mut leaves = Vec::new();
    let mut ids = Vec::new();
    collect(node, &mut leaves, &mut ids);
    ids.push(split_id.to_string());
    leaves.push(Node::Pane {
        id: new_id.to_string(),
        status: None,
        work: None,
        kind: PaneKind::empty(),
    });
    let n = leaves.len();
    let mut ids = ids.into_iter();
    match strategy {
        Placement::MainStack => {
            let main = leaves.remove(0);
            let column = chain_nodes(leaves, Axis::Vertical, &mut ids);
            *node = Node::Split {
                id: ids
                    .next()
                    .expect("the old splits and the new one cover the tree"),
                axis: Axis::Horizontal,
                ratio: 0.5,
                a: Box::new(main),
                b: Box::new(column),
            };
            if n == 2 {
                Dir::Right
            } else {
                Dir::Down
            }
        }
        _ => {
            let columns = (1..=n).find(|c| c * c >= n).unwrap_or(1);
            let mut leaves = leaves.into_iter();
            let mut rows = Vec::new();
            let mut left = n;
            let mut last = 1;
            while left > 0 {
                last = columns.min(left);
                left -= last;
                let row: Vec<Node> = leaves.by_ref().take(last).collect();
                rows.push(chain_nodes(row, Axis::Horizontal, &mut ids));
            }
            *node = chain_nodes(rows, Axis::Vertical, &mut ids);
            if last > 1 {
                Dir::Right
            } else {
                Dir::Down
            }
        }
    }
}

/// The leaves of `node` in reading order, and its split ids in pre-order.
fn collect(node: &Node, leaves: &mut Vec<Node>, ids: &mut Vec<String>) {
    match node {
        Node::Pane { .. } => leaves.push(node.clone()),
        Node::Split { id, a, b, .. } => {
            ids.push(id.clone());
            collect(a, leaves, ids);
            collect(b, leaves, ids);
        }
    }
}

/// `nodes` in equal shares along `axis`: the first takes 1/len, the rest nest.
fn chain_nodes(mut nodes: Vec<Node>, axis: Axis, ids: &mut impl Iterator<Item = String>) -> Node {
    let first_ids: Vec<String> = ids.by_ref().take(nodes.len() - 1).collect();
    let mut result = nodes.pop().expect("a chain holds a node");
    let total = nodes.len() + 1;
    for (index, (node, id)) in nodes.into_iter().zip(first_ids).enumerate().rev() {
        result = Node::Split {
            id,
            axis,
            ratio: 1.0 / (total - index) as f64,
            a: Box::new(node),
            b: Box::new(result),
        };
    }
    result
}

/// Replace pane `pane` with a split holding it and a new empty pane on the `dir` side, sharing
/// the area evenly. False when no pane matches.
pub fn split(node: &mut Node, pane: &str, dir: Dir, split_id: &str, new_id: &str) -> bool {
    let new = Node::Pane {
        id: new_id.to_string(),
        status: None,
        work: None,
        kind: PaneKind::empty(),
    };
    insert_beside(node, pane, dir, split_id, new).is_ok()
}

/// Replace pane `target` with a split holding it and `leaf`, `leaf` on the `dir` side, sharing
/// the area evenly. Gives `leaf` back when no pane matches.
fn insert_beside(
    node: &mut Node,
    target: &str,
    dir: Dir,
    split_id: &str,
    leaf: Node,
) -> Result<(), Box<Node>> {
    match node {
        Node::Pane { id, .. } if id == target => {
            let old = std::mem::replace(node, leaf_placeholder());
            let (axis, a, b) = match dir {
                Dir::Right => (Axis::Horizontal, old, leaf),
                Dir::Left => (Axis::Horizontal, leaf, old),
                Dir::Down => (Axis::Vertical, old, leaf),
                Dir::Up => (Axis::Vertical, leaf, old),
            };
            *node = Node::Split {
                id: split_id.to_string(),
                axis,
                ratio: 0.5,
                a: Box::new(a),
                b: Box::new(b),
            };
            Ok(())
        }
        Node::Pane { .. } => Err(Box::new(leaf)),
        Node::Split { a, b, .. } => match insert_beside(a, target, dir, split_id, leaf) {
            Ok(()) => Ok(()),
            Err(leaf) => insert_beside(b, target, dir, split_id, *leaf),
        },
    }
}

/// Put `leaf` at the end of the tree: a split of the whole area with the old tree on the left.
fn append_end(node: &mut Node, split_id: &str, leaf: Node) {
    let old = std::mem::replace(node, leaf_placeholder());
    *node = Node::Split {
        id: split_id.to_string(),
        axis: Axis::Horizontal,
        ratio: 0.5,
        a: Box::new(old),
        b: Box::new(leaf),
    };
}

/// A space's name: `name` trimmed, else the last component of its directory, else the directory.
fn space_name(name: Option<&str>, cwd: &str) -> String {
    name.map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| {
            Path::new(cwd)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| cwd.to_string())
}

fn leaf_placeholder() -> Node {
    Node::Pane {
        id: String::new(),
        status: None,
        work: None,
        kind: PaneKind::empty(),
    }
}

/// The leaf of pane `pane`, whole: its id and what it shows.
fn find_leaf<'a>(node: &'a Node, pane: &str) -> Option<&'a Node> {
    match node {
        Node::Pane { id, .. } if id == pane => Some(node),
        Node::Pane { .. } => None,
        Node::Split { a, b, .. } => find_leaf(a, pane).or_else(|| find_leaf(b, pane)),
    }
}

pub(crate) fn find_leaf_mut<'a>(node: &'a mut Node, pane: &str) -> Option<&'a mut Node> {
    match node {
        Node::Pane { id, .. } if id == pane => Some(node),
        Node::Pane { .. } => None,
        Node::Split { a, b, .. } => find_leaf_mut(a, pane).or_else(|| find_leaf_mut(b, pane)),
    }
}

/// Take pane `pane` out of whichever tree of `space` holds it, whole, and give the leaf back. The
/// grid left with no pane gets one new empty pane named by `fresh`; a popped tree left with none
/// is gone; a tray leaf leaves the tray.
pub(crate) fn take_leaf(
    space: &mut SpaceInfo,
    pane: &str,
    fresh: &mut dyn FnMut() -> String,
) -> Option<Node> {
    let leaf = space.trees().find_map(|t| find_leaf(t, pane)).cloned()?;
    match space.site_of(pane)? {
        PaneSite::Grid => {
            let root = std::mem::replace(&mut space.root, leaf_placeholder());
            space.root = match remove(root, pane) {
                Removal::Removed { rest, .. } | Removal::NotFound(rest) => rest,
                Removal::Last(_) => Node::Pane {
                    id: fresh(),
                    status: None,
                    work: None,
                    kind: PaneKind::empty(),
                },
            };
        }
        PaneSite::Popped => {
            let at = space.popped.iter().position(|t| t.holds(pane))?;
            let tree = space.popped.remove(at);
            match remove(tree, pane) {
                Removal::Removed { rest, .. } | Removal::NotFound(rest) => {
                    space.popped.insert(at, rest)
                }
                Removal::Last(_) => {}
            }
        }
        PaneSite::Backgrounded => space.backgrounded.retain(|t| !t.holds(pane)),
    }
    Some(leaf)
}

/// Exchange the places of panes `x` and `y`; each keeps its id, its manager and what it shows.
/// False, with the tree unchanged, when either is missing or they are one pane.
pub fn swap(node: &mut Node, x: &str, y: &str) -> bool {
    if x == y {
        return false;
    }
    let (Some(x_leaf), Some(y_leaf)) = (find_leaf(node, x).cloned(), find_leaf(node, y).cloned())
    else {
        return false;
    };
    // Ids are unique, so each slot is found by id while it still holds its own pane: x's slot
    // parks a placeholder, y's takes x, and the placeholder takes y.
    if let Some(slot) = find_leaf_mut(node, x) {
        *slot = leaf_placeholder();
    }
    if let Some(slot) = find_leaf_mut(node, y) {
        *slot = x_leaf;
    }
    if let Some(slot) = find_leaf_mut(node, "") {
        *slot = y_leaf;
    }
    true
}

/// Take pane `pane` out of the tree and put it on the `dir` side of pane `target`, in a new
/// split `split_id` sharing `target`'s area evenly; the split the pane left collapses. An error
/// leaves the tree as it was.
pub fn move_pane(
    node: &mut Node,
    pane: &str,
    target: &str,
    dir: Dir,
    split_id: &str,
) -> Result<(), String> {
    if pane == target {
        return Err(format!("{pane} cannot move beside itself"));
    }
    let Some(leaf) = find_leaf(node, pane).cloned() else {
        return Err(format!("no pane {pane}"));
    };
    if find_leaf(node, target).is_none() {
        return Err(format!("no pane {target}"));
    }
    let root = std::mem::replace(node, leaf_placeholder());
    let Removal::Removed { rest, .. } = remove(root, pane) else {
        unreachable!("{pane} was found above and the tree holds more than it")
    };
    *node = rest;
    insert_beside(node, target, dir, split_id, leaf).map_err(|_| format!("no pane {target}"))
}

/// What taking a pane out of a tree left.
#[derive(Debug, PartialEq)]
pub enum Removal {
    /// No pane matched; the tree, unchanged.
    NotFound(Node),
    /// The pane was the whole tree.
    Last(PaneKind),
    Removed { rest: Node, kind: PaneKind },
}

/// Take pane `pane` out of the tree; its sibling takes the parent split's place.
pub fn remove(node: Node, pane: &str) -> Removal {
    match node {
        Node::Pane { id, kind, .. } if id == pane => Removal::Last(kind),
        leaf @ Node::Pane { .. } => Removal::NotFound(leaf),
        Node::Split {
            id,
            axis,
            ratio,
            a,
            b,
        } => {
            let rebuild = |a: Node, b: Node| Node::Split {
                id,
                axis,
                ratio,
                a: Box::new(a),
                b: Box::new(b),
            };
            match remove(*a, pane) {
                Removal::Last(kind) => Removal::Removed { rest: *b, kind },
                Removal::Removed { rest, kind } => Removal::Removed {
                    rest: rebuild(rest, *b),
                    kind,
                },
                Removal::NotFound(a) => match remove(*b, pane) {
                    Removal::Last(kind) => Removal::Removed { rest: a, kind },
                    Removal::Removed { rest, kind } => Removal::Removed {
                        rest: rebuild(a, rest),
                        kind,
                    },
                    Removal::NotFound(b) => Removal::NotFound(rebuild(a, b)),
                },
            }
        }
    }
}

/// Set split `split`'s ratio. False when no split matches.
pub fn set_ratio(node: &mut Node, split: &str, value: f64) -> bool {
    match node {
        Node::Pane { .. } => false,
        Node::Split {
            id, ratio, a, b, ..
        } => {
            if id == split {
                *ratio = value;
                true
            } else {
                set_ratio(a, split, value) || set_ratio(b, split, value)
            }
        }
    }
}

/// Default size for a pane's shell until a client attaches and resizes it.
pub(crate) const PANE_COLS: u16 = 80;
pub(crate) const PANE_ROWS: u16 = 24;

/// How long a browser pane's saved page waits for the next one before it is written. A person
/// browsing reports a page per load, per redirect and per in-page navigation, and the state file
/// is a convenience for the next start rather than a record of where they have been: writing it
/// once per burst keeps the last page without writing the whole layout dozens of times a minute.
/// Every other change to the layout still writes it at once, and any such write takes a page
/// waiting here with it.
const SAVE_COALESCE: Duration = Duration::from_millis(400);

/// The most characters of a waiting agent's reason a pane keeps.
const REASON_MAX: usize = 300;

/// How often each terminal pane's directory and agent are sampled.
const TERMINAL_SAMPLE: Duration = Duration::from_secs(2);

/// How long a new shell may show no agent without its pane forgetting one it was just told to
/// resume.
const RESUME_GRACE: Duration = Duration::from_secs(30);

/// What `pane_start` is given beyond where to start: for an agent pane a first message, the agent
/// (by name, else the `agent.default` setting), a model and its permissions. A terminal takes none.
#[derive(Debug, Default)]
pub struct StartOptions {
    pub message: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub permissions: AgentPermissions,
}

/// The conversation a forked agent copies, and where and under which account it was kept.
struct ForkSource {
    conversation: String,
    turns: usize,
    cwd: Option<String>,
    home: Option<String>,
}

/// What `pane_start` returns: the pane, and for an agent its agent and state.
#[derive(Debug, Serialize)]
pub struct Started {
    pub pane: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<AgentState>,
}

impl AppState {
    pub(crate) fn new_id(&self, prefix: char) -> String {
        format!("{prefix}{}", self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    pub fn spaces(&self) -> Spaces {
        self.spaces.lock().unwrap().clone()
    }

    /// The id of the space that holds `pane`.
    pub fn space_holding(&self, pane: &str) -> Result<String, String> {
        let spaces = self.spaces.lock().unwrap();
        Ok(spaces.spaces[spaces.holding(pane)?].id.clone())
    }

    /// The layout for a reader that asks (`status`): every agent pane
    /// carries its agent's state beside the agent's name, and every pane and space its
    /// status. The saved layout and the broadcast leave them out, since they change without the
    /// layout changing.
    pub fn listing(&self) -> Spaces {
        let mut spaces = self.spaces();
        for space in &mut spaces.spaces {
            each_space_kind_mut(space, &mut |kind| {
                if let PaneKind::Agent {
                    agent,
                    state,
                    model,
                    context,
                    permissions,
                    ..
                } = kind
                {
                    let running = self.agent(agent).ok();
                    *state = running.as_ref().map(|s| Box::new(s.state()));
                    *model = running.as_ref().and_then(|s| s.model());
                    *context = running.as_ref().and_then(|s| s.context());
                    if let Some(running) = &running {
                        *permissions = self.current_permissions(running);
                    }
                }
            });
        }
        self.with_statuses(spaces)
    }

    /// The permissions `running` is in now: the mode a person may have changed since the start.
    fn current_permissions(&self, running: &crate::agent::Agent) -> Option<Box<AgentPermissions>> {
        let p = running.permissions();
        AgentPermissions::boxed(p.permission_mode, p.sandbox)
    }

    /// Write the saved state — the layout and the id counter. The one place that touches the file.
    /// Each running agent's pane is saved with the mode its agent is in now, so a restart starts
    /// it there and not in the mode it was first started with.
    pub(crate) fn save_state(&self, spaces: &Spaces) {
        let Some(path) = self.state_file.lock().unwrap().clone() else {
            return;
        };
        let mut spaces = spaces.clone();
        for space in &mut spaces.spaces {
            each_space_kind_mut(space, &mut |kind| {
                if let PaneKind::Agent {
                    agent, permissions, ..
                } = kind
                {
                    if let Ok(running) = self.agent(agent) {
                        *permissions = self.current_permissions(&running);
                    }
                }
            });
        }
        let spaces = &spaces;
        let extra = self.saved_extra.lock().unwrap().clone();
        let recent = self.recent.lock().unwrap().clone();
        match save(
            &path,
            spaces,
            self.next_id.load(Ordering::SeqCst),
            recent,
            extra,
        ) {
            Ok(()) => {
                self.saves.fetch_add(1, Ordering::SeqCst);
                tracing::debug!(path = %path.display(), spaces = spaces.spaces.len(), "spaces saved")
            }
            Err(e) => tracing::error!(path = %path.display(), error = %e, "spaces save failed"),
        }
    }

    /// Write the layout now, and let go of a coalesced save that was scheduled: the daemon is
    /// stopping, and a page a person navigated to in the last moment must be in the file they come
    /// back to. Clearing the flag only stops this write being counted against the next burst; a
    /// task already sleeping still writes once more, harmlessly, if the process outlives it.
    pub fn save_layout_now(&self) {
        self.sample_terminals();
        self.save_pending.store(false, Ordering::SeqCst);
        let spaces = self.spaces.lock().unwrap();
        self.save_state(&spaces);
    }

    /// [`AppState::save_soon`] from a caller that does not hold the spaces lock.
    pub(crate) fn save_layout_soon(&self) {
        let _spaces = self.spaces.lock().unwrap();
        self.save_soon();
    }

    /// Save the layout a moment from now, coalescing whatever else asks in the meantime into one
    /// write. For a change that happens as often as a person loads a page; everything else saves
    /// at once through [`AppState::save_state`], and such a save carries a page waiting here with
    /// it, because both write the same whole layout.
    ///
    /// Call with the spaces lock held: this takes no lock itself, and the task it spawns takes the
    /// lock only after the caller has let go. Call from inside the tokio runtime.
    pub(crate) fn save_soon(&self) {
        // One task per burst: the first caller schedules the write and the rest ride on it, which
        // is what makes twenty navigations in a second one write rather than twenty.
        if self.save_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(SAVE_COALESCE).await;
            let Some(state) = me.upgrade() else {
                return;
            };
            state.save_pending.store(false, Ordering::SeqCst);
            // Whatever the layout says *now*, which is the last page reported rather than the one
            // that scheduled this.
            let spaces = state.spaces.lock().unwrap();
            state.save_state(&spaces);
        });
    }

    /// Save and broadcast the layout. Called with the spaces lock held, so clients see changes in
    /// the order they happened.
    pub(crate) fn changed(&self, spaces: &Spaces) {
        self.save_state(spaces);
        self.broadcast_layout(spaces);
        self.sync_settings_watch_with(spaces);
    }

    /// Send the layout to every client without writing it: for a change a restart need not keep,
    /// such as an agent's context figure, which the next save carries and the agent reports again.
    fn broadcast_layout(&self, spaces: &Spaces) {
        let _ = self.global.send(ServerMsg::Spaces {
            spaces: spaces.spaces.clone(),
            sessions: spaces.sessions.clone(),
        });
    }

    /// End what a closed or replaced pane showed: kill its terminal or agent, or stop watching its
    /// artifact and remove the data directory it was given. Whatever was uploaded to the pane goes
    /// too; switching a pane between its chat and its terminal view comes nowhere near here, so the
    /// conversation keeps its files.
    fn drop_content(&self, pane: &str, kind: &PaneKind) {
        self.drop_uploads(pane);
        match kind {
            PaneKind::Terminal { term, .. } => {
                if let Some(held) = self.terms.lock().unwrap().remove(term) {
                    tracing::info!(term = %term, "pty killed");
                    held.term.kill();
                }
            }
            PaneKind::Web { .. } => self.drop_artifact(pane),
            PaneKind::Browser { .. } => self.console.lock().unwrap().clear(pane),
            PaneKind::Agent { agent, .. } => {
                self.end_agent(agent);
                // The caller holds the spaces lock, so the sweep that deletes this agent's
                // artifacts waits for the layout without the pane.
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    let me = self.me.clone();
                    runtime.spawn_blocking(move || {
                        if let Some(state) = me.upgrade() {
                            state.artifact_sweep();
                        }
                    });
                }
            }
            // A browser pane holds only its console messages (dropped above): no folder, no
            // data directory, no watch, no refresh, no terminal. The client's web view goes
            // with the pane.
            PaneKind::Empty { .. } | PaneKind::Unknown(_) => {}
        }
    }

    /// Make a space for `cwd`, with one empty pane. With `background` it runs with no window.
    pub fn space_create(
        &self,
        cwd: String,
        name: Option<String>,
        background: bool,
        from_pane: Option<&str>,
        session: Option<&str>,
    ) -> Result<SpaceInfo, String> {
        let path = Path::new(&cwd);
        if !path.is_absolute() || !path.is_dir() {
            return Err(format!("{cwd} is not an absolute path to a directory"));
        }
        let name = space_name(name.as_deref(), &cwd);
        let (id, pane) = (self.new_id('s'), self.new_id('p'));
        let space = SpaceInfo {
            id: id.clone(),
            name: name.clone(),
            cwd: cwd.clone(),
            root: Node::Pane {
                id: pane.clone(),
                status: None,
                work: None,
                kind: PaneKind::empty(),
            },
            status: None,
            background,
            popped: Vec::new(),
            backgrounded: Vec::new(),
        };
        let mut spaces = self.spaces.lock().unwrap();
        let joined = self.session_to_join(&mut spaces, from_pane, session)?;
        spaces.sessions[joined].spaces.push(id.clone());
        spaces.spaces.push(space.clone());
        tracing::info!(space = %id, %name, %cwd, background, "space created");
        self.record_recent(&cwd);
        self.changed(&spaces);
        Ok(space)
    }

    /// The index of the session a new space joins: the one named, else the pane's, else the
    /// first. With no session at all, "Main" is made.
    fn session_to_join(
        &self,
        spaces: &mut Spaces,
        pane: Option<&str>,
        session: Option<&str>,
    ) -> Result<usize, String> {
        // A session named outright wins over the pane the caller happens to run in.
        if let Some(session) = session {
            return spaces.session_index(session);
        }
        if let Some(pane) = pane {
            // A pane that has since closed (a stale `$WHEEL_PANE`) falls through to the first
            // session rather than failing the new space.
            match spaces.holding(pane) {
                Ok(at) => {
                    let space = spaces.spaces[at].id.clone();
                    if let Some(i) = spaces.session_holding(&space) {
                        return Ok(i);
                    }
                }
                Err(_) => tracing::warn!(%pane, "the pane a new space asked to join no longer exists"),
            }
        }
        if !spaces.sessions.is_empty() {
            return Ok(0);
        }
        let id = self.new_id('n');
        spaces.sessions.push(SessionInfo {
            id,
            name: "Main".into(),
            spaces: Vec::new(),
        });
        Ok(0)
    }

    /// Make an empty session named `name`.
    pub fn session_new(&self, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a session needs a name".into());
        }
        let id = self.new_id('n');
        let mut spaces = self.spaces.lock().unwrap();
        spaces.sessions.push(SessionInfo {
            id: id.clone(),
            name: name.to_string(),
            spaces: Vec::new(),
        });
        tracing::info!(session = %id, %name, "session created");
        self.changed(&spaces);
        Ok(id)
    }

    /// Give a session the name `name`, trimmed; an empty name is refused.
    pub fn session_rename(&self, session: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a session needs a name".into());
        }
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.session_index(session)?;
        let from = std::mem::replace(&mut spaces.sessions[index].name, name.to_string());
        tracing::info!(session = %spaces.sessions[index].id, %from, to = %name, "session renamed");
        self.changed(&spaces);
        Ok(())
    }

    /// Close a session: every space in it closes with its panes, then the session goes.
    pub fn session_close(&self, session: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.session_index(session)?;
        let id = spaces.sessions[index].id.clone();
        for space in spaces.sessions[index].spaces.clone() {
            if let Ok(at) = spaces.index(&space) {
                self.close_locked(&mut spaces, at);
            }
        }
        spaces.sessions.retain(|s| s.id != id);
        tracing::info!(session = %id, "session closed");
        self.changed(&spaces);
        Ok(())
    }

    /// Put a session at `index` among the sessions, clamped to the last position.
    pub fn session_reorder(&self, session: &str, index: usize) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let from = spaces.session_index(session)?;
        let moved = spaces.sessions.remove(from);
        let to = index.min(spaces.sessions.len());
        tracing::info!(session = %moved.id, from, to, "session reordered");
        spaces.sessions.insert(to, moved);
        self.changed(&spaces);
        Ok(())
    }

    /// Move a space into a session, at `index` among its spaces (the end when none). Nothing in
    /// the space stops.
    pub fn session_move(
        &self,
        space: &str,
        session: &str,
        index: Option<usize>,
    ) -> Result<(), String> {
        let close_empty = self.closes_empty_sessions();
        let mut spaces = self.spaces.lock().unwrap();
        let space = spaces.spaces[spaces.index(space)?].id.clone();
        let to = spaces.session_index(session)?;
        let from = spaces.session_holding(&space);
        spaces.detach(&space);
        let held = &mut spaces.sessions[to].spaces;
        let at = index.unwrap_or(held.len()).min(held.len());
        held.insert(at, space.clone());
        tracing::info!(%space, session = %spaces.sessions[to].id, at, "space moved to a session");
        if close_empty {
            self.close_if_empty(&mut spaces, from);
        }
        self.changed(&spaces);
        Ok(())
    }

    /// Put `dir` at the top of the recent list, dropping its older row and the rows past the limit.
    fn record_recent(&self, dir: &str) {
        let opened_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let mut recent = self.recent.lock().unwrap();
        recent.retain(|r| r.dir != dir);
        recent.insert(
            0,
            wheel_proto::RecentDir {
                dir: dir.to_string(),
                opened_ms,
            },
        );
        recent.truncate(RECENT_LIMIT);
        tracing::info!(%dir, opened_ms, rows = recent.len(), "space recent recorded");
    }

    /// Give a space the name `name`, trimmed. An empty name is refused, so a space is never
    /// nameless; the folder on disk is untouched.
    pub fn space_rename(&self, id: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a space needs a name".into());
        }
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.index(id)?;
        let from = std::mem::replace(&mut spaces.spaces[index].name, name.to_string());
        tracing::info!(space = %id, %from, to = %name, "space renamed");
        self.changed(&spaces);
        Ok(())
    }

    /// Close a space and every pane in it. With `if_empty`, only while it holds one empty pane.
    pub fn space_close(&self, id: &str, if_empty: bool) -> Result<(), String> {
        let close_empty = self.closes_empty_sessions();
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.index(id)?;
        let held = &spaces.spaces[index];
        if if_empty
            && (!held.popped.is_empty()
                || !held.backgrounded.is_empty()
                || !matches!(
                    &held.root,
                    Node::Pane {
                        kind: PaneKind::Empty { .. },
                        ..
                    }
                ))
        {
            tracing::info!(space = %id, "space close refused: it no longer holds just one empty pane");
            return Err(format!("space {id} is no longer empty; not closed"));
        }
        let session = spaces.session_holding(id);
        self.close_locked(&mut spaces, index);
        if close_empty {
            self.close_if_empty(&mut spaces, session);
        }
        self.changed(&spaces);
        Ok(())
    }

    /// Whether `sessions.close_empty` is on. Read before the spaces lock is taken.
    fn closes_empty_sessions(&self) -> bool {
        self.setting(SESSIONS_CLOSE_EMPTY, None).is_ok_and(|v| v == "true")
    }

    /// Drop the session at `index` when it holds no space, unless it is the last one, which
    /// stays so there is always a session for a new space to join.
    fn close_if_empty(&self, spaces: &mut Spaces, index: Option<usize>) {
        let Some(index) = index else { return };
        if spaces.sessions.len() > 1 && spaces.sessions[index].spaces.is_empty() {
            let gone = spaces.sessions.remove(index);
            tracing::info!(session = %gone.id, name = %gone.name, "empty session closed");
        }
    }

    /// Remove the space at `index` and end what its panes showed. The caller holds the lock and
    /// announces the change.
    fn close_locked(&self, spaces: &mut Spaces, index: usize) {
        let space = spaces.remove(index);
        self.streams.close_space(&space.id);
        self.end_agent(&crate::orchestrator::agent_name(&space.id));
        for (pane, kind) in space_panes(&space) {
            self.drop_content(pane, kind);
        }
        tracing::info!(space = %space.id, "space closed");
    }

    /// Ask every client to show a space, or a pane in it. The daemon keeps no view: nothing
    /// changes here, and each client decides what it shows and reports it with `view`.
    pub fn focus_requested(&self, space: Option<&str>, pane: Option<&str>) -> Result<(), String> {
        let spaces = self.spaces.lock().unwrap();
        let (space, pane) = match (space, pane) {
            (_, Some(pane)) => {
                let index = spaces.holding(pane)?;
                let held = &spaces.spaces[index].id;
                if space.is_some_and(|s| s != held) {
                    return Err(format!(
                        "pane {pane} is not in space {}",
                        space.unwrap_or_default()
                    ));
                }
                (held.clone(), Some(pane.to_string()))
            }
            (Some(space), None) => (spaces.spaces[spaces.index(space)?].id.clone(), None),
            (None, None) => return Err("focus needs a space or a pane".into()),
        };
        drop(spaces);
        tracing::info!(%space, pane = ?pane, "show requested");
        let _ = self.global.send(ServerMsg::SpaceFocused { space, pane });
        Ok(())
    }

    /// Ask every client to show `pane` in the space that holds it.
    pub fn show_pane(&self, pane: &str) {
        let _ = self.focus_requested(None, Some(pane));
    }

    /// Split `pane`, putting a new empty pane on the `dir` side.
    ///
    /// With no `dir`, `panes.placement` (as it resolves for the pane's space) chooses the side;
    /// the strategy's name comes back with the new pane's id.
    pub fn pane_split(
        &self,
        pane: &str,
        dir: Option<Dir>,
        site: Option<PaneSite>,
    ) -> Result<(String, Option<Placement>), String> {
        let created_apart = matches!(site, Some(PaneSite::Popped | PaneSite::Backgrounded));
        if created_apart && dir.is_some() {
            return Err("a popped or backgrounded pane has no side".into());
        }
        let strategy = match dir {
            Some(_) => None,
            None if created_apart => None,
            None => {
                let space_id = {
                    let spaces = self.spaces.lock().unwrap();
                    spaces.spaces[spaces.holding(pane)?].id.clone()
                };
                let value = self.setting(PLACEMENT, Some(&space_id))?;
                Some(Placement::parse(&value).unwrap_or(Placement::Dwindle))
            }
        };
        let size = match strategy {
            Some(Placement::Dwindle) => self.pane_grid(pane),
            _ => None,
        };
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let (split_id, new_id) = (self.new_id('x'), self.new_id('p'));
        let space = &mut spaces.spaces[index];
        if created_apart {
            let leaf = Node::Pane {
                id: new_id.clone(),
                status: None,
                work: None,
                kind: PaneKind::empty(),
            };
            let placed = match site {
                Some(PaneSite::Popped) => {
                    space.popped.push(leaf);
                    PaneSite::Popped
                }
                _ => {
                    space.backgrounded.push(leaf);
                    PaneSite::Backgrounded
                }
            };
            tracing::info!(space = %space.id, %pane, new = %new_id, site = placed.name(), "pane created apart from the grid");
            self.changed(&spaces);
            return Ok((new_id, None));
        }
        if space.site_of(pane) == Some(PaneSite::Backgrounded) {
            return Err(format!("{pane} is backgrounded; return it before splitting it"));
        }
        let tree = space
            .trees_mut()
            .find(|t| t.holds(pane))
            .ok_or_else(|| format!("no pane {pane}"))?;
        let dir = match (dir, strategy) {
            (Some(dir), _) => {
                split(tree, pane, dir, &split_id, &new_id);
                dir
            }
            (None, Some(strategy)) => place(tree, pane, strategy, size, &split_id, &new_id)
                .ok_or_else(|| format!("no pane {pane}"))?,
            (None, None) => unreachable!("a missing side always has a strategy"),
        };
        let space_id = space.id.clone();
        tracing::info!(space = %space_id, %pane, ?dir, placed_by = strategy.map(Placement::name), ?size, split = %split_id, new = %new_id, "pane split");
        self.changed(&spaces);
        Ok((new_id, strategy))
    }

    /// The terminal grid of `pane`, columns by rows, when it shows a terminal the daemon holds.
    pub(crate) fn pane_grid(&self, pane: &str) -> Option<(u16, u16)> {
        let term = {
            let spaces = self.spaces.lock().unwrap();
            let index = spaces.holding(pane).ok()?;
            space_panes(&spaces.spaces[index])
                .into_iter()
                .find_map(|(id, kind)| match kind {
                    PaneKind::Terminal { term, .. } if id == pane => Some(term.clone()),
                    _ => None,
                })?
        };
        let info = self.term(&term)?.info();
        (info.cols > 0 && info.rows > 0).then_some((info.cols, info.rows))
    }

    /// Swap the places of panes `pane` and `with`, which share a space.
    pub fn pane_swap(&self, pane: &str, with: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        if spaces.holding(with)? != index {
            return Err(format!("{pane} and {with} are in different spaces"));
        }
        let space = &mut spaces.spaces[index];
        let tree = space
            .trees_mut()
            .find(|t| t.holds(pane) && t.holds(with))
            .ok_or_else(|| format!("{pane} and {with} are not in the same place"))?;
        if !swap(tree, pane, with) {
            return Err(format!("{pane} cannot swap with itself"));
        }
        tracing::info!(space = %space.id, %pane, %with, "panes swapped");
        self.changed(&spaces);
        Ok(())
    }

    /// Move `pane` to `to`, keeping its id, its process and its scrollback. Beside a pane in its
    /// own space, the split `pane` left collapses and a new one holds `pane` and the target.
    /// Across spaces (beside a pane, at the end of a space, or in a new space after its own) the
    /// source space shows its remaining panes, or one new empty pane when none remain, and a
    /// target space whose only pane is empty loses that empty pane. An error changes nothing.
    pub fn pane_move(&self, pane: &str, to: MoveTarget) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let from = spaces.holding(pane)?;
        let split_id = self.new_id('x');
        let dest = match &to {
            MoveTarget::Beside { pane: target, .. } => {
                if target == pane {
                    return Err(format!("{pane} cannot move beside itself"));
                }
                let held = spaces.holding(target)?;
                if spaces.spaces[held].site_of(target) == Some(PaneSite::Backgrounded) {
                    return Err(format!("{target} is backgrounded; nothing can be put beside it"));
                }
                Some(held)
            }
            MoveTarget::End { space } => Some(spaces.index(space)?),
            MoveTarget::NewSpace { .. } => None,
        };
        if let (Some(dest), MoveTarget::Beside { pane: target, dir }) = (dest, &to) {
            if dest == from {
                let space = &mut spaces.spaces[from];
                let one_tree = space.trees().any(|t| t.holds(pane) && t.holds(target));
                if one_tree {
                    let tree = space
                        .trees_mut()
                        .find(|t| t.holds(pane))
                        .expect("the tree was found above");
                    move_pane(tree, pane, target, *dir, &split_id)?;
                } else {
                    let mut fresh = || self.new_id('p');
                    let leaf = take_leaf(space, pane, &mut fresh)
                        .ok_or_else(|| format!("no pane {pane}"))?;
                    let tree = space
                        .trees_mut()
                        .find(|t| t.holds(target))
                        .ok_or_else(|| format!("no pane {target}"))?;
                    if insert_beside(tree, target, *dir, &split_id, leaf).is_err() {
                        unreachable!("{target} was found in its space above");
                    }
                }
                tracing::info!(space = %space.id, %pane, %target, ?dir, split = %split_id, "pane moved");
                self.changed(&spaces);
                return Ok(());
            }
        }
        let alone = matches!(&spaces.spaces[from].root, Node::Pane { .. })
            && spaces.spaces[from].site_of(pane) == Some(PaneSite::Grid);
        if alone && dest == Some(from) {
            return Err(format!("{pane} is already the whole of its space"));
        }
        let new_space = match &to {
            MoveTarget::NewSpace { name, .. } => {
                let cwd = spaces.spaces[from].cwd.clone();
                let name = space_name(name.as_deref(), &cwd);
                Some((self.new_id('s'), name, cwd))
            }
            _ => None,
        };
        // Everything that can refuse has; from here the tree changes.
        let fresh = alone.then(|| self.new_id('p'));
        let source = &mut spaces.spaces[from];
        let leaf = take_leaf(source, pane, &mut || {
            fresh.clone().expect("a lone pane was counted above")
        })
        .ok_or_else(|| format!("no pane {pane}"))?;
        let from_id = source.id.clone();
        let dest_id = match (dest, new_space) {
            (Some(dest), _) => {
                let space = &mut spaces.spaces[dest];
                let lone_empty = matches!(
                    &space.root,
                    Node::Pane {
                        kind: PaneKind::Empty { .. },
                        ..
                    }
                ) && match &to {
                    MoveTarget::Beside { pane: target, .. } => {
                        space.site_of(target) == Some(PaneSite::Grid)
                    }
                    _ => true,
                };
                if lone_empty {
                    space.root = leaf;
                } else {
                    match &to {
                        MoveTarget::Beside { pane: target, dir } => {
                            let tree = space
                                .trees_mut()
                                .find(|t| t.holds(target))
                                .expect("the target was found in its space above");
                            if insert_beside(tree, target, *dir, &split_id, leaf).is_err() {
                                unreachable!("{target} was found in its space above");
                            }
                        }
                        _ => append_end(&mut space.root, &split_id, leaf),
                    }
                }
                space.id.clone()
            }
            (None, Some((id, name, cwd))) => {
                spaces.spaces.insert(
                    from + 1,
                    SpaceInfo {
                        id: id.clone(),
                        name,
                        cwd,
                        root: leaf,
                        status: None,
                        background: false,
                        popped: Vec::new(),
                        backgrounded: Vec::new(),
                    },
                );
                let session = spaces.session_holding(&from_id);
                if let Some(session) = session {
                    let held = &mut spaces.sessions[session].spaces;
                    let at = held.iter().position(|x| *x == from_id).map_or(held.len(), |i| i + 1);
                    held.insert(at, id.clone());
                }
                id
            }
            (None, None) => unreachable!("a move without a space goes to a new one"),
        };
        tracing::info!(from = %from_id, to = %dest_id, %pane, split = %split_id, "pane moved to another space");
        self.changed(&spaces);
        Ok(())
    }

    /// Close a pane; closing a space's last grid pane leaves the space with one new empty pane.
    /// A popped tree that loses its last pane is gone; a backgrounded pane leaves the tray.
    pub fn pane_close(&self, pane: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let space = &mut spaces.spaces[index];
        let site = space.site_of(pane).expect("holding found the pane");
        // An empty pane that is the whole space is already what closing it would leave.
        if site == PaneSite::Grid
            && matches!(&space.root, Node::Pane { id, kind: PaneKind::Empty { .. }, .. } if id == pane)
        {
            return Ok(());
        }
        let kind = space_find(space, pane).cloned().expect("holding found the pane");
        let fresh = self.new_id('p');
        take_leaf(space, pane, &mut || fresh.clone());
        self.drop_content(pane, &kind);
        match site {
            PaneSite::Grid if matches!(&space.root, Node::Pane { id, .. } if *id == fresh) => {
                tracing::info!(space = %space.id, %pane, new = %fresh, "pane closed; it was the last, so the space holds one empty pane");
            }
            _ => {
                tracing::info!(space = %space.id, %pane, site = site.name(), "pane closed");
            }
        }
        self.changed(&spaces);
        Ok(())
    }

    /// Split `pane` and show a page or an artifact in the new pane: a browser pane on `url`, or a
    /// web pane on `artifact` (one of the two). A pane that cannot show it is closed again, so a
    /// refusal leaves the layout as it was. Returns the new pane's id.
    pub async fn pane_open(
        &self,
        pane: &str,
        dir: Option<Dir>,
        site: Option<PaneSite>,
        url: Option<String>,
        artifact: Option<String>,
        scope: Option<ArtifactScope>,
    ) -> Result<String, String> {
        let kind = match (&url, &artifact) {
            (Some(_), None) => NewPaneKind::Browser,
            (None, Some(_)) => NewPaneKind::Web,
            _ => return Err("pane_open needs a url or an artifact, and only one".into()),
        };
        let new = self.pane_split(pane, dir, site)?.0;
        if let Err(e) = self.pane_set_kind(&new, kind, artifact, scope, url).await {
            let _ = self.pane_close(&new);
            return Err(e);
        }
        Ok(new)
    }

    /// Change what a pane shows. A terminal or an agent starts in the space's directory; a web
    /// pane shows the artifact `artifact` names in `scope` (or, with no scope, in the first scope
    /// that has an entry by that name), which only it takes; a browser pane opens `url`, or the
    /// `browser.homepage` setting as it resolves for the pane's space when no URL is given.
    /// Whatever the pane showed before goes. Returns what the pane shows now.
    pub async fn pane_set_kind(
        &self,
        pane: &str,
        kind: NewPaneKind,
        artifact: Option<String>,
        scope: Option<ArtifactScope>,
        url: Option<String>,
    ) -> Result<PaneKind, String> {
        if kind != NewPaneKind::Web && (artifact.is_some() || scope.is_some()) {
            return Err(
                format!("artifact and scope are only for a web pane, not {kind:?}").to_lowercase(),
            );
        }
        if kind != NewPaneKind::Browser && url.is_some() {
            return Err(format!("url is only for a browser pane, not {kind:?}").to_lowercase());
        }
        // A web pane's artifact and its space's directory, kept for the watch the swap below sets.
        let mut watching = None;
        let new = match (kind, artifact) {
            (NewPaneKind::Web, Some(name)) => {
                let cwd = {
                    let spaces = self.spaces.lock().unwrap();
                    spaces.spaces[spaces.holding(pane)?].cwd.clone()
                };
                let artifact = self.pane_artifact_ref(scope, &name, &cwd)?;
                // An entry that names a page rather than a folder opens a browser pane on it; a
                // web pane cannot show http from its own origin (ADR-0005).
                // A registry that cannot be read leaves the pane a web pane, which reports it.
                match self
                    .artifact_entry(&artifact, &cwd)
                    .ok()
                    .flatten()
                    .and_then(|e| e.url)
                {
                    Some(url) => PaneKind::Browser {
                        url: page(&url)?,
                        extra: Extra::default(),
                    },
                    None => {
                        watching = Some((artifact.clone(), cwd));
                        PaneKind::Web {
                            artifact,
                            extra: Extra::default(),
                        }
                    }
                }
            }
            (NewPaneKind::Web, None) => {
                return Err("a web pane needs artifact, the name of an artifact".into())
            }
            (NewPaneKind::Browser, _) => {
                let space = {
                    let spaces = self.spaces.lock().unwrap();
                    spaces.spaces[spaces.holding(pane)?].id.clone()
                };
                // The homepage is read for this pane's own space, so a project that sets its own
                // opens that and every other project opens the machine's.
                let url = match url {
                    Some(url) => url,
                    None => self.setting(HOMEPAGE, Some(&space))?,
                };
                PaneKind::Browser {
                    url: page(&url)?,
                    extra: Extra::default(),
                }
            }
            (NewPaneKind::Empty, _) => PaneKind::empty(),
            (NewPaneKind::Terminal, _) => {
                let cwd = {
                    let spaces = self.spaces.lock().unwrap();
                    spaces.spaces[spaces.holding(pane)?].cwd.clone()
                };
                let shell = self.shell_in(&cwd);
                PaneKind::Terminal {
                    term: self
                        .open_term(
                            cwd.clone(),
                            shell,
                            PANE_COLS,
                            PANE_ROWS,
                            Owner::Pane,
                            Some((pane, &cwd)),
                        )
                        .await?
                        .id
                        .clone(),
                    cwd: None,
                    agent: None,
                    extra: Extra::default(),
                }
            }
        };
        self.install(pane, new, watching)
    }

    /// Put `new` in `pane`, ending what it showed, and return what it shows now. `watching` is a
    /// web pane's artifact and its space's directory, for the watch the swap sets.
    pub(crate) fn install(
        &self,
        pane: &str,
        new: PaneKind,
        watching: Option<(ArtifactRef, String)>,
    ) -> Result<PaneKind, String> {
        let mut spaces = self.spaces.lock().unwrap();
        // The pane may have closed while the shell started.
        let Some(slot) = spaces
            .spaces
            .iter_mut()
            .find_map(|s| space_find_mut(s, pane))
        else {
            self.drop_content(pane, &new);
            return Err(format!("no pane {pane}"));
        };
        let old = std::mem::replace(slot, new);
        self.drop_content(pane, &old);
        if let Some(Node::Pane { work, .. }) = spaces
            .spaces
            .iter_mut()
            .find_map(|s| space_leaf_mut(s, pane))
        {
            if work.take().is_some() {
                tracing::info!(%pane, "pane kind changed; its declared work is cleared");
            }
        }
        if let Some((artifact, cwd)) = &watching {
            self.show_artifact(pane, artifact, cwd);
        }
        // A new agent may have its conversation id already.
        self.sync_conversations(&mut spaces);
        let now = space_find(&spaces.spaces[spaces.holding(pane)?], pane)
            .cloned()
            .ok_or_else(|| format!("no pane {pane}"))?;
        tracing::info!(%pane, kind = ?now, "pane kind");
        self.changed(&spaces);
        Ok(now)
    }

    /// Start a terminal or an agent in `pane` of `space` (or, with no pane, in a new pane split
    /// right of the space's last one), whatever the
    /// pane showed going. An agent gets
    /// `message` as its first message and the call waits up to `AgentTiming::start_wait` for it to be
    /// ready: an error when it gave up, otherwise its state, `starting` or `failed` when the
    /// wait ran out first. A message and an `agent` (by name, else the `agent.default` setting)
    /// are only for an agent.
    pub async fn pane_start(
        &self,
        space: &str,
        pane: Option<String>,
        kind: StartKind,
        options: StartOptions,
    ) -> Result<Started, String> {
        let StartOptions {
            message,
            agent,
            model,
            permissions,
        } = options;
        if kind == StartKind::Terminal && message.is_some() {
            return Err("a first message is only for an agent pane".into());
        }
        if kind == StartKind::Terminal && agent.is_some() {
            return Err("an agent name is only for an agent pane".into());
        }
        if kind == StartKind::Terminal && model.is_some() {
            return Err("a model is only for an agent pane".into());
        }
        if kind == StartKind::Terminal
            && (permissions.permission_mode.is_some() || permissions.sandbox.is_some())
        {
            return Err("a permission mode and a sandbox are only for an agent pane".into());
        }
        // Refused before any pane is made or changed, so a program that is missing leaves the
        // layout as it was.
        let agent = agent.unwrap_or_else(|| self.agent_default());
        if kind == StartKind::Agent {
            self.agent_ready(&agent)?;
            let harness = Harness::parse(&agent).ok_or_else(|| format!("no agent {agent}"))?;
            crate::driver::check_permissions(
                harness,
                permissions.permission_mode.as_deref(),
                permissions.sandbox.as_deref(),
            )?;
        }
        let pane = {
            let spaces = self.spaces.lock().unwrap();
            let held = &spaces.spaces[spaces.index(space)?];
            match pane {
                Some(pane) if space_find(held, &pane).is_none() => {
                    return Err(format!("pane {pane} is not in space {space}"))
                }
                Some(pane) => Ok(pane),
                None => Err(last_pane(&held.root).to_string()),
            }
        };
        let (pane, split) = match pane {
            Ok(pane) => (pane, false),
            Err(last) => (self.pane_split(&last, Some(Dir::Right), None)?.0, true),
        };
        let started = match kind {
            StartKind::Terminal => self
                .pane_set_kind(&pane, NewPaneKind::Terminal, None, None, None)
                .await
                .map(|_| None)
                .map_err(|e| (e, false)),
            StartKind::Agent => self
                .start_agent(space, &pane, &agent, message, model, permissions, None)
                .await
                .map(Some),
        };
        match started {
            Ok(agent) => Ok(Started {
                pane,
                agent: agent.as_ref().map(|(agent, _)| agent.clone()),
                state: agent.map(|(_, state)| state),
            }),
            Err((e, kept)) => {
                // A pane made only for this start goes with it, unless it now holds an agent
                // that failed to come up: that pane stays open showing the error.
                if split && !kept {
                    let _ = self.pane_close(&pane);
                }
                Err(e)
            }
        }
    }

    /// Fork agent `agent`'s conversation through its first `turns` turns into a new agent pane
    /// split right of the agent's own, with the agent's harness, model, permissions and account.
    /// The agent's own conversation goes on untouched. Refused for an agent whose harness cannot
    /// fork, or that has no conversation yet; a pane made for this that fails to start is closed.
    pub async fn pane_fork(&self, agent: &str, turns: usize) -> Result<Started, String> {
        let (space, space_cwd, pane, agent, harness, model, permissions, place) = {
            let spaces = self.spaces.lock().unwrap();
            spaces
                .spaces
                .iter()
                .find_map(|held| {
                    space_panes(held).into_iter().find_map(|(pane, kind)| match kind {
                        PaneKind::Agent {
                            agent: a,
                            harness,
                            model,
                            permissions,
                            place,
                            ..
                        } if a == agent || pane == agent => Some((
                            held.id.clone(),
                            held.cwd.clone(),
                            pane.to_string(),
                            a.clone(),
                            harness.clone(),
                            model.clone(),
                            permissions.as_deref().cloned().unwrap_or_default(),
                            place.as_deref().cloned().unwrap_or_default(),
                        )),
                        _ => None,
                    })
                })
                .ok_or_else(|| format!("no agent pane holds agent {agent}"))?
        };
        if Harness::parse(&harness) == Some(Harness::Pi) {
            return Err("Pi cannot fork a conversation at a turn".into());
        }
        if turns == 0 {
            return Err("a fork holds at least one turn".into());
        }
        let conversation = self
            .agent(&agent)?
            .conversation_id()
            .ok_or_else(|| format!("agent {agent} has no conversation to fork yet"))?;
        // Refused before a pane is made: Claude Code's transcript says at once whether it holds
        // that many turns. Codex says so when its fork starts.
        if Harness::parse(&harness) == Some(Harness::Claude) {
            let home = crate::claude::store(place.home.as_deref().map(std::path::Path::new))
                .or_else(|| self.agent_home("claude").map(std::path::PathBuf::from))
                .ok_or("the agent's configuration directory is not known")?;
            let cwd = place.cwd.clone().unwrap_or(space_cwd);
            let source = conversation.clone();
            tokio::task::spawn_blocking(move || {
                crate::claude::fork_point(&home, std::path::Path::new(&cwd), &source, turns)
            })
            .await
            .map_err(|e| e.to_string())??;
        }
        let (new, _) = self.pane_split(&pane, Some(Dir::Right), None)?;
        let source = ForkSource {
            conversation,
            turns,
            cwd: place.cwd,
            home: place.home,
        };
        match self
            .start_agent(&space, &new, &harness, None, model, permissions, Some(source))
            .await
        {
            Ok((started, state)) => Ok(Started {
                pane: new,
                agent: Some(started),
                state: Some(state),
            }),
            // A fork that did not start is no use as an error pane: the pane goes with it.
            Err((e, _)) => {
                let _ = self.pane_close(&new);
                Err(e)
            }
        }
    }

    /// Run the `[runner]` command `name` of `space` in a new terminal pane split right of the
    /// space's last one, and return that pane. The command is typed into the pane's shell, so
    /// the shell stays open and shows its output after it exits.
    pub async fn run_command(
        &self,
        space: &str,
        name: &str,
    ) -> Result<String, String> {
        let command = self
            .runner_ls(Some(space))?
            .into_iter()
            .find(|r| r.name == name)
            .map(|r| r.command)
            .ok_or_else(|| format!("no runner named {name} in space {space}"))?;
        let started = self
            .pane_start(
                space,
                None,
                StartKind::Terminal,
                StartOptions::default(),
            )
            .await?;
        let term = {
            let spaces = self.spaces.lock().unwrap();
            let held = &spaces.spaces[spaces.index(space)?];
            match space_find(held, &started.pane) {
                Some(PaneKind::Terminal { term, .. }) => term.clone(),
                _ => return Err(format!("pane {} did not start a terminal", started.pane)),
            }
        };
        self.term(&term)
            .ok_or_else(|| format!("no terminal {term}"))?
            .write(format!("{command}\n").into_bytes())
            .map_err(|e| e.to_string())?;
        tracing::info!(%space, pane = %started.pane, %name, %command, "runner started");
        Ok(started.pane)
    }

    /// Run `command` in a new terminal pane split right of `pane`, in its space's directory, and
    /// return that pane. Typed into the pane's shell like a runner, so the shell stays open and
    /// shows the output; a pane that cannot start a terminal is closed again.
    pub async fn pane_run(
        &self,
        pane: &str,
        command: &str,
        site: Option<PaneSite>,
    ) -> Result<String, String> {
        if command.trim().is_empty() {
            return Err("there is no command to run".into());
        }
        let apart = matches!(site, Some(PaneSite::Popped | PaneSite::Backgrounded));
        let new = self
            .pane_split(pane, (!apart).then_some(Dir::Right), site)?
            .0;
        let term = match self
            .pane_set_kind(&new, NewPaneKind::Terminal, None, None, None)
            .await
        {
            Ok(PaneKind::Terminal { term, .. }) => term,
            Ok(_) => {
                let _ = self.pane_close(&new);
                return Err(format!("pane {new} did not start a terminal"));
            }
            Err(e) => {
                let _ = self.pane_close(&new);
                return Err(e);
            }
        };
        let typed = match self.term(&term) {
            Some(t) => t
                .write(format!("{command}\n").into_bytes())
                .map_err(|e| e.to_string()),
            None => Err(format!("no terminal {term}")),
        };
        if let Err(e) = typed {
            let _ = self.pane_close(&new);
            return Err(e);
        }
        tracing::info!(%pane, new = %new, %command, "pane run");
        Ok(new)
    }

    /// Start the agent of a saved agent pane under its saved name, loading its saved conversation,
    /// in a space whose directory is `space_cwd` and whose layout is `root`.
    fn start_saved_agent(
        &self,
        space_cwd: &str,
        root: &SpaceInfo,
        saved: AgentPane,
    ) -> Result<(), String> {
        // A conversation switched over from a terminal runs where Claude Code ran it.
        let place = match space_find(root, &saved.pane) {
            Some(PaneKind::Agent {
                place: Some(place), ..
            }) => (**place).clone(),
            _ => Default::default(),
        };
        let wheel_proto::AgentPlace { cwd, home } = place;
        let permissions = match space_find(root, &saved.pane) {
            Some(PaneKind::Agent { permissions, .. }) => {
                permissions.as_deref().cloned().unwrap_or_default()
            }
            _ => AgentPermissions::default(),
        };
        // A hand-edited layout can name a mode the agent does not have; say so here rather than
        // let the agent fail at spawn.
        let harness =
            Harness::parse(&saved.harness).ok_or_else(|| format!("no agent {}", saved.harness))?;
        crate::driver::check_permissions(
            harness,
            permissions.permission_mode.as_deref(),
            permissions.sandbox.as_deref(),
        )?;
        self.spawn_agent(
            &saved.agent,
            (&saved.pane, &saved.harness),
            (cwd.as_deref().unwrap_or(space_cwd), space_cwd),
            AgentStart {
                conversation: saved.conversation,
                fork: None,
                home,
                context: saved.context,
                model: saved.model,
                permissions,
            },
        )
    }

    /// Start the agent panes a restore kept as saved because their agent could not start, which
    /// is a program not yet found. Called once the login shell's PATH arrives late.
    pub(crate) fn start_unstarted_agents(&self) {
        let kept = std::mem::take(&mut *self.kept_agents.lock().unwrap());
        let waiting: Vec<(String, String, SpaceInfo, AgentPane)> = {
            let spaces = self.spaces.lock().unwrap();
            spaces
                .spaces
                .iter()
                .flat_map(|space| {
                    agent_panes(space)
                        .into_iter()
                        .filter(|a| kept.contains(&a.agent))
                        .map(|a| (space.id.clone(), space.cwd.clone(), space.clone(), a))
                })
                .collect()
        };
        for (space, cwd, root, saved) in waiting {
            let (pane, agent) = (saved.pane.clone(), saved.agent.clone());
            match self.start_saved_agent(&cwd, &root, saved) {
                Ok(()) => {
                    tracing::info!(%space, %pane, %agent, "agent started now that its program is found")
                }
                Err(e) => {
                    tracing::warn!(%space, %pane, %agent, error = %e, "agent still cannot start")
                }
            }
        }
    }

    /// Make `pane` an agent pane, send its agent `message` if there is one, and wait for it to
    /// be ready. Returns the agent's name and state; an error carries whether the pane is now
    /// an agent pane, which a caller that made the pane for this start keeps.
    async fn start_agent(
        &self,
        space: &str,
        pane: &str,
        harness: &str,
        message: Option<String>,
        model: Option<String>,
        permissions: AgentPermissions,
        fork: Option<ForkSource>,
    ) -> Result<(String, AgentState), (String, bool)> {
        // A second start on a pane whose first has not heard back from its agent would `install`
        // over that agent and end it, so it is refused; the claim is held through the wait for
        // the agent to be ready, because the second start of a double click lands in that wait.
        let _starting = StartClaim::take(&self.agent_starts, pane).map_err(|e| (e, false))?;
        let space_cwd = {
            let spaces = self.spaces.lock().unwrap();
            spaces.spaces[spaces.holding(pane).map_err(|e| (e, false))?]
                .cwd
                .clone()
        };
        // A fork runs where the conversation it copies was kept, with the account that kept it.
        let cwd = fork
            .as_ref()
            .and_then(|f| f.cwd.clone())
            .unwrap_or_else(|| space_cwd.clone());
        let name = self.new_id('w');
        let home = match &fork {
            Some(fork) => fork.home.clone(),
            None => self
                .shell_agent_home(harness, &cwd)
                .await
                .map_err(|e| (e, false))?,
        };
        self.spawn_agent(
            &name,
            (pane, harness),
            (&cwd, &space_cwd),
            AgentStart {
                fork: fork.as_ref().map(|f| (f.conversation.clone(), f.turns)),
                home: home.clone(),
                model: model.clone(),
                permissions: permissions.clone(),
                ..Default::default()
            },
        )
        .map_err(|e| (e, false))?;
        let new = PaneKind::Agent {
            agent: name.clone(),
            harness: harness.to_string(),
            conversation: None,
            state: None,
            model,
            context: None,
            // The pane keeps the account it started in, so a restart finds its conversation there.
            place: wheel_proto::AgentPlace::boxed(fork.and_then(|f| f.cwd), home),
            permissions: AgentPermissions::boxed(permissions.permission_mode, permissions.sandbox),
            extra: Extra::default(),
        };
        // A pane that closed while the agent spawned ends the agent it was given.
        self.install(pane, new, None).map_err(|e| (e, false))?;
        let live = self.agent(&name).map_err(|e| (e, true))?;
        if let Some(message) = message {
            live.send(message).map_err(|e| (e, true))?;
        }
        tracing::info!(%space, %pane, %name, "agent started");
        let start_wait = self.agent_timing.lock().unwrap().start_wait;
        match tokio::time::timeout(start_wait, live.started()).await {
            Ok(Ok(())) => tracing::info!(%name, "agent ready"),
            Ok(Err(e)) => {
                tracing::warn!(%space, %pane, %name, error = %e, "agent could not start");
                return Err((
                    format!(
                        "agent {name} in pane {pane} could not start: {e}. Its pane stays open showing the error; wheel pane close removes it"
                    ),
                    true,
                ));
            }
            Err(_) => {
                tracing::info!(%name, wait_ms = start_wait.as_millis() as u64, "agent still starting")
            }
        }
        Ok((name, live.state()))
    }

    /// A browser pane is showing `url` now: store it, so the saved layout follows where the person
    /// went and a restored pane comes back there. The client sends this as it navigates; anything
    /// else sending it moves that pane, since every client draws what the layout says.
    ///
    /// Refused for a pane that is not a browser pane, and for a URL no pane may point at — the
    /// same rule the pane's kind went through, so navigation cannot reach a page opening one could
    /// not.
    pub fn pane_browser_url(&self, pane: &str, url: &str) -> Result<PaneKind, String> {
        let url = page(url)?;
        let mut spaces = self.spaces.lock().unwrap();
        let slot = spaces
            .spaces
            .iter_mut()
            .find_map(|s| space_find_mut(s, pane))
            .ok_or_else(|| format!("no pane {pane}"))?;
        let PaneKind::Browser { url: at, .. } = slot else {
            return Err(format!("{pane} is not a browser pane"));
        };
        if *at == url {
            // Nothing changed, so nothing is saved and nothing is broadcast: a client reporting
            // the page it was already on must not make every other client redraw.
            return Ok(slot.clone());
        }
        tracing::info!(%pane, from = %at, to = %url, "browser pane navigated");
        *at = url.clone();
        let now = slot.clone();
        // Not `changed`: this one field changes as often as a person loads a page, so clients are
        // told the field rather than the whole layout, and the file is written once per burst.
        let _ = self.global.send(ServerMsg::PaneUrl {
            pane: pane.to_string(),
            url,
        });
        self.save_soon();
        Ok(now)
    }

    /// The page in browser pane `pane` wrote `entries` to its console. `reset` means the page
    /// just loaded, so what was kept for the pane goes first. Every message is logged.
    pub fn pane_console(
        &self,
        pane: &str,
        reset: bool,
        mut entries: Vec<wheel_proto::ConsoleEntry>,
    ) -> Result<(), String> {
        self.browser_pane(pane)?;
        if reset {
            self.console.lock().unwrap().clear(pane);
            tracing::info!(%pane, "browser pane console cleared by a page load");
        }
        if entries.is_empty() {
            return Ok(());
        }
        for e in &mut entries {
            // A page can write any amount; the buffer, the log and every client get a bounded line.
            if let Some((at, _)) = e.text.char_indices().nth(crate::console::MAX_TEXT) {
                e.text.truncate(at);
                e.text.push('…');
            }
        }
        for e in &entries {
            tracing::info!(%pane, time = e.time, level = %e.level, text = %e.text, url = ?e.url, line = ?e.line, "browser pane console");
        }
        self.console.lock().unwrap().append(pane, entries.clone());
        let _ = self.global.send(ServerMsg::PaneConsoleAppended {
            pane: pane.to_string(),
            entries,
        });
        Ok(())
    }

    /// The console messages kept for browser pane `pane` since its last load.
    pub fn pane_console_read(
        &self,
        pane: &str,
        since: Option<u64>,
    ) -> Result<Vec<wheel_proto::ConsoleEntry>, String> {
        self.browser_pane(pane)?;
        Ok(self.console.lock().unwrap().read(pane, since))
    }

    pub(crate) fn browser_pane(&self, pane: &str) -> Result<(), String> {
        let spaces = self.spaces.lock().unwrap();
        let kind = spaces
            .spaces
            .iter()
            .find_map(|s| space_find(s, pane))
            .ok_or_else(|| format!("no such pane {pane}"))?;
        match kind {
            PaneKind::Browser { .. } => Ok(()),
            _ => Err(format!("{pane} is not a browser pane")),
        }
    }

    /// Record what the agent in terminal pane `pane` says it is doing, and tell clients if the
    /// pane's status changed.
    pub fn pane_report(
        &self,
        pane: &str,
        status: wheel_proto::ReportedStatus,
        reason: Option<String>,
    ) -> Result<(), String> {
        // Only a waiting agent has a reason, and one line of it is what any client shows.
        let reason = reason
            .filter(|_| status == wheel_proto::ReportedStatus::NeedsYou)
            .map(|r| crate::submit::one_line(r.trim(), REASON_MAX))
            .filter(|r| !r.is_empty());
        let term = {
            let spaces = self.spaces.lock().unwrap();
            let kind = spaces
                .spaces
                .iter()
                .flat_map(|s| space_panes(s))
                .find(|(id, _)| *id == pane)
                .map(|(_, kind)| kind)
                .ok_or_else(|| format!("no pane {pane}"))?;
            let PaneKind::Terminal { term, agent, .. } = kind else {
                return Err(format!("{pane} is not a terminal pane"));
            };
            // A report from an agent the sampler has not seen, or has seen quit, would outlive
            // it and show on whatever runs here next.
            if agent.is_none() {
                tracing::debug!(%pane, ?status, "terminal agent report ignored; no agent runs here");
                return Ok(());
            }
            term.clone()
        };
        let new = crate::server::Report { status, reason };
        let old = self.reports.lock().unwrap().insert(term.clone(), new.clone());
        if old.as_ref() != Some(&new) {
            // The reason can quote a command, token and all, so only its length goes in the info line.
            let reason_chars = new.reason.as_ref().map(|r| r.chars().count());
            tracing::info!(%pane, %term, old = ?old.as_ref().map(|r| r.status), new = ?status, ?reason_chars, "terminal agent reported");
            tracing::debug!(%pane, reason = ?new.reason, "terminal agent's reason");
            self.refresh_statuses();
        }
        Ok(())
    }

    /// Input typed into terminal `term` can end what its agent reported, which no hook tells.
    /// Enter while the agent waits on the person submits the prompt's choice, so the agent is
    /// working again: Claude Code has no hook for a granted permission, and the next one,
    /// PostToolUse, fires only when the tool call ends. A denial is followed by Stop, which
    /// reports idle. Esc or Ctrl-C alone while the agent works, or while it waits on the person
    /// (refusing the prompt), interrupts its turn, and Claude Code runs no Stop hook for an
    /// interrupt, so the agent is idle; if the key did not interrupt, the next PostToolUse reports
    /// working again. Arrows, digits and the terminal's own replies
    /// (focus, mouse, cursor position) contain neither, so they change nothing.
    pub fn term_input_answers_prompt(&self, term: &str, bytes: &[u8]) {
        use wheel_proto::ReportedStatus::{Idle, NeedsYou, Working};
        let mut reports = self.reports.lock().unwrap();
        let next = match (reports.get(term).map(|r| r.status), bytes) {
            (Some(NeedsYou), b) if b.iter().any(|b| matches!(b, b'\r' | b'\n')) => Working,
            (Some(Working | NeedsYou), [0x1b | 0x03]) => Idle,
            _ => return,
        };
        reports.insert(
            term.to_string(),
            crate::server::Report {
                status: next,
                reason: None,
            },
        );
        drop(reports);
        tracing::info!(%term, status = ?next, "terminal input changed what the agent reported");
        self.refresh_statuses();
    }

    /// Keep what the agent in `pane` says it is working on. No items, stage or note clears it.
    pub fn work_set(
        &self,
        pane: &str,
        items: Vec<wheel_proto::WorkItem>,
        stage: Option<wheel_proto::WorkStage>,
        note: Option<String>,
    ) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let Some(Node::Pane { work, .. }) = spaces
            .spaces
            .iter_mut()
            .find_map(|s| space_leaf_mut(s, pane))
        else {
            return Err(format!("no pane {pane}"));
        };
        let note = note.filter(|n| !n.trim().is_empty());
        let new = (!items.is_empty() || stage.is_some() || note.is_some())
            .then(|| Box::new(wheel_proto::PaneWork { items, stage, note }));
        if *work == new {
            return Ok(());
        }
        tracing::info!(%pane, old = ?work, new = ?new, "pane work declared");
        *work = new;
        self.changed(&spaces);
        Ok(())
    }

    /// Record how full the context window of the agent in terminal pane `pane` is, as the agent
    /// says; the terminal sampler puts it on the pane at its next tick. Kept before the sampler
    /// has seen the agent too: an agent's first report can beat it.
    pub fn pane_report_context(&self, pane: &str, context: ContextUse) -> Result<(), String> {
        let term = {
            let spaces = self.spaces.lock().unwrap();
            let kind = spaces
                .spaces
                .iter()
                .flat_map(|s| space_panes(s))
                .find(|(id, _)| *id == pane)
                .map(|(_, kind)| kind)
                .ok_or_else(|| format!("no pane {pane}"))?;
            let PaneKind::Terminal { term, .. } = kind else {
                return Err(format!("{pane} is not a terminal pane"));
            };
            term.clone()
        };
        let old = self
            .context_reports
            .lock()
            .unwrap()
            .insert(term.clone(), context);
        if old != Some(context) {
            tracing::debug!(%pane, %term, old = ?old, new = ?context, "terminal agent reported its context");
        }
        Ok(())
    }

    /// Claude Code in a terminal pane says it is in conversation `agent` now. The next sample
    /// saves it as the pane's agent, so a view switch resumes this conversation.
    pub fn pane_report_conversation(&self, pane: &str, conversation: &str, pid: u32) -> Result<(), String> {
        let term = {
            let spaces = self.spaces.lock().unwrap();
            let kind = spaces
                .spaces
                .iter()
                .flat_map(|s| space_panes(s))
                .find(|(id, _)| *id == pane)
                .map(|(_, kind)| kind)
                .ok_or_else(|| format!("no pane {pane}"))?;
            let PaneKind::Terminal { term, .. } = kind else {
                return Err(format!("{pane} is not a terminal pane"));
            };
            term.clone()
        };
        // Only the Claude Code the terminal runs may say which conversation it is in: a hook of a
        // Claude Code that a tool in the pane started (`claude -p`) is not it.
        let foreground = self.term(&term).and_then(|t| t.foreground());
        let reporter = crate::resume::claude_ancestor(pid);
        let is_foreground = |claude: u32| {
            // SAFETY: getpgid reads the kernel's process table and touches no memory of ours.
            let group = unsafe { libc::getpgid(claude as libc::pid_t) };
            Some(claude) == foreground || (group > 0 && Some(group as u32) == foreground)
        };
        if !reporter.is_some_and(is_foreground) {
            tracing::warn!(%pane, %term, %pid, ?reporter, ?foreground, "agent report ignored; it is not from the terminal's Claude Code");
            return Err(format!(
                "{pane}'s foreground program did not send this report"
            ));
        }
        let old = self
            .conversation_reports
            .lock()
            .unwrap()
            .insert(term.clone(), conversation.to_string());
        if old.as_deref() != Some(conversation) {
            tracing::info!(%pane, %term, old = ?old, new = %conversation, "terminal agent reported its conversation");
        }
        Ok(())
    }

    /// Send a browser pane to the page a person means by `text`, typed into its address field:
    /// read by [`wheel_proto::page_typed`] against this daemon's home, then stored as any page is.
    /// Gives back the page.
    pub fn pane_browser_open(&self, pane: &str, text: &str) -> Result<String, String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let url = wheel_proto::page_typed(text, &home)?;
        tracing::info!(%pane, typed = %text, %url, "browser pane opening typed text");
        self.pane_browser_url(pane, &url).map(|_| url)
    }

    pub fn split_ratio(&self, split: &str, ratio: f64) -> Result<(), String> {
        if !(0.1..=0.9).contains(&ratio) {
            return Err(format!("ratio {ratio} is outside 0.1 to 0.9"));
        }
        let mut spaces = self.spaces.lock().unwrap();
        if !spaces
            .spaces
            .iter_mut()
            .any(|s| s.trees_mut().any(|t| set_ratio(t, split, ratio)))
        {
            return Err(format!("no split {split}"));
        }
        tracing::info!(%split, ratio, "split ratio");
        self.changed(&spaces);
        Ok(())
    }

    /// Load the layout saved at `path` (none yet is an empty layout) and save every change there
    /// from now on. Shells do not outlive the daemon, so each terminal pane gets a new one in its
    /// space's directory.
    pub async fn restore_spaces(&self, path: PathBuf) -> anyhow::Result<()> {
        self.sweep_removed_pane_dirs();
        let Saved {
            layout: mut loaded,
            next_id,
            recent,
            mut extra,
        } = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("{} is not a saved layout: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved {
                layout: Spaces::default(),
                next_id: 1,
                recent: Vec::new(),
                extra: Default::default(),
            },
            Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
        };
        self.next_id.fetch_max(next_id, Ordering::SeqCst);
        // The global active space this file once held is gone (ADR-0022).
        extra.remove("active");
        for key in extra.keys() {
            tracing::warn!(%key, "unknown saved key; kept as is");
        }
        *self.saved_extra.lock().unwrap() = extra;
        tracing::info!(rows = recent.len(), "space recent restored");
        *self.recent.lock().unwrap() = recent;
        let (mut respawned, mut agents) = (0, 0);
        let mut resumed_conversations = std::collections::HashSet::new();
        let named_web: Vec<String> = loaded
            .spaces
            .iter()
            .flat_map(|space| {
                // A pane this build cannot read is kept whole, so its data directory stays too.
                let web = crate::web::web_panes(space)
                    .into_iter()
                    .map(|(p, _)| p);
                web.chain(unknown_panes(space).into_iter().map(|(p, _)| p))
            })
            .map(str::to_string)
            .collect();
        let named_all: Vec<String> = loaded
            .spaces
            .iter()
            .flat_map(|space| space_panes(space).into_iter().map(|(p, _)| p.to_string()))
            .collect();
        self.sweep_uploads(&named_all.iter().map(String::as_str).collect());
        for space in &mut loaded.spaces {
            let mut outcomes: HashMap<String, String> = HashMap::new();
            for (pane, kind) in unknown_panes(space) {
                tracing::warn!(space = %space.id, %pane, %kind, "unknown pane kind; kept as is");
            }
            // A restored web pane keeps its artifact whether or not the folder is there: the watch
            // waits for a missing one to appear, as it does for a folder removed while watched.
            for (pane, artifact) in crate::web::web_panes(space) {
                self.show_artifact(pane, artifact, &space.cwd);
            }
            let mut ids = Vec::new();
            collect_terminal_panes(space, &mut ids);
            for pane in ids {
                let (saved_cwd, saved_agent) = match space_find(space, &pane) {
                    Some(PaneKind::Terminal { cwd, agent, .. }) => (cwd.clone(), agent.clone()),
                    _ => (None, None),
                };
                let mut outcome = "shell in the space's directory".to_string();
                let dir = match saved_cwd {
                    Some(dir) if Path::new(&dir).is_dir() => {
                        outcome = format!("shell in {dir}");
                        dir
                    }
                    Some(dir) => {
                        tracing::warn!(space = %space.id, %pane, %dir, "the pane's last directory is gone; its shell starts in the space's directory");
                        space.cwd.clone()
                    }
                    None => space.cwd.clone(),
                };
                let opened = self
                    .open_term(
                        dir.clone(),
                        self.shell_in(&space.cwd),
                        PANE_COLS,
                        PANE_ROWS,
                        Owner::Pane,
                        Some((&pane, &space.cwd)),
                    )
                    .await;
                let space_id = space.id.clone();
                let Some(slot) = space_find_mut(space, &pane) else {
                    continue;
                };
                match opened {
                    // Only the shell is new; the pane keeps the rest of what was saved.
                    Ok(term) => {
                        let mut resumed = None;
                        if let Some(agent) = saved_agent {
                            let line = crate::resume::resume_line(&agent);
                            let key = (agent.name.clone(), agent.conversation.clone());
                            if resumed_conversations.contains(&key) {
                                outcome = format!("{outcome}; {} conversation {} already resumed in another pane, so a plain shell", agent.name, agent.conversation);
                            } else if !crate::resume::conversation_exists(&agent, &dir) {
                                tracing::warn!(space = %space_id, %pane, agent = %agent.name, conversation = %agent.conversation, %dir, "the agent's conversation is gone; the pane starts a plain shell");
                                outcome = format!(
                                    "{outcome}; {} conversation {} is gone, so a plain shell",
                                    agent.name, agent.conversation
                                );
                            } else if let Some(line) = line {
                                match term.write(format!("{line}\n").into_bytes()) {
                                    Ok(()) => {
                                        outcome = format!(
                                            "{outcome}; resumed {} conversation {}",
                                            agent.name, agent.conversation
                                        );
                                        resumed_conversations.insert(key);
                                        resumed = Some(agent);
                                    }
                                    Err(e) => {
                                        tracing::warn!(space = %space_id, %pane, error = %e, "could not type the agent's resume command");
                                        outcome =
                                            format!("{outcome}; could not resume {}", agent.name);
                                    }
                                }
                            }
                        }
                        if let PaneKind::Terminal {
                            term: id,
                            cwd,
                            agent,
                            ..
                        } = slot
                        {
                            respawned += 1;
                            *id = term.id.clone();
                            *cwd = Some(dir);
                            *agent = resumed;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(space = %space_id, %pane, error = %e, "could not restart the pane's shell; it is empty now");
                        outcome = format!("empty: {e}");
                        *slot = PaneKind::empty();
                    }
                }
                outcomes.insert(pane, outcome);
            }
            // Agents do not outlive the daemon: each agent pane's agent starts again under
            // its saved name, and loads its saved conversation where the agent can.
            for saved in agent_panes(space) {
                let (pane, agent) = (saved.pane.clone(), saved.agent.clone());
                match self.start_saved_agent(&space.cwd, space, saved) {
                    Ok(()) => {
                        agents += 1;
                        outcomes.insert(
                            pane,
                            format!("agent {agent} started, loading its saved conversation"),
                        );
                    }
                    // Kept as saved, so a later start with an agent command runs it again.
                    Err(e) => {
                        tracing::warn!(space = %space.id, %pane, %agent, error = %e, "could not restart the pane's agent");
                        self.kept_agents.lock().unwrap().insert(agent.clone());
                        outcomes.insert(
                            pane,
                            format!("agent {agent} kept as saved, not started: {e}"),
                        );
                    }
                }
            }
            // One line per pane, so a log read afterwards says what each came back as and why.
            for (pane, kind) in space_panes(space) {
                let outcome = outcomes.remove(pane).unwrap_or_else(|| match kind {
                    PaneKind::Web { artifact, .. } => format!(
                        "showing artifact {} again, watching its folder",
                        artifact.name()
                    ),
                    PaneKind::Browser { url, .. } => format!("back on {url}"),
                    PaneKind::Empty { .. } => "empty".into(),
                    PaneKind::Unknown(_) => "kept as saved, which this build cannot draw".into(),
                    _ => "restored".into(),
                });
                tracing::info!(space = %space.id, %pane, kind = pane_kind_name(kind), %outcome, "pane restored");
            }
        }
        self.sweep_pane_data(&named_web.iter().map(String::as_str).collect());
        tracing::info!(path = %path.display(), spaces = loaded.spaces.len(), respawned, agents, "spaces restored");
        *self.state_file.lock().unwrap() = Some(path);
        let mut spaces = self.spaces.lock().unwrap();
        *spaces = loaded;
        // A layout saved before sessions, or one a crash left disagreeing, is grouped here.
        if spaces.heal(|| self.new_id('n')) {
            tracing::info!(sessions = spaces.sessions.len(), "sessions grouped on restore");
        }
        self.sync_conversations(&mut spaces);
        self.changed(&spaces);
        drop(spaces);
        self.watch_terminals();
        Ok(())
    }

    /// Sample every terminal pane's directory and agent every few seconds, so the saved layout
    /// is never far behind what the panes are doing. Stops when the daemon is dropped.
    fn watch_terminals(&self) {
        let me = self.me.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(TERMINAL_SAMPLE).await;
                let Some(state) = me.upgrade() else { return };
                let _ = tokio::task::spawn_blocking(move || state.sample_terminals()).await;
            }
        });
    }

    /// Record, in each terminal pane, the directory its shell is in and the coding agent running
    /// in it, and save the layout when either changed. A pane whose agent has ended forgets it,
    /// except in a terminal's first seconds before any program has run there, when a resumed
    /// agent has not started yet.
    pub(crate) fn sample_terminals(&self) -> bool {
        let (terms, saved): (Vec<Arc<Term>>, HashMap<String, wheel_proto::TermAgent>) = {
            let spaces = self.spaces.lock().unwrap();
            let held = self.terms.lock().unwrap();
            let mut terms = Vec::new();
            let mut saved = HashMap::new();
            for (_, kind) in spaces.spaces.iter().flat_map(|space| space_panes(space)) {
                if let PaneKind::Terminal { term, agent, .. } = kind {
                    if let Some(h) = held.get(term) {
                        terms.push(h.term.clone());
                    }
                    if let Some(agent) = agent {
                        saved.insert(term.clone(), (**agent).clone());
                    }
                }
            }
            (terms, saved)
        };
        let samples: HashMap<String, (crate::resume::Sample, bool)> = terms
            .iter()
            .filter(|t| t.info().alive)
            .map(|t| {
                let claimed = self.claimed_agents(&t.id);
                let reported = self.context_reports.lock().unwrap().get(&t.id).copied();
                let reported_conversation = self.conversation_reports.lock().unwrap().get(&t.id).cloned();
                let sample = crate::resume::sample(
                    t.pid,
                    t.foreground(),
                    saved.get(&t.id),
                    &claimed,
                    reported,
                    reported_conversation.as_deref(),
                );
                if sample.agent.is_some() {
                    t.saw_job();
                }
                // Once a program has run here, an agent that is not running has quit; before that
                // it may still be starting, from a resume command typed into a new shell.
                let settled = t.job_seen() || t.age() > RESUME_GRACE;
                (t.id.clone(), (sample, settled))
            })
            .collect();
        let mut changed = false;
        let mut context_changed = false;
        let mut spaces = self.spaces.lock().unwrap();
        for space in &mut spaces.spaces {
            each_space_kind_mut(space, &mut |kind| {
                let PaneKind::Terminal {
                    term, cwd, agent, ..
                } = kind
                else {
                    return;
                };
                let Some((sample, settled)) = samples.get(term.as_str()) else {
                    return;
                };
                let new_cwd = sample.cwd.clone().or_else(|| cwd.clone());
                let new_agent = match &sample.agent {
                    Some(found) => Some(Box::new(found.clone())),
                    None if *settled => None,
                    None => agent.clone(),
                };
                // An agent's first report can beat the sampler to it, so a report waits through
                // the terminal's first seconds; once settled with no agent, it is stale.
                if *settled
                    && sample.agent.is_none()
                    && self.context_reports.lock().unwrap().remove(term).is_some()
                {
                    tracing::info!(%term, "terminal agent context report dropped; no agent runs here");
                }
                if *settled
                    && sample.agent.is_none()
                    && self.conversation_reports.lock().unwrap().remove(term).is_some()
                {
                    tracing::info!(%term, "terminal agent report dropped; no agent runs here");
                }
                // The context figure moves with every turn; the save cares about where the pane
                // is and which conversation it is in.
                let without_context = |agent: &Option<Box<wheel_proto::TermAgent>>| {
                    agent.as_ref().map(|agent| wheel_proto::TermAgent {
                        context: None,
                        ..(**agent).clone()
                    })
                };
                if new_cwd == *cwd
                    && new_agent != *agent
                    && without_context(&new_agent) == without_context(agent)
                {
                    *agent = new_agent;
                    context_changed = true;
                } else if new_cwd != *cwd || new_agent != *agent {
                    tracing::info!(%term, cwd = ?new_cwd, agent = ?new_agent, "terminal pane saves its directory and agent");
                    // The agent quit: what it last reported goes with it.
                    if new_agent.is_none() && self.reports.lock().unwrap().remove(term).is_some() {
                        tracing::info!(%term, "terminal agent report dropped; the agent is gone");
                    }
                    *cwd = new_cwd;
                    *agent = new_agent;
                    changed = true;
                }
            });
        }
        // A closed terminal's report goes with it.
        let live: std::collections::HashSet<&str> = spaces
            .spaces
            .iter()
            .flat_map(|space| space_panes(space))
            .filter_map(|(_, kind)| match kind {
                PaneKind::Terminal { term, .. } => Some(term.as_str()),
                _ => None,
            })
            .collect();
        self.reports
            .lock()
            .unwrap()
            .retain(|term, _| live.contains(term.as_str()));
        self.context_reports
            .lock()
            .unwrap()
            .retain(|term, _| live.contains(term.as_str()));
        self.conversation_reports
            .lock()
            .unwrap()
            .retain(|term, _| live.contains(term.as_str()));
        if changed {
            self.changed(&spaces);
        } else if context_changed {
            self.broadcast_layout(&spaces);
        }
        changed
    }

    /// An agent pane's agent gave its agent a new conversation id: save it in the pane.
    pub(crate) fn conversations_changed(&self) {
        let mut spaces = self.spaces.lock().unwrap();
        if self.sync_conversations(&mut spaces) {
            self.changed(&spaces);
        }
    }

    /// Copy each running agent's conversation id into its agent pane; true when one changed.
    /// Agents run while the spaces lock is free, so a pane takes its agent's id here, under the
    /// lock, whichever of the two comes first.
    fn sync_conversations(&self, spaces: &mut Spaces) -> bool {
        let mut changed = false;
        for space in &mut spaces.spaces {
            each_space_kind_mut(space, &mut |kind| {
                if let PaneKind::Agent {
                    agent, context, ..
                } = kind
                {
                    if let Ok(running) = self.agent(agent).map(|s| s.context()) {
                        if *context != running {
                            *context = running;
                            changed = true;
                        }
                    }
                }
                let PaneKind::Agent {
                    agent,
                    conversation,
                    ..
                } = kind
                else {
                    return;
                };
                let Ok(running) = self.agent(agent).map(|s| s.saved_conversation()) else {
                    return;
                };
                if *conversation != running {
                    tracing::info!(%agent, conversation = ?running, "agent pane saves its conversation");
                    *conversation = running;
                    changed = true;
                }
            });
        }
        changed
    }
}

fn pane_kind_name(kind: &PaneKind) -> &str {
    match kind {
        PaneKind::Empty { .. } => "empty",
        PaneKind::Terminal { .. } => "terminal",
        PaneKind::Web { .. } => "web",
        PaneKind::Browser { .. } => "browser",
        PaneKind::Agent { .. } => "agent",
        PaneKind::Unknown(unknown) => unknown.kind(),
    }
}

/// Each pane of a kind this build does not know, with that kind's name.
fn unknown_panes(space: &SpaceInfo) -> Vec<(&str, &str)> {
    space.trees().flat_map(unknown_in).collect()
}

fn unknown_in(node: &Node) -> Vec<(&str, &str)> {
    match node {
        Node::Pane {
            id,
            kind: PaneKind::Unknown(unknown),
            ..
        } => vec![(id, unknown.kind())],
        Node::Pane { .. } => vec![],
        Node::Split { a, b, .. } => {
            let mut all = unknown_in(a);
            all.extend(unknown_in(b));
            all
        }
    }
}

/// An agent pane as saved: its agent id, the program it runs and the conversation it keeps.
pub(crate) struct AgentPane {
    pub pane: String,
    pub agent: String,
    pub harness: String,
    pub conversation: Option<String>,
    pub context: Option<ContextUse>,
    pub model: Option<String>,
}

/// Each agent pane in `node`.
pub(crate) fn agent_panes(space: &SpaceInfo) -> Vec<AgentPane> {
    space_panes(space)
        .into_iter()
        .filter_map(|(id, kind)| match kind {
            PaneKind::Agent {
                agent,
                harness,
                conversation,
                context,
                model,
                ..
            } => Some(AgentPane {
                pane: id.to_string(),
                agent: agent.clone(),
                harness: harness.clone(),
                conversation: conversation.clone(),
                context: *context,
                model: model.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Every pane of a space, wherever it is: the grid, a popped tree or the tray.
pub fn space_panes(space: &SpaceInfo) -> Vec<(&str, &PaneKind)> {
    space.trees().flat_map(panes).collect()
}

pub fn space_find<'a>(space: &'a SpaceInfo, pane: &str) -> Option<&'a PaneKind> {
    space.trees().find_map(|tree| find_pane(tree, pane))
}

fn space_find_mut<'a>(space: &'a mut SpaceInfo, pane: &str) -> Option<&'a mut PaneKind> {
    space.trees_mut().find_map(|tree| find_pane_mut(tree, pane))
}

fn space_leaf_mut<'a>(space: &'a mut SpaceInfo, pane: &str) -> Option<&'a mut Node> {
    space.trees_mut().find_map(|tree| find_leaf_mut(tree, pane))
}

fn each_space_kind_mut(space: &mut SpaceInfo, f: &mut impl FnMut(&mut PaneKind)) {
    for tree in space.trees_mut() {
        each_kind_mut(tree, f);
    }
}

/// Call `f` on every pane's kind.
fn each_kind_mut(node: &mut Node, f: &mut impl FnMut(&mut PaneKind)) {
    match node {
        Node::Pane { kind, .. } => f(kind),
        Node::Split { a, b, .. } => {
            each_kind_mut(a, f);
            each_kind_mut(b, f);
        }
    }
}

fn collect_terminal_panes(space: &SpaceInfo, out: &mut Vec<String>) {
    for tree in space.trees() {
        collect_terminals_in(tree, out);
    }
}

fn collect_terminals_in(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Pane {
            id,
            kind: PaneKind::Terminal { .. },
            ..
        } => out.push(id.clone()),
        Node::Pane { .. } => {}
        Node::Split { a, b, .. } => {
            collect_terminals_in(a, out);
            collect_terminals_in(b, out);
        }
    }
}

/// Write the layout through a temporary file, so a crash mid-write leaves the old one.
fn save(
    path: &Path,
    layout: &Spaces,
    next_id: u64,
    recent: Vec<wheel_proto::RecentDir>,
    extra: serde_json::Map<String, serde_json::Value>,
) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let saved = Saved {
        layout: layout.clone(),
        next_id,
        recent,
        extra,
    };
    std::fs::write(&tmp, serde_json::to_vec_pretty(&saved)?)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: &str) -> Node {
        Node::Pane {
            id: id.into(),
            status: None,
            work: None,
            kind: PaneKind::empty(),
        }
    }

    /// `H` or `V`, the ratio, and the two halves; a pane is its id.
    fn shape(node: &Node) -> String {
        match node {
            Node::Pane { id, .. } => id.clone(),
            Node::Split {
                axis, ratio, a, b, ..
            } => {
                let axis = if *axis == Axis::Horizontal { 'H' } else { 'V' };
                format!("{axis}{ratio:.2}({},{})", shape(a), shape(b))
            }
        }
    }

    fn split_ids(node: &Node, out: &mut Vec<String>) {
        if let Node::Split { id, a, b, .. } = node {
            out.push(id.clone());
            split_ids(a, out);
            split_ids(b, out);
        }
    }

    /// The tree after each of panes 2 to 5 is placed by `strategy`, splitting the newest pane.
    fn placed_sequence(strategy: Placement) -> Vec<String> {
        let mut root = leaf("p1");
        let mut shapes = vec![shape(&root)];
        for n in 2..=5 {
            let target = format!("p{}", n - 1);
            let dir = place(
                &mut root,
                &target,
                strategy,
                None,
                &format!("x{n}"),
                &format!("p{n}"),
            );
            assert!(dir.is_some());
            let mut ids = Vec::new();
            split_ids(&root, &mut ids);
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), n - 1, "every split keeps its own id");
            let names: Vec<&str> = panes(&root).into_iter().map(|(id, _)| id).collect();
            let want: Vec<String> = (1..=n).map(|i| format!("p{i}")).collect();
            assert_eq!(names, want, "reading order, newest last");
            shapes.push(shape(&root));
        }
        shapes
    }

    #[test]
    fn main_stack_keeps_the_first_pane_left_and_stacks_the_rest() {
        assert_eq!(
            placed_sequence(Placement::MainStack),
            [
                "p1",
                "H0.50(p1,p2)",
                "H0.50(p1,V0.50(p2,p3))",
                "H0.50(p1,V0.33(p2,V0.50(p3,p4)))",
                "H0.50(p1,V0.25(p2,V0.33(p3,V0.50(p4,p5))))",
            ]
        );
    }

    #[test]
    fn grid_arranges_the_squarest_rows_in_reading_order() {
        assert_eq!(
            placed_sequence(Placement::Grid),
            [
                "p1",
                "H0.50(p1,p2)",
                "V0.50(H0.50(p1,p2),p3)",
                "V0.50(H0.50(p1,p2),H0.50(p3,p4))",
                "V0.50(H0.33(p1,H0.50(p2,p3)),H0.50(p4,p5))",
            ]
        );
    }

    #[test]
    fn a_rebuild_with_an_unknown_pane_changes_nothing() {
        let mut root = leaf("p1");
        assert_eq!(
            place(&mut root, "p9", Placement::Grid, None, "x2", "p2"),
            None
        );
        assert_eq!(shape(&root), "p1");
    }

    #[test]
    fn split_puts_the_new_pane_on_the_named_side() {
        let mut root = leaf("p1");
        assert!(split(&mut root, "p1", Dir::Left, "x2", "p3"));
        let Node::Split {
            axis, ratio, a, b, ..
        } = &root
        else {
            panic!("split")
        };
        assert_eq!((*axis, *ratio), (Axis::Horizontal, 0.5));
        assert_eq!((first_pane(a), first_pane(b)), ("p3", "p1"));
        assert!(
            !split(&mut root, "p9", Dir::Down, "x4", "p5"),
            "unknown pane"
        );
    }

    #[test]
    fn removing_a_pane_lets_its_sibling_take_the_split() {
        // p1 | (p3 / p5)
        let mut root = leaf("p1");
        split(&mut root, "p1", Dir::Right, "x2", "p3");
        split(&mut root, "p3", Dir::Down, "x4", "p5");
        let Removal::Removed { rest, kind, .. } = remove(root, "p3") else {
            panic!("removed")
        };
        assert_eq!(kind, PaneKind::empty());
        let Node::Split { id, a, b, .. } = &rest else {
            panic!("split")
        };
        assert_eq!(
            (id.as_str(), first_pane(a), first_pane(b)),
            ("x2", "p1", "p5")
        );
        let Removal::NotFound(rest) = remove(rest, "p9") else {
            panic!("unknown pane leaves the tree")
        };
        let Removal::Removed { rest, .. } = remove(rest, "p1") else {
            panic!("removed")
        };
        assert_eq!(remove(rest, "p5"), Removal::Last(PaneKind::empty()));
    }

    /// A top-level key a newer build added to the saved file survives a load and save by this
    /// build, so a downgrade does not drop what the newer build keeps there.
    #[test]
    fn an_unknown_top_level_key_survives_a_load_and_save() {
        let newer =
            r#"{"spaces":[],"next_id":3,"lead2":{"conversation":"zzz"},"schema":9}"#;
        let loaded: Saved = serde_json::from_str(newer).unwrap();
        assert_eq!(loaded.extra.len(), 2, "{:?}", loaded.extra);
        let back: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&loaded).unwrap()).unwrap();
        let want: serde_json::Value = serde_json::from_str(newer).unwrap();
        assert_eq!(back, want);
    }
}
