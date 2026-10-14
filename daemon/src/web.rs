//! Web panes: each shows an artifact, a folder of HTML the lead or an agent pane writes, named by a
//! scope and a name rather than by a path. wheeld owns the three roots a scope can mean, resolves
//! the folder itself, serves it at `/artifacts/<pane>/` behind the daemon token, and watches it: a
//! change to any file in it sends every client `pane_reload`.
//!
//! A pane also gets a data directory of its own, under the daemon's instance directory and keyed by
//! the pane, served under the reserved `data/` prefix of the pane's URL. So the artifact's own files
//! stay read-only static content, and one shared artifact open in two projects reads and writes two
//! separate directories. The pane watches both, and both send the same `pane_reload`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wheel_proto::{
    plain_name, ArtifactRef, ArtifactScope, Node, PaneKind, ServerMsg, WatchInfo, WatchRole,
    SCOPE_ORDER,
};

use crate::server::AppState;

/// Changes this close together send one `pane_reload`: an editor's save is often several writes.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// The cookie a page's own requests (its stylesheets, scripts, images) carry the token in, since
/// only the URL a client opens has `?token=`.
const COOKIE: &str = "wheel_token";

/// The one URL prefix a web pane serves from its own data directory instead of from the artifact's
/// folder. Reserved both ways: [`crate::artifacts`] reports an artifact folder holding a directory
/// of this name as a problem, because no request could ever reach it.
pub(crate) const DATA: &str = "data";

/// One of a web pane's two directory watches. Dropping it stops the watch.
pub(crate) struct Watch {
    artifact: ArtifactRef,
    role: WatchRole,
    dir: String,
    /// Where the watch is now; the task moves it as the folder comes and goes. None while it
    /// could not be set, until a retry sets it.
    at: Arc<Mutex<Option<Target>>>,
    task: JoinHandle<()>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        // The task owns the watcher, so ending it stops the watch.
        self.task.abort();
    }
}

/// The path a watch is on.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    /// The real path (`/private/tmp`, not `/tmp`): events name real paths.
    path: PathBuf,
    /// `path` is the nearest existing folder above a missing one, watched for it to appear.
    waiting: bool,
}

impl Target {
    /// Where a watch of `dir` belongs: on `dir` when it exists, else on the nearest folder above
    /// it that does. None when no folder above it exists.
    fn of(dir: &Path) -> Option<Target> {
        let (path, waiting) = if dir.is_dir() {
            (dir, false)
        } else {
            (dir.ancestors().skip(1).find(|a| a.is_dir())?, true)
        };
        Some(Target {
            path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
            waiting,
        })
    }

    /// Whether a change at `path` is one a page in the folder could show.
    fn shows(&self, path: &Path) -> bool {
        !self.waiting && path.starts_with(&self.path) && !hidden(&self.path, path)
    }
}

/// Move `watcher` off `from` and onto where a watch of `dir` belongs now.
fn arm<W: Watcher>(
    watcher: &mut W,
    from: Option<&Target>,
    pane: &str,
    role: WatchRole,
    dir: &str,
) -> Result<Target, String> {
    if let Some(from) = from {
        // A removed folder may have taken its watch with it already.
        let _ = watcher.unwatch(&from.path);
    }
    // The folder can appear or go between choosing the path and watching it, and the event that
    // says so lands before the watch does: keep a watch only once its path still belongs.
    for _ in 0..8 {
        let to = Target::of(Path::new(dir)).ok_or("no folder above it exists")?;
        // Above a missing folder only its own entries matter, never the whole tree below.
        let mode = if to.waiting {
            RecursiveMode::NonRecursive
        } else {
            RecursiveMode::Recursive
        };
        watcher
            .watch(&to.path, mode)
            .map_err(|e| format!("{}: {e}", to.path.display()))?;
        if Target::of(Path::new(dir)).as_ref() == Some(&to) {
            tracing::info!(%pane, ?role, %dir, watching = %to.path.display(), waiting = to.waiting, "watching web pane");
            return Ok(to);
        }
        let _ = watcher.unwatch(&to.path);
    }
    Err("the folder kept appearing and going".into())
}

/// How long a watch that could not be set waits before trying again.
const RETRY: Duration = Duration::from_secs(1);

impl AppState {
    /// The folder `artifact` names for a space whose directory is `cwd`. The one place a root is
    /// computed: every caller — the CLI, the lead's tool, a client op, layout restore and serving
    /// — comes through here, so nothing else can aim a pane at a folder of its own choosing.
    pub(crate) fn artifact_dir(&self, artifact: &ArtifactRef, cwd: &str) -> PathBuf {
        let (_, root) = artifact_root(artifact.scope(), cwd, &self.instance_dir);
        root.join(artifact.name())
    }

