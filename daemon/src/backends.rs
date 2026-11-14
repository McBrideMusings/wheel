//! Plugin backends and feeds (ADR-0041). A plugin package's `[backend]` is a process wheeld runs
//! on the first read of one of its feeds and talks to in JSON-RPC 2.0 with Content-Length framing
//! (`docs/plugin-runtime.md`), over the process's stdio or a unix socket wheeld makes. One
//! instance runs per package, whichever spaces read it.
//!
//! The [`Hub`] holds each feed a pane reads, by (source, feed, params): the current value, kept
//! current by applying every patch, and a broadcast of the changes. A pane's subscription is a
//! [`Subscription`], so the backend is told `feed/unsubscribe` when the last one drops. wheeld's
//! own feeds (`wheel.spaces`, `wheel.panes`, `wheel.agents`) live in the same hub with no backend.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_lsp::lsp_types::notification::Notification;
use async_lsp::lsp_types::request::Request;
use async_lsp::router::Router;
use async_lsp::{ErrorCode, MainLoop, ResponseError, ServerSocket};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::sync::{broadcast, watch};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use wheel_proto::{BackendInfo, BackendState, BackendTransport, PaneKind};

use crate::plugins::Package;
use crate::server::AppState;
use crate::spaces::{space_find, space_panes, Spaces};

/// How long a backend has to start in all: connect to its unix socket and answer `initialize`.
/// One deadline covers both, and the `ensure` that waits for it.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a backend has to leave after `shutdown` before its process group is killed.
const STOP_GRACE: Duration = Duration::from_secs(1);

/// The feeds wheeld itself publishes, named `wheel.<name>`.
pub(crate) const BUILTIN_FEEDS: [&str; 3] = ["spaces", "panes", "agents"];

/// `initialize` (request, wheeld to backend): the backend answers with its feeds and actions.
struct Initialize;

#[derive(Serialize, Deserialize)]
struct InitializeParams {
    wheel_version: String,
    plugin_id: String,
    config: Value,
}

#[derive(Serialize, Deserialize, Default)]
struct InitializeResult {
    #[serde(default)]
    feeds: Vec<String>,
    #[serde(default)]
    actions: Vec<String>,
}

impl Request for Initialize {
    type Params = InitializeParams;
    type Result = InitializeResult;
    const METHOD: &'static str = "initialize";
}

/// `wheel/call` (request, backend to wheeld): refused until a later slice enforces `needs`.
struct WheelCall;

impl Request for WheelCall {
    type Params = Value;
    type Result = Value;
    const METHOD: &'static str = "wheel/call";
}

/// A feed and its params, the body of `feed/subscribe` and `feed/unsubscribe`.
#[derive(Serialize, Deserialize)]
struct FeedRef {
    feed: String,
    params: Value,
}

/// `feed/subscribe` (notification, wheeld to backend): start sending `feed/update` for the feed.
struct FeedSubscribe;

impl Notification for FeedSubscribe {
    type Params = FeedRef;
    const METHOD: &'static str = "feed/subscribe";
}

/// `feed/unsubscribe` (notification, wheeld to backend): stop sending the feed.
struct FeedUnsubscribe;

impl Notification for FeedUnsubscribe {
    type Params = FeedRef;
    const METHOD: &'static str = "feed/unsubscribe";
}

/// `shutdown` (notification, wheeld to backend): exit now.
struct Shutdown;

impl Notification for Shutdown {
    type Params = ();
    const METHOD: &'static str = "shutdown";
}

/// `feed/update` (notification, backend to wheeld): `{feed, params, value}` or `{feed, params,
/// patch}`. Read as a raw value so a malformed one is logged, never a stopped connection.
struct FeedUpdateN;

impl Notification for FeedUpdateN {
    type Params = Value;
    const METHOD: &'static str = "feed/update";
}

/// `log` (notification, backend to wheeld): `{level, message}`, written to the backend's log.
struct LogN;

impl Notification for LogN {
    type Params = Value;
    const METHOD: &'static str = "log";
}

/// One change to a feed, as the hub broadcasts it.
#[derive(Debug, Clone)]
pub(crate) enum Update {
    Value(Value),
    Patch(Value),
}

