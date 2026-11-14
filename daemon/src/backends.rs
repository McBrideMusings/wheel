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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_lsp::lsp_types::notification::Notification;
use async_lsp::lsp_types::request::Request;
use async_lsp::router::Router;
use async_lsp::{ErrorCode, MainLoop, ResponseError, ServerSocket};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use wheel_proto::{BackendInfo, BackendState, BackendTransport, PaneKind, PluginPaneInfo};

use crate::external_backends::External;
use crate::plugins::Package;
use crate::server::AppState;
use crate::spaces::{space_find, space_panes, Spaces};

/// How long a backend has to start in all: connect to its unix socket and answer `initialize`.
/// One deadline covers both, and the `ensure` that waits for it.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a backend has to leave after `shutdown` before its process group is killed.
const STOP_GRACE: Duration = Duration::from_secs(1);

/// The wait before the first restart after a process ends; it doubles per consecutive failure.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);

/// The longest wait before a restart.
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// A process that ran this long was healthy: the next end starts the backoff over.
const STABLE_AFTER: Duration = Duration::from_secs(10);

/// A change to a watched file restarts the backend once no matching change came for this long.
const WATCH_QUIET: Duration = Duration::from_millis(300);

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

/// `wheel/call` (request, backend to wheeld): `{method, args}` runs a Wheel operation for the
/// backend's plugin if the plugin's `[backend] needs` names it (see [`wheel_call`]).
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

/// `action/invoke` (request, wheeld to backend): run an action for a pane. The result is what the
/// backend answers, any JSON value.
struct ActionInvoke;

#[derive(Serialize, Deserialize)]
struct ActionInvokeParams {
    action: String,
    args: Value,
    caller: Value,
}

impl Request for ActionInvoke {
    type Params = ActionInvokeParams;
    type Result = Value;
    const METHOD: &'static str = "action/invoke";
}