    /// The folder `artifact` names, but only when it really sits inside the directory its scope
    /// belongs to — the rule serving applies with [`inside`] and [`resolve`], and creation with
    /// [`AppState::artifact_dir_to_make`]. A folder a symlink moved out, which a cloned repo can
    /// ship, is refused, so a manifest is never read — nor its command run — from outside the
    /// scope whose name it would carry. None for a folder that is not there at all.
    pub(crate) fn artifact_dir_inside(&self, artifact: &ArtifactRef, cwd: &str) -> Option<PathBuf> {
        let (base, root) = artifact_root(artifact.scope(), cwd, &self.instance_dir);
        let dir = root.join(artifact.name());
        (inside(&root, &base) && inside(&dir, &root)).then_some(dir)
    }

    /// Make `scope`'s root for a space whose directory is `cwd`, and give back the folder artifact
    /// `name` takes under it — or refuse, when the root leads out of the directory it belongs to,
    /// which a cloned repo shipping `.wheel/artifacts` as a symlink does. Serving applies the same
    /// rule with [`inside`], so what a pane will not serve is also what nothing may write.
    pub(crate) fn artifact_dir_to_make(
        &self,
        artifact: &ArtifactRef,
        cwd: &str,
    ) -> Result<PathBuf, String> {
        let (base, root) = artifact_root(artifact.scope(), cwd, &self.instance_dir);
        let out = || {
            Err(format!(
                "{} leads out of {}, so nothing is written there",
                root.display(),
                base.display()
            ))
        };
        // Checked before the first directory is made, not after: `.wheel` can itself be a symlink
        // out of the space, and `create_dir_all` follows it, so a check that came afterwards would
        // leave an empty `artifacts/` wherever it points. What a new folder is made under is the
        // nearest one that already exists, so that is what has to be inside the base.
        let nearest = root.ancestors().find(|a| a.exists()).unwrap_or(&root);
        if !inside(nearest, &base) {
            return out();
        }
        std::fs::create_dir_all(&root).map_err(|e| format!("make {}: {e}", root.display()))?;
        // Making it cannot have moved it, but a symlink swapped in between the two lines is held to
        // the same rule rather than trusted for having arrived late.
        if !inside(&root, &base) {
            return out();
        }
        Ok(root.join(artifact.name()))
    }

    /// The names of every folder in `scope`'s root, sorted. A root that is not there yet holds no
    /// artifacts, which is not an error: two of the three roots appear with the first artifact.
    pub(crate) fn artifact_names(&self, scope: ArtifactScope, cwd: &str) -> Vec<String> {
        let (_, root) = artifact_root(scope, cwd, &self.instance_dir);
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            // A folder symlinked out of the root is not listed, because it is not served either:
            // listing it would read a manifest from outside the scope and name it as the scope's.
            .filter(|e| e.path().is_dir() && inside(&e.path(), &root))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            // A folder whose name no reference can hold is not an artifact: no pane could name it.
            .filter(|name| ArtifactRef::new(scope, name).is_ok())
            .collect();
        names.sort();
        names
    }

    /// The reference `name` means for a space whose directory is `cwd`: in `scope` when a caller
    /// pinned one, whether or not the folder is there yet, else the first scope of [`SCOPE_ORDER`]
    /// whose folder exists. A name that is nowhere is refused rather than guessed at, so a pane
    /// never stores a scope the caller did not mean.
    pub(crate) fn artifact_ref(
        &self,
        scope: Option<ArtifactScope>,
        name: &str,
        cwd: &str,
    ) -> Result<ArtifactRef, String> {
        if let Some(scope) = scope {
            return ArtifactRef::new(scope, name);
        }
        let mut looked = Vec::new();
        for scope in SCOPE_ORDER {
            let artifact = ArtifactRef::new(scope, name)?;
            let dir = self.artifact_dir(&artifact, cwd);
            if dir.is_dir() {
                return Ok(artifact);
            }
            looked.push(dir.display().to_string());
        }
        Err(format!(
            "no artifact {name}: nothing at {}. Give --scope to name one that does not exist yet",
            looked.join(", ")
        ))
    }

