use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

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
use wheel_proto::{decode_bytes, encode_bytes, ClientMsg, ContextUse, ServerMsg};

use crate::perms::Perms;
use crate::pty::{Attachment, Term, TermEvent};
use crate::session::{Restart, Session, SessionAttachment, SessionConfig};
use crate::shell::{Control, Launch};
use crate::spaces::Spaces;

/// Messages queued for one client. When a client stops reading, its terminal forwarders block
/// here, fall behind the broadcast, and resync with a fresh replay instead of growing memory.
const OUTBOX: usize = 256;

/// The most replay bytes one `term_output` frame carries; a longer replay arrives as several
/// frames, so a large scrollback never becomes one WebSocket frame.
const REPLAY_CHUNK: usize = 64 * 1024;

/// Who ends a terminal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Owner {
    /// A pane shows it; it ends when the pane closes or changes kind, so the pane can still
    /// replay a shell that exited.
    Pane,
    /// Opened with `term_open`; it ends with `term_close`, or once its shell has exited and a
    /// client has been sent the exit. Until then `status` lists it with `alive: false`.
    Client,
}

pub(crate) struct Held {
    pub(crate) term: Arc<Term>,
    pub(crate) owner: Owner,
}

/// The waits around an agent starting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentTiming {
    pub restart: Restart,
    /// How long `start_pane` waits for a new agent to be ready or give up.
    pub start_wait: Duration,
}

impl Default for AgentTiming {
    fn default() -> Self {
        AgentTiming {
            restart: Restart::default(),
            start_wait: Duration::from_secs(60),
        }
    }
}

pub struct AppState {
    started: Instant,
    log_path: String,
    token: String,
    pub(crate) next_id: AtomicU64,
    pub(crate) terms: Mutex<HashMap<String, Held>>,
    /// Daemon-wide events (permission requests, every session's state, layout) every connection
    /// forwards.
    pub(crate) global: broadcast::Sender<ServerMsg>,
    pub(crate) perms: Arc<Perms>,
    /// Every running agent session by name: the ones agent panes own.
    pub(crate) sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// The ACP agent command every agent pane runs. None when the daemon runs without one.
    pub(crate) agent_command: Mutex<Option<String>>,
    pub(crate) agent_timing: Mutex<AgentTiming>,
    pub(crate) spaces: Mutex<Spaces>,
    /// Top-level keys of the saved file this build does not know, written back on every save.
    pub(crate) saved_extra: Mutex<serde_json::Map<String, serde_json::Value>>,
    /// Where the layout is saved; None keeps it in memory only.
    pub(crate) state_file: Mutex<Option<PathBuf>>,
    /// This instance's own directory, holding its token and saved layout. `machine` artifacts live
    /// under it, so every project on this machine can open them.
    pub(crate) instance_dir: PathBuf,
    /// Every Claude login's usage, as the usage strip shows it.
    pub(crate) usage: crate::usage::Usage,
    /// The program a refresh starts as its warden: `wheeld` itself.
    pub(crate) warden: PathBuf,
    /// One writer at a time for a `wheel.toml`, whichever file: a settings write is a read, a
    /// change and a rename, and two of them interleaved would drop a key.
    pub(crate) settings_write: Mutex<()>,
    /// Whether a coalesced save of the layout is already scheduled, so a burst of browser
    /// navigations is one write rather than one each.
    pub(crate) save_pending: AtomicBool,
    /// Panes whose conversation is moving between its chat and its terminal, so a second switch
    /// cannot start a second program on the same conversation.
    pub(crate) switching_views: Mutex<std::collections::HashSet<String>>,
    /// How many times the layout has been written, reported by `status`: the one thing a program
    /// can compare to tell a coalesced save from a save per change.
    pub(crate) saves: AtomicU64,
    /// What each web pane watches — its artifact's folder and its own data directory — keyed by
    /// pane id.
    pub(crate) watchers: Mutex<HashMap<String, Vec<crate::web::Watch>>>,
    /// The refresh runner of each web pane whose artifact declares a command, keyed by pane id.
    pub(crate) refreshers: Mutex<crate::refresh::Refreshers>,
    /// The approvals a person answered "always" to, in a file in the instance directory. The only
    /// durable artifact state the daemon owns, and deliberately not in the saved layout.
    pub(crate) trust: Arc<Mutex<crate::refresh::TrustFile>>,
    /// This state, for callbacks a session runs later and for a task that saves the layout after
    /// the caller has let go of its lock.
    pub(crate) me: Weak<AppState>,
    /// The connections that offered to draw panes, and the draws waiting on them.
    pub(crate) renderers: crate::render::Renderers,
    /// The Claude config directory every agent pane without one of its own runs with,
    /// as `CLAUDE_CONFIG_DIR`. None leaves them the daemon's own environment.
    pub(crate) agent_config_dir: Mutex<Option<PathBuf>>,
    /// Where the `wheel` command finds this daemon, handed to every terminal and agent process.
    /// None for a daemon nothing connects to by address (a test that drives it in process).
    pub(crate) control: Mutex<Option<Control>>,
}

