//! Web panes: each shows a folder of HTML the lead or a worker writes. wheeld serves the folder at
//! `/artifacts/<pane>/` behind the daemon token, and watches it: a change to any file in it sends
//! every client `pane_reload`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wheel_proto::{Node, PaneKind, ServerMsg, WatchInfo};

use crate::server::AppState;

/// Changes this close together send one `pane_reload`: an editor's save is often several writes.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// The cookie a page's own requests (its stylesheets, scripts, images) carry the token in, since
/// only the URL a client opens has `?token=`.
const COOKIE: &str = "wheel_token";

/// One web pane's folder watch. Dropping it stops the watch.
pub(crate) struct Watch {
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
fn arm(
    watcher: &mut RecommendedWatcher,
    from: Option<&Target>,
    pane: &str,
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
            tracing::info!(%pane, %dir, watching = %to.path.display(), waiting = to.waiting, "watching web pane");
            return Ok(to);
        }
        let _ = watcher.unwatch(&to.path);
    }
    Err("the folder kept appearing and going".into())
}

/// How long a watch that could not be set waits before trying again.
const RETRY: Duration = Duration::from_secs(1);

impl AppState {
    /// Watch web pane `pane`'s folder from now on, replacing any earlier watch of that pane, and
    /// send `pane_reload` when anything in it changes. While the folder is missing the watch
    /// waits on the folder above it, and moves back onto the folder once it appears, which also
    /// reloads; a watch that cannot be set is retried every second. Call from inside the tokio
    /// runtime. The pane serves whatever is there throughout.
    pub(crate) fn watch(&self, pane: &str, dir: &str) {
        let (tx, mut rx) = mpsc::unbounded_channel::<notify::Event>();
        let watching = pane.to_string();
        let watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    let _ = tx.send(event);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(pane = %watching, error = %e, "web pane watch error"),
            });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(%pane, %dir, error = %e, "could not watch the web pane's folder; it will not reload");
                return;
            }
        };
        let first = arm(&mut watcher, None, pane, dir)
            .inspect_err(|e| tracing::warn!(%pane, %dir, error = %e, "could not watch the web pane's folder; retrying every second"))
            .ok();
        let at = Arc::new(Mutex::new(first));
        let global = self.global.clone();
        let (id, folder, now) = (pane.to_string(), dir.to_string(), at.clone());
        let task = tokio::spawn(async move {
            loop {
                let Some(was) = now.lock().unwrap().clone() else {
                    tokio::time::sleep(RETRY).await;
                    let Ok(next) = arm(&mut watcher, None, &id, &folder) else {
                        continue;
                    };
                    while rx.try_recv().is_ok() {}
                    // Whatever changed while nothing watched is unknown, so the page reloads.
                    if !next.waiting {
                        tracing::info!(pane = %id, path = %folder, changes = 0, appeared = true, "pane reload");
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
                    let next = arm(&mut watcher, Some(&was), &id, &folder)
                        .inspect_err(|e| tracing::warn!(pane = %id, dir = %folder, error = %e, "could not watch the web pane's folder; retrying every second"))
                        .ok();
                    *now.lock().unwrap() = next;
                }
                let appeared =
                    was.waiting && now.lock().unwrap().as_ref().is_some_and(|t| !t.waiting);
                if shown.is_none() && !appeared {
                    continue;
                }
                let path = shown.unwrap_or_else(|| PathBuf::from(&folder));
                tracing::info!(pane = %id, path = %path.display(), changes = events.len(), appeared, "pane reload");
                let _ = global.send(ServerMsg::PaneReload { pane: id.clone() });
            }
        });
        self.watchers.lock().unwrap().insert(
            pane.to_string(),
            Watch {
                dir: dir.to_string(),
                at,
                task,
            },
        );
    }

    pub(crate) fn unwatch(&self, pane: &str) {
        if self.watchers.lock().unwrap().remove(pane).is_some() {
            tracing::info!(%pane, "stopped watching web pane");
        }
    }

    /// Every web pane's folder watch, sorted by pane.
    pub(crate) fn watches(&self) -> Vec<WatchInfo> {
        let mut all: Vec<_> = self
            .watchers
            .lock()
            .unwrap()
            .iter()
            .map(|(pane, w)| {
                let at = w.at.lock().unwrap();
                WatchInfo {
                    pane: pane.clone(),
                    dir: w.dir.clone(),
                    watching: at.as_ref().map(|t| t.path.to_string_lossy().into_owned()),
                    waiting: at.as_ref().is_some_and(|t| t.waiting),
                }
            })
            .collect();
        all.sort_by(|a, b| a.pane.cmp(&b.pane));
        all
    }

    /// The folder web pane `pane` shows.
    fn web_dir(&self, pane: &str) -> Option<String> {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .spaces
            .iter()
            .find_map(|s| match crate::spaces::find_pane(&s.root, pane) {
                Some(PaneKind::Web { dir, .. }) => Some(dir.clone()),
                _ => None,
            })
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

/// Each web pane in the tree, with its folder.
pub(crate) fn web_panes(node: &Node) -> Vec<(&str, &str)> {
    match node {
        Node::Pane {
            id,
            kind: PaneKind::Web { dir, .. },
        } => vec![(id, dir)],
        Node::Pane { .. } => vec![],
        Node::Split { a, b, .. } => {
            let mut all = web_panes(a);
            all.extend(web_panes(b));
            all
        }
    }
}

/// `GET /artifacts/<pane>/<path>`: a file from web pane `pane`'s folder.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/artifacts/{pane}/", get(artifact))
        .route("/artifacts/{pane}/{*path}", get(artifact))
        .with_state(state)
}

/// The file under `dir` that `path` names — `index.html` for a folder — or None when it does not
/// exist, would leave `dir` (a `..`, an absolute path, a symlink pointing outside), or is hidden
/// (a name starting with `.`, such as `.git` or `.env`, here or behind a symlink).
fn resolve(dir: &Path, path: &str) -> Option<PathBuf> {
    let rel = Path::new(path);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let root = dir.canonicalize().ok()?;
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
    let Some(dir) = state.web_dir(pane) else {
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
    let Some(file) = resolve(Path::new(&dir), path) else {
        tracing::warn!(via, %dir, "artifact not found, or outside the pane's folder");
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
    fn resolve_stays_inside_the_folder() {
        let root = std::env::temp_dir().join(format!("wheel-web-resolve-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("index.html"), "top").unwrap();
        std::fs::write(root.join("sub/index.html"), "sub").unwrap();
        let canon = root.canonicalize().unwrap();
        assert_eq!(resolve(&root, ""), Some(canon.join("index.html")));
        assert_eq!(resolve(&root, "sub/"), Some(canon.join("sub/index.html")));
        assert_eq!(resolve(&root, "sub"), Some(canon.join("sub/index.html")));
        assert_eq!(resolve(&root, "missing.html"), None);
        assert_eq!(resolve(&root, "../index.html"), None);
        assert_eq!(resolve(&root, "sub/../../etc/passwd"), None);
        assert_eq!(resolve(&root, "/etc/passwd"), None);
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        assert_eq!(resolve(&root, ".env"), None, "a hidden file");
        std::os::unix::fs::symlink(root.join(".env"), root.join("env")).unwrap();
        assert_eq!(resolve(&root, "env"), None, "a symlink to a hidden file");
        std::os::unix::fs::symlink("/etc/hosts", root.join("out")).unwrap();
        assert_eq!(resolve(&root, "out"), None, "a symlink out of the folder");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