    /// Where every web pane's own data directory sits: under the instance directory, beside the
    /// token and the saved layout. Never inside an artifact's folder and never in a project's
    /// working tree, so nothing a pane is given dirties a repo or is shared between panes.
    fn panes_root(&self) -> PathBuf {
        self.instance_dir.join("panes")
    }

    /// Web pane `pane`'s own data directory, `panes/<pane>/` under the instance directory. Keyed by
    /// the pane and never by the artifact, so one shared artifact open in two projects gets two
    /// directories. Refused unless the pane's id is one plain name — the rule an artifact's name
    /// goes through too — because a saved layout a person can edit names the pane, and this
    /// directory is made and removed as the pane comes and goes.
    pub(crate) fn pane_data_dir(&self, pane: &str) -> Result<PathBuf, String> {
        plain_name("a pane id", pane)?;
        Ok(self.panes_root().join(pane))
    }

    /// Show artifact `artifact` in web pane `pane`, whose space's directory is `cwd`: make the
    /// pane's data directory, watch both of the pane's roots from now on, replacing any earlier
    /// watch of that pane, and start refreshing if the artifact's manifest declares a command. A
    /// change under either directory sends `pane_reload`. While a directory is missing its watch
    /// waits on the folder above it, and moves back onto it once it appears, which also reloads; a
    /// watch that cannot be set is retried every second. Call from inside the tokio runtime. The
    /// pane serves whatever is there throughout.
    pub(crate) fn show_artifact(&self, pane: &str, artifact: &ArtifactRef, cwd: &str) {
        let mut watches: Vec<Watch> = Vec::new();
        watches.extend(self.spawn_watch(
            pane,
            artifact,
            WatchRole::Artifact,
            &self.artifact_dir(artifact, cwd),
        ));
        match self.pane_data_dir(pane) {
            Ok(dir) => {
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    tracing::warn!(%pane, dir = %dir.display(), error = %e, "could not make the pane's data directory; it serves nothing under data/");
                }
                watches.extend(self.spawn_watch(pane, artifact, WatchRole::Data, &dir));
                // A refresh writes there and nowhere else, so a pane with no data directory to
                // name gets no runner either rather than a command with nowhere to put its work.
                self.start_refresh(pane, artifact, cwd, dir);
            }
            Err(e) => {
                tracing::warn!(%pane, error = %e, "no data directory for this pane; it serves nothing under data/ and nothing refreshes it");
                self.stop_refresh(pane);
            }
        }
        self.watchers
            .lock()
            .unwrap()
            .insert(pane.to_string(), watches);
    }

    /// One watch of `dir` for pane `pane`, running until the [`Watch`] is dropped. None when the
    /// platform would not give the daemon a watcher at all, which no retry would change: the pane
    /// serves that directory's files throughout, it just will not reload on its own.
    fn spawn_watch(
        &self,
        pane: &str,
        artifact: &ArtifactRef,
        role: WatchRole,
        dir: &Path,
    ) -> Option<Watch> {
        let dir = dir.display().to_string();
        let dir = dir.as_str();
        let (tx, rx) = mpsc::unbounded_channel::<notify::Event>();
        let watching = pane.to_string();
        let watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    let _ = tx.send(event);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(pane = %watching, error = %e, "web pane watch error"),
            });
        match watcher {
            Ok(w) => Some(self.run_watch(w, rx, pane, artifact, role, dir)),
            Err(e) => {
                tracing::warn!(%pane, ?role, %dir, error = %e, "could not watch the web pane's folder; it will not reload");
                None
            }
        }
    }

    /// Set `watcher` on `dir` and keep it there, sending `pane_reload` for what `rx` receives
    /// from it. Generic over the watcher so a test can hand it one that refuses.
    fn run_watch<W: Watcher + Send + 'static>(
        &self,
        mut watcher: W,
        mut rx: mpsc::UnboundedReceiver<notify::Event>,
        pane: &str,
        artifact: &ArtifactRef,
        role: WatchRole,
        dir: &str,
    ) -> Watch {
        let first = arm(&mut watcher, None, pane, role, dir)
            .inspect_err(|e| tracing::warn!(%pane, ?role, %dir, error = %e, "could not watch the web pane's folder; retrying every second"))
            .ok();
        let at = Arc::new(Mutex::new(first));
        let global = self.global.clone();
        let (id, folder, now) = (pane.to_string(), dir.to_string(), at.clone());
        let task = tokio::spawn(async move {
            loop {
                let Some(was) = now.lock().unwrap().clone() else {
                    tokio::time::sleep(RETRY).await;
                    let Ok(next) = arm(&mut watcher, None, &id, role, &folder) else {
                        continue;
                    };
                    while rx.try_recv().is_ok() {}
                    // Whatever changed while nothing watched is unknown, so the page reloads.
                    if !next.waiting {
                        tracing::info!(pane = %id, ?role, path = %folder, changes = 0, appeared = true, "pane reload");
                        let _ = global.send(ServerMsg::PaneReload { pane: id.clone() });
                    }
                    *now.lock().unwrap() = Some(next);
                    continue;
                };
                let Some(event) = rx.recv().await else {
                    return;
                };
                tokio::time::sleep(DEBOUNCE).await;
                let mut events = vec![event];
                while let Ok(more) = rx.try_recv() {
                    events.push(more);
                }
                // A hidden file is never served, so `.git` churn or an editor's swap file does not
                // reload the page; the rename onto the real name does.
                let shown = events
                    .iter()
                    .flat_map(|e| e.paths.iter())
                    .find(|p| was.shows(p))
                    .cloned();
                // A folder removed and made again within one debounce still exists, but a watch
                // on it may have died with the old one, so any event on the folder itself re-arms.
                let root_touched =
                    !was.waiting && events.iter().any(|e| e.paths.contains(&was.path));
                if root_touched || Target::of(Path::new(&folder)).as_ref() != Some(&was) {
                    let next = arm(&mut watcher, Some(&was), &id, role, &folder)
                        .inspect_err(|e| tracing::warn!(pane = %id, ?role, dir = %folder, error = %e, "could not watch the web pane's folder; retrying every second"))
                        .ok();
                    *now.lock().unwrap() = next;
                }
                let appeared =
                    was.waiting && now.lock().unwrap().as_ref().is_some_and(|t| !t.waiting);
                if shown.is_none() && !appeared {
                    continue;
                }
                let path = shown.unwrap_or_else(|| PathBuf::from(&folder));
                tracing::info!(pane = %id, ?role, path = %path.display(), changes = events.len(), appeared, "pane reload");
                let _ = global.send(ServerMsg::PaneReload { pane: id.clone() });
            }
        });
        Watch {
            artifact: artifact.clone(),
            role,
            dir: dir.to_string(),
            at,
            task,
        }
    }

    /// Stop showing web pane `pane`'s artifact: stop refreshing it, stop both its watches and
    /// remove its data directory. The directory belongs to the pane, so whatever a refresh left in
    /// it goes with the pane rather than being inherited by whatever the pane shows next.
    pub(crate) fn drop_artifact(&self, pane: &str) {
        // Before the directory goes: dropping the runner cancels every run of this pane in flight,
        // which kills its command, so nothing is still writing what is about to be removed.
        self.stop_refresh(pane);
        if self.watchers.lock().unwrap().remove(pane).is_some() {
            tracing::info!(%pane, "stopped watching web pane");
        }
        let Ok(dir) = self.pane_data_dir(pane) else {
            return;
        };
        // The caller holds the spaces lock, and a refresh may have filled the directory with a large
        // tree. Renaming is one step on the same filesystem, so the pane's directory is gone (and a
        // pane shown again makes a fresh one) before this returns; deleting the renamed tree
        // happens on its own thread, off the lock.
        static GONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = GONE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let gone = self.panes_root().join(format!(".gone-{nanos}-{n}"));
        match std::fs::rename(&dir, &gone) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!(%pane, dir = %dir.display(), error = %e, "could not remove the pane's data directory");
                return;
            }
        }
        let pane = pane.to_string();
        std::thread::spawn(move || match std::fs::remove_dir_all(&gone) {
            Ok(()) => {
                tracing::info!(%pane, dir = %gone.display(), "removed the pane's data directory")
            }
            Err(e) => {
                tracing::warn!(%pane, dir = %gone.display(), error = %e, "could not remove the pane's data directory")
            }
        });
    }

    /// Finish removals a stop interrupted: `drop_artifact` renames a pane's data directory to
    /// `.gone-*` and deletes it on a thread, which dies with the daemon. A pane id never starts
    /// with a dot, so nothing else lives under these names.
    pub(crate) fn sweep_removed_pane_dirs(&self) {
        let Ok(entries) = std::fs::read_dir(self.panes_root()) else {
            return;
        };
        let left: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".gone-"))
            .map(|e| e.path())
            .collect();
        if left.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            for dir in left {
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => {
                        tracing::info!(dir = %dir.display(), "removed a pane data directory left from the last run")
                    }
                    Err(e) => {
                        tracing::warn!(dir = %dir.display(), error = %e, "could not remove a pane data directory left from the last run")
                    }
                }
            }
        });
    }

    /// Remove every directory under `panes/` that no pane in `named` owns. A pane's directory goes
    /// when the pane does, but a crash between the two, or a deleted layout whose pane ids start
    /// again at p1, leaves one behind for the next pane with that id to inherit. Called once, on
    /// restore, with the web panes the loaded layout names.
    pub(crate) fn sweep_pane_data(&self, named: &std::collections::HashSet<&str>) {
        let root = self.panes_root();
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!(dir = %root.display(), error = %e, "could not list the pane data directories to sweep");
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| named.contains(n))
            {
                continue;
            }
            let removed = match entry.file_type() {
                Ok(t) if t.is_dir() => std::fs::remove_dir_all(&path),
                _ => std::fs::remove_file(&path),
            };
            match removed {
                Ok(()) => {
                    tracing::info!(dir = %path.display(), "swept a pane data directory no layout names")
                }
                Err(e) => {
                    tracing::warn!(dir = %path.display(), error = %e, "could not sweep a pane data directory no layout names")
                }
            }
        }
    }

    /// Every directory every web pane watches, sorted by pane and then by role, so a pane's
    /// artifact row comes before its data row.
    pub(crate) fn watches(&self) -> Vec<WatchInfo> {
        let mut all: Vec<_> = self
            .watchers
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(pane, watches)| {
                watches.iter().map(|w| {
                    let at = w.at.lock().unwrap();
                    WatchInfo {
                        pane: pane.clone(),
                        role: w.role,
                        scope: w.artifact.scope(),
                        name: w.artifact.name().to_string(),
                        dir: w.dir.clone(),
                        watching: at.as_ref().map(|t| t.path.to_string_lossy().into_owned()),
                        waiting: at.as_ref().is_some_and(|t| t.waiting),
                    }
                })
            })
            .collect();
        all.sort_by(|a, b| (&a.pane, a.role).cmp(&(&b.pane, b.role)));
        all
    }

    /// Where web pane `pane` looks for the file at `path`. A path under the reserved `data/` prefix
    /// comes from the pane's own data directory, every other path from the artifact's folder; the
    /// artifact's own `data/`, which the manifest loader reports as a problem, is therefore
    /// unreachable. Either way it is one name under a root inside a directory the daemon owns, so
    /// [`resolve`] confines both the same and the artifact's folder is just one more name that may
    /// not leave its root.
    fn serve(&self, pane: &str, path: &str) -> Option<Serve> {
        let spaces = self.spaces.lock().unwrap();
        let (artifact, cwd) = spaces
            .spaces
            .iter()
            .find_map(|s| match crate::spaces::find_pane(&s.root, pane) {
                Some(PaneKind::Web { artifact, .. }) => Some((artifact.clone(), s.cwd.clone())),
                _ => None,
            })?;
        if let Some(path) = under_data(path) {
            return Some(Serve {
                base: self.instance_dir.clone(),
                root: self.panes_root(),
                name: pane.to_string(),
                path: path.to_string(),
            });
        }
        let (base, root) = artifact_root(artifact.scope(), &cwd, &self.instance_dir);
        Some(Serve {
            base,
            root,
            name: artifact.name().to_string(),
            path: path.to_string(),
        })
    }
}

