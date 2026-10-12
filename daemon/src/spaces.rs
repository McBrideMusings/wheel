//! Spaces: one per project, each a split tree of panes (after Ghostty's SplitTree). The daemon
//! owns the layout (ADR-0001), saves it after every change, and broadcasts it to every client.
//! Every caller — a client op, the CLI, the lead's tools — changes it through the `AppState`
//! methods here.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use wheel_proto::{Axis, Dir, Extra, NewPaneKind, Node, PaneKind, ServerMsg, SpaceInfo};

use crate::server::AppState;

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

/// The saved file: the layout, and the id counter so a restarted daemon never hands out an id
/// it gave before — not a closed pane's, not a terminal's a client may still remember.
#[derive(Serialize, Deserialize)]
struct Saved {
    #[serde(flatten)]
    layout: Spaces,
    next_id: u64,
}

pub fn find_pane<'a>(node: &'a Node, pane: &str) -> Option<&'a PaneKind> {
    match node {
        Node::Pane { id, kind } => (id == pane).then_some(kind),
        Node::Split { a, b, .. } => find_pane(a, pane).or_else(|| find_pane(b, pane)),
    }
}

fn find_pane_mut<'a>(node: &'a mut Node, pane: &str) -> Option<&'a mut PaneKind> {
    match node {
        Node::Pane { id, kind } => (id == pane).then_some(kind),
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

/// Every pane's kind, top-left first.
pub fn panes(node: &Node) -> Vec<&PaneKind> {
    match node {
        Node::Pane { kind, .. } => vec![kind],
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
    match node {
        Node::Pane { id, .. } if id == pane => {
            let old = std::mem::replace(
                node,
                Node::Pane {
                    id: String::new(),
                    kind: PaneKind::empty(),
                },
            );
            let new = Node::Pane {
                id: new_id.to_string(),
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
            split(a, pane, dir, split_id, new_id) || split(b, pane, dir, split_id, new_id)
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
        Node::Pane { id, kind } if id == pane => Removal::Last(kind),
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

impl AppState {
    fn new_id(&self, prefix: char) -> String {
        format!("{prefix}{}", self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    pub fn spaces(&self) -> Spaces {
        self.spaces.lock().unwrap().clone()
    }

    /// Save and broadcast the layout. Called with the spaces lock held, so clients see changes in
    /// the order they happened.
    fn changed(&self, spaces: &Spaces) {
        if let Some(path) = self.state_file.lock().unwrap().as_ref() {
            match save(path, spaces, self.next_id.load(Ordering::SeqCst)) {
                Ok(()) => {
                    tracing::debug!(path = %path.display(), spaces = spaces.spaces.len(), "spaces saved")
                }
                Err(e) => tracing::error!(path = %path.display(), error = %e, "spaces save failed"),
            }
        }
        let _ = self.global.send(ServerMsg::Spaces {
            spaces: spaces.spaces.clone(),
            active: spaces.active.clone(),
        });
    }

    /// Kill the terminal a closed or replaced pane showed.
    fn drop_content(&self, kind: &PaneKind) {
        if let PaneKind::Terminal { term, .. } = kind {
            if let Some(t) = self.terms.lock().unwrap().remove(term) {
                tracing::info!(term = %term, "pty killed");
                t.kill();
            }
        }
    }

    pub fn space_create(&self, cwd: String, name: Option<String>) -> Result<SpaceInfo, String> {
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
                kind: PaneKind::empty(),
            },
            focus: pane,
        };
        let mut spaces = self.spaces.lock().unwrap();
        spaces.spaces.push(space.clone());
        spaces.active = Some(id.clone());
        tracing::info!(space = %id, %name, %cwd, "space created");
        self.changed(&spaces);
        Ok(space)
    }

    pub fn space_close(&self, id: &str) -> Result<(), String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.index(id)?;
        let space = spaces.remove(index);
        for kind in panes(&space.root) {
            self.drop_content(kind);
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

    pub fn pane_split(&self, pane: &str, dir: Dir) -> Result<String, String> {
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let (split_id, new_id) = (self.new_id('x'), self.new_id('p'));
        let space = &mut spaces.spaces[index];
        split(&mut space.root, pane, dir, &split_id, &new_id);
        space.focus = new_id.clone();
        let space_id = space.id.clone();
        tracing::info!(space = %space_id, %pane, ?dir, split = %split_id, new = %new_id, "pane split");
        // Splitting focuses the new pane, so its space is the one showing.
        spaces.active = Some(space_id);
        self.changed(&spaces);
        Ok(new_id)
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
                self.drop_content(&kind);
                if space.focus == pane {
                    space.focus = neighbour;
                }
                space.root = rest;
                tracing::info!(space = %space.id, %pane, focus = %space.focus, "pane closed");
            }
            Removal::Last(kind) => {
                self.drop_content(&kind);
                let space = spaces.remove(index);
                tracing::info!(space = %space.id, %pane, "pane closed; it was the last, so space closed");
            }
        }
        self.changed(&spaces);
        Ok(())
    }

    /// Change what a pane shows. A terminal starts in the space's directory; whatever the pane
    /// showed before goes.
    pub async fn pane_set_kind(&self, pane: &str, kind: NewPaneKind) -> Result<(), String> {
        let new = match kind {
            NewPaneKind::Empty => PaneKind::empty(),
            NewPaneKind::Terminal => {
                let cwd = {
                    let spaces = self.spaces.lock().unwrap();
                    spaces.spaces[spaces.holding(pane)?].cwd.clone()
                };
                PaneKind::Terminal {
                    term: self.open_term(cwd, PANE_COLS, PANE_ROWS).await?.id.clone(),
                    extra: Extra::default(),
                }
            }
        };
        let mut spaces = self.spaces.lock().unwrap();
        // The pane may have closed while the shell started.
        let Some(slot) = spaces
            .spaces
            .iter_mut()
            .find_map(|s| find_pane_mut(&mut s.root, pane))
        else {
            self.drop_content(&new);
            return Err(format!("no pane {pane}"));
        };
        let old = std::mem::replace(slot, new);
        self.drop_content(&old);
        tracing::info!(%pane, kind = ?slot, "pane kind");
        self.changed(&spaces);
        Ok(())
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
        let Saved {
            layout: mut loaded,
            next_id,
        } = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("{} is not a saved layout: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved {
                layout: Spaces::default(),
                next_id: 1,
            },
            Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
        };
        self.next_id.fetch_max(next_id, Ordering::SeqCst);
        let mut respawned = 0;
        for space in &mut loaded.spaces {
            for (pane, kind) in unknown_panes(&space.root) {
                tracing::warn!(space = %space.id, %pane, %kind, "unknown pane kind; kept as is");
            }
            let mut ids = Vec::new();
            collect_terminal_panes(&space.root, &mut ids);
            for pane in ids {
                let opened = self
                    .open_term(space.cwd.clone(), PANE_COLS, PANE_ROWS)
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
        }
        tracing::info!(path = %path.display(), spaces = loaded.spaces.len(), respawned, "spaces restored");
        *self.state_file.lock().unwrap() = Some(path);
        let mut spaces = self.spaces.lock().unwrap();
        *spaces = loaded;
        self.changed(&spaces);
        Ok(())
    }
}

/// Each pane of a kind this build does not know, with that kind's name.
fn unknown_panes(node: &Node) -> Vec<(&str, &str)> {
    match node {
        Node::Pane {
            id,
            kind: PaneKind::Unknown(unknown),
        } => vec![(id, unknown.kind())],
        Node::Pane { .. } => vec![],
        Node::Split { a, b, .. } => {
            let mut all = unknown_panes(a);
            all.extend(unknown_panes(b));
            all
        }
    }
}

fn collect_terminal_panes(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Pane {
            id,
            kind: PaneKind::Terminal { .. },
        } => out.push(id.clone()),
        Node::Pane { .. } => {}
        Node::Split { a, b, .. } => {
            collect_terminal_panes(a, out);
            collect_terminal_panes(b, out);
        }
    }
}

/// Write the layout through a temporary file, so a crash mid-write leaves the old one.
fn save(path: &Path, layout: &Spaces, next_id: u64) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let saved = Saved {
        layout: layout.clone(),
        next_id,
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
}
