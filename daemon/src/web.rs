//! Web panes: each shows an artifact, a page of HTML named by a scope and a name rather than by a
//! path. wheeld looks the name up in that scope's `wheel.toml` (`artifacts.rs`), serves the folder
//! the page really sits in at `/artifacts/<pane>/` behind the daemon token, and watches it: a
//! change to any file in it sends every client `pane_reload`.
//!
//! A pane also gets a data directory of its own, under the daemon's instance directory and keyed by
//! the pane, served under the reserved `data/` prefix of the pane's URL. So the artifact's own files
//! stay read-only static content, and one machine artifact open in two projects reads and writes two
//! separate directories. The pane watches both, and both send the same `pane_reload`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wheel_proto::{plain_name, ArtifactRef, Node, PaneKind, ServerMsg, WatchInfo, WatchRole};

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
    /// The folder the watch is for; an artifact's moves when its entry is repointed.
    dir: Arc<Mutex<String>>,
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

/// How often an artifact's watch checks that its entry still names the folder it watches.
const FOLLOW: Duration = Duration::from_millis(250);

/// Reads the folder a watch belongs on now: an artifact's entry is read afresh, so repointing its
/// path moves the watch. None when the entry names no folder at the moment, which keeps the watch
/// where it is.
type Follow = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// The folder a watch starts on, and how it learns the folder moved (None: it never does).
struct Folder {
    dir: String,
    follow: Option<Follow>,
}

impl AppState {
    /// Where every web pane's own data directory sits: under the instance directory, beside the
    /// token and the saved layout. Never inside an artifact's folder and never in a project's
    /// working tree, so nothing a pane is given dirties a repo or is shared between panes.
    fn panes_root(&self) -> PathBuf {
        self.instance_dir.join("panes")
    }

    /// Web pane `pane`'s own data directory, `panes/<pane>/` under the instance directory. Keyed by
    /// the pane and never by the artifact, so one machine artifact open in two projects gets two
    /// directories. Refused unless the pane's id is one plain name — the rule an artifact's name
    /// goes through too — because a saved layout a person can edit names the pane, and this
    /// directory is made and removed as the pane comes and goes.
    pub(crate) fn pane_data_dir(&self, pane: &str) -> Result<PathBuf, String> {
        plain_name("a pane id", pane)?;
        Ok(self.panes_root().join(pane))
    }