/// `params` as a key: a feed read with no params and with `{}` is one feed.
pub(crate) fn canonical(params: &Value) -> String {
    if params.is_null() {
        "{}".into()
    } else {
        params.to_string()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FeedKey {
    /// A backend's key, or `wheel`.
    source: String,
    feed: String,
    params: String,
}

struct CellState {
    value: Value,
    tx: broadcast::Sender<Update>,
    subscribers: usize,
}

/// One feed's current value and its broadcast, under one mutex so a subscriber reads the value
/// and joins the broadcast with no change between.
struct Cell {
    state: Mutex<CellState>,
}

/// Every feed with a subscriber.
#[derive(Default)]
pub(crate) struct Hub {
    cells: Mutex<HashMap<FeedKey, Arc<Cell>>>,
    /// The current backend of each package key. Feeds reach their backend through this, so a
    /// restarted backend is the one a feed's subscribers and unsubscribes go to.
    live: Mutex<HashMap<String, Arc<Backend>>>,
}

impl Hub {
    /// Subscribe to a feed. The first subscriber sends the source's current backend
    /// `feed/subscribe`, and for a built-in feed sets the value to `initial`. Returns the guard,
    /// the value now and the changes after it.
    fn subscribe(
        self: &Arc<Self>,
        source: &str,
        feed: &str,
        params: &Value,
        initial: Option<Value>,
    ) -> (Subscription, Value, broadcast::Receiver<Update>) {
        let key = FeedKey {
            source: source.to_string(),
            feed: feed.to_string(),
            params: canonical(params),
        };
        let mut cells = self.cells.lock().unwrap();
        let cell = cells
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Cell {
                    state: Mutex::new(CellState {
                        value: Value::Null,
                        tx: broadcast::channel(256).0,
                        subscribers: 0,
                    }),
                })
            })
            .clone();
        let mut state = cell.state.lock().unwrap();
        state.subscribers += 1;
        if state.subscribers == 1 {
            if let Some(initial) = initial {
                state.value = initial;
            }
            self.send_feed::<FeedSubscribe>(source, feed, params);
        }
        let value = state.value.clone();
        let rx = state.tx.subscribe();
        drop(state);
        drop(cells);
        let subscription = Subscription {
            hub: self.clone(),
            key,
            cell,
            params: params.clone(),
        };
        (subscription, value, rx)
    }

    /// Tell the current backend of `source` about a feed; nothing when it has none running.
    fn send_feed<N: Notification<Params = FeedRef>>(&self, source: &str, feed: &str, params: &Value) {
        let backend = self.live.lock().unwrap().get(source).cloned();
        if let Some(backend) = backend {
            backend.send_feed::<N>(feed, params);
        }
    }

    /// A backend answered `initialize`: it becomes `source`'s socket, and is sent `feed/subscribe`
    /// for every feed of the source that has a reader, so a restarted backend serves what panes
    /// already read. Under the cells lock, so a subscribe lands either before (and is sent here)
    /// or after (and sends itself), never both.
    fn attach(&self, source: &str, backend: &Backend, socket: ServerSocket) {
        let cells = self.cells.lock().unwrap();
        *backend.socket.lock().unwrap() = Some(socket);
        for (key, cell) in cells.iter() {
            if key.source != source {
                continue;
            }
            if cell.state.lock().unwrap().subscribers > 0 {
                let params = serde_json::from_str(&key.params).unwrap_or(Value::Null);
                backend.send_feed::<FeedSubscribe>(&key.feed, &params);
            }
        }
    }

    /// A backend's `feed/update` for a feed: set or patch its value and tell its subscribers.
    /// Dropped when nobody reads the feed.
    fn apply(&self, source: &str, feed: &str, params: &Value, update: Update) -> Result<(), String> {
        let key = FeedKey {
            source: source.to_string(),
            feed: feed.to_string(),
            params: canonical(params),
        };
        let Some(cell) = self.cells.lock().unwrap().get(&key).cloned() else {
            return Ok(());
        };
        let mut state = cell.state.lock().unwrap();
        match &update {
            Update::Value(value) => state.value = value.clone(),
            Update::Patch(patch) => {
                let patch: json_patch::Patch = serde_json::from_value(patch.clone())
                    .map_err(|e| format!("feed {feed}: not a JSON Patch: {e}"))?;
                let mut next = state.value.clone();
                json_patch::patch(&mut next, &patch)
                    .map_err(|e| format!("feed {feed}: the patch does not apply: {e}"))?;
                state.value = next;
            }
        }
        let _ = state.tx.send(update);
        Ok(())
    }

    /// Whether any pane reads one of wheeld's own feeds.
    pub(crate) fn has_builtin_readers(&self) -> bool {
        self.cells.lock().unwrap().keys().any(|k| k.source == "wheel")
    }

    /// Publish wheeld's feed `feed` (every params): each reader is sent `value` when it differs
    /// from the last one.
    pub(crate) fn publish_builtin(&self, feed: &str, value: &Value) {
        let cells = self.cells.lock().unwrap();
        for (key, cell) in cells.iter() {
            if key.source != "wheel" || key.feed != feed {
                continue;
            }
            let mut state = cell.state.lock().unwrap();
            if state.value != *value {
                state.value = value.clone();
                let _ = state.tx.send(Update::Value(value.clone()));
            }
        }
    }
}

/// A pane's read of one feed. When the last one for a feed drops, the feed is forgotten and its
/// backend is told `feed/unsubscribe`.
pub(crate) struct Subscription {
    hub: Arc<Hub>,
    key: FeedKey,
    cell: Arc<Cell>,
    params: Value,
}

