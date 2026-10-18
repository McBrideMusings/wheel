//! `pane_shot`: a PNG of what a client draws for a pane, written to a file.
//!
//! The daemon holds no pixels, so it asks a client. A client that can draw its panes sends
//! `render_offer` after connecting; `pane_shot` sends each such connection a `render_pane`
//! request, newest connection first, until one answers with an image. This is the daemon's one
//! request to a client that waits for the answer.

use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use tokio::sync::{mpsc, oneshot};
use wheel_proto::ServerMsg;

use crate::server::AppState;

/// How long one client has to draw a pane before the daemon asks the next.
const RENDER_WAIT: Duration = Duration::from_secs(15);

/// What a client says when it does not show the pane it was asked for.
pub const NOT_SHOWN: &str = "not shown";

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

type Answer = Result<Vec<u8>, String>;

/// The connections that offered to draw panes, and the requests waiting on one of them.
#[derive(Default)]
pub(crate) struct Renderers {
    next: AtomicU64,
    /// In connection order; the last is the newest.
    offered: Mutex<Vec<(SocketAddr, mpsc::Sender<ServerMsg>)>>,
    pending: Mutex<HashMap<u64, (SocketAddr, oneshot::Sender<Answer>)>>,
}

impl Renderers {
    pub(crate) fn offer(&self, peer: SocketAddr, out: mpsc::Sender<ServerMsg>) {
        let mut offered = self.offered.lock().unwrap();
        offered.retain(|(p, _)| *p != peer);
        offered.push((peer, out));
        tracing::info!(%peer, offered = offered.len(), "client offers to render panes");
    }

    /// A connection closed: it no longer renders, and what waited on it is refused.
    pub(crate) fn gone(&self, peer: SocketAddr) {
        self.offered.lock().unwrap().retain(|(p, _)| *p != peer);
        let mut pending = self.pending.lock().unwrap();
        let ids: Vec<u64> = pending
            .iter()
            .filter(|(_, (p, _))| *p == peer)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some((_, tx)) = pending.remove(&id) {
                let _ = tx.send(Err("the client disconnected before it answered".into()));
            }
        }
    }

    /// A client answered render request `id`.
    pub(crate) fn answer(&self, peer: SocketAddr, id: u64, answer: Answer) {
        let waiting = {
            let mut pending = self.pending.lock().unwrap();
            match pending.get(&id) {
                Some((p, _)) if *p == peer => pending.remove(&id),
                _ => None,
            }
        };
        match waiting {
            Some((_, tx)) => {
                let _ = tx.send(answer);
            }
            None => {
                tracing::warn!(%peer, id, "render answer for a request nobody waits on");
            }
        }
    }
}

impl AppState {
    /// Ask the connected clients for a PNG of `pane` and write it to `path`, which must be
    /// absolute. Returns the byte count of the finished file.
    pub async fn pane_shot(&self, pane: &str, path: &str) -> Result<u64, String> {
        let target = PathBuf::from(path);
        if !target.is_absolute() {
            return Err(format!("{path} is not an absolute path"));
        }
        let clients: Vec<_> = self
            .renderers
            .offered
            .lock()
            .unwrap()
            .iter()
            .rev()
            .cloned()
            .collect();
        tracing::info!(pane, path, clients = clients.len(), "pane shot requested");
        if clients.is_empty() {
            return Err(format!(
                "no client is connected that can draw pane {pane} (is the Mac app running?)"
            ));
        }
        let mut refusals: Vec<String> = Vec::new();
        for (peer, out) in clients {
            let id = self.renderers.next.fetch_add(1, Ordering::SeqCst) + 1;
            let (tx, rx) = oneshot::channel();
            self.renderers
                .pending
                .lock()
                .unwrap()
                .insert(id, (peer, tx));
            // A connection that closed between the snapshot above and the insert would never
            // wake this request: `gone` ran before it was pending.
            let still_offered = self
                .renderers
                .offered
                .lock()
                .unwrap()
                .iter()
                .any(|(p, _)| *p == peer);
            if !still_offered {
                self.renderers.pending.lock().unwrap().remove(&id);
                tracing::info!(%peer, id, pane, "render skipped: the client is gone");
                refusals.push("the client disconnected before it answered".into());
                continue;
            }
            let sent = tokio::time::timeout(
                RENDER_WAIT,
                out.send(ServerMsg::RenderPane {
                    id,
                    pane: pane.to_string(),
                }),
            )
            .await;
            let sent = matches!(sent, Ok(Ok(())));
            tracing::info!(%peer, id, pane, sent, "render requested");
            let answer = if !sent {
                Err("the client's connection is closed".to_string())
            } else {
                match tokio::time::timeout(RENDER_WAIT, rx).await {
                    Ok(Ok(answer)) => answer,
                    Ok(Err(_)) => Err("the client dropped the request".to_string()),
                    Err(_) => Err(format!("no answer within {}s", RENDER_WAIT.as_secs())),
                }
            };
            self.renderers.pending.lock().unwrap().remove(&id);
            match answer {
                Ok(png) => {
                    if !png.starts_with(PNG_SIGNATURE) {
                        tracing::warn!(%peer, id, pane, bytes = png.len(), "render answer is not a PNG");
                        refusals.push("the client's image is not a PNG".into());
                        continue;
                    }
                    let bytes = write_whole(&target, &png)?;
                    tracing::info!(%peer, id, pane, bytes, path, "pane shot written");
                    return Ok(bytes);
                }
                Err(message) => {
                    tracing::info!(%peer, id, pane, %message, "render refused");
                    refusals.push(message);
                }
            }
        }
        if refusals.iter().all(|m| m == NOT_SHOWN) {
            return Err(format!("no connected client shows pane {pane}"));
        }
        Err(format!("cannot draw pane {pane}: {}", refusals.join("; ")))
    }
}

/// Write `bytes` to `target` so a reader never sees half a file: into a sibling made new for this
/// write, then a rename.
fn write_whole(target: &Path, bytes: &[u8]) -> Result<u64, String> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let name = target
        .file_name()
        .ok_or_else(|| format!("{} names no file", target.display()))?
        .to_string_lossy()
        .into_owned();
    let temp = target.with_file_name(format!(
        ".{name}.wheel-shot-{}-{}",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::SeqCst)
    ));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| std::fs::rename(&temp, target));
    written.map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("cannot write {}: {e}", target.display())
    })?;
    Ok(bytes.len() as u64)
}

/// Decode a `rendered` frame's image.
pub(crate) fn decode_png(png: &str) -> Answer {
    base64::engine::general_purpose::STANDARD
        .decode(png)
        .map_err(|e| format!("the image is not base64: {e}"))
}