    /// Show artifact `artifact` in web pane `pane`, whose space's directory is `cwd`: make the
    /// pane's data directory, watch both of the pane's roots from now on, replacing any earlier
    /// watch of that pane, and start refreshing if the artifact's entry declares a command. A
    /// change under either directory sends `pane_reload`. While a directory is missing its watch
    /// waits on the folder above it, and moves back onto it once it appears, which also reloads; a
    /// watch that cannot be set is retried every second. The artifact's watch re-reads its entry
    /// four times a second and moves onto the folder the entry names now, which also reloads. Call
    /// from inside the tokio runtime. The pane serves whatever is there throughout.
    pub(crate) fn show_artifact(&self, pane: &str, artifact: &ArtifactRef, cwd: &str) {
        let mut watches: Vec<Watch> = Vec::new();
        match self.artifact_entry(artifact, cwd) {
            Ok(Some(entry)) => match entry.watch_dir() {
                Some(dir) => {
                    let (registry, name) = (
                        self.artifact_registry(artifact.scope(), cwd),
                        artifact.name().to_string(),
                    );
                    let follow: Follow = Arc::new(move || {
                        let entry = crate::artifacts::entry(&registry, &name).ok()??;
                        Some(entry.watch_dir()?.display().to_string())
                    });
                    watches.extend(self.spawn_watch(
                        pane,
                        artifact,
                        WatchRole::Artifact,
                        &dir,
                        Some(follow),
                    ))
                }
                None => {
                    tracing::warn!(%pane, entry = %entry.file.display(), "the artifact's entry names no page; nothing to watch until the pane shows it again")
                }
            },
            Ok(None) => {
                tracing::warn!(%pane, scope = artifact.scope().name(), name = artifact.name(), "no entry for the artifact; nothing to watch until the pane shows it again")
            }
            Err(e) => {
                tracing::warn!(%pane, error = %e, "the artifact's wheel.toml cannot be read; nothing to watch until the pane shows it again")
            }
        }
        match self.pane_data_dir(pane) {
            Ok(dir) => {
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    tracing::warn!(%pane, dir = %dir.display(), error = %e, "could not make the pane's data directory; it serves nothing under data/");
                }
                watches.extend(self.spawn_watch(pane, artifact, WatchRole::Data, &dir, None));
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
        follow: Option<Follow>,
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
            Ok(w) => Some(self.run_watch(
                w,
                rx,
                pane,
                artifact,
                role,
                Folder {
                    dir: dir.to_string(),
                    follow,
                },
            )),
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
        folder: Folder,
    ) -> Watch {
        let Folder { dir, follow } = folder;
        let dir = dir.as_str();
        let first = arm(&mut watcher, None, pane, role, dir)
            .inspect_err(|e| tracing::warn!(%pane, ?role, %dir, error = %e, "could not watch the web pane's folder; retrying every second"))
            .ok();
        let at = Arc::new(Mutex::new(first));
        let global = self.global.clone();
        let shown = Arc::new(Mutex::new(dir.to_string()));
        let (id, mut folder, now, shown_dir) =
            (pane.to_string(), dir.to_string(), at.clone(), shown.clone());
        let task = tokio::spawn(async move {
            loop {
                // The entry was repointed: leave the old folder, watch the new one, and reload,
                // since the page is a different one now.
                let named = match follow.clone() {
                    // The entry is a file read, off the runtime's threads.
                    Some(f) => tokio::task::spawn_blocking(move || f())
                        .await
                        .ok()
                        .flatten(),
                    None => None,
                };
                if let Some(next) = named.filter(|d| *d != folder) {
                    let was = now.lock().unwrap().clone();
                    tracing::info!(pane = %id, ?role, from = %folder, to = %next, "artifact repointed; moving the watch");
                    folder = next;
                    *shown_dir.lock().unwrap() = folder.clone();
                    let armed = arm(&mut watcher, was.as_ref(), &id, role, &folder)
                        .inspect_err(|e| tracing::warn!(pane = %id, ?role, dir = %folder, error = %e, "could not watch the web pane's folder; retrying every second"))
                        .ok();
                    while rx.try_recv().is_ok() {}
                    *now.lock().unwrap() = armed;
                    tracing::info!(pane = %id, ?role, path = %folder, changes = 0, appeared = false, "pane reload");
                    let _ = global.send(ServerMsg::PaneReload { pane: id.clone() });
                    continue;
                }
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
                let event = tokio::select! {
                    event = rx.recv() => match event {
                        Some(event) => event,
                        None => return,
                    },
                    // Nothing changed in the folder: look again at where the entry points.
                    () = tokio::time::sleep(FOLLOW), if follow.is_some() => continue,
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
            dir: shown,
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
        self.drop_page(pane);
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
                        dir: w.dir.lock().unwrap().clone(),
                        watching: at.as_ref().map(|t| t.path.to_string_lossy().into_owned()),
                        waiting: at.as_ref().is_some_and(|t| t.waiting),
                    }
                })
            })
            .collect();
        all.sort_by(|a, b| (&a.pane, a.role).cmp(&(&b.pane, b.role)));
        all
    }

    /// Where web pane `pane` looks for the file at `path`: None when there is no such web pane, an
    /// error when its artifact's page is not anywhere. A path under the reserved `data/` prefix
    /// comes from the pane's own data directory, every other path from the folder the artifact's
    /// page really sits in, and the empty path is that page; the artifact's own `data/`, which the
    /// entry reader reports as a problem, is therefore unreachable. [`resolve`] confines both the
    /// same: nothing a request names may leave its root.
    fn serve(&self, pane: &str, path: &str) -> Option<Result<Serve, String>> {
        let spaces = self.spaces.lock().unwrap();
        let (artifact, cwd) = spaces
            .spaces
            .iter()
            .find_map(|s| match crate::spaces::find_pane(&s.root, pane) {
                Some(PaneKind::Web { artifact, .. }) => Some((artifact.clone(), s.cwd.clone())),
                _ => None,
            })?;
        drop(spaces);
        if let Some(path) = under_data(path) {
            return Some(Ok(Serve {
                base: self.instance_dir.clone(),
                root: self.panes_root(),
                name: pane.to_string(),
                path: path.to_string(),
            }));
        }
        Some(self.artifact_located(&artifact, &cwd).map(|located| {
            let path = if path.is_empty() {
                located.page
            } else {
                path.to_string()
            };
            // The root is the folder itself, so there is no name under it and nothing for it to
            // stay inside but itself.
            Serve {
                base: located.root.clone(),
                root: located.root,
                name: String::new(),
                path,
            }
        }))
    }
}