impl Subscription {
    /// The feed's value now and a receiver of the changes after it, taken under one lock, for a
    /// reader that fell behind its receiver.
    pub(crate) fn resync(&self) -> (Value, broadcast::Receiver<Update>) {
        let state = self.cell.state.lock().unwrap();
        (state.value.clone(), state.tx.subscribe())
    }

    /// The feed's value now.
    #[cfg(test)]
    pub(crate) fn current(&self) -> Value {
        self.cell.state.lock().unwrap().value.clone()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut cells = self.hub.cells.lock().unwrap();
        let mut state = self.cell.state.lock().unwrap();
        state.subscribers -= 1;
        if state.subscribers > 0 {
            return;
        }
        if cells.get(&self.key).is_some_and(|c| Arc::ptr_eq(c, &self.cell)) {
            cells.remove(&self.key);
        }
        self.hub
            .send_feed::<FeedUnsubscribe>(&self.key.source, &self.key.feed, &self.params);
    }
}

/// A backend's log file: its stderr (and stdout over a unix socket) and its `log` messages.
#[derive(Clone)]
struct Log {
    path: PathBuf,
    file: Arc<Mutex<(std::fs::File, u64)>>,
}

/// A log file is rotated to `<id>.log.1` (replacing the one there) past this many bytes.
const LOG_LIMIT: u64 = 4 * 1024 * 1024;

fn rotated(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

impl Log {
    fn open(path: &Path) -> std::io::Result<Log> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Log {
            path: path.to_path_buf(),
            file: Arc::new(Mutex::new((file, size))),
        })
    }

    fn line(&self, text: &str) {
        use std::io::Write;
        let mut guard = self.file.lock().unwrap();
        if guard.1 >= LOG_LIMIT {
            let _ = std::fs::rename(&self.path, rotated(&self.path));
            if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
                *guard = (file, 0);
            }
        }
        if writeln!(guard.0, "{text}").is_ok() {
            guard.1 += text.len() as u64 + 1;
        }
    }

    /// A `log` message: `HH:MM:SS UTC <level> <message>`.
    fn message(&self, level: &str, message: &str) {
        let at = jiff::Timestamp::now().strftime("%H:%M:%S");
        self.line(&format!("{at} UTC {level} {message}"));
    }
}

/// Copy `reader`'s lines to the log until it ends.
fn pump(reader: impl AsyncRead + Unpin + Send + 'static, log: Log) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log.line(&line);
        }
    });
}

/// What the router of one backend connection keeps.
struct Handler {
    hub: Arc<Hub>,
    source: String,
    log: Log,
}

/// One backend instance: its process and what it told `initialize`.
pub(crate) struct Backend {
    plugin: String,
    info: Mutex<BackendInfo>,
    state: watch::Sender<BackendState>,
    socket: Mutex<Option<ServerSocket>>,
    pgid: Mutex<Option<u32>>,
    /// When the start (socket connect and `initialize`) must be done by.
    deadline: Instant,
}

impl Backend {
    fn set(&self, change: impl FnOnce(&mut BackendInfo)) {
        let mut info = self.info.lock().unwrap();
        change(&mut info);
        self.state.send_replace(info.state);
    }

    fn send_feed<N: Notification<Params = FeedRef>>(&self, feed: &str, params: &Value) {
        let name = feed.to_string();
        if let Some(socket) = self.socket.lock().unwrap().clone() {
            let _ = socket.notify::<N>(FeedRef {
                feed: name,
                params: params.clone(),
            });
        }
    }

    /// Tell the backend to exit, then kill its process group when it has not within the grace.
    async fn stop(&self) {
        if let Some(socket) = self.socket.lock().unwrap().clone() {
            let _ = socket.notify::<Shutdown>(());
        }
        let mut state = self.state.subscribe();
        let _ = tokio::time::timeout(
            STOP_GRACE,
            state.wait_for(|s| matches!(s, BackendState::Exited)),
        )
        .await;
        self.kill();
    }

    /// SIGKILL the process group this backend started, while it is recorded: the whole group, so
    /// what the `/bin/sh` started does not outlive it.
    fn kill(&self) {
        if let Some(pgid) = *self.pgid.lock().unwrap() {
            // SAFETY: a signal to the process group this backend started.
            unsafe { libc::kill(-(pgid as i32), libc::SIGKILL) };
        }
    }

    /// The process is reaped and its group killed: forget the group id so it is never signalled
    /// after the system can reuse it.
    fn reaped(&self) {
        self.kill();
        self.pgid.lock().unwrap().take();
    }
}

/// What a backend instance is started from.
struct Spec {
    /// The package key the backend's feeds are filed under in the hub.
    key: String,
    plugin: String,
    command: String,
    transport: BackendTransport,
    dir: PathBuf,
    space_dir: String,
    path: String,
    socket: PathBuf,
    log: Log,
}

/// Every backend wheeld started, by package key, and the feeds they and wheeld publish.
#[derive(Default)]
pub(crate) struct Backends {
    pub(crate) hub: Arc<Hub>,
}

