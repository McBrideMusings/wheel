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

/// How long a browser has to say where it is, so `pane info` never hangs on a slow one.
const WHERE_WAIT: Duration = Duration::from_secs(3);

/// What a client says when it does not show the pane it was asked for.
pub const NOT_SHOWN: &str = "not shown";

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

type Answer = Result<Vec<u8>, String>;

/// Which kind of client a request goes to: the Mac app, or a paired device's connection (the phone).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Side {
    Mac,
    Phone,
}

impl Side {
    /// The `on` of `pane_inspect` and `pane_shot`: `mac` when absent.
    pub fn parse(on: Option<&str>) -> Result<Side, String> {
        match on {
            None | Some("mac") => Ok(Side::Mac),
            Some("phone") => Ok(Side::Phone),
            Some(other) => Err(format!("--on takes mac or phone, not {other}")),
        }
    }

    /// `mac` or `phone`, as `on` says it on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Mac => "mac",
            Side::Phone => "phone",
        }
    }
}

/// A connection that offered to draw panes.
struct Offer {
    peer: SocketAddr,
    out: mpsc::Sender<ServerMsg>,
    /// The paired device's name when this connection is a phone.
    phone: Option<String>,
}

/// The connections that offered to draw panes, and the requests waiting on one of them.
#[derive(Default)]
pub(crate) struct Renderers {
    next: AtomicU64,
    /// In connection order; the last is the newest.
    offered: Mutex<Vec<Offer>>,
    pending: Mutex<HashMap<u64, (SocketAddr, oneshot::Sender<Answer>)>>,
    /// Frames and milliseconds a client reported with the movie it answered request `id` with.
    pub(crate) recorded: Mutex<HashMap<u64, (u64, u64)>>,
}

impl Renderers {
    pub(crate) fn offer(&self, peer: SocketAddr, out: mpsc::Sender<ServerMsg>, phone: Option<String>) {
        let mut offered = self.offered.lock().unwrap();
        offered.retain(|o| o.peer != peer);
        offered.push(Offer { peer, out, phone });
        tracing::info!(%peer, offered = offered.len(), "client offers to render panes");
    }

    /// A connection closed: it no longer renders, and what waited on it is refused.
    pub(crate) fn gone(&self, peer: SocketAddr) {
        self.offered.lock().unwrap().retain(|o| o.peer != peer);
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
    /// Whether a request was waiting for the answer.
    pub(crate) fn answer(&self, peer: SocketAddr, id: u64, answer: Answer) -> bool {
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
                true
            }
            None => {
                tracing::warn!(%peer, id, "render answer for a request nobody waits on");
                false
            }
        }
    }
}

impl AppState {
    /// Ask the connected clients for a PNG of `pane` and write it to `path`, which must be
    /// absolute. Returns the byte count of the finished file.
    pub async fn pane_shot(
        &self,
        pane: &str,
        path: &str,
        side: Side,
        full: bool,
        by: Option<String>,
    ) -> Result<u64, String> {
        let target = PathBuf::from(path);
        if !target.is_absolute() {
            return Err(format!("{path} is not an absolute path"));
        }
        tracing::info!(pane, path, "pane shot requested");
        let png = self
            .ask_clients(pane, side, Some("cannot draw"), true, RENDER_WAIT, |id| {
                ServerMsg::RenderPane {
                    id,
                    pane: pane.to_string(),
                    full,
                    by: by.clone(),
                }
            })
            .await?;
        self.write_png(pane, &target, &png)
    }

    /// Run `op` with `arg` on browser pane `pane` through a client that shows it. `eval` and `dom`
    /// return the page's text; `open` sends that client's browser to the page `arg` and returns no text; `shot` writes the PNG of the first element `arg` matches to `path`
    /// and returns no text.
    pub async fn pane_inspect(
        &self,
        pane: &str,
        op: &str,
        arg: Option<String>,
        path: Option<String>,
        side: Side,
        by: Option<String>,
    ) -> Result<InspectOutcome, String> {
        self.browser_pane(pane)?;
        let target = match (op, path) {
            ("eval" | "dom", _) => None,
            ("where", _) => None,
            ("open", _) => {
                if arg.as_deref().is_none_or(str::is_empty) {
                    return Err("open needs a page".into());
                }
                None
            }
            ("shot", Some(path)) => {
                let target = PathBuf::from(&path);
                if !target.is_absolute() {
                    return Err(format!("{path} is not an absolute path"));
                }
                if arg.as_deref().is_none_or(str::is_empty) {
                    return Err("shot needs a selector".into());
                }
                Some(target)
            }
            ("shot", None) => return Err("shot needs a path".into()),
            (other, _) => return Err(format!("unknown inspect op {other}")),
        };
        if op == "eval" && arg.as_deref().is_none_or(str::is_empty) {
            return Err("eval needs a script".into());
        }
        tracing::info!(pane, op, arg = ?arg, "pane inspect requested");
        let wait = if op == "where" { WHERE_WAIT } else { RENDER_WAIT };
        let bytes = self
            .ask_clients(pane, side, None, target.is_some(), wait, |id| {
                ServerMsg::InspectPane {
                    id,
                    pane: pane.to_string(),
                    action: op.to_string(),
                    arg: arg.clone(),
                    by: by.clone(),
                }
            })
            .await?;
        match target {
            Some(target) => Ok(InspectOutcome::Shot(self.write_png(pane, &target, &bytes)?)),
            None if op == "where" => {
                let text = String::from_utf8(bytes)
                    .map_err(|e| format!("the page's answer is not UTF-8: {e}"))?;
                let found = parse_where(&text)?;
                Ok(InspectOutcome::Text(format!("{}\t{}", found.url, found.title)))
            }
            None => Ok(InspectOutcome::Text(
                String::from_utf8(bytes)
                    .map_err(|e| format!("the page's text is not UTF-8: {e}"))?,
            )),
        }
    }