/// How long a backend has to answer `action/invoke`.
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// The backend that publishes the feed is down: the last value, marked stale.
    Stale(Value),
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
    /// The backend that publishes the feed ended and has not sent an update since.
    stale: bool,
    tx: broadcast::Sender<Update>,
    subscribers: usize,
    /// The task that keeps a pumped feed current (an agent's transcript, status or requests),
    /// aborted with the cell when the last reader leaves.
    pump: Option<AbortOnDrop<()>>,
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
    /// the value now, whether it is stale and the changes after it.
    fn subscribe(
        self: &Arc<Self>,
        source: &str,
        feed: &str,
        params: &Value,
        initial: Option<Value>,
    ) -> (Subscription, Value, bool, broadcast::Receiver<Update>) {
        self.subscribe_with(source, feed, params, |state, _| {
            if let Some(initial) = initial {
                state.value = initial;
            }
        })
    }

    /// Subscribe to one of wheeld's pumped feeds: the first reader calls `start` with a
    /// [`Publisher`] for the cell, which returns the value now and the task that keeps it
    /// current. Later readers join the running task.
    pub(crate) fn subscribe_pumped(
        self: &Arc<Self>,
        feed: &str,
        params: &Value,
        start: impl FnOnce(Publisher) -> (Value, tokio::task::JoinHandle<()>),
    ) -> (Subscription, Value, bool, broadcast::Receiver<Update>) {
        let key = FeedKey {
            source: "wheel".into(),
            feed: feed.to_string(),
            params: canonical(params),
        };
        let hub = Arc::downgrade(self);
        self.subscribe_with("wheel", feed, params, |state, cell| {
            let (value, task) = start(Publisher { hub, key, cell });
            state.value = value;
            state.pump = Some(AbortOnDrop(task));
        })
    }

    /// The subscribe both kinds share: `first` sets up the cell for its first reader.
    fn subscribe_with(
        self: &Arc<Self>,
        source: &str,
        feed: &str,
        params: &Value,
        first: impl FnOnce(&mut CellState, Weak<Cell>),
    ) -> (Subscription, Value, bool, broadcast::Receiver<Update>) {
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
                        stale: false,
                        tx: broadcast::channel(256).0,
                        subscribers: 0,
                        pump: None,
                    }),
                })
            })
            .clone();
        let mut state = cell.state.lock().unwrap();
        state.subscribers += 1;
        if state.subscribers == 1 {
            first(&mut state, Arc::downgrade(&cell));
            self.send_feed::<FeedSubscribe>(source, feed, params);
        }
        let value = state.value.clone();
        let stale = state.stale;
        let rx = state.tx.subscribe();
        drop(state);
        drop(cells);
        let subscription = Subscription {
            hub: self.clone(),
            key,
            cell,
            params: params.clone(),
        };
        (subscription, value, stale, rx)
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

    /// An update with no backend instance behind it.
    #[cfg(test)]
    fn apply(&self, source: &str, feed: &str, params: &Value, update: Update) -> Result<(), String> {
        self.apply_from(None, source, feed, params, update)
    }

    /// A backend's `feed/update` for a feed: set or patch its value and tell its subscribers.
    /// Dropped when nobody reads the feed, and when `from` (the instance that sent it) is no
    /// longer the current backend of `source`: an instance that was replaced says nothing more.
    fn apply_from(
        &self,
        from: Option<u64>,
        source: &str,
        feed: &str,
        params: &Value,
        update: Update,
    ) -> Result<(), String> {
        let key = FeedKey {
            source: source.to_string(),
            feed: feed.to_string(),
            params: canonical(params),
        };
        let Some(cell) = self.cells.lock().unwrap().get(&key).cloned() else {
            return Ok(());
        };
        let mut state = cell.state.lock().unwrap();
        if let Some(id) = from {
            let current = self.live.lock().unwrap().get(source).is_some_and(|b| b.id == id);
            if !current {
                return Ok(());
            }
        }
        match &update {
            Update::Stale(_) => return Ok(()),
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
        state.stale = false;
        let _ = state.tx.send(update);
        Ok(())
    }

    /// The backend of `source` ended: each feed of it keeps its value, marked stale until the
    /// next `feed/update`, and its subscribers are sent that value with the flag.
    fn mark_stale(&self, source: &str) {
        let cells = self.cells.lock().unwrap();
        for (key, cell) in cells.iter() {
            if key.source != source {
                continue;
            }
            let mut state = cell.state.lock().unwrap();
            state.stale = true;
            let _ = state.tx.send(Update::Stale(state.value.clone()));
        }
    }

    /// Whether any pane reads one of wheeld's own feeds.
    pub(crate) fn has_builtin_readers(&self) -> bool {
        self.cells
            .lock()
            .unwrap()
            .keys()
            .any(|k| k.source == "wheel" && BUILTIN_FEEDS.contains(&k.feed.as_str()))
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

/// What a pumped feed's task writes through: its cell, for as long as a reader holds it.
pub(crate) struct Publisher {
    hub: Weak<Hub>,
    key: FeedKey,
    cell: Weak<Cell>,
}

impl Publisher {
    /// Make `value` the feed's value; readers are sent it when it differs from the last one.
    /// False once no reader is left.
    pub(crate) fn set(&self, value: Value) -> bool {
        let Some(cell) = self.cell.upgrade() else {
            return false;
        };
        let mut state = cell.state.lock().unwrap();
        if state.value != value {
            state.value = value.clone();
            let _ = state.tx.send(Update::Value(value));
        }
        true
    }

    /// Change the value in place with `change`, which returns the JSON Patch that makes the same
    /// change to a reader's copy (sent to readers), or none when nothing changed. False once no
    /// reader is left.
    pub(crate) fn apply(&self, change: impl FnOnce(&mut Value) -> Option<Value>) -> bool {
        let Some(cell) = self.cell.upgrade() else {
            return false;
        };
        let mut state = cell.state.lock().unwrap();
        if let Some(patch) = change(&mut state.value) {
            let _ = state.tx.send(Update::Patch(patch));
        }
        true
    }

    /// What the feed follows is gone (the agent ended): readers keep the last value, marked
    /// stale, and the feed leaves the hub, so a later read starts again and is told why it can't.
    pub(crate) fn close(&self) {
        let (Some(hub), Some(cell)) = (self.hub.upgrade(), self.cell.upgrade()) else {
            return;
        };
        let mut cells = hub.cells.lock().unwrap();
        if cells.get(&self.key).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
            cells.remove(&self.key);
        }
        let mut state = cell.state.lock().unwrap();
        state.stale = true;
        let _ = state.tx.send(Update::Stale(state.value.clone()));
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
    /// The feed's value now, whether it is stale, and a receiver of the changes after it, taken
    /// under one lock, for a reader that fell behind its receiver.
    pub(crate) fn resync(&self) -> (Value, bool, broadcast::Receiver<Update>) {
        let state = self.cell.state.lock().unwrap();
        (state.value.clone(), state.stale, state.tx.subscribe())
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
    /// The [`Backend::id`] of the instance this connection belongs to.
    instance: u64,
    log: Log,
    /// The plugin's id, and the `[backend] needs` of the manifest it started from: what
    /// `wheel/call` may do.
    plugin: String,
    needs: Vec<String>,
    state: Weak<AppState>,
}

/// One backend instance: its process and what it told `initialize`.
pub(crate) struct Backend {
    /// Which instance this is: no two share one, so a connection can tell whether it still
    /// belongs to its package's current backend.
    id: u64,
    plugin: String,
    info: Mutex<BackendInfo>,
    state: watch::Sender<BackendState>,
    socket: Mutex<Option<ServerSocket>>,
    pgid: Mutex<Option<u32>>,
    /// When the start (socket connect and `initialize`) must be done by.
    deadline: Instant,
    /// An `[[external_backend]]`: wheeld connected to it and never started it, so it is never
    /// told `shutdown` and never signalled.
    external: bool,
}

static NEXT_BACKEND: AtomicU64 = AtomicU64::new(1);

impl Backend {
    fn new(plugin: String, info: BackendInfo) -> Backend {
        Backend {
            external: info.connect.is_some(),
            id: NEXT_BACKEND.fetch_add(1, Ordering::SeqCst),
            plugin,
            state: watch::channel(info.state).0,
            info: Mutex::new(info),
            socket: Mutex::new(None),
            pgid: Mutex::new(None),
            deadline: Instant::now() + START_TIMEOUT,
        }
    }

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

    /// Tell the backend to exit, then kill its process group when its process has not ended
    /// within the grace. A backend with no connection yet (still starting) was never told it
    /// can shut down, so its group is killed at once. Returns how the process ended.
    async fn stop(&self, handle: &mut tokio::task::JoinHandle<Ended>) -> Ended {
        if self.external {
            // Ending the task drops the connection; the backend keeps running.
            handle.abort();
            let _ = (&mut *handle).await;
            return Ended::failed("wheeld closed the connection");
        }
        let socket = self.socket.lock().unwrap().clone();
        match socket {
            Some(socket) => {
                let _ = socket.notify::<Shutdown>(());
            }
            None => self.kill(),
        }
        let first = tokio::time::timeout(STOP_GRACE, &mut *handle).await;
        self.kill();
        let joined = match first {
            Ok(joined) => joined,
            Err(_) => handle.await,
        };
        joined.unwrap_or_else(|e| Ended::failed(format!("its task failed: {e}")))
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

/// When wheeld starts a backend's process again after it ends without wheeld asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    Always,
    OnFailure,
    Never,
}

impl Policy {
    /// The manifest's `restart` value; the manifest parser has refused any other.
    fn parse(name: &str) -> Policy {
        match name {
            "always" => Policy::Always,
            "never" => Policy::Never,
            _ => Policy::OnFailure,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Policy::Always => "always",
            Policy::OnFailure => "on-failure",
            Policy::Never => "never",
        }
    }

    /// Whether a process that ended with exit status `code` (`None`: a signal, or it never
    /// started) is started again.
    fn restarts(self, code: Option<i32>) -> bool {
        match self {
            Policy::Always => true,
            Policy::OnFailure => code != Some(0),
            Policy::Never => false,
        }
    }
}

/// The wait before restart number `failures` in a row (1 is the first): 1 s, doubling, at most
/// 30 s.
fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(5);
    (BACKOFF_FIRST * (1u32 << doublings)).min(BACKOFF_MAX)
}

/// How a process ended, and its exit status when it exited.
struct Ended {
    why: String,
    code: Option<i32>,
}

impl Ended {
    fn failed(why: impl Into<String>) -> Ended {
        Ended { why: why.into(), code: None }
    }

    fn status(status: std::io::Result<std::process::ExitStatus>) -> Ended {
        let code = status.as_ref().ok().and_then(std::process::ExitStatus::code);
        Ended { why: exit_reason(status), code }
    }
}

/// What a backend instance is started from.
#[derive(Clone)]
struct Spec {
    /// The package key the backend's feeds are filed under in the hub.
    key: String,
    plugin: String,
    command: String,
    transport: BackendTransport,
    dir: PathBuf,
    space_dir: String,
    /// The environment `wheel` reads (`WHEEL_URL`, `WHEEL_TOKEN_FILE`) and a `PATH` that finds it,
    /// asked for at each process start: the daemon's control address may be set after the first.
    env: Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
    socket: PathBuf,
    log: Log,
    /// The manifest's `[backend]`, which the instance reports until it says more.
    declared: BackendInfo,
    /// The package folder watched for `declared.watch`; none for a built-in package or an
    /// external backend.
    watch_dir: Option<PathBuf>,
    /// An `[[external_backend]]`'s address: wheeld connects there and starts nothing.
    connect: Option<crate::external_backends::Connect>,
    /// This machine's `wheel.toml`, read again with the space's before each reconnect.
    machine_file: PathBuf,
    /// What `wheel/call` acts on.
    state: Weak<AppState>,
}

impl Spec {
    /// What a wait before the next start is called: an external backend is connected again,
    /// never restarted.
    fn again(&self) -> &'static str {
        if self.connect.is_some() { "reconnecting" } else { "restarting" }
    }
}

/// What a request to a supervisor answers: the instance, or why there is none.
type Reply = oneshot::Sender<Result<Arc<Backend>, String>>;

/// A request to a package's supervisor.
enum Cmd {
    /// Reply with the running instance. Starts one when none runs and none waits to restart; an
    /// instance that waits to restart is refused with the time left, the wait is never skipped.
    Ensure { spec: Box<Spec>, reply: Reply },
    /// Stop the running instance if any, start a new one at once and reply with it.
    Restart {
        spec: Box<Spec>,
        reason: String,
        reply: Reply,
    },
    /// wheeld is stopping: stop the instance, and end.
    Quit(oneshot::Sender<()>),
}

/// A package's supervisor: one task per package that starts its process, starts it again by the
/// restart policy, a watched file or a request, and so never lets two run at once.
struct Slot {
    cmd: mpsc::UnboundedSender<Cmd>,
    log: Log,
}

/// Every backend wheeld started, by package key, and the feeds they and wheeld publish.
#[derive(Default)]
pub(crate) struct Backends {
    pub(crate) hub: Arc<Hub>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    /// Set, under the `slots` lock, when wheeld is stopping: no supervisor or process is started
    /// again.
    closing: Arc<AtomicBool>,
}

const STOPPING: &str = "wheeld is stopping";

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

/// The process of a running instance.
struct Running {
    backend: Arc<Backend>,
    handle: tokio::task::JoinHandle<Ended>,
    started: Instant,
}

/// The state of one package's supervisor.
struct Sup {
    hub: Arc<Hub>,
    closing: Arc<AtomicBool>,
    log: Log,
    rx: mpsc::UnboundedReceiver<Cmd>,
    /// What the last instance was started from, which a restart without a request reuses.
    spec: Option<Spec>,
    running: Option<Running>,
    /// When the instance that ended is started again.
    wait: Option<tokio::time::Instant>,
    /// Why the wait runs: how the process ended, or the file that changed.
    wait_why: String,
    /// When the last process was started.
    last_launch: Option<Instant>,
    /// Consecutive failures, which set the wait.
    failures: u32,
    watch_tx: mpsc::UnboundedSender<String>,
    watch_rx: mpsc::UnboundedReceiver<String>,
    watcher: Option<notify::RecommendedWatcher>,
    /// A change to a watched file, and when it restarts the backend.
    debounce: Option<(tokio::time::Instant, String)>,
}

/// What a supervisor wakes for.
enum Event {
    Cmd(Option<Cmd>),
    Ended(Ended),
    Wait,
    Changed(String),
    Quiet,
}

async fn supervise(mut sup: Sup) {
    loop {
        let event = {
            let running = &mut sup.running;
            let ended = async {
                match running {
                    Some(r) => (&mut r.handle).await,
                    None => std::future::pending().await,
                }
            };
            let waiting = async {
                match sup.wait {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let quiet = async {
                match &sup.debounce {
                    Some((at, _)) => tokio::time::sleep_until(*at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                cmd = sup.rx.recv() => Event::Cmd(cmd),
                joined = ended => Event::Ended(
                    joined.unwrap_or_else(|e| Ended::failed(format!("its task failed: {e}")))
                ),
                () = waiting => Event::Wait,
                Some(file) = sup.watch_rx.recv() => Event::Changed(file),
                () = quiet => Event::Quiet,
            }
        };
        match event {
            Event::Cmd(None) => return,
            Event::Cmd(Some(Cmd::Quit(ack))) => {
                sup.wait = None;
                sup.debounce = None;
                sup.halt().await;
                let _ = ack.send(());
                return;
            }
            Event::Cmd(Some(Cmd::Ensure { spec, reply })) => {
                if sup.closing.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(STOPPING.into()));
                    continue;
                }
                sup.settle_ended().await;
                let answer = match (&sup.running, sup.wait) {
                    (Some(r), _) => Ok(r.backend.clone()),
                    (None, Some(at)) => Err(format!(
                        "backend of {} is {} in {} s: {}",
                        spec.plugin,
                        spec.again(),
                        remaining_secs(at.saturating_duration_since(tokio::time::Instant::now())),
                        sup.wait_why
                    )),
                    (None, None) => {
                        sup.failures = 0;
                        Ok(sup.launch(*spec))
                    }
                };
                let _ = reply.send(answer);
            }
            Event::Cmd(Some(Cmd::Restart { mut spec, reason, reply })) => {
                if sup.closing.load(Ordering::SeqCst) {
                    let _ = reply.send(Err(STOPPING.into()));
                    continue;
                }
                sup.log.message("info", &reason);
                if let Some(last) = &sup.spec {
                    spec.space_dir = last.space_dir.clone();
                }
                let backend = sup.relaunch(Some(*spec)).await;
                let _ = reply.send(Ok(backend));
            }
            Event::Ended(ended) => {
                if let Some(r) = sup.running.take() {
                    sup.lost(r.backend, r.started, ended);
                }
            }
            Event::Wait => {
                sup.wait = None;
                match sup.refresh() {
                    Ok(()) => {
                        if let Some(spec) = sup.spec.clone() {
                            sup.launch(spec);
                        }
                    }
                    Err(reason) => sup.refuse(&reason),
                }
            }
            Event::Changed(file) => {
                let patterns = sup.spec.as_ref().map(|s| s.declared.watch.clone()).unwrap_or_default();
                if patterns.iter().any(|p| crate::plugins::glob_match(p, &file)) {
                    sup.debounce = Some((tokio::time::Instant::now() + WATCH_QUIET, file));
                }
            }
            Event::Quiet => {
                let Some((_, file)) = sup.debounce.take() else { continue };
                // A backend that never started has nothing to restart; one that started and
                // exited is started again too.
                if sup.spec.is_none() {
                    continue;
                }
                if let Err(reason) = sup.refresh() {
                    sup.refuse(&reason);
                    continue;
                }
                // A change while a restart already waits restarts at once: that end was counted.
                let counted = if sup.wait.is_some() {
                    None
                } else {
                    watch_restart(sup.last_launch.map(|t| t.elapsed()), sup.failures)
                };
                match counted {
                    Some((failures, delay)) => {
                        sup.halt().await;
                        sup.failures = failures;
                        let secs = remaining_secs(delay);
                        sup.log.message("info", &format!("restarting in {secs} s: {file} changed"));
                        sup.arm(delay, format!("{file} changed"), format!("{file} changed; restarting in {secs} s"));
                    }
                    None => {
                        sup.log.message("info", &format!("restarting: {file} changed"));
                        sup.relaunch(None).await;
                    }
                }
            }
        }
    }
}

/// Whole seconds of `d`, rounded up, at least 1: what a message says is left to wait.
fn remaining_secs(d: Duration) -> u64 {
    d.as_millis().div_ceil(1000).max(1) as u64
}

/// A change to a watched file restarts the backend. One within [`STABLE_AFTER`] of the previous
/// launch counts as a failure, so a backend that rewrites its own watched file is slowed to the
/// backoff schedule: `Some((failures now, wait))`. Otherwise the backoff starts over: `None`.
fn watch_restart(since_launch: Option<Duration>, failures: u32) -> Option<(u32, Duration)> {
    match since_launch {
        Some(ago) if ago < STABLE_AFTER => {
            let failures = failures.saturating_add(1);
            Some((failures, backoff(failures)))
        }
        _ => None,
    }
}

/// Whether the `[[external_backend]]` an external spec came from still names this connection:
/// the same id, address and token in the files now. When it was removed or changed, this
/// connection is not made again; a read then goes to the entry as it is now.
fn external_still_named(spec: &Spec) -> Result<(), String> {
    let space = Some(spec.space_dir.as_str()).filter(|d| !d.is_empty());
    let now = crate::external_backends::resolve(&spec.machine_file, space);
    match now.iter().find(|e| e.id == spec.plugin) {
        None => Err(format!("no [[external_backend]] names {} now", spec.plugin)),
        Some(e) if e.key() != spec.key => Err(format!("the [[external_backend]] {} changed", spec.plugin)),
        Some(_) => Ok(()),
    }
}

/// `old` run from the manifest text `text`: its command, transport and `[backend]` keys are the
/// manifest's now. Fails when the manifest does not parse or has no `[backend]`.
fn spec_from_manifest(old: &Spec, text: &str) -> Result<Spec, String> {
    let manifest = crate::plugins::parse_manifest(text)?;
    let Some(declared) = manifest.backend else {
        return Err(format!("{} has no [backend]", crate::plugins::MANIFEST));
    };
    Ok(Spec {
        command: declared.command.clone(),
        transport: declared.transport,
        declared,
        ..old.clone()
    })
}

impl Sup {
    /// Start a process from `spec`, which the next restart reuses. Starts the watch on the
    /// package folder the first time.
    fn launch(&mut self, spec: Spec) -> Arc<Backend> {
        self.wait = None;
        let backend = Arc::new(Backend::new(
            spec.plugin.clone(),
            BackendInfo {
                state: BackendState::Starting,
                log: Some(spec.log.path.display().to_string()),
                ..spec.declared.clone()
            },
        ));
        match &spec.connect {
            Some(c) => spec.log.message("info", &format!("connecting to {}", c.address())),
            None => spec.log.message("info", &format!("starting `{}`", spec.command)),
        }
        self.hub.live.lock().unwrap().insert(spec.key.clone(), backend.clone());
        let handle = {
            let (backend, hub, spec) = (backend.clone(), self.hub.clone(), spec.clone());
            tokio::spawn(async move { serve(&backend, &hub, &spec).await })
        };
        let started = Instant::now();
        self.last_launch = Some(started);
        self.running = Some(Running {
            backend: backend.clone(),
            handle,
            started,
        });
        self.spec = Some(spec);
        self.watch_folder();
        backend
    }

    /// Stop the running process, if any, and start another from `spec`, or the last one's:
    /// a request or a changed file, so the failures start over.
    async fn relaunch(&mut self, spec: Option<Spec>) -> Arc<Backend> {
        self.halt().await;
        self.failures = 0;
        self.debounce = None;
        let spec = spec.or_else(|| self.spec.clone()).expect("a restart follows a start");
        self.launch(spec)
    }

    /// Read the package's manifest again and make the next process from it: command, transport,
    /// restart policy and watch patterns. A built-in package has no folder and keeps its spec.
    /// Fails with the reason when the manifest cannot be read, does not parse or has no
    /// `[backend]`; the spec is then left as it was.
    fn refresh(&mut self) -> Result<(), String> {
        let Some(spec) = &self.spec else { return Ok(()) };
        if spec.connect.is_some() {
            return external_still_named(spec);
        }
        let Some(dir) = &spec.watch_dir else { return Ok(()) };
        let path = dir.join(crate::plugins::MANIFEST);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let next = spec_from_manifest(spec, &text)?;
        self.spec = Some(next);
        self.watch_folder();
        Ok(())
    }

    /// The instance last started, running or exited.
    fn current(&self) -> Option<Arc<Backend>> {
        let key = &self.spec.as_ref()?.key;
        self.hub.live.lock().unwrap().get(key).cloned()
    }

    /// An automatic restart is not made: say why in the log and in the instance's `error`.
    fn refuse(&mut self, reason: &str) {
        self.wait = None;
        self.log.message("warn", &format!("not restarted: {reason}"));
        if let Some(backend) = self.current() {
            backend.set(|i| i.error = Some(format!("not restarted: {reason}")));
        }
    }

    /// Start the process again in `delay`: `why` is what a request meanwhile is told, `error`
    /// what the instance reports.
    fn arm(&mut self, delay: Duration, why: String, error: String) {
        self.wait = Some(tokio::time::Instant::now() + delay);
        self.wait_why = why;
        if let Some(backend) = self.current() {
            backend.set(|i| i.error = Some(error));
        }
    }

    /// Stop the running process, if any, and record that it ended because wheeld asked.
    async fn halt(&mut self) {
        self.wait = None;
        if let Some(mut r) = self.running.take() {
            let ended = r.backend.stop(&mut r.handle).await;
            self.finish(&r.backend, &ended, ended.why.clone());
        }
    }

    /// A process that ended on its own is waiting in the handle: take it as `lost`.
    async fn settle_ended(&mut self) {
        if self.running.as_ref().is_some_and(|r| r.handle.is_finished()) {
            if let Some(mut r) = self.running.take() {
                let ended = (&mut r.handle)
                    .await
                    .unwrap_or_else(|e| Ended::failed(format!("its task failed: {e}")));
                self.lost(r.backend, r.started, ended);
            }
        }
    }

    /// The process ended: its feeds go stale, its connection goes, and the instance is `exited`
    /// with `error` saying how and what happens next.
    fn finish(&self, backend: &Backend, ended: &Ended, error: String) {
        let key = self.spec.as_ref().map(|s| s.key.clone()).unwrap_or_default();
        self.log.message("info", &format!("backend stopped: {}", ended.why));
        backend.socket.lock().unwrap().take();
        self.hub.mark_stale(&key);
        backend.set(|i| {
            i.state = BackendState::Exited;
            i.pid = None;
            i.error = Some(error);
        });
    }

    /// The process ended without wheeld asking: start it again after the backoff, or say it is not
    /// restarted, by the policy.
    fn lost(&mut self, backend: Arc<Backend>, started: Instant, ended: Ended) {
        if self.closing.load(Ordering::SeqCst) {
            self.finish(&backend, &ended, ended.why.clone());
            return;
        }
        let policy = Policy::parse(&backend.info.lock().unwrap().restart);
        if started.elapsed() >= STABLE_AFTER {
            self.failures = 0;
        }
        if !policy.restarts(ended.code) {
            let error = format!("{}; not restarted (restart = {})", ended.why, policy.name());
            self.finish(&backend, &ended, error);
            return;
        }
        self.failures += 1;
        let delay = backoff(self.failures);
        let secs = remaining_secs(delay);
        let again = self.spec.as_ref().map_or("restarting", Spec::again);
        self.finish(&backend, &ended, format!("{}; {again} in {secs} s", ended.why));
        self.log
            .message("info", &format!("{again} in {secs} s after: {}", ended.why));
        self.wait = Some(tokio::time::Instant::now() + delay);
        self.wait_why = ended.why;
    }

    /// Watch the package folder for the files `watch` names, once, when the first process starts.
    fn watch_folder(&mut self) {
        let Some(spec) = &self.spec else { return };
        let Some(dir) = spec.watch_dir.clone() else { return };
        if self.watcher.is_some() || spec.declared.watch.is_empty() {
            return;
        }
        let base = std::fs::canonicalize(&dir).unwrap_or(dir);
        let tx = self.watch_tx.clone();
        let root = base.clone();
        let made = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            for path in &event.paths {
                if let Ok(rel) = path.strip_prefix(&root) {
                    let _ = tx.send(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        });
        let result = made.map_err(|e| e.to_string()).and_then(|mut w| {
            notify::Watcher::watch(&mut w, &base, notify::RecursiveMode::Recursive)
                .map(|()| w)
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(w) => self.watcher = Some(w),
            Err(e) => self
                .log
                .message("warn", &format!("cannot watch {}: {e}", base.display())),
        }
    }
}

/// Start the process, connect, `initialize`, then keep the connection until either side ends.
/// Returns why it ended.
async fn serve(backend: &Arc<Backend>, hub: &Arc<Hub>, spec: &Spec) -> Ended {
    if let Some(connect) = &spec.connect {
        return serve_external(backend, hub, spec, connect).await;
    }
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(&spec.command)
        .current_dir(&spec.dir)
        .envs((spec.env)())
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
                Err(e) => return Ended::failed(format!("could not bind {}: {e}", spec.socket.display())),
            }
        }
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Ended::failed(format!("could not start `{}`: {e}", spec.command)),
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
) -> Ended {
    let stderr = child.stderr.take().expect("stderr is piped");
    pump(stderr, spec.log.clone());
    let stdout = child.stdout.take().expect("stdout is piped");

    let handler = Handler {
        hub: hub.clone(),
        source: spec.key.clone(),
        instance: backend.id,
        log: spec.log.clone(),
        plugin: spec.plugin.clone(),
        needs: spec.declared.needs.clone(),
        state: spec.state.clone(),
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
                    let ended = Ended::status(status);
                    return Ended { why: format!("{} before it connected to its socket", ended.why), code: ended.code };
                }
            };
            let _ = std::fs::remove_file(&spec.socket);
            match accepted {
                Ok(Ok((stream, _))) => {
                    let (read, write) = stream.into_split();
                    connect(read, write, handler)
                }
                Ok(Err(e)) => return Ended::failed(format!("its socket failed: {e}")),
                Err(_) => {
                    return Ended::failed(format!(
                        "did not connect to its socket within {} s",
                        START_TIMEOUT.as_secs()
                    ))
                }
            }
        }
    };
    // The connection's loop ends with this function, whichever way it returns.
    let mut main = AbortOnDrop(main);

    let init = initialize(backend, spec, &server);
    let answered = tokio::select! {
        init = init => init,
        status = child.wait() => {
            let ended = Ended::status(status);
            return Ended { why: format!("{} before it answered initialize", ended.why), code: ended.code };
        }
    };
    match answered {
        Ok(Ok(result)) => initialized(backend, hub, spec, server, result),
        Ok(Err(e)) => {
            // A closed connection is often a process that has exited: its status decides whether
            // the restart policy starts it again.
            if !matches!(e, async_lsp::Error::Response(_)) {
                if let Ok(status) = tokio::time::timeout(STOP_GRACE, child.wait()).await {
                    let ended = Ended::status(status);
                    return Ended {
                        why: format!("{} before it answered initialize", ended.why),
                        code: ended.code,
                    };
                }
            }
            return Ended::failed(format!("initialize failed: {e}"));
        }
        Err(_) => {
            return Ended::failed(format!("did not answer initialize within {} s", START_TIMEOUT.as_secs()));
        }
    }

    tokio::select! {
        status = child.wait() => Ended::status(status),
        ended = &mut main.0 => {
            let how = match ended {
                Ok(Ok(())) => "closed its connection".to_string(),
                Ok(Err(e)) => format!("its connection failed: {e}"),
                Err(e) => format!("its connection task failed: {e}"),
            };
            match tokio::time::timeout(STOP_GRACE, child.wait()).await {
                Ok(status) => {
                    let ended = Ended::status(status);
                    Ended { why: format!("{how}; it {}", ended.why), code: ended.code }
                }
                Err(_) => Ended::failed(format!("{how}; it was killed")),
            }
        }
    }
}

/// `initialize` the backend behind `server`, by the start's deadline.
async fn initialize(
    backend: &Backend,
    spec: &Spec,
    server: &ServerSocket,
) -> Result<async_lsp::Result<InitializeResult>, tokio::time::error::Elapsed> {
    let params = InitializeParams {
        wheel_version: env!("CARGO_PKG_VERSION").into(),
        plugin_id: spec.plugin.clone(),
        config: json!({}),
    };
    tokio::time::timeout_at(
        tokio::time::Instant::from_std(backend.deadline),
        server.request::<Initialize>(params),
    )
    .await
}

/// The backend answered `initialize`: it is the source's backend now, running with the feeds and
/// actions it named.
fn initialized(backend: &Backend, hub: &Hub, spec: &Spec, server: ServerSocket, result: InitializeResult) {
    hub.attach(&spec.key, backend, server);
    backend.set(|i| {
        i.state = BackendState::Running;
        i.feeds = result.feeds;
        i.actions = result.actions;
        i.error = None;
    });
}

/// Connect to an external backend, `initialize` it, then keep the connection until either side
/// closes it. Returns why it ended; nothing is started or killed.
async fn serve_external(
    backend: &Arc<Backend>,
    hub: &Arc<Hub>,
    spec: &Spec,
    connect: &crate::external_backends::Connect,
) -> Ended {
    use crate::external_backends::Connect;
    let handler = Handler {
        hub: hub.clone(),
        source: spec.key.clone(),
        instance: backend.id,
        log: spec.log.clone(),
        plugin: spec.plugin.clone(),
        needs: spec.declared.needs.clone(),
        state: spec.state.clone(),
    };
    let deadline = tokio::time::Instant::from_std(backend.deadline);
    let address = connect.address();
    let late = || Ended::failed(format!("{address} did not accept within {} s", START_TIMEOUT.as_secs()));
    let (main, server, _bridge) = match connect {
        Connect::Unix(path) => match tokio::time::timeout_at(deadline, tokio::net::UnixStream::connect(path)).await {
            Ok(Ok(stream)) => {
                let (read, write) = stream.into_split();
                let (main, server) = self::connect(read, write, handler);
                (main, server, None)
            }
            Ok(Err(e)) => return Ended::failed(format!("could not connect to {address}: {e}")),
            Err(_) => return late(),
        },
        Connect::Ws { url, token } => {
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut request = match url.as_str().into_client_request() {
                Ok(r) => r,
                Err(e) => return Ended::failed(format!("{address} is not a WebSocket URL: {e}")),
            };
            match format!("Bearer {token}").parse() {
                Ok(value) => {
                    request.headers_mut().insert(http::header::AUTHORIZATION, value);
                }
                Err(_) => return Ended::failed("its token cannot be sent in a header".to_string()),
            }
            match tokio::time::timeout_at(deadline, tokio_tungstenite::connect_async(request)).await {
                Ok(Ok((ws, _))) => {
                    let (ours, theirs) = tokio::io::duplex(1 << 16);
                    let bridge = AbortOnDrop(tokio::spawn(ws_bridge(ws, ours)));
                    let (read, write) = tokio::io::split(theirs);
                    let (main, server) = self::connect(read, write, handler);
                    (main, server, Some(bridge))
                }
                Ok(Err(e)) => return Ended::failed(format!("could not connect to {address}: {e}")),
                Err(_) => return late(),
            }
        }
    };
    let mut main = AbortOnDrop(main);
    match initialize(backend, spec, &server).await {
        Ok(Ok(result)) => initialized(backend, hub, spec, server, result),
        Ok(Err(e)) => return Ended::failed(format!("initialize failed: {e}")),
        Err(_) => return Ended::failed(format!("did not answer initialize within {} s", START_TIMEOUT.as_secs())),
    }
    Ended::failed(match (&mut main.0).await {
        Ok(Ok(())) => "closed its connection".to_string(),
        Ok(Err(e)) => format!("its connection failed: {e}"),
        Err(e) => format!("its connection task failed: {e}"),
    })
}

/// Carry JSON-RPC between a WebSocket, one text message per JSON-RPC message, and `ours`, which
/// speaks the Content-Length framing the connection loop reads, until either side ends.
async fn ws_bridge<S>(ws: tokio_tungstenite::WebSocketStream<S>, ours: tokio::io::DuplexStream)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::AsyncWriteExt;
    use tokio_tungstenite::tungstenite::Message;
    let (mut sink, mut stream) = ws.split();
    let (read, mut write) = tokio::io::split(ours);
    let mut read = BufReader::new(read);
    let inbound = async {
        while let Some(Ok(message)) = stream.next().await {
            let body: Vec<u8> = match message {
                Message::Text(text) => text.as_bytes().to_vec(),
                Message::Binary(bytes) => bytes.to_vec(),
                Message::Close(_) => break,
                _ => continue,
            };
            let head = format!("Content-Length: {}\r\n\r\n", body.len());
            if write.write_all(head.as_bytes()).await.is_err() || write.write_all(&body).await.is_err() {
                break;
            }
        }
    };
    let outbound = async {
        while let Some(body) = read_frame(&mut read).await {
            if sink.send(Message::text(String::from_utf8_lossy(&body).into_owned())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    };
    tokio::select! {
        () = inbound => {}
        () = outbound => {}
    }
}

/// The body of the next Content-Length framed message on `read`, or None when it ends or a frame
/// has no length.
async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(read: &mut R) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut length = None;
    loop {
        let mut line = String::new();
        if read.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = vec![0u8; length?];
    read.read_exact(&mut body).await.ok()?;
    Some(body)
}

/// Aborts its task when dropped.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
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
            .request::<WheelCall, _>(|h, params| {
                let (plugin, needs, state) = (h.plugin.clone(), h.needs.clone(), h.state.clone());
                let log = h.log.clone();
                async move {
                    let result = wheel_call(&plugin, &needs, &state, &params).await;
                    match &result {
                        Ok(_) => log.message("debug", &format!("wheel/call {} done", call_name(&params))),
                        Err(e) => log.message("warn", &format!("wheel/call {} refused: {}", call_name(&params), e.message)),
                    }
                    result
                }
            })
            .unhandled_notification(|_, _| ControlFlow::Continue(()));
        router
    });
    let task = tokio::spawn(main_loop.run_buffered(read.compat(), write.compat_write()));
    (task, server)
}