/// Where one request's file is looked for.
struct Serve {
    /// The directory the root has to stay inside: a space's, or the instance's.
    base: PathBuf,
    /// The root the name resolves under.
    root: PathBuf,
    /// The one name under the root: the artifact's folder, or the pane's own directory.
    name: String,
    /// The path under that name.
    path: String,
}

/// The path under the reserved `data/` prefix, or None when `path` is not under it: `database` is
/// not `data`, and `data` alone is the directory itself.
fn under_data(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(DATA)?;
    match rest.strip_prefix('/') {
        Some(rest) => Some(rest),
        None => rest.is_empty().then_some(rest),
    }
}

/// Where every artifact of `scope` sits: the directory the daemon owns — a space's, whose is
/// `cwd`, or the instance's own, holding its token and saved layout — and the folder under it that
/// holds the artifacts. Both come from one `match`, so a scope cannot gain a root without the
/// directory that root has to stay inside.
fn artifact_root(scope: ArtifactScope, cwd: &str, instance: &Path) -> (PathBuf, PathBuf) {
    let (base, under) = match scope {
        ArtifactScope::Repo => (Path::new(cwd), ".wheel/artifacts"),
        ArtifactScope::Local => (Path::new(cwd), ".wheel/local/artifacts"),
        ArtifactScope::Shared => (instance, "artifacts"),
    };
    (base.to_path_buf(), base.join(under))
}