fn exit_reason(status: std::io::Result<std::process::ExitStatus>) -> String {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(s) => match (s.code(), s.signal()) {
            (Some(code), _) => format!("exited with status {code}"),
            (None, Some(signal)) => format!("ended by signal {signal}"),
            _ => "exited".into(),
        },
        Err(e) => format!("could not be waited on: {e}"),
    }
}

/// Run one backend instance until its process ends, and record how it ended.
async fn run(backend: Arc<Backend>, hub: Arc<Hub>, spec: Spec) {
    let why = serve(&backend, &hub, &spec).await;
    spec.log.message("info", &format!("backend stopped: {why}"));
    backend.set(|i| {
        i.state = BackendState::Exited;
        i.pid = None;
        i.error = Some(why);
    });
    backend.socket.lock().unwrap().take();
}

/// Start the process, connect, `initialize`, then keep the connection until either side ends.
/// Returns why it ended.
async fn serve(backend: &Arc<Backend>, hub: &Arc<Hub>, spec: &Spec) -> String {
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(&spec.command)
        .current_dir(&spec.dir)
        .env("PATH", &spec.path)
        .env("WHEEL_PLUGIN_ID", &spec.plugin)
        .env("WHEEL_SPACE_DIR", &spec.space_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let listener = match spec.transport {
        BackendTransport::Stdio => {
            command.stdin(Stdio::piped());
            None
        }
        BackendTransport::Unix => {
            if let Some(dir) = spec.socket.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::remove_file(&spec.socket);
            match tokio::net::UnixListener::bind(&spec.socket) {
                Ok(l) => {
                    command.stdin(Stdio::null()).env("WHEEL_PLUGIN_SOCKET", &spec.socket);
                    Some(l)
                }
                Err(e) => return format!("could not bind {}: {e}", spec.socket.display()),
            }
        }
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return format!("could not start `{}`: {e}", spec.command),
    };
    let pid = child.id();
    *backend.pgid.lock().unwrap() = pid;
    backend.set(|i| i.pid = pid);
    let why = drive(backend, hub, spec, &mut child, listener).await;
    // Every way out: the whole process group dies, not only the shell, and the shell is reaped.
    backend.reaped();
    let _ = tokio::time::timeout(STOP_GRACE, child.wait()).await;
    why
}

/// Connect to a started process, `initialize` it, then keep the connection until either side
/// ends. Returns why it ended; the caller kills the process group.
async fn drive(
    backend: &Arc<Backend>,
    hub: &Arc<Hub>,
    spec: &Spec,
    child: &mut tokio::process::Child,
    listener: Option<tokio::net::UnixListener>,
) -> String {
    let stderr = child.stderr.take().expect("stderr is piped");
    pump(stderr, spec.log.clone());
    let stdout = child.stdout.take().expect("stdout is piped");

    let handler = Handler {
        hub: hub.clone(),
        source: spec.key.clone(),
        log: spec.log.clone(),
    };
    let (main, server) = match listener {
        None => {
            let stdin = child.stdin.take().expect("stdin is piped");
            connect(stdout, stdin, handler)
        }
        Some(listener) => {
            pump(stdout, spec.log.clone());
            let deadline = tokio::time::Instant::from_std(backend.deadline);
            let accepted = tokio::select! {
                accepted = tokio::time::timeout_at(deadline, listener.accept()) => accepted,
                status = child.wait() => {
                    let _ = std::fs::remove_file(&spec.socket);
                    return format!("{} before it connected to its socket", exit_reason(status));
                }
            };
            let _ = std::fs::remove_file(&spec.socket);
            match accepted {
                Ok(Ok((stream, _))) => {
                    let (read, write) = stream.into_split();
                    connect(read, write, handler)
                }
                Ok(Err(e)) => return format!("its socket failed: {e}"),
                Err(_) => {
                    return format!("did not connect to its socket within {} s", START_TIMEOUT.as_secs())
                }
            }
        }
    };
    let mut main = main;

    let params = InitializeParams {
        wheel_version: env!("CARGO_PKG_VERSION").into(),
        plugin_id: spec.plugin.clone(),
        config: json!({}),
    };
    let init = tokio::time::timeout_at(
        tokio::time::Instant::from_std(backend.deadline),
        server.request::<Initialize>(params),
    );
    let initialized = tokio::select! {
        init = init => init,
        status = child.wait() => return format!("{} before it answered initialize", exit_reason(status)),
    };
    match initialized {
        Ok(Ok(result)) => {
            hub.attach(&spec.key, backend, server);
            backend.set(|i| {
                i.state = BackendState::Running;
                i.feeds = result.feeds;
                i.actions = result.actions;
                i.error = None;
            });
        }
        Ok(Err(e)) => {
            return format!("initialize failed: {e}");
        }
        Err(_) => {
            return format!("did not answer initialize within {} s", START_TIMEOUT.as_secs());
        }
    }

    tokio::select! {
        status = child.wait() => exit_reason(status),
        ended = &mut main => {
            let how = match ended {
                Ok(Ok(())) => "closed its connection".to_string(),
                Ok(Err(e)) => format!("its connection failed: {e}"),
                Err(e) => format!("its connection task failed: {e}"),
            };
            let status = match tokio::time::timeout(STOP_GRACE, child.wait()).await {
                Ok(status) => exit_reason(status),
                Err(_) => "was killed".into(),
            };
            format!("{how}; it {status}")
        }
    }
}

/// Run a JSON-RPC connection over `read` and `write` as the client side: `feed/update` and `log`
/// notifications are taken, `wheel/call` is refused. Returns the loop's task and the socket to
/// call the backend through.
fn connect<R, W>(
    read: R,
    write: W,
    handler: Handler,
) -> (tokio::task::JoinHandle<async_lsp::Result<()>>, ServerSocket)
where
    R: AsyncRead + Send + 'static,
    W: AsyncWrite + Send + 'static,
{
    let (main_loop, server) = MainLoop::new_client(|_| {
        let mut router = Router::new(handler);
        router
            .notification::<FeedUpdateN>(|h, params| {
                match feed_update(h, &params) {
                    Ok(()) => {}
                    Err(e) => h.log.message("warn", &e),
                }
                ControlFlow::Continue(())
            })
            .notification::<LogN>(|h, params| {
                let level = params.get("level").and_then(Value::as_str).unwrap_or("info");
                let message = params.get("message").and_then(Value::as_str).unwrap_or_default();
                h.log.message(level, message);
                ControlFlow::Continue(())
            })
            .request::<WheelCall, _>(|_, _| async {
                Err::<Value, ResponseError>(ResponseError::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    "wheel/call is not available to backends yet",
                ))
            })
            .unhandled_notification(|_, _| ControlFlow::Continue(()));
        router
    });
    let task = tokio::spawn(main_loop.run_buffered(read.compat(), write.compat_write()));
    (task, server)
}

