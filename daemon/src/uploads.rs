//! Files a client sends to an agent pane, so a photo or document on a phone reaches the agent
//! running on this machine. `PUT /uploads/<pane>/<name>` streams the request body into the pane's
//! own directory, `uploads/<pane>/` under the instance directory, and answers with the file's
//! absolute path, which the client puts in its message for the agent to read with its own tools.
//! `DELETE /uploads/<pane>/<stored name>` takes one back. The directory goes when the pane's
//! content is closed or replaced, and a restore removes those of panes its layout no longer names.
//!
//! Only a native client gets here: the request needs the token in a bearer header, and any
//! `Origin` is refused, so an artifact page (whose cookie is scoped to its own folder) cannot.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use axum::{Extension, Json, Router};
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use wheel_proto::{plain_name, UPLOAD_MAX_BYTES};

use crate::server::{AppState, Listener};

/// How long an upload may go without a byte before it is dropped, so a phone that lost its
/// network holds no file and no task for ever.
const IDLE: std::time::Duration = std::time::Duration::from_secs(60);

/// A number no other call in this process got, for the names of files on their way in or out.
fn next_serial() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The routes, each with no body limit of axum's own: [`put_upload`] counts against
/// [`UPLOAD_MAX_BYTES`] as the body streams in.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/uploads/{pane}/{name}",
            put(put_upload).delete(delete_upload),
        )
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}

impl AppState {
    fn uploads_root(&self) -> PathBuf {
        self.instance_dir.join("uploads")
    }

    /// Pane `pane`'s upload directory. Refused unless the id is one plain name, since it becomes a
    /// directory name.
    fn pane_uploads_dir(&self, pane: &str) -> Result<PathBuf, String> {
        plain_name("a pane id", pane)?;
        Ok(self.uploads_root().join(pane))
    }

    /// Delete everything uploaded to `pane`. Called with the spaces lock held, so the directory is
    /// renamed away at once and the tree deleted on a thread of its own.
    pub(crate) fn drop_uploads(&self, pane: &str) {
        let Ok(dir) = self.pane_uploads_dir(pane) else {
            return;
        };
        let gone = self
            .uploads_root()
            .join(format!(".gone-{pane}-{}", next_serial()));
        match std::fs::rename(&dir, &gone) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!(%pane, dir = %dir.display(), error = %e, "could not remove the pane's uploads");
                return;
            }
        }
        let pane = pane.to_string();
        std::thread::spawn(move || match std::fs::remove_dir_all(&gone) {
            Ok(()) => tracing::info!(%pane, dir = %dir.display(), "removed the pane's uploads"),
            Err(e) => {
                tracing::warn!(%pane, dir = %gone.display(), error = %e, "could not remove the pane's uploads")
            }
        });
    }

    /// Remove every entry under `uploads/` that no pane in `named` owns: a directory left by a
    /// crash, by a removal a stop cut short, or by a deleted layout whose pane ids start again.
    /// Called once, on restore, with every pane the loaded layout names.
    pub(crate) fn sweep_uploads(&self, named: &HashSet<&str>) {
        let root = self.uploads_root();
        let Ok(entries) = std::fs::read_dir(&root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if named.contains(name.as_str()) {
                continue;
            }
            match std::fs::remove_dir_all(entry.path()) {
                Ok(()) => {
                    tracing::info!(dir = %entry.path().display(), "removed uploads no pane owns")
                }
                Err(e) => {
                    tracing::warn!(dir = %entry.path().display(), error = %e, "could not remove uploads no pane owns")
                }
            }
        }
    }
}

