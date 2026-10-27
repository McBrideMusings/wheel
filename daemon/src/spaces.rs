//! Spaces: one per project, each a split tree of panes (after Ghostty's SplitTree). The daemon
//! owns the layout (ADR-0001), saves it after every change, and broadcasts it to every client.
//! Every caller — a client op or the CLI — changes it through the `AppState`
//! methods here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wheel_proto::{
    ArtifactRef, ArtifactScope, Axis, ContextUse, Dir, Extra, NewPaneKind, Node, PaneKind,
    ServerMsg, SessionState, SpaceInfo, StartKind,
};

use crate::pty::Term;
use crate::server::{AppState, Owner};
use crate::settings::{page, HOMEPAGE};

/// Every space, in tab order, and which one is showing. Also the saved file's format.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Spaces {
    pub spaces: Vec<SpaceInfo>,
    pub active: Option<String>,
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
            .position(|s| find_pane(&s.root, pane).is_some())
            .ok_or_else(|| format!("no pane {pane}"))
    }

    /// Remove a space, moving `active` to its neighbour when it was showing.
    fn remove(&mut self, index: usize) -> SpaceInfo {
        let space = self.spaces.remove(index);
        if self.active.as_deref() == Some(&space.id) {
            self.active = self
                .spaces
                .get(index)
                .or_else(|| self.spaces.last())
                .map(|s| s.id.clone());
        }
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
fn last_pane(node: &Node) -> &str {
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

/// Replace pane `pane` with a split holding it and a new empty pane on the `dir` side, sharing
/// the area evenly. False when no pane matches.
pub fn split(node: &mut Node, pane: &str, dir: Dir, split_id: &str, new_id: &str) -> bool {
    let new = Node::Pane {
        id: new_id.to_string(),
        status: None,
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

fn leaf_placeholder() -> Node {
    Node::Pane {
        id: String::new(),
        status: None,
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

fn find_leaf_mut<'a>(node: &'a mut Node, pane: &str) -> Option<&'a mut Node> {
    match node {
        Node::Pane { id, .. } if id == pane => Some(node),
        Node::Pane { .. } => None,
        Node::Split { a, b, .. } => find_leaf_mut(a, pane).or_else(|| find_leaf_mut(b, pane)),
    }
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
    Removed {
        rest: Node,
        kind: PaneKind,
        /// The leaf of the removed pane's sibling nearest to where it was.
        neighbour: String,
    },
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
                Removal::Last(kind) => Removal::Removed {
                    neighbour: first_pane(&b).to_string(),
                    rest: *b,
                    kind,
                },
                Removal::Removed {
                    rest,
                    kind,
                    neighbour,
                } => Removal::Removed {
                    rest: rebuild(rest, *b),
                    kind,
                    neighbour,
                },
                Removal::NotFound(a) => match remove(*b, pane) {
                    Removal::Last(kind) => Removal::Removed {
                        neighbour: last_pane(&a).to_string(),
                        rest: a,
                        kind,
                    },
                    Removal::Removed {
                        rest,
                        kind,
                        neighbour,
                    } => Removal::Removed {
                        rest: rebuild(a, rest),
                        kind,
                        neighbour,
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

/// How often each terminal pane's directory and agent are sampled.
const TERMINAL_SAMPLE: Duration = Duration::from_secs(2);

/// How long a new shell may show no agent without its pane forgetting one it was just told to
/// resume.
const RESUME_GRACE: Duration = Duration::from_secs(30);

/// What `pane_start` returns: the pane, and for an agent its session and state.
#[derive(Debug, Serialize)]
pub struct Started {
    pub pane: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<SessionState>,
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
    /// carries its session's state beside the session's name, and every pane and space its
    /// status. The saved layout and the broadcast leave them out, since they change without the
    /// layout changing.
    pub fn listing(&self) -> Spaces {
        let mut spaces = self.spaces();
        for space in &mut spaces.spaces {
            each_kind_mut(&mut space.root, &mut |kind| {
                if let PaneKind::Agent {
                    session,
                    state,
                    context,
                    ..
                } = kind
                {
                    let running = self.session(session).ok();
                    *state = running.as_ref().map(|s| Box::new(s.state()));
                    *context = running.as_ref().and_then(|s| s.context());
                }
            });
        }
        self.with_statuses(spaces)
    }

    /// Write the saved state — the layout and the id counter. The one place that touches the file.
    pub(crate) fn save_state(&self, spaces: &Spaces) {
        let Some(path) = self.state_file.lock().unwrap().clone() else {
            return;
        };
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
    }

    /// Send the layout to every client without writing it: for a change a restart need not keep,
    /// such as an agent's context figure, which the next save carries and the agent reports again.
    fn broadcast_layout(&self, spaces: &Spaces) {
        let _ = self.global.send(ServerMsg::Spaces {
            spaces: spaces.spaces.clone(),
            active: spaces.active.clone(),
        });
    }

    /// End what a closed or replaced pane showed: kill its terminal or agent, or stop watching its
    /// artifact and remove the data directory it was given.
    fn drop_content(&self, pane: &str, kind: &PaneKind) {
        match kind {
            PaneKind::Terminal { term, .. } => {
                if let Some(held) = self.terms.lock().unwrap().remove(term) {
                    tracing::info!(term = %term, "pty killed");
                    held.term.kill();
                }
            }
            PaneKind::Web { .. } => self.drop_artifact(pane),
            PaneKind::Agent { session, .. } => {
                self.end_session(session);
                // The caller holds the spaces lock, so the sweep that deletes this session's
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
            // A browser pane holds nothing of the daemon's: no folder, no data directory, no
            // watch, no refresh, no terminal. The client's web view goes with the pane.
            PaneKind::Browser { .. } | PaneKind::Empty { .. } | PaneKind::Unknown(_) => {}
        }
    }

    /// Make a space for `cwd`, with one empty pane. With `focus` it becomes the active space;
    /// without, the person's view stays as it was.
    pub fn space_create(
        &self,
        cwd: String,
        name: Option<String>,
        focus: bool,
    ) -> Result<SpaceInfo, String> {
        let path = Path::new(&cwd);
        if !path.is_absolute() || !path.is_dir() {
            return Err(format!("{cwd} is not an absolute path to a directory"));
        }
        let name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| cwd.clone());
        let (id, pane) = (self.new_id('s'), self.new_id('p'));
        let space = SpaceInfo {
            id: id.clone(),
            name: name.clone(),
            cwd: cwd.clone(),
            root: Node::Pane {
                id: pane.clone(),
                status: None,
                kind: PaneKind::empty(),
            },
            focus: pane,
            status: None,
        };
        let mut spaces = self.spaces.lock().unwrap();
        spaces.spaces.push(space.clone());
        if focus {
            spaces.active = Some(id.clone());
        }
        tracing::info!(space = %id, %name, %cwd, focus, "space created");
        self.record_recent(&cwd);
        self.changed(&spaces);
        Ok(space)
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
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.index(id)?;
        if if_empty
            && !matches!(
                &spaces.spaces[index].root,
                Node::Pane {
                    kind: PaneKind::Empty { .. },
                    ..
                }
            )
        {
            tracing::info!(space = %id, "space close refused: it no longer holds just one empty pane");
            return Err(format!("space {id} is no longer empty; not closed"));
        }
        let space = spaces.remove(index);
        for (pane, kind) in panes(&space.root) {
            self.drop_content(pane, kind);
        }
        tracing::info!(space = %id, "space closed");
        self.changed(&spaces);
        Ok(())
    }

    pub fn focus(&self, space: Option<&str>, pane: Option<&str>) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = match (space, pane) {
            (_, Some(pane)) => {
                let index = spaces.holding(pane)?;
                if space.is_some_and(|s| s != spaces.spaces[index].id) {
                    return Err(format!(
                        "pane {pane} is not in space {}",
                        space.unwrap_or_default()
                    ));
                }
                spaces.spaces[index].focus = pane.to_string();
                index
            }
            (Some(space), None) => spaces.index(space)?,
            (None, None) => return Err("focus needs a space or a pane".into()),
        };
        let id = spaces.spaces[index].id.clone();
        spaces.active = Some(id.clone());
        tracing::info!(space = %id, pane = ?pane, "focus");
        self.changed(&spaces);
        Ok(())
    }

    /// The space the person is looking at and its focused pane: the pair a split moves and
    /// [`AppState::undo_split`] puts back.
    fn shown(&self) -> Option<(String, String)> {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .active
            .as_ref()
            .and_then(|id| spaces.spaces.iter().find(|s| &s.id == id))
            .map(|s| (s.id.clone(), s.focus.clone()))
    }

    /// Take back a split made only for a start or open that was refused: close `pane` and, when the split
    /// had moved the view (`focus`), show `was` again. Closing the pane puts the focus on a
    /// neighbour, which is not necessarily the one the split moved off, and leaves whatever space
    /// was shown before another space showing.
    fn undo_split(&self, pane: &str, focus: bool, was: &Option<(String, String)>) {
        let _ = self.pane_close(pane);
        if let (true, Some((space, was_focus))) = (focus, was) {
            let _ = self.focus(Some(space), Some(was_focus));
        }
    }

    /// Split `pane`, putting a new empty pane on the `dir` side. With `focus`, the new pane takes
    /// the focus and its space shows; without, the person's view stays as it was.
    pub fn pane_split(&self, pane: &str, dir: Dir, focus: bool) -> Result<String, String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let (split_id, new_id) = (self.new_id('x'), self.new_id('p'));
        let space = &mut spaces.spaces[index];
        split(&mut space.root, pane, dir, &split_id, &new_id);
        let space_id = space.id.clone();
        if focus {
            space.focus = new_id.clone();
            spaces.active = Some(space_id.clone());
        }
        tracing::info!(space = %space_id, %pane, ?dir, split = %split_id, new = %new_id, focus, "pane split");
        self.changed(&spaces);
        Ok(new_id)
    }

    /// Swap the places of panes `pane` and `with`, which share a space.
    pub fn pane_swap(&self, pane: &str, with: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        if spaces.holding(with)? != index {
            return Err(format!("{pane} and {with} are in different spaces"));
        }
        let space = &mut spaces.spaces[index];
        if !swap(&mut space.root, pane, with) {
            return Err(format!("{pane} cannot swap with itself"));
        }
        tracing::info!(space = %space.id, %pane, %with, "panes swapped");
        self.changed(&spaces);
        Ok(())
    }

    /// Move `pane` to the `dir` side of `target`, which shares its space: the split `pane` left
    /// collapses and a new one holds `pane` and `target`.
    pub fn pane_move(&self, pane: &str, target: &str, dir: Dir) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        if spaces.holding(target)? != index {
            return Err(format!("{pane} and {target} are in different spaces"));
        }
        let split_id = self.new_id('x');
        let space = &mut spaces.spaces[index];
        move_pane(&mut space.root, pane, target, dir, &split_id)?;
        tracing::info!(space = %space.id, %pane, %target, ?dir, split = %split_id, "pane moved");
        self.changed(&spaces);
        Ok(())
    }

    /// Close a pane; closing a space's last pane leaves the space with one new empty pane.
    pub fn pane_close(&self, pane: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let space = &mut spaces.spaces[index];
        // An empty pane that is the whole space is already what closing it would leave.
        if matches!(&space.root, Node::Pane { id, kind: PaneKind::Empty { .. }, .. } if id == pane)
        {
            return Ok(());
        }
        let root = std::mem::replace(
            &mut space.root,
            Node::Pane {
                id: String::new(),
                status: None,
                kind: PaneKind::empty(),
            },
        );
        match remove(root, pane) {
            Removal::NotFound(root) => {
                space.root = root;
                return Err(format!("no pane {pane}"));
            }
            Removal::Removed {
                rest,
                kind,
                neighbour,
            } => {
                self.drop_content(pane, &kind);
                if space.focus == pane {
                    space.focus = neighbour;
                }
                space.root = rest;
                tracing::info!(space = %space.id, %pane, focus = %space.focus, "pane closed");
            }
            Removal::Last(kind) => {
                self.drop_content(pane, &kind);
                let fresh = self.new_id('p');
                space.root = Node::Pane {
                    id: fresh.clone(),
                    status: None,
                    kind: PaneKind::empty(),
                };
                space.focus = fresh.clone();
                tracing::info!(space = %space.id, %pane, new = %fresh, "pane closed; it was the last, so the space holds one empty pane");
            }
        }
        self.changed(&spaces);
        Ok(())
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
                watching = Some((artifact.clone(), cwd));
                PaneKind::Web {
                    artifact,
                    extra: Extra::default(),
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
                let (space, cwd) = {
                    let spaces = self.spaces.lock().unwrap();
                    let held = &spaces.spaces[spaces.holding(pane)?];
                    (held.id.clone(), held.cwd.clone())
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
                            Some((&space, pane, Some(&cwd))),
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
            .find_map(|s| find_pane_mut(&mut s.root, pane))
        else {
            self.drop_content(pane, &new);
            return Err(format!("no pane {pane}"));
        };
        let old = std::mem::replace(slot, new);
        self.drop_content(pane, &old);
        if let Some((artifact, cwd)) = &watching {
            self.show_artifact(pane, artifact, cwd);
        }
        // A new agent may have its conversation id already.
        self.sync_conversations(&mut spaces);
        let now = find_pane(&spaces.spaces[spaces.holding(pane)?].root, pane)
            .cloned()
            .ok_or_else(|| format!("no pane {pane}"))?;
        tracing::info!(%pane, kind = ?now, "pane kind");
        self.changed(&spaces);
        Ok(now)
    }

    /// Start a terminal or an agent in `pane` of `space` (or, with no pane, in a new pane split
    /// right of the space's focused one and focused only with `focus`), whatever the
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
        message: Option<String>,
        agent: Option<String>,
        focus: bool,
    ) -> Result<Started, String> {
        if kind == StartKind::Terminal && message.is_some() {
            return Err("a first message is only for an agent pane".into());
        }
        if kind == StartKind::Terminal && agent.is_some() {
            return Err("an agent name is only for an agent pane".into());
        }
        // Refused before any pane is made or changed, so a program that is missing leaves the
        // layout as it was.
        let agent = agent.unwrap_or_else(|| self.agent_default());
        if kind == StartKind::Agent {
            self.agent_ready(&agent)?;
        }
        let was = self.shown();
        let pane = {
            let spaces = self.spaces.lock().unwrap();
            let held = &spaces.spaces[spaces.index(space)?];
            match pane {
                Some(pane) if find_pane(&held.root, &pane).is_none() => {
                    return Err(format!("pane {pane} is not in space {space}"))
                }
                Some(pane) => Ok(pane),
                None => Err(held.focus.clone()),
            }
        };
        let (pane, split) = match pane {
            Ok(pane) => (pane, false),
            Err(focused) => (self.pane_split(&focused, Dir::Right, focus)?, true),
        };
        let started = match kind {
            StartKind::Terminal => self
                .pane_set_kind(&pane, NewPaneKind::Terminal, None, None, None)
                .await
                .map(|_| None)
                .map_err(|e| (e, false)),
            StartKind::Agent => self
                .start_agent(space, &pane, &agent, message)
                .await
                .map(Some),
        };
        match started {
            Ok(agent) => Ok(Started {
                pane,
                session: agent.as_ref().map(|(session, _)| session.clone()),
                state: agent.map(|(_, state)| state),
            }),
            Err((e, kept)) => {
                // A pane made only for this start goes with it, unless it now holds an agent
                // that failed to come up: that pane stays open showing the error.
                if split && !kept {
                    self.undo_split(&pane, focus, &was);
                }
                Err(e)
            }
        }
    }

    /// Run the `[runner]` command `name` of `space` in a new terminal pane split right of the
    /// space's focused one, and return that pane. The command is typed into the pane's shell, so
    /// the shell stays open and shows its output after it exits.
    pub async fn run_command(&self, space: &str, name: &str) -> Result<String, String> {
        let command = self
            .runner_ls(Some(space))?
            .into_iter()
            .find(|r| r.name == name)
            .map(|r| r.command)
            .ok_or_else(|| format!("no runner named {name} in space {space}"))?;
        let started = self
            .pane_start(space, None, StartKind::Terminal, None, None, true)
            .await?;
        let term = {
            let spaces = self.spaces.lock().unwrap();
            let held = &spaces.spaces[spaces.index(space)?];
            match find_pane(&held.root, &started.pane) {
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

    /// Start the agent of a saved agent pane under its saved name, loading its saved conversation,
    /// in `space`, whose directory is `space_cwd` and whose layout is `root`.
    fn start_saved_agent(
        &self,
        space: &str,
        space_cwd: &str,
        root: &Node,
        saved: AgentPane,
    ) -> Result<(), String> {
        // A conversation switched over from a terminal runs where Claude Code ran it.
        let place = match find_pane(root, &saved.pane) {
            Some(PaneKind::Agent {
                place: Some(place), ..
            }) => (**place).clone(),
            _ => Default::default(),
        };
        let wheel_proto::AgentPlace { cwd, home } = place;
        self.spawn_agent(
            &saved.session,
            (space, &saved.pane, &saved.agent),
            cwd.as_deref().unwrap_or(space_cwd),
            saved.conversation,
            home,
            saved.context,
        )
    }

    /// Start the agent panes a restore kept as saved because their agent could not start, which
    /// is a program not yet found. Called once the login shell's PATH arrives late.
    pub(crate) fn start_unstarted_agents(&self) {
        let kept = std::mem::take(&mut *self.kept_agents.lock().unwrap());
        let waiting: Vec<(String, String, Node, AgentPane)> = {
            let spaces = self.spaces.lock().unwrap();
            spaces
                .spaces
                .iter()
                .flat_map(|space| {
                    agent_panes(&space.root)
                        .into_iter()
                        .filter(|a| kept.contains(&a.session))
                        .map(|a| (space.id.clone(), space.cwd.clone(), space.root.clone(), a))
                })
                .collect()
        };
        for (space, cwd, root, saved) in waiting {
            let (pane, session) = (saved.pane.clone(), saved.session.clone());
            match self.start_saved_agent(&space, &cwd, &root, saved) {
                Ok(()) => {
                    tracing::info!(%space, %pane, %session, "agent started now that its program is found")
                }
                Err(e) => {
                    tracing::warn!(%space, %pane, %session, error = %e, "agent still cannot start")
                }
            }
        }
    }

    /// Make `pane` an agent pane, send its agent `message` if there is one, and wait for it to
    /// be ready. Returns the session's name and state; an error carries whether the pane is now
    /// an agent pane, which a caller that made the pane for this start keeps.
    async fn start_agent(
        &self,
        space: &str,
        pane: &str,
        agent: &str,
        message: Option<String>,
    ) -> Result<(String, SessionState), (String, bool)> {
        let cwd = {
            let spaces = self.spaces.lock().unwrap();
            spaces.spaces[spaces.holding(pane).map_err(|e| (e, false))?]
                .cwd
                .clone()
        };
        let session = self.new_id('w');
        self.spawn_agent(&session, (space, pane, agent), &cwd, None, None, None)
            .map_err(|e| (e, false))?;
        let new = PaneKind::Agent {
            session: session.clone(),
            agent: agent.to_string(),
            conversation: None,
            state: None,
            context: None,
            place: None,
            extra: Extra::default(),
        };
        // A pane that closed while the agent spawned ends the session it was given.
        self.install(pane, new, None).map_err(|e| (e, false))?;
        let agent = self.session(&session).map_err(|e| (e, true))?;
        if let Some(message) = message {
            agent.send(message).map_err(|e| (e, true))?;
        }
        tracing::info!(%space, %pane, %session, "agent started");
        let start_wait = self.agent_timing.lock().unwrap().start_wait;
        match tokio::time::timeout(start_wait, agent.started()).await {
            Ok(Ok(())) => tracing::info!(%session, "agent ready"),
            Ok(Err(e)) => {
                tracing::warn!(%space, %pane, %session, error = %e, "agent could not start");
                return Err((
                    format!(
                        "agent {session} in pane {pane} could not start: {e}. Its pane stays open showing the error; wheel pane close removes it"
                    ),
                    true,
                ));
            }
            Err(_) => {
                tracing::info!(%session, wait_ms = start_wait.as_millis() as u64, "agent still starting")
            }
        }
        Ok((session, agent.state()))
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
            .find_map(|s| find_pane_mut(&mut s.root, pane))
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

    /// Record what the agent in terminal pane `pane` says it is doing, and tell clients if the
    /// pane's status changed.
    pub fn pane_report(
        &self,
        pane: &str,
        status: wheel_proto::ReportedStatus,
    ) -> Result<(), String> {
        let term = {
            let spaces = self.spaces.lock().unwrap();
            let kind = spaces
                .spaces
                .iter()
                .flat_map(|s| panes(&s.root))
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
        let old = self.reports.lock().unwrap().insert(term.clone(), status);
        if old != Some(status) {
            tracing::info!(%pane, %term, old = ?old, new = ?status, "terminal agent reported");
            self.refresh_statuses();
        }
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
                .flat_map(|s| panes(&s.root))
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

    /// Claude Code in a terminal pane says it is in conversation `session` now. The next sample
    /// saves it as the pane's agent session, so a view switch resumes this conversation.
    pub fn pane_report_session(&self, pane: &str, session: &str, pid: u32) -> Result<(), String> {
        let term = {
            let spaces = self.spaces.lock().unwrap();
            let kind = spaces
                .spaces
                .iter()
                .flat_map(|s| panes(&s.root))
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
            tracing::warn!(%pane, %term, %pid, ?reporter, ?foreground, "session report ignored; it is not from the terminal's Claude Code");
            return Err(format!(
                "{pane}'s foreground program did not send this report"
            ));
        }
        let old = self
            .session_reports
            .lock()
            .unwrap()
            .insert(term.clone(), session.to_string());
        if old.as_deref() != Some(session) {
            tracing::info!(%pane, %term, old = ?old, new = %session, "terminal agent reported its session");
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
            .any(|s| set_ratio(&mut s.root, split, ratio))
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
            extra,
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
        for key in extra.keys() {
            tracing::warn!(%key, "unknown saved key; kept as is");
        }
        *self.saved_extra.lock().unwrap() = extra;
        tracing::info!(rows = recent.len(), "space recent restored");
        *self.recent.lock().unwrap() = recent;
        let (mut respawned, mut agents) = (0, 0);
        let mut resumed_sessions = std::collections::HashSet::new();
        let named_web: Vec<String> = loaded
            .spaces
            .iter()
            .flat_map(|space| {
                // A pane this build cannot read is kept whole, so its data directory stays too.
                let web = crate::web::web_panes(&space.root)
                    .into_iter()
                    .map(|(p, _)| p);
                web.chain(unknown_panes(&space.root).into_iter().map(|(p, _)| p))
            })
            .map(str::to_string)
            .collect();
        for space in &mut loaded.spaces {
            let mut outcomes: HashMap<String, String> = HashMap::new();
            for (pane, kind) in unknown_panes(&space.root) {
                tracing::warn!(space = %space.id, %pane, %kind, "unknown pane kind; kept as is");
            }
            // A restored web pane keeps its artifact whether or not the folder is there: the watch
            // waits for a missing one to appear, as it does for a folder removed while watched.
            for (pane, artifact) in crate::web::web_panes(&space.root) {
                self.show_artifact(pane, artifact, &space.cwd);
            }
            let mut ids = Vec::new();
            collect_terminal_panes(&space.root, &mut ids);
            for pane in ids {
                let (saved_cwd, saved_agent) = match find_pane(&space.root, &pane) {
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
                        Some((&space.id, &pane, Some(&space.cwd))),
                    )
                    .await;
                let Some(slot) = find_pane_mut(&mut space.root, &pane) else {
                    continue;
                };
                match opened {
                    // Only the shell is new; the pane keeps the rest of what was saved.
                    Ok(term) => {
                        let mut resumed = None;
                        if let Some(agent) = saved_agent {
                            let line = crate::resume::resume_line(&agent);
                            let key = (agent.name.clone(), agent.session.clone());
                            if resumed_sessions.contains(&key) {
                                outcome = format!("{outcome}; {} session {} already resumed in another pane, so a plain shell", agent.name, agent.session);
                            } else if !crate::resume::session_exists(&agent, &dir) {
                                tracing::warn!(space = %space.id, %pane, agent = %agent.name, session = %agent.session, %dir, "the agent's session is gone; the pane starts a plain shell");
                                outcome = format!(
                                    "{outcome}; {} session {} is gone, so a plain shell",
                                    agent.name, agent.session
                                );
                            } else if let Some(line) = line {
                                match term.write(format!("{line}\n").into_bytes()) {
                                    Ok(()) => {
                                        outcome = format!(
                                            "{outcome}; resumed {} session {}",
                                            agent.name, agent.session
                                        );
                                        resumed_sessions.insert(key);
                                        resumed = Some(agent);
                                    }
                                    Err(e) => {
                                        tracing::warn!(space = %space.id, %pane, error = %e, "could not type the agent's resume command");
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
                        tracing::warn!(space = %space.id, %pane, error = %e, "could not restart the pane's shell; it is empty now");
                        outcome = format!("empty: {e}");
                        *slot = PaneKind::empty();
                    }
                }
                outcomes.insert(pane, outcome);
            }
            // Agents do not outlive the daemon: each agent pane's agent starts again under
            // its saved name, and loads its saved conversation where the agent can.
            for saved in agent_panes(&space.root) {
                let (pane, session) = (saved.pane.clone(), saved.session.clone());
                match self.start_saved_agent(&space.id, &space.cwd, &space.root, saved) {
                    Ok(()) => {
                        agents += 1;
                        outcomes.insert(
                            pane,
                            format!("agent {session} started, loading its saved conversation"),
                        );
                    }
                    // Kept as saved, so a later start with an agent command runs it again.
                    Err(e) => {
                        tracing::warn!(space = %space.id, %pane, %session, error = %e, "could not restart the pane's agent");
                        self.kept_agents.lock().unwrap().insert(session.clone());
                        outcomes.insert(
                            pane,
                            format!("agent {session} kept as saved, not started: {e}"),
                        );
                    }
                }
            }
            // One line per pane, so a log read afterwards says what each came back as and why.
            for (pane, kind) in panes(&space.root) {
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
            for (_, kind) in spaces.spaces.iter().flat_map(|space| panes(&space.root)) {
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
                let claimed = saved
                    .iter()
                    .filter(|(term, _)| **term != t.id)
                    .map(|(_, agent)| agent.session.clone())
                    .collect();
                let reported = self.context_reports.lock().unwrap().get(&t.id).copied();
                let reported_session = self.session_reports.lock().unwrap().get(&t.id).cloned();
                let sample = crate::resume::sample(
                    t.pid,
                    t.foreground(),
                    saved.get(&t.id),
                    &claimed,
                    reported,
                    reported_session.as_deref(),
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
            each_kind_mut(&mut space.root, &mut |kind| {
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
                    && self.session_reports.lock().unwrap().remove(term).is_some()
                {
                    tracing::info!(%term, "terminal agent session report dropped; no agent runs here");
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
            .flat_map(|space| panes(&space.root))
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
        self.session_reports
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

    /// An agent pane's agent gave its session a new conversation id: save it in the pane.
    pub(crate) fn conversations_changed(&self) {
        let mut spaces = self.spaces.lock().unwrap();
        if self.sync_conversations(&mut spaces) {
            self.changed(&spaces);
        }
    }

    /// Copy each running agent session's conversation id into its agent pane; true when one changed.
    /// Agents run while the spaces lock is free, so a pane takes its session's id here, under the
    /// lock, whichever of the two comes first.
    fn sync_conversations(&self, spaces: &mut Spaces) -> bool {
        let mut changed = false;
        for space in &mut spaces.spaces {
            each_kind_mut(&mut space.root, &mut |kind| {
                if let PaneKind::Agent {
                    session, context, ..
                } = kind
                {
                    if let Ok(running) = self.session(session).map(|s| s.context()) {
                        if *context != running {
                            *context = running;
                            changed = true;
                        }
                    }
                }
                let PaneKind::Agent {
                    session,
                    conversation,
                    ..
                } = kind
                else {
                    return;
                };
                let Ok(running) = self.session(session).map(|s| s.saved_conversation()) else {
                    return;
                };
                if *conversation != running {
                    tracing::info!(%session, conversation = ?running, "agent pane saves its conversation");
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
fn unknown_panes(node: &Node) -> Vec<(&str, &str)> {
    match node {
        Node::Pane {
            id,
            kind: PaneKind::Unknown(unknown),
            ..
        } => vec![(id, unknown.kind())],
        Node::Pane { .. } => vec![],
        Node::Split { a, b, .. } => {
            let mut all = unknown_panes(a);
            all.extend(unknown_panes(b));
            all
        }
    }
}

/// An agent pane as saved: its session's name, the agent it runs and the conversation it keeps.
pub(crate) struct AgentPane {
    pub pane: String,
    pub session: String,
    pub agent: String,
    pub conversation: Option<String>,
    pub context: Option<ContextUse>,
}

/// Each agent pane in `node`.
pub(crate) fn agent_panes(node: &Node) -> Vec<AgentPane> {
    panes(node)
        .into_iter()
        .filter_map(|(id, kind)| match kind {
            PaneKind::Agent {
                session,
                agent,
                conversation,
                context,
                ..
            } => Some(AgentPane {
                pane: id.to_string(),
                session: session.clone(),
                agent: agent.clone(),
                conversation: conversation.clone(),
                context: *context,
            }),
            _ => None,
        })
        .collect()
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

fn collect_terminal_panes(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Pane {
            id,
            kind: PaneKind::Terminal { .. },
            ..
        } => out.push(id.clone()),
        Node::Pane { .. } => {}
        Node::Split { a, b, .. } => {
            collect_terminal_panes(a, out);
            collect_terminal_panes(b, out);
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
            kind: PaneKind::empty(),
        }
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

    fn neighbour(root: Node, pane: &str) -> String {
        let Removal::Removed { neighbour, .. } = remove(root, pane) else {
            panic!("removed")
        };
        neighbour
    }

    #[test]
    fn a_removed_pane_names_the_pane_beside_it() {
        // p1 | (p3 / p5): closing p5 lands on p3, not the top-left p1.
        let mut root = leaf("p1");
        split(&mut root, "p1", Dir::Right, "x2", "p3");
        split(&mut root, "p3", Dir::Down, "x4", "p5");
        assert_eq!(neighbour(root.clone(), "p5"), "p3");
        assert_eq!(neighbour(root.clone(), "p3"), "p5");
        // Closing the left side lands on the right side's top-left pane.
        assert_eq!(neighbour(root, "p1"), "p3");

        // (p1 / p3) | p5: closing p5 lands on the left side's bottom-right pane.
        let mut root = leaf("p1");
        split(&mut root, "p1", Dir::Right, "x2", "p5");
        split(&mut root, "p1", Dir::Down, "x4", "p3");
        assert_eq!(neighbour(root, "p5"), "p3");
    }

    /// A top-level key a newer build added to the saved file survives a load and save by this
    /// build, so a downgrade does not drop what the newer build keeps there.
    #[test]
    fn an_unknown_top_level_key_survives_a_load_and_save() {
        let newer =
            r#"{"spaces":[],"active":null,"next_id":3,"lead2":{"conversation":"zzz"},"schema":9}"#;
        let loaded: Saved = serde_json::from_str(newer).unwrap();
        assert_eq!(loaded.extra.len(), 2, "{:?}", loaded.extra);
        let back: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&loaded).unwrap()).unwrap();
        let want: serde_json::Value = serde_json::from_str(newer).unwrap();
        assert_eq!(back, want);
    }
}