/// Apply a backend's `feed/update` to the hub.
fn feed_update(handler: &Handler, params: &Value) -> Result<(), String> {
    let feed = params
        .get("feed")
        .and_then(Value::as_str)
        .ok_or("feed/update has no feed")?;
    let feed_params = params.get("params").cloned().unwrap_or(Value::Null);
    let update = match (params.get("value"), params.get("patch")) {
        (Some(value), None) => Update::Value(value.clone()),
        (None, Some(patch)) => Update::Patch(patch.clone()),
        _ => return Err(format!("feed/update for {feed} needs one of value and patch")),
    };
    handler.hub.apply(&handler.source, feed, &feed_params, update)
}

impl Backends {
    /// The running instance for `package`, started if there is none or it exited, once it has
    /// answered `initialize`. `space_dir` is the space that read the feed.
    pub(crate) async fn ensure(
        &self,
        state: &AppState,
        package: &Package,
        space_dir: &str,
    ) -> Result<Arc<Backend>, String> {
        let key = package.key();
        let Some(declared) = package.info.backend.clone() else {
            return Err(format!("plugin {} has no [backend]", package.info.id));
        };
        let backend = {
            let mut map = self.hub.live.lock().unwrap();
            match map.get(&key) {
                Some(b) if !matches!(*b.state.borrow(), BackendState::Exited) => b.clone(),
                _ => {
                    let log_path = state.plugin_log_path(&package.info.id);
                    let log = Log::open(&log_path)
                        .map_err(|e| format!("cannot open {}: {e}", log_path.display()))?;
                    let dir = package
                        .dir()
                        .map_or_else(|| state.instance_dir.clone(), Path::to_path_buf);
                    let backend = Arc::new(Backend {
                        plugin: package.info.id.clone(),
                        info: Mutex::new(BackendInfo {
                            state: BackendState::Starting,
                            log: Some(log_path.display().to_string()),
                            ..declared.clone()
                        }),
                        state: watch::channel(BackendState::Starting).0,
                        socket: Mutex::new(None),
                        pgid: Mutex::new(None),
                        deadline: Instant::now() + START_TIMEOUT,
                    });
                    let spec = Spec {
                        key: key.clone(),
                        plugin: package.info.id.clone(),
                        command: declared.command.clone(),
                        transport: declared.transport,
                        dir,
                        space_dir: space_dir.to_string(),
                        path: state.path(),
                        socket: socket_path(&state.instance_dir, &key),
                        log,
                    };
                    spec.log.message("info", &format!("starting `{}`", spec.command));
                    tokio::spawn(run(backend.clone(), self.hub.clone(), spec));
                    map.insert(key, backend.clone());
                    backend
                }
            }
        };
        let mut watcher = backend.state.subscribe();
        // The start's own deadline, which `serve` enforces, and the time to record how it failed.
        let settled = tokio::time::timeout_at(
            tokio::time::Instant::from_std(backend.deadline) + STOP_GRACE * 2,
            watcher.wait_for(|s| !matches!(s, BackendState::Starting)),
        )
        .await
        .is_ok();
        let info = backend.info.lock().unwrap().clone();
        match (settled, info.state) {
            (true, BackendState::Running) => Ok(backend),
            (_, BackendState::Starting) => Err(format!("backend of {} did not start in time", backend.plugin)),
            _ => Err(format!(
                "backend of {} is not running: {}",
                backend.plugin,
                info.error.unwrap_or_else(|| "it stopped".into())
            )),
        }
    }

