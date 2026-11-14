//! `wheel-runner backend`: the plugin backend `wheeld` runs for the `runner` package. It speaks
//! Wheel's JSON-RPC 2.0 over standard input and output (Content-Length framing, `async-lsp`;
//! `docs/plugin-runtime.md`, "Plugin backends") as the server side.
//!
//! One process serves every project: a feed's params name the project (`{dir}`).
//!
//! - `commands {dir}`: the project's `[runner]` commands (the machine's too), its recent runs and the
//!   reading of each command's status check.
//! - `log {dir, id}`: one run's last [`LOG_LINES`] lines and its state.
//!
//! Both send the whole value on subscribe and a JSON Patch after, read again when the project's runs
//! folder or a `wheel.toml` changes (a file watcher), once a second while a run goes (its duration
//! is in the value) and, for the status checks, every [`CHECK_EVERY`]. Actions are `run` and `stop`.
//! A run belongs to this process: it keeps going while no pane reads it, and ends with the process.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_lsp::lsp_types::notification::Notification;
use async_lsp::lsp_types::request::Request;
use async_lsp::router::Router;
use async_lsp::{ClientSocket, ErrorCode, MainLoop, ResponseError};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::check::Reading;
use crate::config;
use crate::runs::{Runs, StartError};
use crate::store::{self, Record, State};

/// How many of a log's last lines the `log` feed holds.
const LOG_LINES: usize = 300;

/// How often every subscribed project's status checks are read again.
const CHECK_EVERY: Duration = Duration::from_secs(15);

/// How long file events must stop before the changed projects are read again.
const QUIET: Duration = Duration::from_millis(100);
/// The longest a refresh waits after the first event, however long events keep coming.
const MAX_WAIT: Duration = Duration::from_secs(1);

const FEEDS: [&str; 2] = ["commands", "log"];
const ACTIONS: [&str; 2] = ["run", "stop"];

struct Initialize;

#[derive(Serialize, Deserialize, Default)]
struct InitializeResult {
    feeds: Vec<String>,
    actions: Vec<String>,
}

impl Request for Initialize {
    type Params = Value;
    type Result = InitializeResult;
    const METHOD: &'static str = "initialize";
}

#[derive(Serialize, Deserialize)]
struct FeedRef {
    feed: String,
    #[serde(default)]
    params: Value,
}

struct FeedSubscribe;

impl Notification for FeedSubscribe {
    type Params = FeedRef;
    const METHOD: &'static str = "feed/subscribe";
}

struct FeedUnsubscribe;

impl Notification for FeedUnsubscribe {
    type Params = FeedRef;
    const METHOD: &'static str = "feed/unsubscribe";
}

struct Shutdown;

impl Notification for Shutdown {
    type Params = Value;
    const METHOD: &'static str = "shutdown";
}

#[derive(Serialize, Deserialize)]
struct ActionInvokeParams {
    action: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    caller: Value,
}

struct ActionInvoke;

impl Request for ActionInvoke {
    type Params = ActionInvokeParams;
    type Result = Value;
    const METHOD: &'static str = "action/invoke";
}

struct FeedUpdate;

impl Notification for FeedUpdate {
    type Params = Value;
    const METHOD: &'static str = "feed/update";
}

struct LogN;

impl Notification for LogN {
    type Params = Value;
    const METHOD: &'static str = "log";
}

enum Msg {
    /// Something a subscription reads may have changed.
    Refresh,
    /// The status checks of this project were read.
    Checked(String, HashMap<String, Reading>),
}

struct Sub {
    feed: String,
    /// The params as subscribed, echoed in every update.
    params: Value,
    dir: String,
    /// The run a `log` subscription follows.
    id: Option<String>,
    /// The value last sent; none until the first.
    last: Option<Value>,
}

fn key(r: &FeedRef) -> String {
    format!("{}\u{0}{}", r.feed, r.params)
}