/// Where one request's file is looked for.
struct Serve {
    /// The directory the root has to stay inside: the instance's, for a pane's data directory.
    base: PathBuf,
    /// The root the name resolves under.
    root: PathBuf,
    /// The one name under the root: the pane's own directory, or nothing for an artifact's folder.
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

/// Whether `path` really sits inside `within`, following every symlink in both: a new artifact's
/// default root inside the directory it belongs to (the space's, or the instance's), or the panes'
/// root inside the instance directory. A cloned repo can ship `.wheel` as a symlink, and `resolve`
/// confines the files it serves to its root the same way.
pub(crate) fn inside(path: &Path, within: &Path) -> bool {
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

/// The page-side half of the two-way channel (`page.rs`), served at `.wheel/client.js`.
const CLIENT_JS: &str = include_str!("page_client.js");

/// The page-facing endpoints answer for a pane that is a web pane and a caller holding the token;
/// the status and message to refuse with otherwise.
fn page_for(
    state: &AppState,
    pane: &str,
    via: &str,
    headers: &HeaderMap,
) -> Result<Arc<crate::page::Page>, (StatusCode, String)> {
    authorize(state, via, headers, None).map_err(|status| (status, String::new()))?;
    state.page(pane).map_err(|why| {
        tracing::warn!(via, %why, "page request refused");
        (StatusCode::NOT_FOUND, why)
    })
}

async fn page_client(
    State(state): State<Arc<AppState>>,
    UrlPath(pane): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    let via = format!("artifacts {pane}/.wheel/client.js");
    if let Err(r) = page_for(&state, &pane, &via, &headers) {
        return r.into_response();
    }
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        CLIENT_JS,
    )
        .into_response()
}

/// `PUT /artifacts/<pane>/.wheel/state/<key>`: store the request body, which must be JSON, as the
/// page's value for `key`.
async fn page_state(
    State(state): State<Arc<AppState>>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let pane = params.get("pane").map(String::as_str).unwrap_or_default();
    let key = params.get("key").map(String::as_str).unwrap_or_default();
    let via = format!("artifacts {pane}/.wheel/state/{key}");
    let page = match page_for(&state, pane, &via, &headers) {
        Ok(page) => page,
        Err(r) => return r.into_response(),
    };
    if let Err(why) = plain_name("a state key", key) {
        tracing::warn!(via, %why, "page state refused");
        return (StatusCode::BAD_REQUEST, why).into_response();
    }
    if body.len() > crate::page::MAX_VALUE {
        tracing::warn!(via, bytes = body.len(), "page state refused: over 1 MiB");
        return (StatusCode::PAYLOAD_TOO_LARGE, "the value is over 1 MiB").into_response();
    }
    match serde_json::from_slice(&body) {
        Ok(value) => match page.set(key, value) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(why) => {
                tracing::warn!(via, %why, "page state refused");
                (StatusCode::INSUFFICIENT_STORAGE, why).into_response()
            }
        },
        Err(e) => {
            tracing::warn!(via, error = %e, "page state refused: not JSON");
            (
                StatusCode::BAD_REQUEST,
                format!("the body is not JSON: {e}"),
            )
                .into_response()
        }
    }
}

/// `GET /artifacts/<pane>/.wheel/events`: a server-sent-events stream of the values pushed with
/// `wheel artifact data`, the latest first.
async fn page_events(
    State(state): State<Arc<AppState>>,
    UrlPath(pane): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    let via = format!("artifacts {pane}/.wheel/events");
    let page = match page_for(&state, &pane, &via, &headers) {
        Ok(page) => page,
        Err(r) => return r.into_response(),
    };
    tracing::info!(via, "page event stream opened");
    // `seen` starts at 0, so a page that loads after a push gets the latest value at once.
    let rx = page.subscribe();
    let stream = futures_util::stream::unfold(
        (page, rx, 0u64, true, via),
        |(page, mut rx, seen, first, via)| async move {
            if first {
                return Some((
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::from("retry: 1000\n\n")),
                    (page, rx, seen, false, via),
                ));
            }
            loop {
                if let Some((seq, value)) = page.data_after(seen) {
                    tracing::info!(via, seq, "page event sent");
                    let frame = format!("data: {value}\n\n");
                    return Some((Ok(frame.into()), (page, rx, seq, false, via)));
                }
                // A comment now and then, so a closed page is noticed and its stream dropped.
                if tokio::time::timeout(Duration::from_secs(15), rx.changed())
                    .await
                    .is_err()
                {
                    return Some((Ok(": keep-alive\n\n".into()), (page, rx, seen, false, via)));
                }
            }
        },
    );
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// `GET /artifacts/<pane>/<path>`: a file from web pane `pane`'s artifact folder, or from the
/// pane's own data directory for a path under `data/`.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/artifacts/{pane}/.wheel/client.js", get(page_client))
        .route("/artifacts/{pane}/.wheel/events", get(page_events))
        .route("/artifacts/{pane}/.wheel/state/{key}", put(page_state))
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
/// here: the real folder holding the artifact's page, with no name under it, and the pane's own
/// directory under the instance's `panes`. A link inside the folder that points out of it serves
/// nothing, so a page reads what is beside and below it and nothing else (ADR-0003).
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