    /// `info` with what the running instance of the package keyed `key` says of itself.
    pub(crate) fn overlay(&self, key: &str, info: &mut BackendInfo) {
        if let Some(backend) = self.hub.live.lock().unwrap().get(key) {
            let live = backend.info.lock().unwrap().clone();
            info.state = live.state;
            info.pid = live.pid;
            info.feeds = live.feeds;
            info.actions = live.actions;
            info.log = live.log;
            info.error = live.error;
        }
    }

    /// Ask every backend to exit and kill the ones that do not: wheeld is stopping.
    pub(crate) async fn stop_all(&self) {
        let all: Vec<Arc<Backend>> = self.hub.live.lock().unwrap().values().cloned().collect();
        futures_util::future::join_all(all.iter().map(|b| b.stop())).await;
    }
}

/// Where a backend's unix socket is: under wheeld's support directory, in a folder of short names
/// (a socket path is at most 104 bytes on macOS), or under the temp directory when that is still
/// too long.
fn socket_path(support: &Path, key: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    let name = format!("{}.sock", digest[..4].iter().map(|b| format!("{b:02x}")).collect::<String>());
    let path = support.join("b").join(&name);
    if path.as_os_str().len() < 100 {
        path
    } else {
        std::env::temp_dir().join(format!("wheel-b-{name}"))
    }
}

/// The lines of a log `plugin_logs` returns when the caller names no count.
const DEFAULT_TAIL: usize = 200;

/// The last `n` lines of the file at `path`, read by seeking back from its end.
fn tail_lines(path: &Path, n: usize) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let mut pos = file.metadata()?.len();
    let mut bytes: Vec<u8> = Vec::new();
    // Lines wanted plus the partial one before them, so the first kept line is whole.
    while pos > 0 && bytes.iter().filter(|b| **b == b'\n').count() <= n {
        let step = pos.min(64 * 1024);
        pos -= step;
        file.seek(SeekFrom::Start(pos))?;
        let mut chunk = vec![0u8; step as usize];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&bytes);
        bytes = chunk;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    // Unless the read reached the file's start, the first line may be cut: drop it.
    let skip = usize::from(pos > 0);
    let lines = &lines[skip.min(lines.len())..];
    let from = lines.len().saturating_sub(n);
    let mut kept = lines[from..].join("\n");
    if !kept.is_empty() {
        kept.push('\n');
    }
    Ok(kept)
}

/// The value of one of wheeld's own feeds.
fn builtin_value(state: &AppState, spaces: &Spaces, name: &str) -> Value {
    match name {
        "spaces" => Value::Array(
            spaces
                .spaces
                .iter()
                .map(|space| {
                    let session = spaces
                        .sessions
                        .iter()
                        .find(|s| s.spaces.contains(&space.id))
                        .map(|s| s.id.clone());
                    json!({"id": space.id, "name": space.name, "cwd": space.cwd, "session": session})
                })
                .collect(),
        ),
        "panes" => Value::Array(
            spaces
                .spaces
                .iter()
                .flat_map(|space| {
                    space_panes(space)
                        .into_iter()
                        .map(|(pane, kind)| json!({"id": pane, "space": space.id, "kind": kind_name(kind)}))
                })
                .collect(),
        ),
        _ => {
            let book = state.statuses.lock().unwrap();
            Value::Array(
                spaces
                    .spaces
                    .iter()
                    .flat_map(|space| {
                        space_panes(space).into_iter().filter_map(|(pane, kind)| {
                            let agent = match kind {
                                PaneKind::Agent { harness, .. } => harness.clone(),
                                PaneKind::Terminal { agent: Some(agent), .. } => agent.name.clone(),
                                _ => return None,
                            };
                            let status = book
                                .pane(pane)
                                .and_then(|s| serde_json::to_value(s).ok())
                                .and_then(|s| s.get("kind").cloned())
                                .unwrap_or(Value::Null);
                            Some(json!({
                                "pane": pane,
                                "space": space.id,
                                "kind": kind_name(kind),
                                "agent": agent,
                                "status": status,
                            }))
                        })
                    })
                    .collect(),
            )
        }
    }
}

/// A pane kind's name on the wire.
fn kind_name(kind: &PaneKind) -> Value {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.get("kind").cloned())
        .unwrap_or(Value::Null)
}

impl AppState {
    /// Where wheeld writes its logs: the folder of `wheeld.log`.
    fn log_dir(&self) -> PathBuf {
        Path::new(&self.log_path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| self.instance_dir.join("logs"), Path::to_path_buf)
    }