/// The `method` of a `wheel/call`, for the log.
fn call_name(params: &Value) -> &str {
    params.get("method").and_then(Value::as_str).unwrap_or("?")
}

/// The `[backend] needs` entry a `wheel/call` method requires: `pane.open` and `notify` by their
/// names, `command.run` as `run:<command>` for exactly the command it runs.
fn required_need(method: &str, args: &Value) -> Result<String, ResponseError> {
    match method {
        "pane.open" | "notify" => Ok(method.to_string()),
        "command.run" => {
            let command = args.get("command").and_then(Value::as_str).unwrap_or_default();
            Ok(format!("run:{command}"))
        }
        other => Err(ResponseError::new(
            ErrorCode::METHOD_NOT_FOUND,
            format!("wheel/call has no method {other:?} (pane.open, notify, command.run)"),
        )),
    }
}

fn bad_args(method: &str, why: impl std::fmt::Display) -> ResponseError {
    ResponseError::new(ErrorCode::INVALID_PARAMS, format!("{method}: {why}"))
}

/// Run `wheel/call {method, args}` for `plugin`: the method must be one of `needs` (checked
/// before any argument is read), then it does what the same `wheel` command does.
async fn wheel_call(
    plugin: &str,
    needs: &[String],
    state: &Weak<AppState>,
    params: &Value,
) -> Result<Value, ResponseError> {
    let method = params
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_args("wheel/call", "no method"))?;
    let args = params.get("args").cloned().unwrap_or(Value::Null);
    let need = required_need(method, &args)?;
    if !needs.contains(&need) {
        return Err(ResponseError::new(
            ErrorCode::REQUEST_FAILED,
            format!("plugin {plugin} may not call {method}: its [backend] needs does not name {need:?}"),
        ));
    }
    let state = state
        .upgrade()
        .ok_or_else(|| ResponseError::new(ErrorCode::REQUEST_FAILED, STOPPING))?;
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string);
    let parsed = |key: &str| -> Result<Option<Value>, ResponseError> { Ok(args.get(key).cloned()) };
    match method {
        "pane.open" => {
            let pane = text("pane").ok_or_else(|| bad_args(method, "needs pane"))?;
            let dir = parsed("dir")?
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| bad_args(method, e))?;
            let site = parsed("site")?
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| bad_args(method, e))?;
            let scope = parsed("scope")?
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| bad_args(method, e))?;
            let opened = state
                .pane_open(&pane, dir, site, text("url"), text("artifact"), scope)
                .await
                .map_err(|e| ResponseError::new(ErrorCode::REQUEST_FAILED, e))?;
            tracing::info!(%plugin, pane = %opened, "plugin opened a pane");
            Ok(json!({ "pane": opened }))
        }
        "notify" => {
            let body = text("text").filter(|t| !t.trim().is_empty() && t.len() <= 4000);
            let body = body.ok_or_else(|| bad_args(method, "needs a text of 1 to 4000 bytes"))?;
            let level = parsed("level")?
                .map_or(Ok(wheel_proto::NoticeLevel::Notice), serde_json::from_value)
                .map_err(|e| bad_args(method, e))?;
            let id = text("id").unwrap_or_else(|| format!("plugin:{plugin}"));
            if id.is_empty() || id.len() > 200 {
                return Err(bad_args(method, "id is 1 to 200 bytes"));
            }
            tracing::info!(%plugin, %id, ?level, bytes = body.len(), "plugin toast sent");
            let _ = state.global.send(wheel_proto::ServerMsg::Toast {
                id,
                level,
                text: body,
                pane: text("pane"),
            });
            Ok(json!({}))
        }
        _ => {
            let pane = text("pane").ok_or_else(|| bad_args(method, "needs pane"))?;
            let command = text("command").ok_or_else(|| bad_args(method, "needs command"))?;
            let site = parsed("site")?
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| bad_args(method, e))?;
            let ran = state
                .pane_run(&pane, &command, site)
                .await
                .map_err(|e| ResponseError::new(ErrorCode::REQUEST_FAILED, e))?;
            tracing::info!(%plugin, pane = %ran, "plugin ran an approved command");
            Ok(json!({ "pane": ran }))
        }
    }
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
    handler
        .hub
        .apply_from(Some(handler.instance), &handler.source, feed, &feed_params, update)
}