impl AppState {
    /// `warden` is the program a refresh starts as its warden ([`crate::refresh::WARDEN_ARG`]):
    /// the `wheeld` binary.
    pub fn new(
        log_path: String,
        token: String,
        instance_dir: PathBuf,
        warden: PathBuf,
    ) -> Arc<AppState> {
        // The daemon's own directory: its machine settings and trust file are written there
        // whether or not anything else made it first.
        if let Err(e) = std::fs::create_dir_all(&instance_dir) {
            tracing::warn!(dir = %instance_dir.display(), error = %e, "cannot create the instance directory");
        }
        let (global, _) = broadcast::channel(1024);
        Arc::new_cyclic(|me| AppState {
            started: Instant::now(),
            log_path,
            token,
            next_id: AtomicU64::new(1),
            terms: Mutex::new(HashMap::new()),
            perms: Arc::new(Perms::new(global.clone())),
            global,
            sessions: Mutex::new(HashMap::new()),
            agent_command: Mutex::new(None),
            agent_timing: Mutex::new(AgentTiming::default()),
            spaces: Mutex::new(Spaces::default()),
            saved_extra: Mutex::new(Default::default()),
            state_file: Mutex::new(None),
            trust: Arc::new(Mutex::new(crate::refresh::TrustFile::new(&instance_dir))),
            settings_write: Mutex::new(()),
            save_pending: AtomicBool::new(false),
            switching_views: Mutex::new(Default::default()),
            saves: AtomicU64::new(0),
            instance_dir,
            usage: crate::usage::Usage::default(),
            warden,
            watchers: Mutex::new(HashMap::new()),
            refreshers: Mutex::new(crate::refresh::Refreshers::new()),
            me: me.clone(),
            renderers: Default::default(),
            agent_config_dir: Mutex::new(None),
            control: Mutex::new(None),
        })
    }