struct Core {
    client: ClientSocket,
    subs: Mutex<HashMap<String, Sub>>,
    watcher: Mutex<notify::RecommendedWatcher>,
    /// The directories watched (never recursively), and why: a project's runs folder, a project
    /// directory or the machine file's directory.
    watched: Mutex<HashSet<PathBuf>>,
    /// The last reading of each project's status checks, by command name.
    checks: Mutex<HashMap<String, HashMap<String, Reading>>>,
    tx: UnboundedSender<Msg>,
    /// One refresh at a time, so two reads never swap order.
    reading: tokio::sync::Mutex<()>,
}

/// A run as a feed shows it.
fn run_value(r: &Record) -> Value {
    json!({ "id": r.id, "name": r.name, "state": r.state, "exit_code": r.exit_code, "started": r.started, "ended": r.ended })
}

/// The `commands` feed's value for `dir`. `now` is 0 when no run goes, so an idle project's value
/// never changes with the clock.
fn commands_value(dir: &str, checks: &HashMap<String, Reading>) -> Value {
    let runners = config::list(&config::machine_file(), dir);
    let runs = Runs::new(dir).list();
    let running = runs.iter().any(|r| r.state == State::Running);
    let rows: Vec<Value> = runners
        .iter()
        .map(|r| {
            json!({ "name": r.name, "label": r.label, "group": r.group, "desc": r.desc, "command": r.command, "confirm": r.confirm,
                    "origin": r.origin, "worktree": r.worktree, "status": r.status.is_some() })
        })
        .collect();
    json!({
        "dir": dir,
        "now": if running { crate::now() } else { 0 },
        "runners": rows,
        "runs": runs.iter().map(run_value).collect::<Vec<_>>(),
        "checks": checks,
    })
}

/// The `log` feed's value: the run's state and its last `LOG_LINES` lines, or why it cannot be read.
fn log_value(dir: &str, id: &str) -> Value {
    let runs = Runs::new(dir);
    match (runs.get(id), runs.log(id, 0, Some(LOG_LINES))) {
        (Ok(r), Ok((text, _))) => {
            let lines: Vec<&str> = text.lines().collect();
            json!({ "id": id, "name": r.name, "state": r.state, "exit_code": r.exit_code, "started": r.started, "ended": r.ended, "lines": lines, "error": null })
        }
        (Err(e), _) | (_, Err(e)) => json!({ "id": id, "name": null, "state": null, "exit_code": null, "started": null, "ended": null, "lines": [], "error": e }),
    }
}

/// An RFC 6902 patch that takes `old` to `new`. A log's `lines` slide: lines leave at the front and
/// arrive at the end, which the patch says as removes of index 0 and adds at `/lines/-` rather than a
/// replace of every index. None when the two are the same.
fn patch(old: &Value, new: &Value) -> Option<Value> {
    if old == new {
        return None;
    }
    if let (Some(a), Some(b)) = (old["lines"].as_array(), new["lines"].as_array()) {
        // The smallest number of lines `k` that left the front such that what remains is a prefix of `b`.
        if let Some(k) = (0..=a.len()).find(|&k| b.starts_with(&a[k..])) {
            let mut ops: Vec<Value> = vec![];
            if let Some(map) = new.as_object() {
                for (field, value) in map.iter().filter(|(f, _)| f.as_str() != "lines") {
                    if old.get(field) != Some(value) {
                        ops.push(json!({ "op": "replace", "path": format!("/{field}"), "value": value }));
                    }
                }
            }
            ops.extend((0..k).map(|_| json!({ "op": "remove", "path": "/lines/0" })));
            ops.extend(b[a.len() - k..].iter().map(|l| json!({ "op": "add", "path": "/lines/-", "value": l })));
            return Some(Value::Array(ops));
        }
    }
    serde_json::to_value(json_patch::diff(old, new)).ok()
}

/// Whether a changed path matters: anything in a runs folder, a `wheel.toml` elsewhere.
fn relevant(p: &Path, root: &Path) -> bool {
    p.file_name().is_some_and(|n| n == "wheel.toml") || p.parent().is_some_and(|d| d.starts_with(root))
}

impl Core {
    fn log(&self, level: &str, message: &str) {
        eprintln!("wheel-runner backend: {message}");
        let _ = self.client.notify::<LogN>(json!({ "level": level, "message": message }));
    }