/// What serves a plugin's feeds and actions in a space: a package's `[backend]`, which wheeld
/// runs, or an `[[external_backend]]` with the plugin's id, which it connects to instead.
pub(crate) enum Source {
    Package(Package),
    External(External),
}

impl Source {
    /// What its feeds are filed under in the hub, and its supervisor by.
    fn key(&self) -> String {
        match self {
            Source::Package(p) => p.key(),
            Source::External(e) => e.key(),
        }
    }

    fn id(&self) -> &str {
        match self {
            Source::Package(p) => &p.info.id,
            Source::External(e) => &e.id,
        }
    }
}

impl Backends {
    /// The supervisor of `source`, started if it has none: its log is opened once, so every
    /// process or connection of it appends through one handle.
    fn slot(&self, state: &AppState, source: &Source) -> Result<Arc<Slot>, String> {
        let key = source.key();
        let mut slots = self.slots.lock().unwrap();
        if self.closing.load(Ordering::SeqCst) {
            return Err(STOPPING.into());
        }
        if let Some(slot) = slots.get(&key) {
            return Ok(slot.clone());
        }
        let log_path = state.plugin_log_path(source.id());
        let log = Log::open(&log_path).map_err(|e| format!("cannot open {}: {e}", log_path.display()))?;
        let (cmd, rx) = mpsc::unbounded_channel();
        let (watch_tx, watch_rx) = mpsc::unbounded_channel();
        tokio::spawn(supervise(Sup {
            hub: self.hub.clone(),
            closing: self.closing.clone(),
            log: log.clone(),
            rx,
            spec: None,
            running: None,
            wait: None,
            wait_why: String::new(),
            last_launch: None,
            failures: 0,
            watch_tx,
            watch_rx,
            watcher: None,
            debounce: None,
        }));
        let slot = Arc::new(Slot { cmd, log });
        slots.insert(key, slot.clone());
        Ok(slot)
    }