    /// Tell every terminal and agent started from here where to reach this daemon with the `wheel`
    /// command, and write the shell integration they start with. Call before restoring a layout.
    pub fn set_control(&self, url: String, token_file: PathBuf) {
        let cli = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("wheel")))
            .filter(|cli| cli.is_file());
        if let Err(e) = crate::shell::install(&self.instance_dir, cli.as_deref()) {
            tracing::warn!(dir = %self.instance_dir.display(), error = %e, "cannot write the shell integration");
        }
        tracing::info!(%url, token_file = %token_file.display(), cli = ?cli, "wheel control for terminals and agents");
        *self.control.lock().unwrap() = Some(Control { url, token_file });
    }

    /// How to start a terminal for `seat` (its space and pane), running `shell` or the login shell.
    fn term_launch(&self, shell: Option<String>, seat: Option<(&str, &str)>) -> Launch {
        match self.control.lock().unwrap().as_ref() {
            Some(control) => crate::shell::launch(
                shell,
                std::env::var("SHELL").ok().as_deref(),
                std::env::var("ZDOTDIR").ok().as_deref(),
                control,
                &self.instance_dir,
                seat,
            ),
            None => Launch {
                program: shell,
                ..Default::default()
            },
        }
    }

    /// The environment for the agent in `pane` of `space`: where `wheel` reaches this daemon, and a
    /// `PATH` that finds it. Empty when this daemon has no address to give.
    pub(crate) fn agent_env(&self, space: &str, pane: &str) -> Vec<(String, String)> {
        let Some(control) = self.control.lock().unwrap().clone() else {
            return Vec::new();
        };
        let mut env = crate::shell::env(&control, &self.instance_dir, Some((space, pane)));
        env.push(("PATH".into(), crate::shell::agent_path(&self.instance_dir)));
        env
    }

    /// Start `shell`, else the login shell, on a new PTY and keep it until its owner ends it.
    /// `seat` is the space and pane the terminal belongs to.
    pub(crate) async fn open_term(
        &self,
        cwd: String,
        shell: Option<String>,
        cols: u16,
        rows: u16,
        owner: Owner,
        seat: Option<(&str, &str)>,
    ) -> Result<Arc<Term>, String> {
        let id = format!("t{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let launch = self.term_launch(shell, seat);
        let spawned = {
            let (id, cwd) = (id.clone(), cwd.clone());
            tokio::task::spawn_blocking(move || {
                Term::spawn(id, cwd, launch, cols.max(1), rows.max(1))
            })
            .await
        };
        match spawned {
            Ok(Ok(term)) => {
                tracing::info!(term = %id, pid = ?term.pid, %cwd, cols, rows, ?owner, "pty spawned");
                let held = Held {
                    term: term.clone(),
                    owner,
                };
                self.terms.lock().unwrap().insert(id, held);
                Ok(term)
            }
            Ok(Err(e)) => {
                tracing::error!(error = %e, %cwd, "pty spawn failed");
                Err(format!("spawn failed: {e}"))
            }
            Err(e) => Err(format!("spawn task failed: {e}")),
        }
    }

    /// Let agent panes run `command`. Call before restoring a layout, which starts a saved agent
    /// pane's agent with it.
    pub fn set_agent_command(&self, command: String) {
        *self.agent_command.lock().unwrap() = Some(command);
    }

    /// Replace how long agents take to restart and how long `start_pane` waits for one. Tests
    /// shorten these; the daemon runs on the defaults.
    pub fn set_agent_timing(&self, timing: AgentTiming) {
        *self.agent_timing.lock().unwrap() = timing;
    }

    /// Start agent session `name` for `pane`, in `cwd`, with the person's Claude configuration (or
    /// `claude_dir`, else the one set for every agent) and Wheel's brief. `acp_session` is a
    /// conversation to load: the one a restored pane saved, or one switched over from a terminal.
    /// A load that fails falls through to a new one.
    pub(crate) fn spawn_agent(
        &self,
        name: &str,
        (space, pane): (&str, &str),
        cwd: &str,
        acp_session: Option<String>,
        claude_dir: Option<String>,
        context: Option<ContextUse>,
    ) -> Result<(), String> {
        let command = self
            .agent_command
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "this daemon runs without an agent command".to_string())?;
        let config = SessionConfig {
            command,
            cwd: cwd.into(),
            claude_dir: claude_dir
                .map(PathBuf::from)
                .or_else(|| self.agent_config_dir()),
            brief: Some(crate::brief::text(pane, name)),
            env: self.agent_env(space, pane),
            restored: acp_session.is_some(),
            acp_session,
            context,
            on_acp_session: Some(Arc::new({
                let me = self.me.clone();
                move || {
                    if let Some(state) = me.upgrade() {
                        state.acp_sessions_changed();
                    }
                }
            })),
            restart: self.agent_timing.lock().unwrap().restart,
        };
        let session = Session::start(
            name.into(),
            Some(pane.into()),
            config,
            self.perms.clone(),
            self.global.clone(),
        );
        self.sessions.lock().unwrap().insert(name.into(), session);
        Ok(())
    }

    /// End an agent session for good; its agent process is killed.
    pub(crate) fn end_session(&self, name: &str) {
        let session = self.sessions.lock().unwrap().remove(name);
        if let Some(session) = session {
            session.stop(&self.perms);
        }
    }

    /// [`AppState::end_session`], returning once the agent's process is gone.
    pub(crate) async fn end_session_wait(&self, name: &str) {
        let session = self.sessions.lock().unwrap().remove(name);
        if let Some(session) = session {
            session.stop_and_wait(&self.perms).await;
        }
    }

    /// End every agent session; their agent processes are killed. A daemon
    /// that stops serving without its process ending (an in-process restart in a test) calls this
    /// first, so its agents do not outlive it.
    pub fn stop_sessions(&self) {
        let sessions: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, s)| s)
            .collect();
        for session in sessions {
            session.stop(&self.perms);
        }
    }

    /// Let a request in, or say what to refuse it with. Browsers always send Origin and native
    /// clients never do, so no web page gets a shell or an agent; everyone else needs the
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
        self.check_token(via, token)
    }

    /// Let a request carrying `token` in, or refuse it with 401.
    pub(crate) fn check_token(&self, via: &str, token: Option<&str>) -> Result<(), StatusCode> {
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

    pub(crate) fn session(&self, name: &str) -> Result<Arc<Session>, String> {
        match self.sessions.lock().unwrap().get(name) {
            Some(session) => Ok(session.clone()),
            None => Err(format!("no session {name}")),
        }
    }

    pub(crate) fn term(&self, id: &str) -> Option<Arc<Term>> {
        self.terms.lock().unwrap().get(id).map(|h| h.term.clone())
    }

    /// Forget a `term_open` terminal whose exit a client has just been sent. Clients already
    /// streaming it hold their own reference and get the exit too; attaching later finds nothing.
    fn exit_delivered(&self, id: &str) {
        let mut terms = self.terms.lock().unwrap();
        if terms.get(id).is_some_and(|h| h.owner == Owner::Client) {
            terms.remove(id);
            tracing::info!(term = %id, "exited terminal dropped");
        }
    }

    /// Hang up a terminal opened with `term_open` and forget it.
    fn term_close(&self, id: &str) -> Result<(), String> {
        let mut terms = self.terms.lock().unwrap();
        match terms.get(id).map(|h| h.owner) {
            None => Err(format!("no terminal {id}")),
            Some(Owner::Pane) => Err(format!(
                "terminal {id} belongs to a pane; close the pane or change its kind instead"
            )),
            Some(Owner::Client) => {
                if let Some(held) = terms.remove(id) {
                    held.term.kill();
                }
                tracing::info!(term = %id, "term closed");
                Ok(())
            }
        }
    }

    fn status(&self, req: Option<u64>) -> ServerMsg {
        let mut terms: Vec<_> = self
            .terms
            .lock()
            .unwrap()
            .values()
            .map(|h| h.term.info())
            .collect();
        terms.sort_by(|a, b| a.id.cmp(&b.id));
        let Spaces { spaces, active } = self.listing();
        ServerMsg::Status {
            req,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: self.started.elapsed().as_secs(),
            log_path: self.log_path.clone(),
            terms,
            perms: self.perms.list(),
            spaces,
            active,
            layout_saves: self.saves.load(Ordering::SeqCst),
            watches: self.watches(),
            refreshes: self.refreshes(),
            usage_board: self.usage_board(),
        }
    }

    /// Run every agent pane without a Claude config directory of its own with `dir`. Call before
    /// restoring a layout, which starts agents with it.
    pub fn set_agent_config_dir(&self, dir: Option<PathBuf>) {
        *self.agent_config_dir.lock().unwrap() = dir;
    }

    /// The Claude config directory set for every agent, when one is.
    pub(crate) fn agent_config_dir(&self) -> Option<PathBuf> {
        self.agent_config_dir.lock().unwrap().clone()
    }
}

