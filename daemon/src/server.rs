use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use wheel_proto::{decode_bytes, encode_bytes, ClientMsg, ServerMsg};

use crate::lead::{Lead, LeadAttachment, LeadConfig};
use crate::perms::Perms;
use crate::pty::{Attachment, Term, TermEvent};
use crate::spaces::Spaces;

/// Messages queued for one client. When a client stops reading, its terminal forwarders block
/// here, fall behind the broadcast, and resync with a fresh replay instead of growing memory.
const OUTBOX: usize = 256;

pub struct AppState {
    started: Instant,
    log_path: String,
    token: String,
    pub(crate) next_id: AtomicU64,
    pub(crate) terms: Mutex<HashMap<String, Arc<Term>>>,
    /// Daemon-wide events (permission requests, lead state, layout) every connection forwards.
    pub(crate) global: broadcast::Sender<ServerMsg>,
    perms: Arc<Perms>,
    lead: Mutex<Option<Arc<Lead>>>,
    pub(crate) spaces: Mutex<Spaces>,
    /// Where the layout is saved; None keeps it in memory only.
    pub(crate) state_file: Mutex<Option<PathBuf>>,
}

impl AppState {
    pub fn new(log_path: String, token: String) -> Arc<AppState> {
        let (global, _) = broadcast::channel(1024);
        Arc::new(AppState {
            started: Instant::now(),
            log_path,
            token,
            next_id: AtomicU64::new(1),
            terms: Mutex::new(HashMap::new()),
            perms: Arc::new(Perms::new(global.clone())),
            global,
            lead: Mutex::new(None),
            spaces: Mutex::new(Spaces::default()),
            state_file: Mutex::new(None),
        })
    }

