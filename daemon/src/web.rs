//! Web panes: each shows an artifact, a folder of HTML the lead or a worker writes, named by a
//! scope and a name rather than by a path. wheeld owns the three roots a scope can mean, resolves
//! the folder itself, serves it at `/artifacts/<pane>/` behind the daemon token, and watches it: a
//! change to any file in it sends every client `pane_reload`.

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
use wheel_proto::{ArtifactRef, ArtifactScope, Node, PaneKind, ServerMsg, WatchInfo, SCOPE_ORDER};

use crate::server::AppState;

/// Changes this close together send one `pane_reload`: an editor's save is often several writes.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// The cookie a page's own requests (its stylesheets, scripts, images) carry the token in, since
/// only the URL a client opens has `?token=`.
const COOKIE: &str = "wheel_token";

/// One web pane's folder watch. Dropping it stops the watch.
pub(crate) struct Watch {
    artifact: ArtifactRef,
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
    /// The folder `artifact` names for a space whose directory is `cwd`. The one place a root is
    /// computed: every caller — the CLI, the lead's tool, a client op, layout restore and serving
    /// — comes through here, so nothing else can aim a pane at a folder of its own choosing.
    pub(crate) fn artifact_dir(&self, artifact: &ArtifactRef, cwd: &str) -> PathBuf {
        let (_, root) = artifact_root(artifact.scope(), cwd, &self.instance_dir);
        root.join(artifact.name())
    }

    /// Make `scope`'s root for a space whose directory is `cwd`, and give back the folder artifact
    /// `name` takes under it — or refuse, when the root leads out of the directory it belongs to,
    /// which a cloned repo shipping `.wheel/artifacts` as a symlink does. Serving applies the same
    /// rule with [`root_inside`], so what a pane will not serve is also what nothing may write.
    pub(crate) fn artifact_dir_to_make(
        &self,
        artifact: &ArtifactRef,
        cwd: &str,
    ) -> Result<PathBuf, String> {
        let (base, root) = artifact_root(artifact.scope(), cwd, &self.instance_dir);
        std::fs::create_dir_all(&root).map_err(|e| format!("make {}: {e}", root.display()))?;
        if !root_inside(&root, &base) {
            return Err(format!(
                "{} leads out of {}, so nothing is written there",
                root.display(),
                base.display()
            ));
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
            .filter(|e| e.path().is_dir())
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

    /// Watch web pane `pane`'s artifact folder from now on, replacing any earlier watch of that
    /// pane, and send `pane_reload` when anything in it changes. While the folder is missing the
    /// watch waits on the folder above it, and moves back onto the folder once it appears, which
    /// also reloads; a watch that cannot be set is retried every second. Call from inside the
    /// tokio runtime. The pane serves whatever is there throughout.
    pub(crate) fn watch(&self, pane: &str, artifact: &ArtifactRef, cwd: &str) {
        let dir = self.artifact_dir(artifact, cwd).display().to_string();
        let dir = dir.as_str();
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
                artifact: artifact.clone(),
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
                    scope: w.artifact.scope(),
                    name: w.artifact.name().to_string(),
                    dir: w.dir.clone(),
                    watching: at.as_ref().map(|t| t.path.to_string_lossy().into_owned()),
                    waiting: at.as_ref().is_some_and(|t| t.waiting),
                }
            })
            .collect();
        all.sort_by(|a, b| a.pane.cmp(&b.pane));
        all
    }

    /// The directory web pane `pane`'s scope belongs to, the root its artifacts sit in, and the
    /// artifact's name. Serving resolves a file under the root rather than under the artifact's
    /// own folder, so the folder is just one more name that may not leave the root — an artifact
    /// folder symlinked out of its scope serves nothing.
    fn web_root(&self, pane: &str) -> Option<(PathBuf, PathBuf, String)> {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .spaces
            .iter()
            .find_map(|s| match crate::spaces::find_pane(&s.root, pane) {
                Some(PaneKind::Web { artifact, .. }) => {
                    let (base, root) = artifact_root(artifact.scope(), &s.cwd, &self.instance_dir);
                    Some((base, root, artifact.name().to_string()))
                }
                _ => None,
            })
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

/// Whether the scope root `root` really sits inside `base` (the space's directory, or the
/// instance's): a cloned repo can ship `.wheel/artifacts` itself as a symlink, and `resolve`
/// confines files to the root as it resolves, so the root must not lead out of its base.
fn root_inside(root: &Path, base: &Path) -> bool {
    match (root.canonicalize(), base.canonicalize()) {
        (Ok(root), Ok(base)) => root.starts_with(base),
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
        } => vec![(id, artifact)],
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

/// The file artifact `name` under `root` serves for `path` — `index.html` for a folder — or None
/// when it does not exist, would leave `root` (a `..`, an absolute path, a symlink pointing
/// outside — the artifact's own folder turned into one included), or is hidden (a name starting
/// with `.`, such as `.git` or `.env`, here or behind a symlink).
///
/// Staying inside the scope's root is the whole rule, written once. The artifact's folder is just
/// the first component of the path under that root, so a cloned repo whose `.wheel/artifacts/x` is
/// a symlink to somewhere else serves nothing, with no second check to keep in step.
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
    let Some((base, root, name)) = state.web_root(pane) else {
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
    if !root_inside(&root, &base) {
        tracing::warn!(via, root = %root.display(), base = %base.display(), "artifact refused: the scope's root leads out of its directory");
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let Some(file) = resolve(&root, &name, path) else {
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
        assert!(!root_inside(&root, &base), "a root not made yet");
        std::os::unix::fs::symlink("/etc", &root).unwrap();
        assert!(!root_inside(&root, &base), "a root that is a symlink out");
        std::fs::remove_file(&root).unwrap();
        std::os::unix::fs::symlink(base.join("real"), &root).unwrap();
        assert!(root_inside(&root, &base), "a symlink that stays inside");
        std::fs::remove_dir_all(&base).unwrap();
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
            // Every root is inside the directory it must stay in, so `root_inside` only ever
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