    fn watch(&self, dir: PathBuf) {
        if !dir.is_dir() {
            return;
        }
        let mut watched = self.watched.lock().unwrap();
        if watched.contains(&dir) {
            return;
        }
        match self.watcher.lock().unwrap().watch(&dir, RecursiveMode::NonRecursive) {
            Ok(()) => {
                watched.insert(dir);
            }
            Err(e) => {
                drop(watched);
                self.log("warn", &format!("cannot watch {}: {e}", dir.display()));
            }
        }
    }

    fn subscribe(self: &Arc<Self>, r: FeedRef) {
        if !FEEDS.contains(&r.feed.as_str()) {
            return self.log("error", &format!("no feed {}", r.feed));
        }
        let Some(dir) = r.params.get("dir").and_then(Value::as_str).filter(|d| d.starts_with('/')).map(String::from) else {
            return self.log("error", &format!("feed {} needs params {{\"dir\": <absolute directory>}}, got {}", r.feed, r.params));
        };
        let id = r.params.get("id").and_then(Value::as_str).map(String::from);
        if r.feed == "log" && id.as_deref().is_none_or(|i| !store::valid_id(i)) {
            return self.log("error", &format!("feed log needs params {{\"dir\", \"id\": <a run's id>}}, got {}", r.params));
        }
        // The runs folder is made before it is watched, so the first run is an event.
        let folder = store::folder(&dir);
        let _ = std::fs::create_dir_all(&folder);
        self.watch(folder.canonicalize().unwrap_or(folder));
        self.watch(PathBuf::from(&dir).canonicalize().unwrap_or_else(|_| PathBuf::from(&dir)));
        if let Some(parent) = config::machine_file().parent() {
            self.watch(parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf()));
        }
        let first_for_dir = {
            let mut subs = self.subs.lock().unwrap();
            let first = !subs.values().any(|s| s.feed == "commands" && s.dir == dir);
            subs.insert(key(&r), Sub { feed: r.feed.clone(), params: r.params, dir: dir.clone(), id, last: None });
            first && r.feed == "commands"
        };
        if first_for_dir {
            self.read_checks(dir);
        }
        let _ = self.tx.send(Msg::Refresh);
    }

    fn unsubscribe(&self, r: FeedRef) {
        self.subs.lock().unwrap().remove(&key(&r));
    }

    /// Read every status check of `dir` once, off the runtime's threads, and tell the loop.
    fn read_checks(self: &Arc<Self>, dir: String) {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut found = HashMap::new();
            for r in config::list(&config::machine_file(), &dir) {
                if let Some(c) = &r.status {
                    found.insert(r.name.clone(), crate::check::run(c, Path::new(&dir)));
                }
            }
            let _ = tx.send(Msg::Checked(dir, found));
        });
    }

    /// Read every subscription again and send what changed: the whole value to one that has none yet,
    /// a patch to the others, nothing when the value is the same.
    async fn refresh(&self) {
        let _one_at_a_time = self.reading.lock().await;
        let subs: Vec<(String, String, String, Option<String>)> =
            self.subs.lock().unwrap().iter().map(|(k, s)| (k.clone(), s.feed.clone(), s.dir.clone(), s.id.clone())).collect();
        let checks = self.checks.lock().unwrap().clone();
        let read = tokio::task::spawn_blocking(move || {
            subs.into_iter()
                .map(|(k, feed, dir, id)| {
                    let value = match feed.as_str() {
                        "log" => log_value(&dir, id.as_deref().unwrap_or_default()),
                        _ => commands_value(&dir, checks.get(&dir).unwrap_or(&HashMap::new())),
                    };
                    (k, value)
                })
                .collect::<Vec<_>>()
        })
        .await;
        let values = match read {
            Ok(v) => v,
            Err(e) => return self.log("error", &format!("reading: {e}")),
        };
        let mut subs = self.subs.lock().unwrap();
        for (k, value) in values {
            let Some(sub) = subs.get_mut(&k) else { continue };
            let update = match &sub.last {
                None => json!({ "feed": sub.feed, "params": sub.params, "value": value }),
                Some(old) => match patch(old, &value) {
                    None => continue,
                    Some(p) => json!({ "feed": sub.feed, "params": sub.params, "patch": p }),
                },
            };
            match self.client.notify::<FeedUpdate>(update) {
                Ok(()) => sub.last = Some(value),
                Err(e) => eprintln!("wheel-runner backend: feed/update: {e}"),
            }
        }
    }

    /// Keep what a message carries: a status check's reading.
    fn take(&self, msg: Msg) {
        if let Msg::Checked(dir, found) = msg {
            self.checks.lock().unwrap().insert(dir, found);
        }
    }

    /// Whether a subscribed project has a run going, from the last value sent: its duration needs a
    /// new value each second.
    fn running(&self) -> bool {
        self.subs.lock().unwrap().values().any(|s| s.feed == "commands" && s.last.as_ref().is_some_and(|v| v["now"].as_i64().unwrap_or(0) > 0))
    }

    fn dirs(&self) -> BTreeSet<String> {
        self.subs.lock().unwrap().values().filter(|s| s.feed == "commands").map(|s| s.dir.clone()).collect()
    }

    async fn invoke(self: &Arc<Self>, p: ActionInvokeParams) -> Result<Value, ResponseError> {
        let bad = |m: String| ResponseError::new(ErrorCode::INVALID_PARAMS, m);
        let dir = p.args.get("dir").and_then(Value::as_str).filter(|d| d.starts_with('/')).map(String::from)
            .ok_or_else(|| bad(format!("{} needs args {{\"dir\": <absolute directory>}}", p.action)))?;
        let text = |field: &str| p.args.get(field).and_then(Value::as_str).map(String::from);
        let result = match p.action.as_str() {
            "run" => {
                let name = text("name").ok_or_else(|| bad("run needs args {dir, name}".into()))?;
                let (confirmed, worktree) = (text("confirmed"), text("worktree"));
                let started = tokio::task::spawn_blocking(move || -> Result<Value, String> {
                    let Some(runner) = config::list(&config::machine_file(), &dir).into_iter().find(|r| r.name == name) else {
                        return Err(format!("no runner named {name}"));
                    };
                    let runs = match &worktree {
                        Some(w) => Runs::new(&dir).in_worktree(w),
                        None => Runs::new(&dir),
                    };
                    match runs.start(&runner, confirmed.as_deref(), None) {
                        Ok(s) => Ok(json!({ "started": true, "id": s.record.id, "log": s.record.log })),
                        Err(StartError::Confirm(q)) => Ok(json!({ "started": false, "confirm": q, "command": runner.command })),
                        Err(StartError::Guard(m)) => Ok(json!({ "started": false, "guard": m })),
                        Err(StartError::Failed(e)) => Err(e),
                    }
                })
                .await;
                started.map_err(|e| bad(e.to_string()))?.map_err(bad)?
            }
            "stop" => {
                let id = text("id").ok_or_else(|| bad("stop needs args {dir, id}".into()))?;
                // Ending a run waits for its processes to go; the backend's other work goes on.
                let ended = tokio::task::spawn_blocking(move || Runs::new(&dir).stop(&id)).await;
                let record = ended.map_err(|e| bad(e.to_string()))?.map_err(bad)?;
                json!({ "stopping": record.id })
            }
            other => return Err(ResponseError::new(ErrorCode::METHOD_NOT_FOUND, format!("no action {other}"))),
        };
        let _ = self.tx.send(Msg::Refresh);
        Ok(result)
    }
}