    /// The log file of plugin `plugin`'s backend.
    pub(crate) fn plugin_log_path(&self, plugin: &str) -> PathBuf {
        self.log_dir().join("plugins").join(format!("{plugin}.log"))
    }

    /// Publish wheeld's own feeds that have a reader, each only when its value changed. Called
    /// with the spaces lock held, wherever the layout or a status changes.
    pub(crate) fn publish_builtin_feeds(&self, spaces: &Spaces) {
        let hub = &self.backends.hub;
        if !hub.has_builtin_readers() {
            return;
        }
        for name in BUILTIN_FEEDS {
            hub.publish_builtin(name, &builtin_value(self, spaces, name));
        }
    }

    /// The directory of the space holding plugin pane `pane`.
    fn plugin_pane_cwd(&self, pane: &str) -> Result<String, String> {
        let spaces = self.spaces.lock().unwrap();
        let (space, kind) = spaces
            .spaces
            .iter()
            .find_map(|s| space_find(s, pane).map(|k| (s, k)))
            .ok_or_else(|| format!("no pane {pane}"))?;
        match kind {
            PaneKind::Plugin { .. } => Ok(space.cwd.clone()),
            _ => Err(format!("{pane} is not a plugin pane")),
        }
    }

    /// Read feed `feed` (`<plugin>.<name>`, or `wheel.<name>`) for plugin pane `pane`: starts the
    /// plugin's backend if it is not running. Returns the subscription, the value now and the
    /// changes after it.
    pub(crate) async fn feed_subscribe(
        &self,
        pane: &str,
        feed: &str,
        params: &Value,
    ) -> Result<(Subscription, Value, broadcast::Receiver<Update>), String> {
        let cwd = self.plugin_pane_cwd(pane)?;
        let (plugin, name) = feed
            .split_once('.')
            .filter(|(p, n)| !p.is_empty() && !n.is_empty())
            .ok_or_else(|| format!("feed {feed:?} is not <plugin>.<name>"))?;
        let hub = &self.backends.hub;
        if plugin == "wheel" {
            if !BUILTIN_FEEDS.contains(&name) {
                return Err(format!(
                    "wheeld has no feed {feed}; it has {}",
                    BUILTIN_FEEDS.map(|f| format!("wheel.{f}")).join(", ")
                ));
            }
            // The spaces lock keeps a publish from slipping between the value read and the join.
            let spaces = self.spaces.lock().unwrap();
            let value = builtin_value(self, &spaces, name);
            return Ok(hub.subscribe("wheel", name, params, Some(value)));
        }
        let package = {
            let state = self.me.upgrade().ok_or("wheeld is stopping")?;
            let (cwd, plugin) = (cwd.clone(), plugin.to_string());
            tokio::task::spawn_blocking(move || {
                let packages = crate::plugins::discover(Some(&cwd), &state.machine_plugins());
                crate::plugins::resolve(&packages, &plugin).cloned()
            })
            .await
            .map_err(|e| format!("plugin lookup failed: {e}"))??
        };
        let backend = self.backends.ensure(self, &package, &cwd).await?;
        let feeds = backend.info.lock().unwrap().feeds.clone();
        if !feeds.iter().any(|f| f == name) {
            return Err(format!(
                "plugin {plugin} has no feed {name}; its feeds: {}",
                if feeds.is_empty() { "none".into() } else { feeds.join(", ") }
            ));
        }
        let source = package.key();
        Ok(hub.subscribe(&source, name, params, None))
    }