/// The name an upload is stored under, from the one the client sent: its last path component, with
/// leading dots and dashes, control characters and colons dropped, each run of whitespace made one
/// `_` (the path goes into a message as plain text, where a space would split it), cut to 200
/// bytes. A name with nothing left becomes `upload`.
fn stored_name(sent: &str) -> String {
    let last = sent.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && *c != ':')
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join("_");
    let trimmed = joined.trim_start_matches(['.', '-']);
    // Names are limited in bytes (255 on APFS), and `<n>-` goes in front: cut at 200 bytes, on a
    // character boundary.
    let mut cut = String::new();
    for c in trimmed.chars() {
        if cut.len() + c.len_utf8() > 200 {
            break;
        }
        cut.push(c);
    }
    if cut.is_empty() {
        "upload".into()
    } else {
        cut
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

fn refuse(pane: &str, status: StatusCode, why: String) -> Response {
    tracing::warn!(%pane, status = status.as_u16(), %why, "upload refused");
    (status, why).into_response()
}

/// Checks shared by both routes: the token, the pane id, and that the pane is an agent pane.
fn admit(
    state: &AppState,
    listener: Listener,
    headers: &HeaderMap,
    pane: &str,
) -> Result<PathBuf, Box<Response>> {
    let via = format!("uploads {pane}");
    if let Err(status) = state.admit(&via, headers, bearer(headers), listener) {
        return Err(Box::new(status.into_response()));
    }
    let dir = state
        .pane_uploads_dir(pane)
        .map_err(|why| Box::new(refuse(pane, StatusCode::BAD_REQUEST, why)))?;
    if !state.is_agent_pane(pane) {
        return Err(Box::new(refuse(
            pane,
            StatusCode::NOT_FOUND,
            format!("no agent pane {pane}"),
        )));
    }
    Ok(dir)
}

#[derive(serde::Serialize)]
struct Stored {
    path: String,
    name: String,
    bytes: u64,
}

async fn put_upload(
    State(state): State<Arc<AppState>>,
    Extension(listener): Extension<Listener>,
    UrlPath((pane, sent)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let dir = match admit(&state, listener, &headers, &pane) {
        Ok(dir) => dir,
        Err(response) => return *response,
    };
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > UPLOAD_MAX_BYTES) {
        return refuse(
            &pane,
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("over {} MB", UPLOAD_MAX_BYTES >> 20),
        );
    }
    // The bytes land in a staging directory first, so a pane closed while they stream in takes
    // nothing half-written with it, and a crash's leftovers go with the restore sweep (no pane is
    // called `.partial`).
    let staging = state.uploads_root().join(".partial");
    if let Err(e) = tokio::fs::create_dir_all(&staging).await {
        return refuse(
            &pane,
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot make {}: {e}", staging.display()),
        );
    }
    let started = Instant::now();
    let name = stored_name(&sent);
    // Removed however the handler ends, a connection that drops mid-body included (hyper then
    // drops this future before any line below runs).
    let partial = Partial(staging.join(next_serial().to_string()));
    let received = match receive(body, &partial.0).await {
        Ok(n) => n,
        Err((status, why)) => return refuse(&pane, status, why),
    };
    // Placed under the spaces lock, which closing a pane holds while it removes the pane's
    // uploads: a pane that went while the bytes streamed in gets nothing.
    let placed = {
        let spaces = state.spaces.lock().unwrap();
        if !crate::spaces::is_agent_pane(&spaces, &pane) {
            Err((
                StatusCode::NOT_FOUND,
                format!("agent pane {pane} closed during the upload"),
            ))
        } else if let Err(e) = std::fs::create_dir_all(&dir) {
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot make {}: {e}", dir.display()),
            ))
        } else {
            place(&partial.0, &dir, &name).ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot store {name}"),
            ))
        }
    };
    let path = match placed {
        Ok(path) => path,
        Err((status, why)) => return refuse(&pane, status, why),
    };
    let stored = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    tracing::info!(
        %pane, path = %path.display(), bytes = received,
        ms = started.elapsed().as_millis() as u64, "upload stored"
    );
    (
        StatusCode::CREATED,
        Json(Stored {
            path: path.display().to_string(),
            name: stored,
            bytes: received,
        }),
    )
        .into_response()
}

/// Stream `body` into `to`, refusing once it passes [`UPLOAD_MAX_BYTES`].
async fn receive(body: Body, to: &Path) -> Result<u64, (StatusCode, String)> {
    let mut file = tokio::fs::File::create(to).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot write: {e}"),
        )
    })?;
    let mut stream = body.into_data_stream();
    let mut received: u64 = 0;
    loop {
        let next = tokio::time::timeout(IDLE, stream.next())
            .await
            .map_err(|_| {
                (
                    StatusCode::REQUEST_TIMEOUT,
                    format!("no bytes for {} seconds", IDLE.as_secs()),
                )
            })?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("the upload broke off: {e}"),
            )
        })?;
        received += chunk.len() as u64;
        if received > UPLOAD_MAX_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("over {} MB", UPLOAD_MAX_BYTES >> 20),
            ));
        }
        file.write_all(&chunk).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot write: {e}"),
            )
        })?;
    }
    file.flush().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot write: {e}"),
        )
    })?;
    Ok(received)
}

/// A file being received, deleted when this goes.
struct Partial(PathBuf);

impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Link the finished `partial` as the first free `<n>-<name>` in `dir`; the caller removes
/// `partial`. A hard link claims the name only when nothing holds it, so two uploads at once never
/// take the same one.
fn place(partial: &Path, dir: &Path, name: &str) -> Option<PathBuf> {
    for n in 1..10_000 {
        let target = dir.join(format!("{n}-{name}"));
        match std::fs::hard_link(partial, &target) {
            Ok(()) => return Some(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                tracing::warn!(target = %target.display(), error = %e, "cannot place an upload");
                return None;
            }
        }
    }
    None
}

async fn delete_upload(
    State(state): State<Arc<AppState>>,
    Extension(listener): Extension<Listener>,
    UrlPath((pane, stored)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let dir = match admit(&state, listener, &headers, &pane) {
        Ok(dir) => dir,
        Err(response) => return *response,
    };
    if let Err(why) = plain_name("an uploaded file's name", &stored) {
        return refuse(&pane, StatusCode::BAD_REQUEST, why);
    }
    let path = dir.join(&stored);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {
            tracing::info!(%pane, path = %path.display(), "upload deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            refuse(&pane, StatusCode::NOT_FOUND, format!("no upload {stored}"))
        }
        Err(e) => refuse(
            &pane,
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot delete {stored}: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::stored_name;

    #[test]
    fn a_sent_name_keeps_only_a_safe_last_component() {
        assert_eq!(stored_name("IMG_2041.jpg"), "IMG_2041.jpg");
        assert_eq!(stored_name("../../etc/passwd"), "passwd");
        assert_eq!(stored_name("a\\b\\c.txt"), "c.txt");
        assert_eq!(stored_name(".env"), "env");
        assert_eq!(stored_name("--rf"), "rf");
        assert_eq!(stored_name(".."), "upload");
        assert_eq!(stored_name(""), "upload");
        assert_eq!(stored_name("a\u{0}bc:d"), "abcd");
        assert_eq!(stored_name("  My Photo  (2).jpg "), "My_Photo_(2).jpg");
        assert_eq!(stored_name(&"x".repeat(300)).len(), 200);
        let wide = stored_name(&"写".repeat(100));
        assert!(
            wide.len() <= 200 && wide.chars().all(|c| c == '写'),
            "{wide}"
        );
    }
}