/// Reads again 100 ms after the last event, once a second while a run goes, and takes each status
/// check reading; the status checks are read again every `CHECK_EVERY`.
async fn events(core: Arc<Core>, mut rx: UnboundedReceiver<Msg>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut checked = tokio::time::interval(CHECK_EVERY);
    checked.tick().await;
    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else { return };
                core.take(msg);
                // Further events inside the quiet window are one refresh, but a command that prints
                // without pause still gets one at most MAX_WAIT after the first event.
                let deadline = tokio::time::Instant::now() + MAX_WAIT;
                loop {
                    let until = (tokio::time::Instant::now() + QUIET).min(deadline);
                    match tokio::time::timeout_at(until, rx.recv()).await {
                        Ok(Some(m)) => core.take(m),
                        Ok(None) => return,
                        Err(_) => break,
                    }
                }
                core.refresh().await;
            }
            _ = tick.tick() => {
                if core.running() {
                    core.refresh().await;
                }
            }
            _ = checked.tick() => {
                for dir in core.dirs() {
                    core.read_checks(dir);
                }
            }
        }
    }
}

pub async fn run() -> anyhow::Result<()> {
    let (tx, rx) = unbounded_channel::<Msg>();
    let (main_loop, _) = MainLoop::new_server(|client| {
        let events_tx = tx.clone();
        // Events name real paths, so the runs root is compared as one.
        let root = store::root().canonicalize().unwrap_or_else(|_| store::root());
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(e) = event {
                if e.paths.iter().any(|p| relevant(p, &root)) {
                    let _ = events_tx.send(Msg::Refresh);
                }
            }
        })
        .expect("a file watcher");
        let core = Arc::new(Core {
            client,
            subs: Mutex::default(),
            watcher: Mutex::new(watcher),
            watched: Mutex::default(),
            checks: Mutex::default(),
            tx: tx.clone(),
            reading: tokio::sync::Mutex::new(()),
        });
        tokio::spawn(events(core.clone(), rx));
        let mut router = Router::new(core);
        router
            .request::<Initialize, _>(|_, _| async {
                Ok(InitializeResult { feeds: FEEDS.map(String::from).to_vec(), actions: ACTIONS.map(String::from).to_vec() })
            })
            .notification::<FeedSubscribe>(|core, r| {
                core.subscribe(r);
                ControlFlow::Continue(())
            })
            .notification::<FeedUnsubscribe>(|core, r| {
                core.unsubscribe(r);
                ControlFlow::Continue(())
            })
            .notification::<Shutdown>(|_, _| ControlFlow::Break(Ok(())))
            .request::<ActionInvoke, _>(|core, p| {
                let core = core.clone();
                async move { core.invoke(p).await }
            })
            .unhandled_notification(|_, _| ControlFlow::Continue(()));
        router
    });
    let result = main_loop.run_buffered(tokio::io::stdin().compat(), tokio::io::stdout().compat_write()).await;
    if let Err(e) = &result {
        eprintln!("wheel-runner backend: {e}");
    }
    std::process::exit(if result.is_ok() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(state: &str, lines: &[&str]) -> Value {
        json!({ "id": "r", "state": state, "lines": lines })
    }

    fn apply(v: &Value, patch: &Value) -> Value {
        let mut out = v.clone();
        let patch: json_patch::Patch = serde_json::from_value(patch.clone()).unwrap();
        json_patch::patch(&mut out, &patch).unwrap();
        out
    }

    #[test]
    fn a_log_that_grows_is_appended_to() {
        let (a, b) = (log("running", &["1", "2"]), log("running", &["1", "2", "3"]));
        let p = patch(&a, &b).unwrap();
        assert_eq!(p, json!([{ "op": "add", "path": "/lines/-", "value": "3" }]));
        assert_eq!(apply(&a, &p), b);
    }

    #[test]
    fn a_full_window_slides_and_a_state_change_replaces_the_state() {
        let (a, b) = (log("running", &["1", "2", "3"]), log("exited", &["2", "3", "4", "5"]));
        let p = patch(&a, &b).unwrap();
        assert_eq!(
            p,
            json!([
                { "op": "replace", "path": "/state", "value": "exited" },
                { "op": "remove", "path": "/lines/0" },
                { "op": "add", "path": "/lines/-", "value": "4" },
                { "op": "add", "path": "/lines/-", "value": "5" },
            ])
        );
        assert_eq!(apply(&a, &p), b);
    }

    #[test]
    fn a_log_that_shares_nothing_with_the_last_is_a_diff_and_an_equal_one_is_none() {
        let (a, b) = (log("running", &["1", "2"]), log("running", &["x", "y"]));
        assert_eq!(apply(&a, &patch(&a, &b).unwrap()), b);
        assert_eq!(patch(&a, &a), None);
        let (c, d) = (json!({ "now": 0, "runs": [] }), json!({ "now": 5, "runs": [] }));
        assert_eq!(apply(&c, &patch(&c, &d).unwrap()), d);
    }
}