    /// Film browser pane `pane` for `seconds` through a client that shows it and write the movie
    /// to `path`, which must be absolute.
    pub async fn pane_record(
        &self,
        pane: &str,
        seconds: u64,
        path: &str,
        side: Side,
        by: Option<String>,
    ) -> Result<Recording, String> {
        self.browser_pane(pane)?;
        let target = PathBuf::from(path);
        if !target.is_absolute() {
            return Err(format!("{path} is not an absolute path"));
        }
        if !(1..=MAX_RECORD_SECONDS).contains(&seconds) {
            return Err(format!(
                "record takes 1 to {MAX_RECORD_SECONDS} seconds, not {seconds}"
            ));
        }
        tracing::info!(pane, seconds, path, "pane record requested");
        let asked = AtomicU64::new(0);
        let mov = self
            .ask_clients(
                pane,
                side,
                None,
                false,
                Duration::from_secs(seconds) + RENDER_WAIT,
                |id| {
                    asked.store(id, Ordering::SeqCst);
                    ServerMsg::InspectPane {
                        id,
                        pane: pane.to_string(),
                        action: "record".into(),
                        arg: Some(seconds.to_string()),
                        by: by.clone(),
                    }
                },
            )
            .await?;
        let (frames, millis) = self
            .renderers
            .recorded
            .lock()
            .unwrap()
            .remove(&asked.load(Ordering::SeqCst))
            .unwrap_or_default();
        let bytes = write_whole(&target, &mov)?;
        tracing::info!(pane, bytes, frames, millis, path = %target.display(), "pane recording written");
        Ok(Recording {
            bytes,
            frames,
            millis,
        })
    }

    fn write_png(&self, pane: &str, target: &Path, png: &[u8]) -> Result<u64, String> {
        let bytes = write_whole(target, png)?;
        tracing::info!(pane, bytes, path = %target.display(), "pane shot written");
        Ok(bytes)
    }