    /// Start a shell on a new PTY and keep it.
    pub(crate) async fn open_term(
        &self,
        cwd: String,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<Term>, String> {
        let id = format!("t{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let spawned = {
            let (id, cwd) = (id.clone(), cwd.clone());
            tokio::task::spawn_blocking(move || Term::spawn(id, cwd, cols.max(1), rows.max(1)))
                .await
        };
        match spawned {
            Ok(Ok(term)) => {
                tracing::info!(term = %id, pid = ?term.pid, %cwd, cols, rows, "pty spawned");
                self.terms.lock().unwrap().insert(id, term.clone());
                Ok(term)
            }
            Ok(Err(e)) => {
                tracing::error!(error = %e, %cwd, "pty spawn failed");
                Err(format!("spawn failed: {e}"))
            }
            Err(e) => Err(format!("spawn task failed: {e}")),
        }
    }

    /// Start the lead session. Call once, from inside the tokio runtime.
    pub fn start_lead(&self, config: LeadConfig) {
        let mcp = crate::mcp::for_lead(&config.mcp_url, &self.token);
        let lead = Lead::start(config, mcp, self.perms.clone(), self.global.clone());
        *self.lead.lock().unwrap() = Some(lead);
    }

    /// Let a request in, or say what to refuse it with. Browsers always send Origin and native
    /// clients never do, so no web page gets a shell or the lead's tools; everyone else needs the
    /// token. `via` names the route in the log.
    pub(crate) fn admit(
        &self,
        via: &str,
        headers: &HeaderMap,
        token: Option<&str>,
    ) -> Result<(), StatusCode> {
        if let Some(origin) = headers.get("origin") {
            tracing::warn!(via, ?origin, "refused: browser origin");
            return Err(StatusCode::FORBIDDEN);
        }
        // Compare every byte, so the time taken says nothing about how much of a guess matched.
        let matches = token.is_some_and(|t| {
            t.len() == self.token.len()
                && t.bytes()
                    .zip(self.token.bytes())
                    .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                    == 0
        });
        if !matches {
            tracing::warn!(via, "refused: missing or wrong token");
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(())
    }

    fn lead(&self) -> Result<Arc<Lead>, String> {
        self.lead
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "this daemon runs without a lead".to_string())
    }

    fn term(&self, id: &str) -> Option<Arc<Term>> {
        self.terms.lock().unwrap().get(id).cloned()
    }

    fn status(&self, req: Option<u64>) -> ServerMsg {
        let mut terms: Vec<_> = self
            .terms
            .lock()
            .unwrap()
            .values()
            .map(|t| t.info())
            .collect();
        terms.sort_by(|a, b| a.id.cmp(&b.id));
        let Spaces { spaces, active } = self.spaces();
        ServerMsg::Status {
            req,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: self.started.elapsed().as_secs(),
            log_path: self.log_path.clone(),
            terms,
            lead: self.lead.lock().unwrap().as_ref().map(|l| l.state()),
            perms: self.perms.list(),
            spaces,
            active,
        }
    }
}

/// Serve clients on `listener` until the process ends.
pub async fn serve(listener: TcpListener, state: Arc<AppState>) -> anyhow::Result<()> {
    let mcp = crate::mcp::router(state.clone(), listener.local_addr()?);
    let app = Router::new()
        .route("/ws", get(upgrade))
        .with_state(state)
        .merge(mcp);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Err(status) = state.admit(
        &format!("ws {peer}"),
        &headers,
        query.get("token").map(String::as_str),
    ) {
        return status.into_response();
    }
    ws.on_upgrade(move |socket| connection(socket, state, peer))
}

async fn connection(socket: WebSocket, state: Arc<AppState>, peer: SocketAddr) {
    tracing::info!(%peer, "ws connect");
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::channel::<ServerMsg>(OUTBOX);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let text = serde_json::to_string(&msg).expect("server messages serialize");
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    // Every connection sees permission requests and lead state changes, attached or not.
    let mut global = state.global.subscribe();
    let global_out = out.clone();
    let global_state = state.clone();
    let global_task = tokio::spawn(async move {
        loop {
            match global.recv().await {
                Ok(msg) => {
                    if global_out.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(%peer, skipped = n, "client fell behind on daemon events; sending a fresh status");
                    if global_out.send(global_state.status(None)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    // One output stream per terminal (and one for the lead) on this connection; attaching again replaces it.
    let mut forwards: HashMap<String, JoinHandle<()>> = HashMap::new();

    while let Some(Ok(frame)) = stream.next().await {
        let Message::Text(text) = frame else { continue };
        match serde_json::from_str::<ClientMsg>(&text) {
            Ok(msg) => handle(&state, msg, &out, &mut forwards, peer).await,
            Err(e) => {
                tracing::warn!(%peer, error = %e, "bad client message");
                let _ = out
                    .send(ServerMsg::Error {
                        req: None,
                        message: format!("bad message: {e}"),
                    })
                    .await;
            }
        }
    }

    for (_, task) in forwards {
        task.abort();
    }
    global_task.abort();
    writer.abort();
    tracing::info!(%peer, "ws close");
}

async fn handle(
    state: &Arc<AppState>,
    msg: ClientMsg,
    out: &mpsc::Sender<ServerMsg>,
    forwards: &mut HashMap<String, JoinHandle<()>>,
    peer: SocketAddr,
) {
    let reply = match msg {
        ClientMsg::Status { req } => Some(state.status(req)),
        ClientMsg::LeadAttach { req } => match state.lead() {
            Ok(lead) => {
                if let Some(old) =
                    forwards.insert("lead".into(), forward_lead(lead, req, out.clone(), peer))
                {
                    old.abort();
                }
                None
            }
            Err(message) => Some(ServerMsg::Error { req, message }),
        },
        ClientMsg::LeadSend { req, text } => {
            tracing::info!(%peer, chars = text.len(), "lead send");
            match state.lead().and_then(|lead| lead.send(text)) {
                Ok(turn) => req.map(|req| ServerMsg::LeadQueued { req, turn }),
                Err(message) => Some(ServerMsg::Error { req, message }),
            }
        }
        ClientMsg::LeadCancel { req } => match state.lead() {
            Ok(lead) => {
                lead.cancel();
                req.map(|req| ServerMsg::Ok { req })
            }
            Err(message) => Some(ServerMsg::Error { req, message }),
        },
        ClientMsg::PermAnswer { req, id, choice } => {
            match state.perms.answer(&id, &choice, &peer.to_string()) {
                Ok(_) => req.map(|req| ServerMsg::Ok { req }),
                Err(message) => Some(ServerMsg::Error { req, message }),
            }
        }
        ClientMsg::TermOpen {
            req,
            cwd,
            cols,
            rows,
        } => {
            let cwd = cwd.unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            match state.open_term(cwd, cols, rows).await {
                Ok(term) => Some(ServerMsg::TermOpened {
                    req,
                    id: term.id.clone(),
                    pid: term.pid,
                }),
                Err(message) => Some(ServerMsg::Error { req, message }),
            }
        }
        ClientMsg::SpaceCreate { req, cwd, name } => {
            created(req, state.space_create(cwd, name).map(|s| s.id))
        }
        ClientMsg::SpaceClose { req, id } => done(req, state.space_close(&id)),
        ClientMsg::Focus { req, space, pane } => {
            done(req, state.focus(space.as_deref(), pane.as_deref()))
        }
        ClientMsg::PaneSplit { req, pane, dir } => created(req, state.pane_split(&pane, dir)),
        ClientMsg::PaneClose { req, pane } => done(req, state.pane_close(&pane)),
        ClientMsg::PaneSetKind { req, pane, kind } => {
            done(req, state.pane_set_kind(&pane, kind).await)
        }
        ClientMsg::SplitRatio { req, split, ratio } => done(req, state.split_ratio(&split, ratio)),
        ClientMsg::TermAttach { req, id } => match state.term(&id) {
            Some(term) => {
                if let Some(old) = forwards.insert(id, forward(term, req, out.clone(), peer)) {
                    old.abort();
                }
                None
            }
            None => Some(ServerMsg::Error {
                req,
                message: format!("no terminal {id}"),
            }),
        },
        ClientMsg::TermDetach { id } => {
            if let Some(task) = forwards.remove(&id) {
                task.abort();
                tracing::info!(%peer, term = %id, "detach");
            }
            None
        }
        ClientMsg::TermInput { req, id, data } => {
            let result = state
                .term(&id)
                .ok_or_else(|| anyhow::anyhow!("no terminal {id}"))
                .and_then(|t| t.write(decode_bytes(&data)?));
            // Input is logged by size only: it carries whatever the user types, passwords included.
            tracing::debug!(term = %id, bytes = data.len(), "input");
            match result {
                Ok(()) => req.map(|req| ServerMsg::Ok { req }),
                Err(e) => Some(ServerMsg::Error {
                    req,
                    message: e.to_string(),
                }),
            }
        }
        ClientMsg::TermResize {
            req,
            id,
            cols,
            rows,
        } => {
            match state
                .term(&id)
                .ok_or_else(|| anyhow::anyhow!("no terminal {id}"))
                .and_then(|t| t.resize(cols.max(1), rows.max(1)))
            {
                Ok(()) => {
                    tracing::info!(term = %id, cols, rows, "resize");
                    req.map(|req| ServerMsg::Ok { req })
                }
                Err(e) => Some(ServerMsg::Error {
                    req,
                    message: e.to_string(),
                }),
            }
        }
    };
    if let Some(reply) = reply {
        let _ = out.send(reply).await;
    }
}

/// The reply to a request with no result of its own: `ok` when it carried a `req`.
fn done(req: Option<u64>, result: Result<(), String>) -> Option<ServerMsg> {
    match result {
        Ok(()) => req.map(|req| ServerMsg::Ok { req }),
        Err(message) => Some(ServerMsg::Error { req, message }),
    }
}

/// The reply to a request that made something: `created` with its id.
fn created(req: Option<u64>, result: Result<String, String>) -> Option<ServerMsg> {
    match result {
        Ok(id) => req.map(|req| ServerMsg::Created { req, id }),
        Err(message) => Some(ServerMsg::Error { req, message }),
    }
}

/// Stream one terminal to one client: replay, then live output. When the client falls far
/// enough behind that output was dropped, start over with a fresh replay.
fn forward(
    term: Arc<Term>,
    req: Option<u64>,
    out: mpsc::Sender<ServerMsg>,
    peer: SocketAddr,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut req = req;
        let mut resync = false;
        loop {
            let Attachment {
                replay,
                exited,
                mut rx,
            } = term.attach();
            tracing::info!(%peer, term = %term.id, replay_bytes = replay.len(), resync, "attach");
            let id = term.id.clone();
            if out
                .send(ServerMsg::TermAttached {
                    req: req.take(),
                    id: id.clone(),
                    replay_bytes: replay.len(),
                })
                .await
                .is_err()
            {
                return;
            }
            if !replay.is_empty()
                && out
                    .send(ServerMsg::TermOutput {
                        id: id.clone(),
                        data: encode_bytes(&replay),
                    })
                    .await
                    .is_err()
            {
                return;
            }
            if let Some(code) = exited {
                let _ = out.send(ServerMsg::TermExited { id, code }).await;
                return;
            }
            loop {
                match rx.recv().await {
                    Ok(TermEvent::Output(bytes)) => {
                        if out
                            .send(ServerMsg::TermOutput {
                                id: id.clone(),
                                data: encode_bytes(&bytes),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(TermEvent::Exited(code)) => {
                        let _ = out.send(ServerMsg::TermExited { id, code }).await;
                        return;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(%peer, term = %id, skipped, "client fell behind; resyncing");
                        resync = true;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    })
}

/// Stream the lead's conversation to one client: the transcript so far, then live events.
fn forward_lead(
    lead: Arc<Lead>,
    req: Option<u64>,
    out: mpsc::Sender<ServerMsg>,
    peer: SocketAddr,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut req = req;
        loop {
            let LeadAttachment {
                state,
                events,
                mut rx,
            } = lead.attach();
            tracing::info!(%peer, events = events.len(), "lead attach");
            if out
                .send(ServerMsg::LeadAttached {
                    req: req.take(),
                    state,
                    events,
                })
                .await
                .is_err()
            {
                return;
            }
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if out.send(ServerMsg::Lead { event }).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(%peer, skipped, "client fell behind the lead; resending the transcript");
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    })
}