    /// What `source` is started from, run for the space directory `space_dir`.
    fn spec(&self, state: &AppState, source: &Source, slot: &Slot, space_dir: &str) -> Result<Spec, String> {
        let package = match source {
            Source::Package(package) => package,
            Source::External(ext) => {
                let connect = ext.connect.clone().map_err(|why| {
                    format!("the [[external_backend]] {} in {} is refused: {why}", ext.id, ext.file.display())
                })?;
                return Ok(Spec {
                    key: ext.key(),
                    plugin: ext.id.clone(),
                    command: String::new(),
                    transport: BackendTransport::default(),
                    dir: state.instance_dir.clone(),
                    space_dir: space_dir.to_string(),
                    env: Arc::new(Vec::new),
                    socket: PathBuf::new(),
                    log: slot.log.clone(),
                    declared: external_declared(ext),
                    watch_dir: None,
                    connect: Some(connect),
                    machine_file: state.machine_file.clone(),
                    state: state.me.clone(),
                });
            }
        };
        let Some(declared) = package.info.backend.clone() else {
            return Err(format!("plugin {} has no [backend]", package.info.id));
        };
        let key = package.key();
        Ok(Spec {
            plugin: package.info.id.clone(),
            command: declared.command.clone(),
            transport: declared.transport,
            dir: package
                .dir()
                .map_or_else(|| state.instance_dir.clone(), Path::to_path_buf),
            space_dir: space_dir.to_string(),
            env: {
                let me = state.me.clone();
                Arc::new(move || {
                    me.upgrade().map_or_else(Vec::new, |s| {
                        crate::shell::command_env(s.control.lock().unwrap().as_ref(), &s.instance_dir, &s.path())
                    })
                })
            },
            socket: socket_path(&state.instance_dir, &key),
            log: slot.log.clone(),
            watch_dir: package.dir().map(Path::to_path_buf),
            declared,
            key,
            connect: None,
            machine_file: state.machine_file.clone(),
            state: state.me.clone(),
        })
    }

    /// The running instance for `package`, started if there is none or it exited, once it has
    /// answered `initialize`. `space_dir` is the space that read the feed. An instance that waits
    /// to restart is refused with the time left; one that ended and is not restarted by the
    /// policy is started again.
    pub(crate) async fn ensure(
        &self,
        state: &AppState,
        source: &Source,
        space_dir: &str,
    ) -> Result<Arc<Backend>, String> {
        let slot = self.slot(state, source)?;
        let spec = self.spec(state, source, &slot, space_dir)?;
        let (reply, answer) = oneshot::channel();
        slot.cmd
            .send(Cmd::Ensure { spec: Box::new(spec), reply })
            .map_err(|_| STOPPING.to_string())?;
        let backend = answer.await.map_err(|_| STOPPING.to_string())??;
        settle(backend).await
    }