    /// Send the request `make(id)` builds to each connected client that renders, newest first,
    /// until one answers; returns its answer. A refusal moves on to the next client. When every
    /// client refused, `refused_as` prefixes their messages (`cannot draw pane P: …`), and without
    /// it they stand alone, so a page's own error reads as the page wrote it. With `png`, an
    /// answer that is not a PNG counts as a refusal.
    async fn ask_clients(
        &self,
        pane: &str,
        side: Side,
        refused_as: Option<&str>,
        png: bool,
        wait: Duration,
        make: impl Fn(u64) -> ServerMsg,
    ) -> Result<Vec<u8>, String> {
        let mut refusals: Vec<String> = Vec::new();
        let clients: Vec<_> = self
            .renderers
            .offered
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|o| o.phone.is_some() == (side == Side::Phone))
            .map(|o| (o.peer, o.out.clone()))
            .collect();
        if clients.is_empty() {
            return Err(match side {
                Side::Phone => "the phone is not attached".to_string(),
                Side::Mac => format!(
                    "no client is connected that can draw pane {pane} (is the Mac app running?)"
                ),
            });
        }
        for (peer, out) in clients {
            match self.ask_client(pane, peer, out, png, wait, &make).await {
                Ok(bytes) => return Ok(bytes),
                Err(message) => refusals.push(message),
            }
        }
        if refusals.iter().all(|m| m == NOT_SHOWN) {
            return Err(format!("no connected client shows pane {pane}"));
        }
        match refused_as {
            Some(verb) => Err(format!("{verb} pane {pane}: {}", refusals.join("; "))),
            None => Err(refusals.join("; ")),
        }
    }

    /// Send the request `make(id)` builds to one client and wait `wait` for its answer. With
    /// `png`, an answer that is not a PNG is an error.
    async fn ask_client(
        &self,
        pane: &str,
        peer: SocketAddr,
        out: mpsc::Sender<ServerMsg>,
        png: bool,
        wait: Duration,
        make: &impl Fn(u64) -> ServerMsg,
    ) -> Result<Vec<u8>, String> {
        let id = self.renderers.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.renderers
            .pending
            .lock()
            .unwrap()
            .insert(id, (peer, tx));
        // A connection that closed between the snapshot the caller took and the insert would
        // never wake this request: `gone` ran before it was pending.
        let still_offered = self
            .renderers
            .offered
            .lock()
            .unwrap()
            .iter()
            .any(|o| o.peer == peer);
        if !still_offered {
            self.renderers.pending.lock().unwrap().remove(&id);
            tracing::info!(%peer, id, pane, "render skipped: the client is gone");
            return Err("the client disconnected before it answered".into());
        }
        let sent = tokio::time::timeout(RENDER_WAIT, out.send(make(id))).await;
        let sent = matches!(sent, Ok(Ok(())));
        tracing::info!(%peer, id, pane, sent, "render requested");
        let answer = if !sent {
            Err("the client's connection is closed".to_string())
        } else {
            match tokio::time::timeout(wait, rx).await {
                Ok(Ok(answer)) => answer,
                Ok(Err(_)) => Err("the client dropped the request".to_string()),
                Err(_) => Err(format!("no answer within {}s", wait.as_secs())),
            }
        };
        self.renderers.pending.lock().unwrap().remove(&id);
        match answer {
            Ok(bytes) if png && !bytes.starts_with(PNG_SIGNATURE) => {
                tracing::warn!(%peer, id, pane, bytes = bytes.len(), "render answer is not a PNG");
                Err("the client's image is not a PNG".into())
            }
            Ok(bytes) => Ok(bytes),
            Err(message) => {
                tracing::info!(%peer, id, pane, %message, "render refused");
                Err(message)
            }
        }
    }

    /// Where one browser is, asked of the Mac app (`phone` None) or of the paired phone
    /// `phone` names: its page and title, in 3 seconds at most.
    pub async fn pane_where_of(&self, pane: &str, phone: Option<&str>) -> Result<Where, String> {
        let text = match phone {
            None => self.ask_where(pane, Side::Mac, None).await?,
            Some(name) => self.ask_where(pane, Side::Phone, Some(name)).await?,
        };
        parse_where(&text)
    }

    async fn ask_where(&self, pane: &str, side: Side, phone: Option<&str>) -> Result<String, String> {
        let make = |id| ServerMsg::InspectPane {
            id,
            pane: pane.to_string(),
            action: "where".into(),
            arg: None,
            by: None,
        };
        let ask = async {
            match phone {
            None => {
                self.ask_clients(pane, side, None, false, WHERE_WAIT, make)
                    .await
            }
            Some(name) => {
                let found = self
                    .renderers
                    .offered
                    .lock()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|o| o.phone.as_deref() == Some(name))
                    .map(|o| (o.peer, o.out.clone()));
                let Some((peer, out)) = found else {
                    return Err("the phone is not attached".to_string());
                };
                self.ask_client(pane, peer, out, false, WHERE_WAIT, &make)
                    .await
                    .map_err(|m| {
                        if m == NOT_SHOWN {
                            format!("no connected client shows pane {pane}")
                        } else {
                            m
                        }
                    })
            }
            }
        };
        // One deadline for the whole side, sends included, however many clients it has.
        let bytes = tokio::time::timeout(WHERE_WAIT, ask)
            .await
            .map_err(|_| format!("no answer within {}s", WHERE_WAIT.as_secs()))??;
        String::from_utf8(bytes).map_err(|e| format!("the page's answer is not UTF-8: {e}"))
    }

    /// The names of the paired phones that offered rendering, each once.
    pub fn attached_phones(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for o in self.renderers.offered.lock().unwrap().iter() {
            if let Some(name) = &o.phone {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    /// Whether a Mac app offered rendering.
    pub fn mac_attached(&self) -> bool {
        self.renderers.offered.lock().unwrap().iter().any(|o| o.phone.is_none())
    }
}

/// The longest a recording may run.
const MAX_RECORD_SECONDS: u64 = 60;

/// What `pane_record` wrote.
pub struct Recording {
    pub bytes: u64,
    pub frames: u64,
    pub millis: u64,
}

/// A browser's page, as it answers `where`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Where {
    pub url: String,
    pub title: String,
}

/// Read the `{"url": …, "title": …}` a browser answers `where` with.
fn parse_where(text: &str) -> Result<Where, String> {
    #[derive(serde::Deserialize)]
    struct Raw {
        url: String,
        title: String,
    }
    let raw: Raw = serde_json::from_str(text)
        .map_err(|e| format!("the browser's answer to where is not {{url, title}}: {e}"))?;
    if raw.url.is_empty() {
        return Err("the browser answered where with no url".into());
    }
    Ok(Where {
        url: raw.url,
        title: raw.title,
    })
}

/// What `pane_inspect` produced.
pub enum InspectOutcome {
    /// The page's text, for `eval` and `dom`.
    Text(String),
    /// Bytes written to the file, for `shot`.
    Shot(u64),
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

/// Decode a `rendered` frame's image or a `recorded` frame's movie.
pub(crate) fn decode_base64(png: &str) -> Answer {
    base64::engine::general_purpose::STANDARD
        .decode(png)
        .map_err(|e| format!("the image is not base64: {e}"))
}