/// Whether `path` really sits inside `within`, following every symlink in both: a scope's root
/// inside the directory it belongs to (the space's, or the instance's), or an artifact's folder
/// inside that root. A cloned repo can ship either as a symlink, and `resolve` confines the files
/// it serves to the root the same way, so one rule answers for reading, writing and listing.
fn inside(path: &Path, within: &Path) -> bool {
    match (path.canonicalize(), within.canonicalize()) {
        (Ok(path), Ok(within)) => path.starts_with(within),
        _ => false,
    }
}

/// Whether `path` under `root` has a component starting with `.`.
fn hidden(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .map(|rel| {
            rel.components()
                .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        })
        .unwrap_or(false)
}

/// Each web pane in the tree, with the artifact it shows.
pub(crate) fn web_panes(node: &Node) -> Vec<(&str, &ArtifactRef)> {
    match node {
        Node::Pane {
            id,
            kind: PaneKind::Web { artifact, .. },
            ..
        } => vec![(id, artifact)],
        Node::Pane { .. } => vec![],
        Node::Split { a, b, .. } => {
            let mut all = web_panes(a);
            all.extend(web_panes(b));
            all
        }
    }
}

/// `GET /artifacts/<pane>/<path>`: a file from web pane `pane`'s artifact folder, or from the
/// pane's own data directory for a path under `data/`.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/artifacts/{pane}/", get(artifact))
        .route("/artifacts/{pane}/{*path}", get(artifact))
        .with_state(state)
}