/// Whether `path` under the folder `name` of `root` names a folder rather than a file.
fn is_folder(root: &Path, name: &str, path: &str) -> bool {
    root.canonicalize()
        .map(|root| root.join(name).join(path).is_dir())
        .unwrap_or(false)
}

fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

fn from_query_token(query: &HashMap<String, String>) -> Option<&str> {
    query.get("token").map(String::as_str)
}

/// Check a request to a pane's URLs: the page's own requests may carry an Origin, and only the
/// daemon's own is let through (`/ws` still refuses any, so a page cannot reach a shell or an
/// agent); then the token, from the query, a bearer header or the cookie.
fn authorize(
    state: &AppState,
    via: &str,
    headers: &HeaderMap,
    from_query: Option<&str>,
) -> Result<(), StatusCode> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        let own = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(|h| format!("http://{h}"));
        if origin.to_str().ok() != own.as_deref() {
            tracing::warn!(via, ?origin, "refused: browser origin");
            return Err(StatusCode::FORBIDDEN);
        }
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let token = from_query.or(bearer).or_else(|| cookie_token(headers));
    state.check_token(via, token)
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
    let from_query = from_query_token(&query);
    if let Err(status) = authorize(&state, &via, &headers, from_query) {
        return status.into_response();
    }
    let serve = match state.serve(pane, path) {
        None => {
            tracing::warn!(via, "artifact refused: no web pane {pane}");
            return (StatusCode::NOT_FOUND, format!("no web pane {pane}")).into_response();
        }
        Some(Err(why)) => {
            tracing::warn!(via, %why, "artifact not found: its page is not anywhere");
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
        Some(Ok(serve)) => serve,
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
    // A page in a folder resolves its relative URLs against the address it was loaded from, so a
    // folder asked for without its trailing slash goes to the address with one.
    if !uri.path().ends_with('/') && is_folder(&root, &name, &path) {
        let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
        let Ok(location) = HeaderValue::from_str(&format!("{}/{query}", uri.path())) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        tracing::info!(
            via,
            ?location,
            "artifact folder redirected to its trailing slash"
        );
        return (
            StatusCode::PERMANENT_REDIRECT,
            [
                (header::LOCATION, location),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
        )
            .into_response();
    }
    let opened = match tokio::fs::File::open(&file).await {
        Ok(f) => f.metadata().await.map(|m| (f, m.len())),
        Err(e) => Err(e),
    };
    let (handle, bytes) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            tracing::warn!(via, file = %file.display(), error = %e, "artifact read failed");
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
    };
    tracing::info!(via, file = %file.display(), bytes, "artifact served");
    let mime = mime_guess::from_path(&file).first_or_octet_stream();
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::CONTENT_LENGTH, bytes.to_string()),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(handle)),
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
        assert!(is_folder(&root, "site", "sub"), "a folder");
        assert!(!is_folder(&root, "site", "index.html"), "a file");
        assert!(!is_folder(&root, "site", "missing"), "nothing");
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
    /// pane's own directory, so an artifact's own `data/` — which the entry reader reports as a
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
        let artifact = ArtifactRef::new(wheel_proto::ArtifactScope::Project, "site").unwrap();
        let watch = state.run_watch(
            watcher,
            rx,
            "p1",
            &artifact,
            WatchRole::Artifact,
            Folder {
                dir: folder.display().to_string(),
                follow: None,
            },
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
