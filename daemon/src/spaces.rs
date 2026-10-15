//! Spaces: one per project, each a split tree of panes (after Ghostty's SplitTree). The daemon
//! owns the layout (ADR-0001), saves it after every change, and broadcasts it to every client.
//! Every caller — a client op, the CLI, the lead's tools — changes it through the `AppState`
//! methods here.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wheel_proto::{
    ArtifactRef, ArtifactScope, Axis, CreatedBy, Dir, Extra, NewPaneKind, Node, PaneKind,
    ServerMsg, SessionState, SpaceInfo, StartKind,
};

use crate::pty::Term;
use crate::server::{AppState, Owner};
use crate::session::LEAD;
use crate::settings::{page, HOMEPAGE};

/// Every space, in tab order, and which one is showing. Also the saved file's format.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Spaces {
    pub spaces: Vec<SpaceInfo>,
    pub active: Option<String>,
}

impl Spaces {
    fn index(&self, id: &str) -> Result<usize, String> {
        self.spaces
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| format!("no space {id}"))
    }

    /// The index of the space holding `pane`.
    fn holding(&self, pane: &str) -> Result<usize, String> {
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

/// The saved file: the layout, the id counter so a restarted daemon never hands out an id it gave
/// before — not a closed pane's, not a terminal's a client may still remember — and the lead's
/// conversation, which has no pane to live in. A top-level key this build does not know — one a
/// newer build wrote — rides in `extra` and is written back, as an unknown pane kind is.
#[derive(Serialize, Deserialize)]
struct Saved {
    #[serde(flatten)]
    layout: Spaces,
    next_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lead: Option<LeadState>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// What the lead carries across a restart. A file written before this existed has no `lead` key,
/// and reads as a lead with no conversation yet.
#[derive(Serialize, Deserialize, Clone)]
pub struct LeadState {
    /// The agent's id for the conversation the lead was last in.
    pub acp_session: String,
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

/// Who made pane `pane`.
fn find_created_by(node: &Node, pane: &str) -> Option<CreatedBy> {
    match node {
        Node::Pane { id, created_by, .. } => (id == pane).then_some(*created_by),
        Node::Split { a, b, .. } => find_created_by(a, pane).or_else(|| find_created_by(b, pane)),
    }
}

/// Every pane with who made it, top-left first.
pub fn creators(node: &Node) -> Vec<(&str, CreatedBy)> {
    match node {
        Node::Pane { id, created_by, .. } => vec![(id, *created_by)],
        Node::Split { a, b, .. } => {
            let mut all = creators(a);
            all.extend(creators(b));
            all
        }
    }
}

/// Replace pane `pane` with a split holding it and a new empty pane, made by `by`, on the `dir`
/// side, sharing the area evenly. False when no pane matches.
pub fn split(
    node: &mut Node,
    pane: &str,
    dir: Dir,
    split_id: &str,
    new_id: &str,
    by: CreatedBy,
) -> bool {
    match node {
        Node::Pane { id, .. } if id == pane => {
            let old = std::mem::replace(
                node,
                Node::Pane {
                    id: String::new(),
                    created_by: CreatedBy::Person,
                    kind: PaneKind::empty(),
                },
            );
            let new = Node::Pane {
                id: new_id.to_string(),
                created_by: by,
                kind: PaneKind::empty(),
            };
            let (axis, a, b) = match dir {
                Dir::Right => (Axis::Horizontal, old, new),
                Dir::Left => (Axis::Horizontal, new, old),
                Dir::Down => (Axis::Vertical, old, new),
                Dir::Up => (Axis::Vertical, new, old),
            };
            *node = Node::Split {
                id: split_id.to_string(),
                axis,
                ratio: 0.5,
                a: Box::new(a),
                b: Box::new(b),
            };
            true
        }
        Node::Pane { .. } => false,
        Node::Split { a, b, .. } => {
            split(a, pane, dir, split_id, new_id, by) || split(b, pane, dir, split_id, new_id, by)
        }
    }
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
const PANE_COLS: u16 = 80;
const PANE_ROWS: u16 = 24;

/// How long a browser pane's saved page waits for the next one before it is written. A person
/// browsing reports a page per load, per redirect and per in-page navigation, and the state file
/// is a convenience for the next start rather than a record of where they have been: writing it
/// once per burst keeps the last page without writing the whole layout dozens of times a minute.
/// Every other change to the layout still writes it at once, and any such write takes a page
/// waiting here with it.
const SAVE_COALESCE: Duration = Duration::from_millis(400);

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
    fn new_id(&self, prefix: char) -> String {
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

    /// The layout for a reader that asks (`status`, the lead's `list_spaces`): every agent pane
    /// carries its session's state beside the session's name. The saved layout and the broadcast
    /// leave it out, since a state changes without the layout changing.
    pub fn listing(&self) -> Spaces {
        let mut spaces = self.spaces();
        for space in &mut spaces.spaces {
            each_kind_mut(&mut space.root, &mut |kind| {
                if let PaneKind::Agent { session, state, .. } = kind {
                    *state = self.session(session).ok().map(|s| s.state());
                }
            });
        }
        spaces
    }

    /// Write the saved state — the layout, the id counter and the lead's conversation. The one
    /// place that touches the file, so a lead that changes conversation and a pane that moves are
    /// written the same way.
    pub(crate) fn save_state(&self, spaces: &Spaces) {
        let Some(path) = self.state_file.lock().unwrap().clone() else {
            return;
        };
        let lead = self.lead_acp_session.lock().unwrap().clone();
        let extra = self.saved_extra.lock().unwrap().clone();
        match save(
            &path,
            spaces,
            self.next_id.load(Ordering::SeqCst),
            lead,
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
    fn changed(&self, spaces: &Spaces) {
        self.save_state(spaces);
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
            PaneKind::Agent { session, .. } => self.end_session(session),
            // A browser pane holds nothing of the daemon's: no folder, no data directory, no
            // watch, no refresh, no terminal. The client's web view goes with the pane.
            PaneKind::Browser { .. } | PaneKind::Empty { .. } | PaneKind::Unknown(_) => {}
        }
    }

    /// Who made `pane`.
    pub fn pane_created_by(&self, pane: &str) -> Result<CreatedBy, String> {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .spaces
            .iter()
            .find_map(|s| find_created_by(&s.root, pane))
            .ok_or_else(|| format!("no pane {pane}"))
    }

    /// Every pane in `space` with who made it.
    pub fn space_creators(&self, space: &str) -> Result<Vec<(String, CreatedBy)>, String> {
        let spaces = self.spaces.lock().unwrap();
        let held = &spaces.spaces[spaces.index(space)?];
        Ok(creators(&held.root)
            .into_iter()
            .map(|(pane, by)| (pane.to_string(), by))
            .collect())
    }

    /// Make a space for `cwd`, with one empty pane made by `by`, and show it.
    pub fn space_create(
        &self,
        cwd: String,
        name: Option<String>,
        by: CreatedBy,
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
                created_by: by,
                kind: PaneKind::empty(),
            },
            focus: pane,
        };
        let mut spaces = self.spaces.lock().unwrap();
        spaces.spaces.push(space.clone());
        spaces.active = Some(id.clone());
        tracing::info!(space = %id, %name, %cwd, ?by, "space created");
        self.changed(&spaces);
        Ok(space)
    }

    /// Close a space and every pane in it. With `approved`, the person's panes the caller asked
    /// about: a pane of the person's outside it refuses the close, so nothing they never answered
    /// for goes with the space.
    pub fn space_close(&self, id: &str, approved: Option<&[String]>) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.index(id)?;
        if let Some(approved) = approved {
            let unasked: Vec<&str> = creators(&spaces.spaces[index].root)
                .into_iter()
                .filter(|(pane, by)| {
                    *by == CreatedBy::Person && !approved.iter().any(|a| a == pane)
                })
                .map(|(pane, _)| pane)
                .collect();
            if !unasked.is_empty() {
                return Err(format!(
                    "{id} now holds the person's {}, which they were not asked about; nothing was closed",
                    unasked.join(", ")
                ));
            }
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

    /// Split `pane`, putting a new empty pane made by `by` on the `dir` side. With `focus`, the
    /// new pane takes the focus and its space shows; without, the person's view stays as it was.
    pub fn pane_split(
        &self,
        pane: &str,
        dir: Dir,
        by: CreatedBy,
        focus: bool,
    ) -> Result<String, String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let (split_id, new_id) = (self.new_id('x'), self.new_id('p'));
        let space = &mut spaces.spaces[index];
        split(&mut space.root, pane, dir, &split_id, &new_id, by);
        let space_id = space.id.clone();
        if focus {
            space.focus = new_id.clone();
            spaces.active = Some(space_id.clone());
        }
        tracing::info!(space = %space_id, %pane, ?dir, split = %split_id, new = %new_id, ?by, focus, "pane split");
        self.changed(&spaces);
        Ok(new_id)
    }

    /// The terminal a pane shows.
    pub(crate) fn pane_term(&self, pane: &str) -> Result<Arc<Term>, String> {
        let spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        match find_pane(&spaces.spaces[index].root, pane) {
            Some(PaneKind::Terminal { term, .. }) => self
                .term(term)
                .ok_or_else(|| format!("{pane}'s terminal {term} is gone")),
            Some(other) => {
                let kind = serde_json::to_value(other)
                    .ok()
                    .and_then(|v| v["kind"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                Err(format!("{pane} is not a terminal pane; its kind is {kind}"))
            }
            None => Err(format!("no pane {pane}")),
        }
    }

    /// Close a pane; closing a space's last pane closes the space.
    pub fn pane_close(&self, pane: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let space = &mut spaces.spaces[index];
        let root = std::mem::replace(
            &mut space.root,
            Node::Pane {
                id: String::new(),
                created_by: CreatedBy::Person,
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
                let space = spaces.remove(index);
                tracing::info!(space = %space.id, %pane, "pane closed; it was the last, so space closed");
            }
        }
        self.changed(&spaces);
        Ok(())
    }

    /// Change what a pane shows. A terminal or an agent starts in the space's directory; a web
    /// pane shows the artifact `artifact` names in `scope` (or, with no scope, in the first scope
    /// that has a folder by that name), which only it takes; a browser pane opens `url`, or the
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
                let artifact = self.artifact_ref(scope, &name, &cwd)?;
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
                let cwd = {
                    let spaces = self.spaces.lock().unwrap();
                    spaces.spaces[spaces.holding(pane)?].cwd.clone()
                };
                let shell = self.shell_in(&cwd);
                PaneKind::Terminal {
                    term: self
                        .open_term(cwd, shell, PANE_COLS, PANE_ROWS, Owner::Pane)
                        .await?
                        .id
                        .clone(),
                    extra: Extra::default(),
                }
            }
        };
        self.install(pane, new, watching)
    }

    /// Put `new` in `pane`, ending what it showed, and return what it shows now. `watching` is a
    /// web pane's artifact and its space's directory, for the watch the swap sets.
    fn install(
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
        self.sync_acp_sessions(&mut spaces);
        let now = find_pane(&spaces.spaces[spaces.holding(pane)?].root, pane)
            .cloned()
            .ok_or_else(|| format!("no pane {pane}"))?;
        tracing::info!(%pane, kind = ?now, "pane kind");
        self.changed(&spaces);
        Ok(now)
    }

    /// Start a terminal or an agent in `pane` of `space` (or, with no pane, in a new pane split
    /// right of the space's focused one, made by `by` and focused only with `focus`), whatever the
    /// pane showed going. An agent gets
    /// `message` as its first message and the call waits up to `AgentTiming::start_wait` for it to be
    /// ready: an error when it gave up, otherwise its state, `starting` or `failed` when the
    /// wait ran out first. A message is only for an agent.
    pub async fn pane_start(
        &self,
        space: &str,
        pane: Option<String>,
        kind: StartKind,
        message: Option<String>,
        by: CreatedBy,
        focus: bool,
    ) -> Result<Started, String> {
        if kind == StartKind::Terminal && message.is_some() {
            return Err("a first message is only for an agent pane".into());
        }
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
            Err(focused) => (self.pane_split(&focused, Dir::Right, by, focus)?, true),
        };
        let started = match kind {
            StartKind::Terminal => self
                .pane_set_kind(&pane, NewPaneKind::Terminal, None, None, None)
                .await
                .map(|_| None)
                .map_err(|e| (e, false)),
            StartKind::Agent => self.start_agent(space, &pane, message).await.map(Some),
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
                    let _ = self.pane_close(&pane);
                }
                Err(e)
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
        message: Option<String>,
    ) -> Result<(String, SessionState), (String, bool)> {
        let cwd = {
            let spaces = self.spaces.lock().unwrap();
            spaces.spaces[spaces.holding(pane).map_err(|e| (e, false))?]
                .cwd
                .clone()
        };
        let session = self.new_id('w');
        self.spawn_agent(&session, pane, &cwd, false, None)
            .map_err(|e| (e, false))?;
        let new = PaneKind::Agent {
            session: session.clone(),
            acp_session: None,
            state: None,
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
                        "agent {session} in pane {pane} could not start: {e}. Its pane stays open showing the error; close_pane removes it"
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

    /// Send a browser pane to the page a person means by `text`, typed into its address field:
    /// read by [`wheel_proto::page_typed`] against this daemon's home, then stored as any page is.
    /// Gives back the page.
    pub fn pane_browser_open(&self, pane: &str, text: &str) -> Result<String, String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let url = wheel_proto::page_typed(text, &home)?;
        tracing::info!(%pane, typed = %text, %url, "browser pane opening typed text");
        self.pane_browser_url(pane, &url).map(|_| url)
    }

    /// Show artifact `name` in `pane` — or, with none, in a new pane split right of the focused
    /// pane of `space` (the active space when none is named), made by `by` and focused only with
    /// `focus`, which is how starting an agent already behaves. Gives back the pane and what it
    /// now shows.
    ///
    /// The artifact is named, never pathed, and resolved by [`AppState::pane_set_kind`] like any
    /// other caller's: a name that is not one plain component, or that no scope has, is refused
    /// there. A pane made only for this artifact is closed again when that refusal comes, and the
    /// focus and the showing space go back to what they were — splitting moves both — so a name
    /// the lead has to correct leaves the person looking at what they were looking at.
    pub async fn open_artifact(
        &self,
        name: &str,
        scope: Option<ArtifactScope>,
        space: Option<&str>,
        pane: Option<String>,
        by: CreatedBy,
        focus: bool,
    ) -> Result<(String, PaneKind), String> {
        let (was, chosen) = {
            let spaces = self.spaces.lock().unwrap();
            // What the person is looking at, to put back if the open is refused. A showing space
            // and a focused pane are one pair: the focus kept here is that space's own.
            let was = spaces
                .active
                .as_ref()
                .and_then(|id| spaces.spaces.iter().find(|s| &s.id == id))
                .map(|s| (s.id.clone(), s.focus.clone()));
            let chosen = match pane {
                Some(pane) => {
                    let held = spaces.holding(&pane)?;
                    if let Some(space) = space {
                        if held != spaces.index(space)? {
                            return Err(format!("pane {pane} is not in space {space}"));
                        }
                    }
                    Ok(pane)
                }
                None => {
                    let id = space.or(spaces.active.as_deref()).ok_or(
                        "no space to open an artifact in; make one with create_space, or name one",
                    )?;
                    Err(spaces.spaces[spaces.index(id)?].focus.clone())
                }
            };
            (was, chosen)
        };
        let (pane, split) = match chosen {
            Ok(pane) => (pane, false),
            Err(focused) => (self.pane_split(&focused, Dir::Right, by, focus)?, true),
        };
        match self
            .pane_set_kind(&pane, NewPaneKind::Web, Some(name.to_string()), scope, None)
            .await
        {
            Ok(kind) => {
                tracing::info!(%pane, %name, scope = scope.map(|s| s.name()).unwrap_or("-"), split, "artifact opened");
                Ok((pane, kind))
            }
            Err(e) => {
                // A pane made only for this artifact goes with the refusal, and the split's own
                // two effects — the new pane focused, its space shown — go with it: closing the
                // pane puts the focus on a neighbour, which is not necessarily the one it moved
                // off, and leaves whatever space was shown before another space showing.
                if split {
                    let _ = self.pane_close(&pane);
                    if let (true, Some((space, was_focus))) = (focus, &was) {
                        let _ = self.focus(Some(space), Some(was_focus));
                    }
                }
                Err(e)
            }
        }
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
            lead,
            extra,
        } = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("{} is not a saved layout: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved {
                layout: Spaces::default(),
                next_id: 1,
                lead: None,
                extra: Default::default(),
            },
            Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
        };
        self.next_id.fetch_max(next_id, Ordering::SeqCst);
        for key in extra.keys() {
            tracing::warn!(%key, "unknown saved key; kept as is");
        }
        *self.saved_extra.lock().unwrap() = extra;
        *self.lead_acp_session.lock().unwrap() = lead.map(|lead| lead.acp_session);
        let (mut respawned, mut agents) = (0, 0);
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
                let opened = self
                    .open_term(
                        space.cwd.clone(),
                        self.shell_in(&space.cwd),
                        PANE_COLS,
                        PANE_ROWS,
                        Owner::Pane,
                    )
                    .await;
                let Some(slot) = find_pane_mut(&mut space.root, &pane) else {
                    continue;
                };
                match opened {
                    // Only the shell is new; the pane keeps the rest of what was saved.
                    Ok(term) => {
                        if let PaneKind::Terminal { term: id, .. } = slot {
                            respawned += 1;
                            *id = term.id.clone();
                        }
                    }
                    Err(e) => {
                        tracing::warn!(space = %space.id, %pane, error = %e, "could not restart the pane's shell; it is empty now");
                        *slot = PaneKind::empty();
                    }
                }
            }
            // Agents do not outlive the daemon either: each agent pane's agent starts again under
            // its saved name, and loads its saved conversation where the agent can.
            for (pane, session, acp_session) in agent_panes(&space.root) {
                match self.spawn_agent(&session, &pane, &space.cwd, true, acp_session) {
                    Ok(()) => agents += 1,
                    // Kept as saved, so a later start with a lead runs it again.
                    Err(e) => {
                        tracing::warn!(space = %space.id, %pane, %session, error = %e, "could not restart the pane's agent")
                    }
                }
            }
        }
        self.sweep_pane_data(&named_web.iter().map(String::as_str).collect());
        tracing::info!(path = %path.display(), spaces = loaded.spaces.len(), respawned, agents, "spaces restored");
        *self.state_file.lock().unwrap() = Some(path);
        let mut spaces = self.spaces.lock().unwrap();
        *spaces = loaded;
        self.sync_acp_sessions(&mut spaces);
        self.changed(&spaces);
        Ok(())
    }

    /// An agent pane's agent gave its session a new conversation id: save it in the pane.
    pub(crate) fn acp_sessions_changed(&self) {
        let mut spaces = self.spaces.lock().unwrap();
        if self.sync_acp_sessions(&mut spaces) {
            self.changed(&spaces);
        }
    }

    /// The lead's agent gave it a new conversation id: save it in the state file. The lead has no
    /// pane to hold it, so it rides beside the layout, written by the same save.
    pub(crate) fn lead_acp_session_changed(&self) {
        let Ok(lead) = self.session(LEAD) else { return };
        *self.lead_acp_session.lock().unwrap() = lead.acp_session();
        let spaces = self.spaces.lock().unwrap();
        self.save_state(&spaces);
    }

    /// Copy each running agent session's conversation id into its agent pane; true when one changed.
    /// Agents run while the spaces lock is free, so a pane takes its session's id here, under the
    /// lock, whichever of the two comes first.
    fn sync_acp_sessions(&self, spaces: &mut Spaces) -> bool {
        let mut changed = false;
        for space in &mut spaces.spaces {
            each_kind_mut(&mut space.root, &mut |kind| {
                if let PaneKind::Agent {
                    session,
                    acp_session,
                    ..
                } = kind
                {
                    let running = self.session(session).ok().and_then(|s| s.acp_session());
                    if let Some(id) = running.filter(|id| acp_session.as_ref() != Some(id)) {
                        tracing::info!(%session, acp_session = %id, "agent pane saves its conversation");
                        *acp_session = Some(id);
                        changed = true;
                    }
                }
            });
        }
        changed
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

/// Each agent pane with its session name and saved conversation id.
pub(crate) fn agent_panes(node: &Node) -> Vec<(String, String, Option<String>)> {
    panes(node)
        .into_iter()
        .filter_map(|(id, kind)| match kind {
            PaneKind::Agent {
                session,
                acp_session,
                ..
            } => Some((id.to_string(), session.clone(), acp_session.clone())),
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
    lead: Option<String>,
    extra: serde_json::Map<String, serde_json::Value>,
) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let saved = Saved {
        layout: layout.clone(),
        next_id,
        lead: lead.map(|acp_session| LeadState { acp_session }),
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
            created_by: CreatedBy::Person,
            kind: PaneKind::empty(),
        }
    }

    #[test]
    fn split_puts_the_new_pane_on_the_named_side() {
        let mut root = leaf("p1");
        assert!(split(
            &mut root,
            "p1",
            Dir::Left,
            "x2",
            "p3",
            CreatedBy::Person
        ));
        let Node::Split {
            axis, ratio, a, b, ..
        } = &root
        else {
            panic!("split")
        };
        assert_eq!((*axis, *ratio), (Axis::Horizontal, 0.5));
        assert_eq!((first_pane(a), first_pane(b)), ("p3", "p1"));
        assert!(
            !split(&mut root, "p9", Dir::Down, "x4", "p5", CreatedBy::Person),
            "unknown pane"
        );
    }

    #[test]
    fn removing_a_pane_lets_its_sibling_take_the_split() {
        // p1 | (p3 / p5)
        let mut root = leaf("p1");
        split(&mut root, "p1", Dir::Right, "x2", "p3", CreatedBy::Person);
        split(&mut root, "p3", Dir::Down, "x4", "p5", CreatedBy::Person);
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
        split(&mut root, "p1", Dir::Right, "x2", "p3", CreatedBy::Person);
        split(&mut root, "p3", Dir::Down, "x4", "p5", CreatedBy::Person);
        assert_eq!(neighbour(root.clone(), "p5"), "p3");
        assert_eq!(neighbour(root.clone(), "p3"), "p5");
        // Closing the left side lands on the right side's top-left pane.
        assert_eq!(neighbour(root, "p1"), "p3");

        // (p1 / p3) | p5: closing p5 lands on the left side's bottom-right pane.
        let mut root = leaf("p1");
        split(&mut root, "p1", Dir::Right, "x2", "p5", CreatedBy::Person);
        split(&mut root, "p1", Dir::Down, "x4", "p3", CreatedBy::Person);
        assert_eq!(neighbour(root, "p5"), "p3");
    }

    /// The lead's conversation rides in the saved file beside the layout, and a file written
    /// before it existed still loads — it just has no conversation to come back to.
    #[test]
    fn the_saved_file_carries_the_leads_conversation_and_reads_one_without_it() {
        let written = serde_json::to_string(&Saved {
            layout: Spaces::default(),
            next_id: 7,
            lead: Some(LeadState {
                acp_session: "abc-123".into(),
            }),
            extra: Default::default(),
        })
        .unwrap();
        let back: Saved = serde_json::from_str(&written).unwrap();
        assert_eq!(back.next_id, 7);
        assert_eq!(back.lead.map(|l| l.acp_session).as_deref(), Some("abc-123"));

        let older: Saved =
            serde_json::from_str(r#"{"spaces":[],"active":null,"next_id":3}"#).unwrap();
        assert_eq!(older.next_id, 3);
        assert!(older.lead.is_none());

        // A lead that has no conversation yet writes no key at all, so the file keeps its shape.
        let empty = serde_json::to_string(&Saved {
            layout: Spaces::default(),
            next_id: 1,
            lead: None,
            extra: Default::default(),
        })
        .unwrap();
        assert!(!empty.contains("lead"), "{empty}");
    }

    /// A top-level key a newer build added to the saved file survives a load and save by this
    /// build, so a downgrade does not drop what the newer build keeps there.
    #[test]
    fn an_unknown_top_level_key_survives_a_load_and_save() {
        let newer =
            r#"{"spaces":[],"active":null,"next_id":3,"lead2":{"acp_session":"zzz"},"schema":9}"#;
        let loaded: Saved = serde_json::from_str(newer).unwrap();
        assert_eq!(loaded.extra.len(), 2, "{:?}", loaded.extra);
        let back: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&loaded).unwrap()).unwrap();
        let want: serde_json::Value = serde_json::from_str(newer).unwrap();
        assert_eq!(back, want);
    }
}