/// The file the folder `name` under `root` serves for `path` — `index.html` for a folder — or None
/// when it does not exist, would leave `root` (a `..`, an absolute path, a symlink pointing
/// outside — the named folder turned into one included), or is hidden (a name starting with `.`,
/// such as `.git` or `.env`, here or behind a symlink).
///
/// Staying inside the root is the whole rule, written once, and both of a pane's roots come through
/// here: the artifact's folder under its scope's root, and the pane's own directory under the
/// instance's `panes`. The folder is just the first component of the path under the root, so a
/// cloned repo whose `.wheel/artifacts/x` is a symlink to somewhere else serves nothing, with no
/// second check to keep in step.
fn resolve(root: &Path, name: &str, path: &str) -> Option<PathBuf> {
    let rel = Path::new(name).join(path);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let mut file = root.join(rel).canonicalize().ok()?;
    if file.is_dir() {
        file = file.join("index.html");
    }
    (file.starts_with(&root) && !hidden(&root, &file) && file.is_file()).then_some(file)
}

fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

async fn artifact(
    State(state): State<Arc<AppState>>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
    uri: Uri,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let pane = params.get("pane").map(String::as_str).unwrap_or_default();
    let path = params.get("path").map(String::as_str).unwrap_or_default();
    let via = format!("artifacts {pane}/{path}");
    // The page's own requests may carry an Origin; only the daemon's own origin is let through,
    // and `/ws` and `/mcp` still refuse any, so a page cannot reach a shell or the lead's tools.
    if let Some(origin) = headers.get(header::ORIGIN) {
        let own = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(|h| format!("http://{h}"));
        if origin.to_str().ok() != own.as_deref() {
            tracing::warn!(via, ?origin, "refused: browser origin");
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let from_query = query.get("token").map(String::as_str);
    let token = from_query.or(bearer).or_else(|| cookie_token(&headers));
    if let Err(status) = state.check_token(&via, token) {
        return status.into_response();
    }
    let Some(serve) = state.serve(pane, path) else {
        tracing::warn!(via, "artifact refused: no web pane {pane}");
        return (StatusCode::NOT_FOUND, format!("no web pane {pane}")).into_response();
    };
    // A token in the URL is traded for a cookie only this pane's folder gets, and the page loads
    // again from its address without it, so no script in the page reads the token off its URL.
    if let Some(token) = from_query {
        let cookie =
            format!("{COOKIE}={token}; Path=/artifacts/{pane}/; HttpOnly; SameSite=Strict");
        let (Ok(cookie), Ok(location)) = (
            HeaderValue::from_str(&cookie),
            HeaderValue::from_str(uri.path()),
        ) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        tracing::info!(via, "artifact token traded for a cookie");
        return (
            StatusCode::SEE_OTHER,
            [
                (header::SET_COOKIE, cookie),
                (header::LOCATION, location),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
        )
            .into_response();
    }
    let Serve {
        base,
        root,
        name,
        path,
    } = serve;
    if !inside(&root, &base) {
        tracing::warn!(via, root = %root.display(), base = %base.display(), "artifact refused: the root leads out of its directory");
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let Some(file) = resolve(&root, &name, &path) else {
        tracing::warn!(via, dir = %root.join(&name).display(), "artifact not found, or outside the pane's folder");
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let bytes = match tokio::fs::read(&file).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(via, file = %file.display(), error = %e, "artifact read failed");
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
    };
    tracing::info!(via, file = %file.display(), bytes = bytes.len(), "artifact served");
    let mime = mime_guess::from_path(&file).first_or_octet_stream();
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_stays_inside_the_scopes_root() {
        let root = std::env::temp_dir().join(format!("wheel-web-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("site/sub")).unwrap();
        std::fs::write(root.join("site/index.html"), "top").unwrap();
        std::fs::write(root.join("site/sub/index.html"), "sub").unwrap();
        let at = root.canonicalize().unwrap().join("site");
        assert_eq!(resolve(&root, "site", ""), Some(at.join("index.html")));
        assert_eq!(
            resolve(&root, "site", "sub/"),
            Some(at.join("sub/index.html"))
        );
        assert_eq!(
            resolve(&root, "site", "sub"),
            Some(at.join("sub/index.html"))
        );
        assert_eq!(resolve(&root, "site", "missing.html"), None);
        assert_eq!(resolve(&root, "site", "../index.html"), None);
        assert_eq!(resolve(&root, "site", "sub/../../etc/passwd"), None);
        assert_eq!(resolve(&root, "site", "/etc/passwd"), None);
        std::fs::write(root.join("site/.env"), "SECRET=1").unwrap();
        assert_eq!(resolve(&root, "site", ".env"), None, "a hidden file");
        std::os::unix::fs::symlink(root.join("site/.env"), root.join("site/env")).unwrap();
        assert_eq!(
            resolve(&root, "site", "env"),
            None,
            "a symlink to a hidden file"
        );
        std::os::unix::fs::symlink("/etc/hosts", root.join("site/out")).unwrap();
        assert_eq!(
            resolve(&root, "site", "out"),
            None,
            "a symlink out of the root"
        );
        // The artifact's own folder is one more name under the root, so a repo that ships its
        // `.wheel/artifacts/<name>` as a symlink to somewhere else serves nothing.
        std::os::unix::fs::symlink("/etc", root.join("linked")).unwrap();
        assert_eq!(
            resolve(&root, "linked", "hosts"),
            None,
            "an artifact folder symlinked out of its root"
        );
        // A folder missing altogether is not an error either, just nothing to serve.
        assert_eq!(resolve(&root, "gone", "index.html"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_root_symlinked_out_of_its_base_is_refused() {
        let base = std::env::temp_dir().join(format!("wheel-web-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(".wheel")).unwrap();
        std::fs::create_dir_all(base.join("real/x")).unwrap();
        let root = base.join(".wheel/artifacts");
        assert!(!inside(&root, &base), "a root not made yet");
        std::os::unix::fs::symlink("/etc", &root).unwrap();
        assert!(!inside(&root, &base), "a root that is a symlink out");
        std::fs::remove_file(&root).unwrap();
        std::os::unix::fs::symlink(base.join("real"), &root).unwrap();
        assert!(inside(&root, &base), "a symlink that stays inside");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Which paths the reserved prefix claims. Everything the prefix claims is resolved under the
    /// pane's own directory, so an artifact's own `data/` — which the manifest loader reports as a
    /// problem — is never reachable, and a `database/` in the artifact still is.
    #[test]
    fn the_data_prefix_claims_exactly_its_own_paths() {
        assert_eq!(under_data("data"), Some(""), "the directory itself");
        assert_eq!(under_data("data/"), Some(""));
        assert_eq!(under_data("data/rows.json"), Some("rows.json"));
        assert_eq!(under_data("data/sub/rows.json"), Some("sub/rows.json"));
        // Refused by `resolve` once dispatched here, not by a check of its own.
        assert_eq!(
            under_data("data/../../etc/passwd"),
            Some("../../etc/passwd")
        );
        assert_eq!(under_data("database/rows.json"), None, "a longer name");
        assert_eq!(under_data("index.html"), None);
        assert_eq!(under_data(""), None);
        assert_eq!(under_data("sub/data/rows.json"), None, "not at the front");
    }

    /// The three roots, written out: a reference adds one plain name to one of these and nothing
    /// else, so there is no input that names a folder elsewhere. `ArtifactRef::new` is what makes
    /// the name one component — [`wheel_proto`]'s own test covers every way out of it.
    #[test]
    fn each_scope_has_one_root() {
        let instance = Path::new("/var/instance");
        let cwd = "/Users/p/proj";
        let roots = [
            (ArtifactScope::Repo, cwd, "/Users/p/proj/.wheel/artifacts"),
            (
                ArtifactScope::Local,
                cwd,
                "/Users/p/proj/.wheel/local/artifacts",
            ),
            (
                ArtifactScope::Shared,
                "/var/instance",
                "/var/instance/artifacts",
            ),
        ];
        for (scope, base, root) in roots {
            assert_eq!(
                artifact_root(scope, cwd, instance),
                (PathBuf::from(base), PathBuf::from(root))
            );
            // Every root is inside the directory it must stay in, so `inside` only ever
            // refuses one a symlink moved.
            assert!(Path::new(root).starts_with(base));
            let artifact = ArtifactRef::new(scope, "status-dash").unwrap();
            let dir = Path::new(root).join(artifact.name());
            assert_eq!(
                dir.strip_prefix(root).unwrap(),
                Path::new("status-dash"),
                "a name adds exactly one component"
            );
        }
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A watcher that refuses `watch` while `refuse` is set and otherwise is the platform's own.
    /// The platform never refuses a folder that exists, so this is the only way to reach the retry.
    struct Refusing {
        inner: notify::RecommendedWatcher,
        refuse: Arc<AtomicBool>,
    }

    impl Watcher for Refusing {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Err(notify::Error::generic("built by the test, not by notify"))
        }
        fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
            if self.refuse.load(Ordering::SeqCst) {
                return Err(notify::Error::generic("refused by the test"));
            }
            self.inner.watch(path, mode)
        }
        fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
            self.inner.unwatch(path)
        }
        fn configure(&mut self, config: notify::Config) -> notify::Result<bool> {
            self.inner.configure(config)
        }
        fn kind() -> notify::WatcherKind {
            notify::RecommendedWatcher::kind()
        }
    }

    #[tokio::test]
    async fn a_refused_watch_is_retried_and_reloads_the_page_once_set() {
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("wheel-web-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let folder = base.join("site");
        std::fs::create_dir_all(&folder).unwrap();

        let state = AppState::new(String::new(), "t".into(), base.join("inst"), PathBuf::new());
        let mut events = state.global.subscribe();
        let (tx, rx) = mpsc::unbounded_channel::<notify::Event>();
        let refuse = Arc::new(AtomicBool::new(true));
        let inner = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
            if let Ok(e) = e {
                let _ = tx.send(e);
            }
        })
        .unwrap();
        let watcher = Refusing {
            inner,
            refuse: refuse.clone(),
        };
        let artifact = ArtifactRef::new(ArtifactScope::Repo, "site").unwrap();
        let watch = state.run_watch(
            watcher,
            rx,
            "p1",
            &artifact,
            WatchRole::Artifact,
            &folder.display().to_string(),
        );
        state
            .watchers
            .lock()
            .unwrap()
            .insert("p1".into(), vec![watch]);

        let row = state.watches().remove(0);
        assert_eq!(row.watching, None, "refused: status reports no watch");
        assert!(!row.waiting);

        // Two retry periods with the watch still refused: no reload, still not watching.
        tokio::time::sleep(RETRY * 2).await;
        assert!(events.try_recv().is_err(), "nothing reloads while refused");
        assert_eq!(state.watches()[0].watching, None);

        refuse.store(false, Ordering::SeqCst);
        let msg = tokio::time::timeout(RETRY * 3, events.recv())
            .await
            .expect("a retry reloads once the watch is set")
            .unwrap();
        assert!(matches!(msg, ServerMsg::PaneReload { pane } if pane == "p1"));
        let row = state.watches().remove(0);
        assert_eq!(row.watching, Some(folder.display().to_string()));

        // The watch works after the retry: a write reloads the page again.
        std::fs::write(folder.join("index.html"), "hi").unwrap();
        let msg = tokio::time::timeout(RETRY * 3, events.recv())
            .await
            .expect("a write reloads")
            .unwrap();
        assert!(matches!(msg, ServerMsg::PaneReload { pane } if pane == "p1"));

        state.watchers.lock().unwrap().clear();
        std::fs::remove_dir_all(&base).unwrap();
    }
}