/// Serve clients on `listener` until the process ends.
pub async fn serve(listener: TcpListener, state: Arc<AppState>) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/ws", get(upgrade))
        .with_state(state.clone())
        .merge(crate::web::router(state));
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
    // Every connection sees permission requests and session state changes, attached or not.
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
    // One output stream per terminal and session on this connection; attaching again replaces it.
    let mut forwards: HashMap<String, JoinHandle<()>> = HashMap::new();

    while let Some(Ok(frame)) = stream.next().await {
        let Message::Text(text) = frame else { continue };
        match serde_json::from_str::<ClientMsg>(&text) {
            Ok(msg) => handle(&state, msg, &out, &mut forwards, peer).await,
            Err(e) => {
                tracing::warn!(%peer, error = %e, "bad client message");
                let _ = out.send(unparsed_reply(&text, &e)).await;
            }
        }
    }

    for (_, task) in forwards {
        task.abort();
    }
    state.renderers.gone(peer);
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
        ClientMsg::SessionAttach { req, session } => match state.resolve_agent(&session) {
            Ok(found) => {
                let found = found.session;
                // Keyed apart from terminal ids, which share the map, and by the session's name,
                // so attaching by pane id and by name is one stream.
                let key = format!("session:{}", found.name);
                if let Some(old) =
                    forwards.insert(key, forward_session(found, req, out.clone(), peer))
                {
                    old.abort();
                }
                None
            }
            Err(message) => Some(failed(req, message)),
        },
        ClientMsg::Usage { req, due_only } => {
            // On a task of its own: a read calls the network.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let Some(req) = req else { return };
                state.usage_read(!due_only).await;
                let reply = ServerMsg::Usage {
                    req,
                    board: state.usage_board(),
                };
                let _ = out.send(reply).await;
            });
            None
        }
        ClientMsg::SessionDetach { session } => {
            let session = state
                .resolve_agent(&session)
                .map_or(session, |a| a.session.name.clone());
            if let Some(task) = forwards.remove(&format!("session:{session}")) {
                task.abort();
                tracing::info!(%peer, %session, "session detach");
            }
            None
        }
        ClientMsg::SessionNoticeDismiss { req, session } => {
            match state.resolve_agent(&session).map(|a| a.session) {
                Ok(found) => {
                    tracing::info!(%peer, session = %found.name, "session notice dismissed");
                    found.set_notice(None);
                    req.map(|req| ServerMsg::Ok { req })
                }
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::SessionSend { req, session, text } => {
            // Sized, never the text: a prompt can quote anything.
            tracing::info!(%peer, %session, chars = text.len(), "session send");
            match state.resolve_agent(&session).and_then(|a| {
                let turn = a.session.send(text)?;
                Ok((a.session.name.clone(), turn))
            }) {
                Ok((session, turn)) => {
                    req.map(|req| ServerMsg::SessionQueued { req, session, turn })
                }
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::SessionWait {
            req,
            session,
            timeout_secs,
        } => {
            // On a task of its own, so this connection keeps reading while it waits.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let result = if (0.0..=3600.0).contains(&timeout_secs) {
                    state
                        .agent_wait(&session, std::time::Duration::from_secs_f64(timeout_secs))
                        .await
                } else {
                    Err(format!("timeout_secs is {timeout_secs}; use 0 to 3600"))
                };
                let reply = match result {
                    Ok(w) => req.map(|req| ServerMsg::SessionWaited {
                        req,
                        session: w.session,
                        pane: (!w.pane.is_empty()).then_some(w.pane),
                        state: w.state,
                        ready: w.ready,
                    }),
                    Err(message) => Some(failed(req, message)),
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::SessionCancel { req, session } => {
            match state
                .resolve_agent(&session)
                .and_then(|a| a.session.cancel())
            {
                Ok(_) => req.map(|req| ServerMsg::Ok { req }),
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::SessionConfigSet {
            req,
            session,
            config,
            value,
        } => {
            let result = match state.resolve_agent(&session).map(|a| a.session) {
                Ok(found) => found.set_config(config.clone(), value.clone()).await,
                Err(message) => Err(message),
            };
            match result {
                Ok(()) => {
                    tracing::info!(%peer, %session, %config, %value, "session config set");
                    req.map(|req| ServerMsg::Ok { req })
                }
                Err(message) => {
                    tracing::warn!(%peer, %session, %config, %value, %message, "session config set failed");
                    Some(failed(req, message))
                }
            }
        }
        // On tasks of their own, never inline: each waits on the agent, which takes up to a minute
        // to load a conversation, and this connection has to keep reading frames meanwhile.
        ClientMsg::SessionConversationLs { req, session } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let listed = match state.session(&session) {
                    Ok(found) => found.conversations().await,
                    Err(message) => Err(message),
                };
                let reply = match listed {
                    Ok(conversations) => req.map(|req| ServerMsg::Conversations {
                        req,
                        session,
                        conversations,
                    }),
                    Err(message) => {
                        tracing::warn!(%peer, %message, "session conversation ls failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::SessionConversationNew { req, session } => {
            spawn_switch(state, out, req, session, None, peer);
            None
        }
        ClientMsg::SessionConversationResume { req, session, id } => {
            spawn_switch(state, out, req, session, Some(id), peer);
            None
        }
        ClientMsg::PermAnswer { req, id, choice } => {
            match state.perms.answer(&id, &choice, &peer.to_string()) {
                Ok(_) => req.map(|req| ServerMsg::Ok { req }),
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::TermOpen {
            req,
            cwd,
            cols,
            rows,
        } => {
            let cwd = cwd.unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            match state
                .open_term(cwd, None, cols, rows, Owner::Client, None)
                .await
            {
                Ok(term) => Some(ServerMsg::TermOpened {
                    req,
                    id: term.id.clone(),
                    pid: term.pid,
                }),
                Err(message) => Some(failed(req, message)),
            }
        }
        ClientMsg::SpaceCreate { req, cwd, name } => {
            created(req, state.space_create(cwd, name).map(|s| s.id))
        }
        ClientMsg::SpaceClose { req, id } => done(req, state.space_close(&id)),
        ClientMsg::Focus { req, space, pane } => {
            done(req, state.focus(space.as_deref(), pane.as_deref()))
        }
        ClientMsg::PaneSplit { req, pane, dir } => created(req, state.pane_split(&pane, dir, true)),
        ClientMsg::PaneSwap { req, pane, with } => done(req, state.pane_swap(&pane, &with)),
        ClientMsg::PaneMove {
            req,
            pane,
            target,
            dir,
        } => done(req, state.pane_move(&pane, &target, dir)),
        ClientMsg::PaneClose { req, pane } => done(req, state.pane_close(&pane)),
        ClientMsg::PaneSetKind {
            req,
            pane,
            kind,
            artifact,
            scope,
            url,
        } => done(
            req,
            state
                .pane_set_kind(&pane, kind, artifact, scope, url)
                .await
                .map(|_| ()),
        ),
        // On a task of its own, never inline: an agent takes up to a minute to come up, and this
        // connection has to keep reading frames meanwhile — terminal input, session attaches and
        // permission answers are on the same socket.
        ClientMsg::PaneStart {
            req,
            pane,
            kind,
            message,
        } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let started = match state.space_holding(&pane) {
                    Ok(space) => {
                        state
                            .pane_start(&space, Some(pane), kind, message, true)
                            .await
                    }
                    Err(e) => Err(e),
                };
                let reply = match (req, started) {
                    (Some(req), Ok(s)) => Some(ServerMsg::Started {
                        req,
                        pane: s.pane,
                        session: s.session,
                        state: s.state,
                    }),
                    (_, Ok(_)) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%peer, %message, "pane start failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        // On a task of its own: a switch waits for the other view's process to stop.
        ClientMsg::PaneView { req, pane, to } => {
            let (state, out) = (state.clone(), out.clone());
            tokio::spawn(async move {
                let reply = match (req, state.pane_view(&pane, to).await) {
                    (Some(req), Ok((view, conversation, switch_ms))) => Some(ServerMsg::PaneView {
                        req,
                        pane,
                        view,
                        conversation,
                        switch_ms,
                    }),
                    (_, Ok(_)) => None,
                    (req, Err(message)) => {
                        tracing::warn!(%peer, %pane, ?to, %message, "pane view switch failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::PaneBrowserUrl { req, pane, url } => match state.pane_browser_url(&pane, &url) {
            Ok(_) => req.map(|req| ServerMsg::Ok { req }),
            Err(message) => {
                tracing::warn!(%peer, %pane, %message, "pane browser url failed");
                Some(failed(req, message))
            }
        },
        ClientMsg::PaneBrowserOpen { req, pane, text } => {
            match state.pane_browser_open(&pane, &text) {
                Ok(url) => req.map(|req| ServerMsg::Opened { req, pane, url }),
                Err(message) => {
                    tracing::warn!(%peer, %pane, %message, "pane browser open failed");
                    Some(failed(req, message))
                }
            }
        }
        ClientMsg::RenderOffer {} => {
            state.renderers.offer(peer, out.clone());
            None
        }
        ClientMsg::Rendered { id, png } => {
            tracing::info!(%peer, id, base64_chars = png.len(), "rendered");
            state
                .renderers
                .answer(peer, id, crate::render::decode_png(&png));
            None
        }
        ClientMsg::RenderRefused { id, message } => {
            tracing::info!(%peer, id, %message, "render refused by client");
            state.renderers.answer(peer, id, Err(message));
            None
        }
        ClientMsg::PaneShot { req, pane, path } => {
            // On a task of its own: a client takes as long as it takes to draw.
            let state = state.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let reply = match state.pane_shot(&pane, &path).await {
                    Ok(bytes) => req.map(|req| ServerMsg::PaneShot {
                        req,
                        pane,
                        path,
                        bytes,
                    }),
                    Err(message) => {
                        tracing::warn!(%peer, %pane, %message, "pane shot failed");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::SplitRatio { req, split, ratio } => done(req, state.split_ratio(&split, ratio)),
        ClientMsg::ArtifactNew {
            req,
            pane,
            name,
            scope,
            space,
            path,
        } => one_artifact(
            req,
            state.artifact_new(&name, scope, space.as_deref(), pane, path.as_deref()),
            "artifact new",
        ),
        ClientMsg::ArtifactAdd {
            req,
            path,
            name,
            scope,
            space,
            title,
            command,
            every,
            pane,
        } => one_artifact(
            req,
            state.artifact_add(
                &path,
                name.as_deref(),
                scope,
                space.as_deref(),
                crate::artifacts::Declared {
                    title,
                    command,
                    every,
                    pane,
                },
            ),
            "artifact add",
        ),
        ClientMsg::ArtifactRemove {
            req,
            name,
            scope,
            space,
        } => one_artifact(
            req,
            state.artifact_remove(&name, scope, space.as_deref()),
            "artifact remove",
        ),
        ClientMsg::ArtifactShow {
            req,
            name,
            scope,
            space,
        } => one_artifact(
            req,
            state.artifact_show(&name, scope, space.as_deref()),
            "artifact show",
        ),
        ClientMsg::SettingsLs { req, space } => {
            settings_reply(req, state.settings_ls(space.as_deref()), "settings ls")
        }
        ClientMsg::SettingSet {
            req,
            key,
            value,
            space,
            machine,
        } => {
            let written = state.setting_set(&key, &value, space.as_deref(), machine);
            settings_changed(state, req, written, &key, &format!("setting set {key}"))
        }
        ClientMsg::SettingUnset {
            req,
            key,
            space,
            machine,
        } => {
            let removed = state.setting_unset(&key, space.as_deref(), machine);
            settings_changed(state, req, removed, &key, &format!("setting unset {key}"))
        }
        ClientMsg::ArtifactLs { req, space, pane } => {
            match state.artifact_ls(space.as_deref(), pane) {
                Ok((artifacts, resolves)) => req.map(|req| ServerMsg::Artifacts {
                    req,
                    artifacts,
                    resolves,
                }),
                Err(message) => {
                    tracing::warn!(%peer, %message, "artifact ls failed");
                    Some(failed(req, message))
                }
            }
        }
        // On a task of its own, never inline: a run waits for a person to answer its permission
        // request, and this connection has to keep reading frames meanwhile — the answer may be
        // coming from this very client, and its terminals' input is on the same socket.
        ClientMsg::ArtifactRefresh { req, pane } => {
            let out = out.clone();
            // Started here, in frame order; only the wait for the run is on the task.
            let run = state.refresh_pane(&pane);
            tokio::spawn(async move {
                let result = match run {
                    Ok(wait) => wait.await,
                    Err(message) => Err(message),
                };
                let reply = match result {
                    Ok(refresh) => req.map(|req| ServerMsg::Refresh { req, refresh }),
                    Err(message) => {
                        tracing::warn!(%peer, %pane, %message, "artifact refresh refused");
                        Some(failed(req, message))
                    }
                };
                if let Some(reply) = reply {
                    let _ = out.send(reply).await;
                }
            });
            None
        }
        ClientMsg::ArtifactTrust { req } => req.map(|req| ServerMsg::Trusted {
            req,
            trusted: state.trusted(),
        }),
        ClientMsg::ArtifactTrustForget { req, name } => match state.trust_forget(&name) {
            Ok(trusted) => req.map(|req| ServerMsg::Trusted { req, trusted }),
            Err(message) => {
                tracing::warn!(%peer, %name, %message, "artifact trust forget refused");
                Some(failed(req, message))
            }
        },
        ClientMsg::TermAttach { req, id } => match state.term(&id) {
            Some(term) => {
                if let Some(old) =
                    forwards.insert(id, forward(state.clone(), term, req, out.clone(), peer))
                {
                    old.abort();
                }
                None
            }
            None => term_done(req, &id, Err(format!("no terminal {id}"))),
        },
        ClientMsg::TermClose { req, id } => term_done(req, &id, state.term_close(&id)),
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
            term_done(req, &id, result.map_err(|e| e.to_string()))
        }
        ClientMsg::TermResize {
            req,
            id,
            cols,
            rows,
        } => {
            let result = state
                .term(&id)
                .ok_or_else(|| anyhow::anyhow!("no terminal {id}"))
                .and_then(|t| t.resize(cols.max(1), rows.max(1)));
            if result.is_ok() {
                tracing::info!(term = %id, cols, rows, "resize");
            }
            term_done(req, &id, result.map_err(|e| e.to_string()))
        }
    };
    if let Some(reply) = reply {
        let _ = out.send(reply).await;
    }
}

/// Move a session into another conversation — a fresh one, or the past one `load` names — off the
/// connection's own task, and reply with the conversation it ended up in.
fn spawn_switch(
    state: &Arc<AppState>,
    out: &mpsc::Sender<ServerMsg>,
    req: Option<u64>,
    session: String,
    load: Option<String>,
    peer: SocketAddr,
) {
    let (state, out) = (state.clone(), out.clone());
    tokio::spawn(async move {
        let switched = match state.session(&session) {
            Ok(found) => found.switch_conversation(load.clone()).await,
            Err(message) => Err(message),
        };
        let reply = match switched {
            Ok(id) => {
                tracing::info!(%peer, %session, acp_session = %id, resumed = load.is_some(), "session conversation changed");
                req.map(|req| ServerMsg::Conversation { req, session, id })
            }
            Err(message) => {
                tracing::warn!(%peer, %session, resumed = load.is_some(), %message, "session conversation change failed");
                Some(failed(req, message))
            }
        };
        if let Some(reply) = reply {
            let _ = out.send(reply).await;
        }
    });
}

/// The reply to a request with no result of its own: `ok` when it carried a `req`.
fn done(req: Option<u64>, result: Result<(), String>) -> Option<ServerMsg> {
    match result {
        Ok(()) => req.map(|req| ServerMsg::Ok { req }),
        Err(message) => Some(failed(req, message)),
    }
}

/// The error for a frame that did not parse. A frame carrying a `req` gets the error under that
/// `req`, so the client waiting on it hears at once. An `op` this build has no variant for is
/// named as unsupported; a known op with bad fields keeps serde's message.
fn unparsed_reply(text: &str, error: &serde_json::Error) -> ServerMsg {
    let raw: Option<serde_json::Value> = serde_json::from_str(text).ok();
    let req = raw
        .as_ref()
        .and_then(|v| v.get("req"))
        .and_then(|v| v.as_u64());
    let op = raw
        .as_ref()
        .and_then(|v| v.get("op"))
        .and_then(|v| v.as_str());
    // Parsing the op alone: only an op with no variant fails as "unknown variant"; a known op
    // fails on its missing fields instead.
    let op_unknown = op.is_some_and(|op| {
        serde_json::from_value::<ClientMsg>(serde_json::json!({ "op": op }))
            .is_err_and(|e| e.to_string().starts_with("unknown variant"))
    });
    let message = match op {
        Some(op) if op_unknown => {
            format!(
                "this wheeld doesn't support {op}; it may be older than the client that sent it"
            )
        }
        _ => format!("bad message: {error}"),
    };
    failed(req, message)
}

/// The error reply to a request that is not a terminal op.
fn failed(req: Option<u64>, message: String) -> ServerMsg {
    ServerMsg::Error {
        req,
        term: None,
        message,
    }
}

/// The reply to a terminal op: `ok` when it carried a `req`, or an error naming terminal `id`.
fn term_done(req: Option<u64>, id: &str, result: Result<(), String>) -> Option<ServerMsg> {
    match result {
        Ok(()) => req.map(|req| ServerMsg::Ok { req }),
        Err(message) => {
            // Debug, like input: a client typing into a terminal that is gone fails per keystroke.
            tracing::debug!(term = %id, error = %message, "terminal op failed");
            Some(ServerMsg::Error {
                req,
                term: Some(id.to_string()),
                message,
            })
        }
    }
}

/// The reply to a request about one artifact: the record it read off disk. A record with problems
/// is a success — the problems are what the caller asked for.
fn one_artifact(
    req: Option<u64>,
    result: Result<wheel_proto::ArtifactInfo, String>,
    via: &str,
) -> Option<ServerMsg> {
    match result {
        Ok(artifact) => req.map(|req| ServerMsg::Artifact { req, artifact }),
        Err(message) => {
            tracing::warn!(via, %message, "artifact request refused");
            Some(failed(req, message))
        }
    }
}

/// The reply to a settings request: every setting as the daemon resolved it, labelled with the
/// space it resolved them for. That space is what the daemon resolved against, never the echo of
/// the request: a caller that named none is told which space it got.
fn settings_reply(
    req: Option<u64>,
    result: Result<(Option<String>, Vec<wheel_proto::SettingInfo>), String>,
    via: &str,
) -> Option<ServerMsg> {
    match result {
        Ok((space, settings)) => req.map(|req| ServerMsg::Settings {
            req: Some(req),
            space,
            settings,
        }),
        Err(message) => {
            tracing::warn!(via, %message, "settings request refused");
            Some(failed(req, message))
        }
    }
}

/// The reply to a write or a removal of a setting, after every connection hears it, labelled with
/// the space the list was resolved for — a machine-wide change alters what every space falls back
/// to, so a client holding another space's view asks again for that space.
fn settings_changed(
    state: &AppState,
    req: Option<u64>,
    result: Result<(Option<String>, Vec<wheel_proto::SettingInfo>), String>,
    key: &str,
    via: &str,
) -> Option<ServerMsg> {
    if let Ok((space, settings)) = &result {
        if key.starts_with("usage.") {
            let _ = state.global.send(ServerMsg::UsageChanged {
                board: state.usage_board(),
            });
        }
        let _ = state.global.send(ServerMsg::Settings {
            req: None,
            space: space.clone(),
            settings: settings.clone(),
        });
    }
    settings_reply(req, result, via)
}

/// The reply to a request that made something: `created` with its id.
fn created(req: Option<u64>, result: Result<String, String>) -> Option<ServerMsg> {
    match result {
        Ok(id) => req.map(|req| ServerMsg::Created { req, id }),
        Err(message) => Some(failed(req, message)),
    }
}

/// Stream one terminal to one client: replay, then live output. When the client falls far
/// enough behind that output was dropped, start over with a fresh replay.
fn forward(
    state: Arc<AppState>,
    term: Arc<Term>,
    req: Option<u64>,
    out: mpsc::Sender<ServerMsg>,
    peer: SocketAddr,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut req = req;
        let mut resync = false;
        loop {
            let Some(Attachment {
                replay,
                exited,
                mut rx,
            }) = term.attach().await
            else {
                tracing::warn!(%peer, term = %term.id, "attach failed; the terminal's screen is gone");
                let message = format!("terminal {}'s screen is gone", term.id);
                if let Some(error) = term_done(req.take(), &term.id, Err(message)) {
                    let _ = out.send(error).await;
                }
                return;
            };
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
            for chunk in replay.chunks(REPLAY_CHUNK) {
                if out
                    .send(ServerMsg::TermOutput {
                        id: id.clone(),
                        data: encode_bytes(chunk),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if let Some(code) = exited {
                exit(&state, &out, id, code).await;
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
                        exit(&state, &out, id, code).await;
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

/// Tell one client a terminal exited; once it has the message, the terminal can go.
async fn exit(state: &AppState, out: &mpsc::Sender<ServerMsg>, id: String, code: Option<u32>) {
    let sent = out
        .send(ServerMsg::TermExited {
            id: id.clone(),
            code,
        })
        .await;
    if sent.is_ok() {
        state.exit_delivered(&id);
    }
}

/// Stream a session's conversation to one client: the transcript so far, then live events. Holds
/// the session weakly, so the stream ends once the session has ended and is dropped.
///
/// Three things send the transcript again: attaching, falling far enough behind that events were
/// dropped, and the session moving to another conversation. A client replaces its whole transcript
/// with whatever arrives, so it never has to tell them apart — and a resend it did not strictly
/// need costs it nothing but the bytes.
fn forward_session(
    session: Arc<Session>,
    req: Option<u64>,
    out: mpsc::Sender<ServerMsg>,
    peer: SocketAddr,
) -> JoinHandle<()> {
    let name = session.name.clone();
    let weak = Arc::downgrade(&session);
    drop(session);
    tokio::spawn(async move {
        let mut req = req;
        loop {
            // Subscribed before the transcript is read, so a conversation that changes between the
            // two is seen as a change rather than missed.
            let Some((mut switched, attachment)) = weak
                .upgrade()
                .map(|s| (s.conversation_changes(), s.attach()))
            else {
                return;
            };
            let SessionAttachment {
                state,
                events,
                config,
                context,
                mut rx,
            } = attachment;
            tracing::info!(%peer, session = %name, events = events.len(), config = config.len(), "session attach");
            if out
                .send(ServerMsg::SessionAttached {
                    req: req.take(),
                    session: name.clone(),
                    state,
                    events,
                    config,
                    context,
                    now: wheel_proto::unix_ms_now(),
                })
                .await
                .is_err()
            {
                return;
            }
            // Read when it is sent, not with the transcript: a dismissal landing in between has
            // already told this connection it is down, and a copy taken earlier would put it back.
            let notice = weak.upgrade().and_then(|s| s.notice());
            if let Some(notice) = notice {
                let up = ServerMsg::SessionNotice {
                    session: name.clone(),
                    notice: Some(notice),
                };
                if out.send(up).await.is_err() {
                    return;
                }
            }
            loop {
                // Biased to the events: a session that ends drops both channels at once, and the
                // `session ended` error it queued first must reach the client before the closed
                // watch ends this task.
                let event = tokio::select! {
                    biased;
                    event = rx.recv() => event,
                    changed = switched.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        tracing::info!(%peer, session = %name, "the session is in another conversation; resending the transcript");
                        break;
                    }
                };
                match event {
                    Ok(event) => {
                        let msg = ServerMsg::Session {
                            session: name.clone(),
                            event,
                        };
                        if out.send(msg).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(%peer, session = %name, skipped, "client fell behind a session; resending the transcript");
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    })
}