    /// The log of plugin `plugin`'s backend (as resolved in `space`): `(path, text)`, the last
    /// `tail` lines when given.
    pub(crate) fn plugin_logs(
        &self,
        plugin: &str,
        space: Option<&str>,
        tail: Option<usize>,
    ) -> Result<(String, String), String> {
        let cwd = self.settings_space(space)?.map(|(_, cwd)| cwd);
        let packages = crate::plugins::discover(cwd.as_deref(), &self.machine_plugins());
        let package = crate::plugins::resolve(&packages, plugin)?;
        if package.info.backend.is_none() {
            return Err(format!("plugin {plugin} has no [backend]"));
        }
        let path = self.plugin_log_path(&package.info.id);
        let text = tail_lines(&path, tail.unwrap_or(DEFAULT_TAIL)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("plugin {plugin} has no log yet: its backend has not started ({})", path.display())
            } else {
                format!("cannot read {}: {e}", path.display())
            }
        })?;
        Ok((path.display().to_string(), text))
    }

    /// Ask every plugin backend to exit; wheeld is stopping.
    pub async fn stop_backends(&self) {
        self.backends.stop_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscribe(hub: &Arc<Hub>, feed: &str) -> (Subscription, Value, broadcast::Receiver<Update>) {
        hub.subscribe("src", feed, &json!({}), None)
    }

    #[test]
    fn a_patch_keeps_the_value_current_and_a_late_reader_sees_it() {
        let hub = Arc::new(Hub::default());
        let (first, value, mut rx) = subscribe(&hub, "lines");
        assert_eq!(value, Value::Null);
        hub.apply("src", "lines", &json!({}), Update::Value(json!({"lines": []}))).unwrap();
        hub.apply(
            "src",
            "lines",
            &json!({}),
            Update::Patch(json!([{"op": "add", "path": "/lines/-", "value": "a"}])),
        )
        .unwrap();
        assert!(matches!(rx.try_recv().unwrap(), Update::Value(_)));
        assert!(matches!(rx.try_recv().unwrap(), Update::Patch(_)));
        let (_second, value, mut rx2) = subscribe(&hub, "lines");
        assert_eq!(value, json!({"lines": ["a"]}));
        assert!(rx2.try_recv().is_err(), "no duplicate of what the snapshot holds");
        assert_eq!(first.current(), value);
    }

    #[test]
    fn a_resync_takes_the_value_and_a_receiver_with_no_replay() {
        let hub = Arc::new(Hub::default());
        let (sub, _, mut rx) = subscribe(&hub, "lines");
        hub.apply("src", "lines", &json!({}), Update::Value(json!({"lines": []}))).unwrap();
        let add = |v: &str| Update::Patch(json!([{"op": "add", "path": "/lines/-", "value": v}]));
        hub.apply("src", "lines", &json!({}), add("a")).unwrap();
        let (value, mut fresh) = sub.resync();
        assert_eq!(value, json!({"lines": ["a"]}));
        assert!(fresh.try_recv().is_err(), "the patches the value holds are not replayed");
        hub.apply("src", "lines", &json!({}), add("b")).unwrap();
        assert!(matches!(fresh.try_recv().unwrap(), Update::Patch(_)));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn a_log_tail_reads_the_last_lines_from_the_end() {
        let dir = std::env::temp_dir().join(format!("wheel-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.log");
        let text: String = (0..30000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &text).unwrap();
        assert_eq!(tail_lines(&path, 2).unwrap(), "line 29998\nline 29999\n");
        assert_eq!(tail_lines(&path, 50000).unwrap(), text);
        std::fs::write(&path, "").unwrap();
        assert_eq!(tail_lines(&path, 5).unwrap(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_patch_is_refused_and_keeps_the_value() {
        let hub = Arc::new(Hub::default());
        let (sub, _, mut rx) = subscribe(&hub, "f");
        hub.apply("src", "f", &json!({}), Update::Value(json!({"a": 1}))).unwrap();
        let _ = rx.try_recv();
        let bad = Update::Patch(json!([{"op": "remove", "path": "/missing"}]));
        assert!(hub.apply("src", "f", &json!({}), bad).is_err());
        assert!(hub.apply("src", "f", &json!({}), Update::Patch(json!("nope"))).is_err());
        assert!(rx.try_recv().is_err());
        assert_eq!(sub.current(), json!({"a": 1}));
    }

    #[test]
    fn the_last_reader_leaving_forgets_the_feed() {
        let hub = Arc::new(Hub::default());
        let (a, _, _) = subscribe(&hub, "f");
        let (b, _, _) = subscribe(&hub, "f");
        hub.apply("src", "f", &json!({}), Update::Value(json!(1))).unwrap();
        drop(a);
        assert_eq!(hub.cells.lock().unwrap().len(), 1);
        drop(b);
        assert!(hub.cells.lock().unwrap().is_empty());
        let (_c, value, _) = subscribe(&hub, "f");
        assert_eq!(value, Value::Null, "a feed read again starts empty");
    }

    #[test]
    fn no_params_and_empty_params_are_one_feed() {
        let hub = Arc::new(Hub::default());
        let (_a, _, mut rx) = hub.subscribe("src", "f", &Value::Null, None);
        hub.apply("src", "f", &json!({}), Update::Value(json!(7))).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), Update::Value(_)));
    }

    #[test]
    fn built_in_feeds_publish_only_a_change() {
        let hub = Arc::new(Hub::default());
        assert!(!hub.has_builtin_readers());
        let (_s, value, mut rx) = hub.subscribe("wheel", "spaces", &json!({}), Some(json!([1])));
        assert_eq!(value, json!([1]));
        assert!(hub.has_builtin_readers());
        hub.publish_builtin("spaces", &json!([1]));
        assert!(rx.try_recv().is_err(), "the same value is not sent again");
        hub.publish_builtin("spaces", &json!([1, 2]));
        assert!(matches!(rx.try_recv().unwrap(), Update::Value(v) if v == json!([1, 2])));
    }

    #[test]
    fn a_socket_path_fits_a_unix_socket() {
        let short = socket_path(Path::new("/tmp/s"), "/some/package");
        assert!(short.starts_with("/tmp/s/b"));
        let long = socket_path(Path::new(&format!("/{}", "x".repeat(120))), "/some/package");
        assert!(long.as_os_str().len() < 104, "{}", long.display());
    }
}