    /// Stop the instance of `source` if one runs and start a new one, or start one when none
    /// does, once it has answered `initialize`; an external backend is disconnected and connected
    /// again. `space_dir` is used only when the source never started before; a restart runs in
    /// the space it last started with.
    pub(crate) async fn restart(
        &self,
        state: &AppState,
        source: &Source,
        space_dir: &str,
    ) -> Result<(), String> {
        let slot = self.slot(state, source)?;
        let spec = self.spec(state, source, &slot, space_dir)?;
        let (reply, answer) = oneshot::channel();
        slot.cmd
            .send(Cmd::Restart {
                spec: Box::new(spec),
                reason: "restart requested".into(),
                reply,
            })
            .map_err(|_| STOPPING.to_string())?;
        let backend = answer.await.map_err(|_| STOPPING.to_string())??;
        settle(backend).await.map(|_| ())
    }

    /// What `plugin ls` reports of an `[[external_backend]]`: its address and how its connection
    /// is doing, or why the entry is refused.
    pub(crate) fn external_info(&self, ext: &External) -> BackendInfo {
        let mut info = external_declared(ext);
        if let Err(why) = &ext.connect {
            info.error = Some(format!("refused: {why}"));
            return info;
        }
        self.overlay(&ext.key(), &mut info);
        info
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

    /// Ask every backend to exit and kill the ones that do not, and end every supervisor: wheeld
    /// is stopping, so nothing starts again.
    pub(crate) async fn stop_all(&self) {
        // Set under the lock `slot` reads it under, so no supervisor is made after this list.
        let slots: Vec<Arc<Slot>> = {
            let slots = self.slots.lock().unwrap();
            self.closing.store(true, Ordering::SeqCst);
            slots.values().cloned().collect()
        };
        let acks = slots.iter().filter_map(|slot| {
            let (ack, done) = oneshot::channel();
            slot.cmd.send(Cmd::Quit(ack)).ok().map(|()| done)
        });
        futures_util::future::join_all(acks.collect::<Vec<_>>()).await;
    }
}

/// What an `[[external_backend]]` declares: its address, and that a lost connection is always
/// made again.
fn external_declared(ext: &External) -> BackendInfo {
    BackendInfo {
        connect: Some(ext.connect.as_ref().map_or_else(|_| String::new(), |c| c.address())),
        restart: "always".into(),
        ..BackendInfo::default()
    }
}

/// Wait for a started instance to answer `initialize`, or fail with how it did not.
async fn settle(backend: Arc<Backend>) -> Result<Arc<Backend>, String> {
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

/// Where a plugin pane is: its space and which plugin pane it shows.
pub(crate) struct PaneCtx {
    space: String,
    cwd: String,
    plugin: String,
    plugin_pane: String,
}

/// `items` joined by commas, or `none`.
fn list_or_none(items: &[String]) -> String {
    if items.is_empty() { "none".into() } else { items.join(", ") }
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

    /// The space, directory, plugin and plugin pane id that `pane` shows (a plugin pane, or an agent pane's view).
    fn plugin_pane_ctx(&self, pane: &str) -> Result<PaneCtx, String> {
        let spaces = self.spaces.lock().unwrap();
        let (space, kind) = spaces
            .spaces
            .iter()
            .find_map(|s| space_find(s, pane).map(|k| (s, k)))
            .ok_or_else(|| format!("no pane {pane}"))?;
        let (plugin, plugin_pane) = crate::plugins::shown_plugin(kind)
            .ok_or_else(|| format!("{pane} is not a plugin or agent pane"))?;
        Ok(PaneCtx {
            space: space.id.clone(),
            cwd: space.cwd.clone(),
            plugin,
            plugin_pane,
        })
    }

    /// The package `plugin` resolved in the space directory `cwd`.
    async fn resolve_plugin(&self, cwd: &str, plugin: &str) -> Result<Package, String> {
        let state = self.me.upgrade().ok_or("wheeld is stopping")?;
        let (cwd, plugin) = (cwd.to_string(), plugin.to_string());
        tokio::task::spawn_blocking(move || {
            let packages = crate::plugins::discover(Some(&cwd), &state.machine_plugins());
            crate::plugins::resolve(&packages, &plugin).cloned()
        })
        .await
        .map_err(|e| format!("plugin lookup failed: {e}"))?
    }

    /// What serves plugin `plugin`'s feeds and actions in the space directory `cwd`: the
    /// `[[external_backend]]` with its id when this machine's or the space's `wheel.toml` names
    /// one, else its package.
    pub(crate) async fn backend_source(&self, cwd: Option<&str>, plugin: &str) -> Result<Source, String> {
        let state = self.me.upgrade().ok_or("wheeld is stopping")?;
        let (cwd, plugin) = (cwd.map(str::to_string), plugin.to_string());
        tokio::task::spawn_blocking(move || {
            let externals = crate::external_backends::resolve(&state.machine_file, cwd.as_deref());
            if let Some(ext) = externals.into_iter().find(|e| e.id == plugin) {
                return Ok(Source::External(ext));
            }
            let packages = crate::plugins::discover(cwd.as_deref(), &state.machine_plugins());
            crate::plugins::resolve(&packages, &plugin).cloned().map(Source::Package)
        })
        .await
        .map_err(|e| format!("plugin lookup failed: {e}"))?
    }

    /// The `[[pane]]` entry of the calling pane's own package.
    async fn pane_entry(&self, ctx: &PaneCtx) -> Result<PluginPaneInfo, String> {
        let package = self.resolve_plugin(&ctx.cwd, &ctx.plugin).await?;
        package
            .info
            .panes
            .iter()
            .find(|p| p.id == ctx.plugin_pane)
            .cloned()
            .ok_or_else(|| format!("plugin {} has no pane {}", ctx.plugin, ctx.plugin_pane))
    }

    /// Whether `pane` is a plugin pane whose package's `reads` match `feed` now, read from the
    /// manifest on disk. Checked at subscribe and again whenever the layout or a plugin folder
    /// changes, so a grant ends with the pane or the manifest line that gave it.
    pub(crate) async fn feed_grant(&self, pane: &str, feed: &str) -> Result<PaneCtx, String> {
        let ctx = self.plugin_pane_ctx(pane)?;
        let entry = self.pane_entry(&ctx).await?;
        if !entry.reads.iter().any(|r| crate::plugins::glob_match(r, feed)) {
            return Err(format!(
                "pane {pane} ({}.{}) may not read {feed}; its reads: {}",
                ctx.plugin,
                ctx.plugin_pane,
                list_or_none(&entry.reads)
            ));
        }
        Ok(ctx)
    }

    /// Read feed `feed` (`<plugin>.<name>`, or `wheel.<name>`) for plugin pane `pane`: starts the
    /// plugin's backend if it is not running. Refused unless the pane's `reads` match the feed.
    /// Returns the subscription, the value now, whether it is stale and the changes after it.
    pub(crate) async fn feed_subscribe(
        &self,
        pane: &str,
        feed: &str,
        params: &Value,
    ) -> Result<(Subscription, Value, bool, broadcast::Receiver<Update>), String> {
        let ctx = self.feed_grant(pane, feed).await?;
        let cwd = ctx.cwd.clone();
        let (plugin, name) = feed
            .split_once('.')
            .filter(|(p, n)| !p.is_empty() && !n.is_empty())
            .ok_or_else(|| format!("feed {feed:?} is not <plugin>.<name>"))?;
        let hub = &self.backends.hub;
        if plugin == "wheel" {
            if crate::agent_feeds::AGENT_FEEDS.contains(&name) {
                return self.agent_feed_subscribe(name, params);
            }
            if !BUILTIN_FEEDS.contains(&name) {
                return Err(format!(
                    "wheeld has no feed {feed}; it has {}",
                    BUILTIN_FEEDS
                        .iter()
                        .chain(&crate::agent_feeds::AGENT_FEEDS)
                        .map(|f| format!("wheel.{f}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            // The spaces lock keeps a publish from slipping between the value read and the join.
            let spaces = self.spaces.lock().unwrap();
            let value = builtin_value(self, &spaces, name);
            return Ok(hub.subscribe("wheel", name, params, Some(value)));
        }
        let source = self.backend_source(Some(&cwd), plugin).await?;
        let backend = self.backends.ensure(self, &source, &cwd).await?;
        let feeds = backend.info.lock().unwrap().feeds.clone();
        if !feeds.iter().any(|f| f == name) {
            return Err(format!(
                "plugin {plugin} has no feed {name}; its feeds: {}",
                list_or_none(&feeds)
            ));
        }
        Ok(hub.subscribe(&source.key(), name, params, None))
    }

    /// Whether the package of plugin pane `pane` lists `action` under its `actions` now.
    async fn action_grant(&self, ctx: &PaneCtx, pane: &str, action: &str) -> Result<(), String> {
        let entry = self.pane_entry(ctx).await?;
        if !entry.actions.iter().any(|a| crate::plugins::glob_match(a, action)) {
            return Err(format!(
                "pane {pane} ({}.{}) may not invoke {action}; its actions: {}",
                ctx.plugin,
                ctx.plugin_pane,
                list_or_none(&entry.actions)
            ));
        }
        Ok(())
    }

    /// Run action `action` (`<plugin>.<name>`) with `args` for plugin pane `pane`: refused unless
    /// the pane's `actions` match it and the plugin's backend named `<name>` in `initialize`;
    /// starts the backend if it is not running and returns what its `action/invoke` answered.
    pub(crate) async fn plugin_action(&self, pane: &str, action: &str, args: Value) -> Result<Value, String> {
        let ctx = self.plugin_pane_ctx(pane)?;
        let (plugin, name) = action
            .split_once('.')
            .filter(|(p, n)| !p.is_empty() && !n.is_empty())
            .ok_or_else(|| format!("action {action:?} is not <plugin>.<name>"))?;
        if plugin == "wheel" {
            if !crate::agent_feeds::AGENT_ACTIONS.contains(&name) {
                return Err(format!(
                    "wheeld has no action {action}; it has {}",
                    crate::agent_feeds::AGENT_ACTIONS.map(|a| format!("wheel.{a}")).join(", ")
                ));
            }
            self.action_grant(&ctx, pane, action).await?;
            return self.agent_action(pane, name, &args);
        }
        self.action_grant(&ctx, pane, action).await?;
        let source = self.backend_source(Some(&ctx.cwd), plugin).await?;
        let backend = self.backends.ensure(self, &source, &ctx.cwd).await?;
        // The backend may have taken seconds to start: the grant is checked again right before
        // the call.
        let ctx = self.plugin_pane_ctx(pane)?;
        self.action_grant(&ctx, pane, action).await?;
        let actions = backend.info.lock().unwrap().actions.clone();
        if !actions.iter().any(|a| a == name) {
            return Err(format!(
                "plugin {plugin} has no action {name}; its actions: {}",
                list_or_none(&actions)
            ));
        }
        let socket = backend
            .socket
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| format!("backend of {plugin} is not running"))?;
        let params = ActionInvokeParams {
            action: name.to_string(),
            args,
            caller: json!({
                "pane": pane,
                "space": ctx.space,
                "plugin": ctx.plugin,
                "plugin_pane": ctx.plugin_pane,
            }),
        };
        match tokio::time::timeout(ACTION_TIMEOUT, socket.request::<ActionInvoke>(params)).await {
            Err(_) => Err(format!("{action} did not answer in {} s", ACTION_TIMEOUT.as_secs())),
            Ok(Err(async_lsp::Error::Response(e))) => Err(format!("{action} failed: {}", e.message)),
            Ok(Err(e)) => Err(format!("{action} failed: {e}")),
            Ok(Ok(result)) => Ok(result),
        }
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
        let external = crate::external_backends::resolve(&self.machine_file, cwd.as_deref())
            .into_iter()
            .any(|e| e.id == plugin);
        if !external {
            let packages = crate::plugins::discover(cwd.as_deref(), &self.machine_plugins());
            let package = crate::plugins::resolve(&packages, plugin)?;
            if package.info.backend.is_none() {
                return Err(format!("plugin {plugin} has no [backend]"));
            }
        }
        let path = self.plugin_log_path(plugin);
        let text = tail_lines(&path, tail.unwrap_or(DEFAULT_TAIL)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("plugin {plugin} has no log yet: its backend has not started ({})", path.display())
            } else {
                format!("cannot read {}: {e}", path.display())
            }
        })?;
        Ok((path.display().to_string(), text))
    }

    /// Restart plugin `plugin`'s backend (as resolved in `space`): stop its process if one runs
    /// and start a new one, or start one when none does. Returns once the new process answered
    /// `initialize`.
    pub(crate) async fn plugin_restart(&self, plugin: &str, space: Option<&str>) -> Result<(), String> {
        let cwd = self.settings_space(space)?.map(|(_, cwd)| cwd);
        let source = self.backend_source(cwd.as_deref(), plugin).await?;
        if let Source::Package(package) = &source {
            if package.info.backend.is_none() {
                return Err(format!("plugin {plugin} has no [backend]"));
            }
        }
        self.backends
            .restart(self, &source, cwd.as_deref().unwrap_or_default())
            .await
    }

    /// Ask every plugin backend to exit; wheeld is stopping.
    pub async fn stop_backends(&self) {
        self.backends.stop_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscribe(hub: &Arc<Hub>, feed: &str) -> (Subscription, Value, bool, broadcast::Receiver<Update>) {
        hub.subscribe("src", feed, &json!({}), None)
    }

    #[test]
    fn a_patch_keeps_the_value_current_and_a_late_reader_sees_it() {
        let hub = Arc::new(Hub::default());
        let (first, value, _, mut rx) = subscribe(&hub, "lines");
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
        let (_second, value, _, mut rx2) = subscribe(&hub, "lines");
        assert_eq!(value, json!({"lines": ["a"]}));
        assert!(rx2.try_recv().is_err(), "no duplicate of what the snapshot holds");
        assert_eq!(first.current(), value);
    }

    #[test]
    fn a_resync_takes_the_value_and_a_receiver_with_no_replay() {
        let hub = Arc::new(Hub::default());
        let (sub, _, _, mut rx) = subscribe(&hub, "lines");
        hub.apply("src", "lines", &json!({}), Update::Value(json!({"lines": []}))).unwrap();
        let add = |v: &str| Update::Patch(json!([{"op": "add", "path": "/lines/-", "value": v}]));
        hub.apply("src", "lines", &json!({}), add("a")).unwrap();
        let (value, _, mut fresh) = sub.resync();
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
        let (sub, _, _, mut rx) = subscribe(&hub, "f");
        hub.apply("src", "f", &json!({}), Update::Value(json!({"a": 1}))).unwrap();
        let _ = rx.try_recv();
        let bad = Update::Patch(json!([{"op": "remove", "path": "/missing"}]));
        assert!(hub.apply("src", "f", &json!({}), bad).is_err());
        assert!(hub.apply("src", "f", &json!({}), Update::Patch(json!("nope"))).is_err());
        assert!(rx.try_recv().is_err());
        assert_eq!(sub.current(), json!({"a": 1}));
    }

    #[tokio::test]
    async fn a_pumped_feed_appends_in_place_and_its_task_ends_with_the_last_reader() {
        let hub = Arc::new(Hub::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Publisher>();
        let start = |out: Publisher| {
            tx.send(out).ok();
            (json!({"events": [1]}), tokio::spawn(std::future::pending()))
        };
        let (sub, value, _, mut changes) = hub.subscribe_pumped("agent.transcript", &json!({"agent": "a"}), start);
        assert_eq!(value, json!({"events": [1]}));
        let out = rx.recv().await.unwrap();
        let add = json!([{"op": "add", "path": "/events/-", "value": 2}]);
        assert!(out.apply(|v| {
            v["events"].as_array_mut().unwrap().push(json!(2));
            Some(add.clone())
        }));
        assert_eq!(sub.current(), json!({"events": [1, 2]}));
        match changes.recv().await.unwrap() {
            Update::Patch(p) => assert_eq!(p, json!([{"op": "add", "path": "/events/-", "value": 2}])),
            _ => panic!("expected a patch"),
        }
        // A second reader joins the running pump: its start is never called.
        let (second, value, _, _) =
            hub.subscribe_pumped("agent.transcript", &json!({"agent": "a"}), |_| unreachable!());
        assert_eq!(value, json!({"events": [1, 2]}));
        assert!(!hub.has_builtin_readers(), "agent feeds do not wake the built-in publisher");
        drop(sub);
        assert!(out.set(json!({"events": []})));
        drop(second);
        assert!(!out.set(json!({"events": [3]})), "no reader is left");
    }

    #[tokio::test]
    async fn a_closed_pumped_feed_goes_stale_and_the_next_read_starts_a_new_pump() {
        let hub = Arc::new(Hub::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Publisher>();
        let start = |n: i64| {
            let tx = tx.clone();
            move |out: Publisher| {
                tx.send(out).ok();
                (json!(n), tokio::spawn(std::future::pending()))
            }
        };
        let (_first, _, _, mut changes) = hub.subscribe_pumped("agent.status", &json!({"agent": "a"}), start(1));
        rx.recv().await.unwrap().close();
        assert!(matches!(changes.recv().await.unwrap(), Update::Stale(v) if v == json!(1)));
        let (_second, value, stale, _) = hub.subscribe_pumped("agent.status", &json!({"agent": "a"}), start(2));
        assert_eq!((value, stale), (json!(2), false));
    }

    #[test]
    fn the_last_reader_leaving_forgets_the_feed() {
        let hub = Arc::new(Hub::default());
        let (a, _, _, _) = subscribe(&hub, "f");
        let (b, _, _, _) = subscribe(&hub, "f");
        hub.apply("src", "f", &json!({}), Update::Value(json!(1))).unwrap();
        drop(a);
        assert_eq!(hub.cells.lock().unwrap().len(), 1);
        drop(b);
        assert!(hub.cells.lock().unwrap().is_empty());
        let (_c, value, _, _) = subscribe(&hub, "f");
        assert_eq!(value, Value::Null, "a feed read again starts empty");
    }

    #[test]
    fn no_params_and_empty_params_are_one_feed() {
        let hub = Arc::new(Hub::default());
        let (_a, _, _, mut rx) = hub.subscribe("src", "f", &Value::Null, None);
        hub.apply("src", "f", &json!({}), Update::Value(json!(7))).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), Update::Value(_)));
    }

    #[test]
    fn built_in_feeds_publish_only_a_change() {
        let hub = Arc::new(Hub::default());
        assert!(!hub.has_builtin_readers());
        let (_s, value, _, mut rx) = hub.subscribe("wheel", "spaces", &json!({}), Some(json!([1])));
        assert_eq!(value, json!([1]));
        assert!(hub.has_builtin_readers());
        hub.publish_builtin("spaces", &json!([1]));
        assert!(rx.try_recv().is_err(), "the same value is not sent again");
        hub.publish_builtin("spaces", &json!([1, 2]));
        assert!(matches!(rx.try_recv().unwrap(), Update::Value(v) if v == json!([1, 2])));
    }

    #[test]
    fn a_backend_ending_marks_its_feeds_stale_until_the_next_update() {
        let hub = Arc::new(Hub::default());
        let (sub, _, stale, mut rx) = subscribe(&hub, "now");
        assert!(!stale);
        let (_other, _, _, mut other_rx) = hub.subscribe("elsewhere", "now", &json!({}), None);
        hub.apply("src", "now", &json!({}), Update::Value(json!({"t": 1}))).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), Update::Value(_)));
        hub.mark_stale("src");
        assert!(matches!(rx.try_recv().unwrap(), Update::Stale(v) if v == json!({"t": 1})));
        assert!(other_rx.try_recv().is_err(), "another package's feeds are not marked");
        let (value, stale, _) = sub.resync();
        assert!((value, stale) == (json!({"t": 1}), true), "a resync reports the flag");
        let (_late, value, stale, _) = subscribe(&hub, "now");
        assert!(stale && value == json!({"t": 1}), "a subscribe reply carries the flag and the value");
        hub.apply("src", "now", &json!({}), Update::Value(json!({"t": 2}))).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), Update::Value(_)), "the update itself is fresh");
        let (_next, value, stale, _) = subscribe(&hub, "now");
        assert!(!stale && value == json!({"t": 2}));
        hub.mark_stale("src");
        hub.apply(
            "src",
            "now",
            &json!({}),
            Update::Patch(json!([{"op": "replace", "path": "/t", "value": 3}])),
        )
        .unwrap();
        assert!(!sub.resync().1, "a patch clears the flag too");
    }

    #[test]
    fn the_backoff_doubles_from_one_second_to_thirty() {
        let secs: Vec<u64> = (1..=8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(backoff(0).as_secs(), 1);
        assert_eq!(backoff(u32::MAX).as_secs(), 30);
    }

    #[test]
    fn the_policy_decides_from_the_exit_status() {
        for (policy, code, restarts) in [
            (Policy::Always, Some(0), true),
            (Policy::Always, Some(1), true),
            (Policy::Always, None, true),
            (Policy::OnFailure, Some(0), false),
            (Policy::OnFailure, Some(1), true),
            (Policy::OnFailure, None, true),
            (Policy::Never, Some(0), false),
            (Policy::Never, Some(1), false),
            (Policy::Never, None, false),
        ] {
            assert_eq!(policy.restarts(code), restarts, "{policy:?} after {code:?}");
        }
        assert_eq!(Policy::parse("always"), Policy::Always);
        assert_eq!(Policy::parse("never"), Policy::Never);
        assert_eq!(Policy::parse("on-failure"), Policy::OnFailure);
        assert_eq!(Policy::parse(""), Policy::OnFailure);
    }

    #[test]
    fn the_time_left_rounds_up_and_is_at_least_one_second() {
        let secs = |ms: u64| remaining_secs(Duration::from_millis(ms));
        assert_eq!([secs(0), secs(1), secs(999), secs(1000), secs(1001), secs(30_000)], [1, 1, 1, 1, 2, 30]);
    }

    #[test]
    fn a_quick_watch_restart_counts_as_a_failure_and_a_late_one_resets() {
        let at = |s: u64| Some(Duration::from_secs(s));
        assert_eq!(watch_restart(at(0), 0), Some((1, Duration::from_secs(1))));
        assert_eq!(watch_restart(at(9), 1), Some((2, Duration::from_secs(2))));
        assert_eq!(watch_restart(at(2), 5), Some((6, Duration::from_secs(30))));
        assert_eq!(watch_restart(at(10), 4), None, "a launch that stood 10 s resets the backoff");
        assert_eq!(watch_restart(None, 3), None, "no earlier launch resets it too");
    }

    fn spec_for(dir: &Path) -> Spec {
        let declared = BackendInfo {
            command: "old".into(),
            restart: "never".into(),
            watch: vec!["old".into()],
            ..BackendInfo::default()
        };
        Spec {
            key: "k".into(),
            plugin: "p".into(),
            command: declared.command.clone(),
            transport: BackendTransport::Stdio,
            dir: dir.to_path_buf(),
            space_dir: "/space".into(),
            env: Arc::new(Vec::new),
            socket: dir.join("s.sock"),
            log: Log::open(&dir.join("p.log")).unwrap(),
            declared,
            watch_dir: Some(dir.to_path_buf()),
            connect: None,
            machine_file: dir.join("wheel.toml"),
            state: Weak::new(),
        }
    }

    #[test]
    fn a_wheel_call_method_names_the_need_it_requires() {
        assert_eq!(required_need("pane.open", &Value::Null).unwrap(), "pane.open");
        assert_eq!(required_need("notify", &Value::Null).unwrap(), "notify");
        assert_eq!(required_need("command.run", &json!({"command": "ls -l"})).unwrap(), "run:ls -l");
        assert_eq!(required_need("nope", &Value::Null).unwrap_err().code, ErrorCode::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn a_wheel_call_outside_the_needs_is_refused_before_it_acts() {
        let needs = vec!["notify".to_string(), "run:ls".to_string()];
        let none = Weak::new();
        let refused = wheel_call("p", &needs, &none, &json!({"method": "pane.open", "args": {}})).await;
        assert_eq!(refused.unwrap_err().code, ErrorCode::REQUEST_FAILED);
        let other = wheel_call("p", &needs, &none, &json!({"method": "command.run", "args": {"command": "rm -rf /"}})).await;
        assert!(other.unwrap_err().message.contains("run:rm -rf /"));
        let allowed = wheel_call("p", &needs, &none, &json!({"method": "notify", "args": {}})).await;
        assert_eq!(allowed.unwrap_err().message, STOPPING, "an allowed call goes on to the state");
    }

    #[tokio::test]
    async fn a_frame_reader_splits_content_length_messages_and_stops_at_the_end() {
        let bytes: &[u8] = b"Content-Length: 2\r\n\r\n{}content-length:  7\r\nX: y\r\n\r\n[1,2,3]Content-Length: 9\r\n\r\nshort";
        let mut read = BufReader::new(bytes);
        assert_eq!(read_frame(&mut read).await.as_deref(), Some(&b"{}"[..]));
        assert_eq!(read_frame(&mut read).await.as_deref(), Some(&b"[1,2,3]"[..]));
        assert_eq!(read_frame(&mut read).await, None, "a cut frame is the end");
        let mut none = BufReader::new(&b"X: y\r\n\r\n{}"[..]);
        assert_eq!(read_frame(&mut none).await, None, "a frame with no length is the end");
    }

    #[test]
    fn a_restart_runs_what_the_manifest_says_now() {
        let dir = std::env::temp_dir().join(format!("wheel-spec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = spec_for(&dir);
        let text = "[plugin]\nid = \"p\"\nversion = \"1\"\n\n[backend]\ncommand = \"new\"\ntransport = \"unix\"\nrestart = \"always\"\nwatch = [\"*\"]\n";
        let next = spec_from_manifest(&old, text).unwrap();
        assert_eq!(next.command, "new");
        assert_eq!(next.transport, BackendTransport::Unix);
        assert_eq!((next.declared.restart.as_str(), next.declared.watch.clone()), ("always", vec!["*".to_string()]));
        assert_eq!((next.space_dir.as_str(), next.key.as_str()), ("/space", "k"), "what the manifest does not say stays");
        let none = spec_from_manifest(&old, "[plugin]\nid = \"p\"\nversion = \"1\"\n").err().unwrap();
        assert!(none.contains("no [backend]"), "{none}");
        assert!(spec_from_manifest(&old, "[plugin\n").err().unwrap().contains("wheel-plugin.toml"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_from_a_replaced_instance_is_dropped() {
        let hub = Arc::new(Hub::default());
        let (sub, _, _, mut rx) = subscribe(&hub, "f");
        let first = Arc::new(Backend::new("p".into(), BackendInfo::default()));
        let second = Arc::new(Backend::new("p".into(), BackendInfo::default()));
        assert_ne!(first.id, second.id);
        hub.live.lock().unwrap().insert("src".into(), first.clone());
        hub.live.lock().unwrap().insert("src".into(), second.clone());
        let send = |from: &Backend, v: i32| hub.apply_from(Some(from.id), "src", "f", &json!({}), Update::Value(json!(v)));
        send(&first, 1).unwrap();
        assert!(rx.try_recv().is_err(), "the replaced instance's update is dropped");
        assert_eq!(sub.current(), Value::Null);
        send(&second, 2).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), Update::Value(v) if v == json!(2)));
    }

    #[test]
    fn a_closing_backends_refuses_new_supervisors() {
        let backends = Backends::default();
        assert!(!backends.closing.load(Ordering::SeqCst));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(backends.stop_all());
        assert!(backends.closing.load(Ordering::SeqCst));
    }

    #[test]
    fn a_socket_path_fits_a_unix_socket() {
        let short = socket_path(Path::new("/tmp/s"), "/some/package");
        assert!(short.starts_with("/tmp/s/b"));
        let long = socket_path(Path::new(&format!("/{}", "x".repeat(120))), "/some/package");
        assert!(long.as_os_str().len() < 104, "{}", long.display());
    }
}
