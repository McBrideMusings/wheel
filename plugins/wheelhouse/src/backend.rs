//! `wheelhouse backend`: the plugin backend `wheeld` runs for the wheelhouse package. It speaks
//! Wheel's JSON-RPC 2.0 over standard input and output (Content-Length framing, `async-lsp`;
//! `docs/plugin-runtime.md`, "Plugin backends") as the server side.
//!
//! One process serves every project: a feed's params name the project (`{dir}`). The `beads`
//! feed sends the whole value on every subscribe and a JSON Patch on each later change, found by
//! watching the files that can change it. Actions are `press` and `run_list`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_lsp::lsp_types::notification::Notification;
use async_lsp::lsp_types::request::Request;
use async_lsp::router::Router;
use async_lsp::{ClientSocket, ErrorCode, MainLoop, ResponseError};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::beads::feed;

const FEEDS: [&str; 1] = ["beads"];
const ACTIONS: [&str; 2] = ["press", "run_list"];

/// How long file events must stop before the changed projects are read again.
const QUIET: Duration = Duration::from_millis(150);
/// The longest a refresh waits after the first relevant file event, however long events keep coming.
const MAX_WAIT: Duration = Duration::from_secs(1);

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

struct ActionInvoke;

#[derive(Serialize, Deserialize)]
struct ActionInvokeParams {
    action: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    caller: Value,
}

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
    /// Files changed.
    Fs(Vec<PathBuf>),
    /// Read this project again: a press claimed or released one of its beads.
    Refresh(String),
}

struct Sub {
    /// The params as subscribed, echoed in every update.
    params: Value,
    dir: String,
    /// `dir` as the file watcher reports paths under it.
    real: PathBuf,
    /// The value last sent; none until the first.
    last: Option<Value>,
}

struct Core {
    client: ClientSocket,
    subs: Mutex<HashMap<String, Sub>>,
    watched: Mutex<HashSet<PathBuf>>,
    watcher: Mutex<notify::RecommendedWatcher>,
    tx: UnboundedSender<Msg>,
    /// Held while a project is read and its value sent, so two reads never swap order.
    work: tokio::sync::Mutex<()>,
}

fn records_dir() -> Option<PathBuf> {
    crate::beads::buttons::records_file().parent().map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
}

impl Core {
    fn log(&self, level: &str, message: &str) {
        eprintln!("wheelhouse backend: {message}");
        let _ = self.client.notify::<LogN>(json!({ "level": level, "message": message }));
    }

    fn watch(&self, path: PathBuf) {
        let mut watched = self.watched.lock().unwrap();
        if watched.contains(&path) || !path.is_dir() {
            return;
        }
        match self.watcher.lock().unwrap().watch(&path, RecursiveMode::NonRecursive) {
            Ok(()) => {
                watched.insert(path);
            }
            Err(e) => {
                drop(watched);
                self.log("warn", &format!("cannot watch {}: {e}", path.display()));
            }
        }
    }

    /// Watch the project's directory (its `wheel.toml`, and `.beads` appearing) and its `.beads`.
    fn watch_project(&self, real: &std::path::Path) {
        self.watch(real.to_path_buf());
        self.watch(real.join(".beads"));
    }

    /// Stop watching `real` when no subscription in `subs` reads it; `subs` is the locked map.
    fn unwatch_unused(&self, subs: &HashMap<String, Sub>, real: &std::path::Path) {
        if subs.values().any(|s| s.real == real) {
            return;
        }
        let mut watched = self.watched.lock().unwrap();
        for p in [real.to_path_buf(), real.join(".beads")] {
            if watched.remove(&p) {
                let _ = self.watcher.lock().unwrap().unwatch(&p);
            }
        }
    }

    fn subscribe(self: &Arc<Self>, r: FeedRef) {
        if !FEEDS.contains(&r.feed.as_str()) {
            self.log("error", &format!("no feed {}", r.feed));
            return;
        }
        let Some(dir) = r.params.get("dir").and_then(Value::as_str).filter(|d| d.starts_with('/')).map(String::from) else {
            self.log("error", &format!("feed {} needs params {{\"dir\": <absolute directory>}}, got {}", r.feed, r.params));
            return;
        };
        let real = PathBuf::from(&dir).canonicalize().unwrap_or_else(|_| PathBuf::from(&dir));
        {
            // Watching happens while the subscription is added, so an unsubscribe never runs between them.
            let mut subs = self.subs.lock().unwrap();
            self.watch_project(&real);
            subs.insert(key(&r), Sub { params: r.params, dir: dir.clone(), real, last: None });
        }
        let core = self.clone();
        tokio::spawn(async move { core.refresh(&dir).await });
    }

    fn unsubscribe(&self, r: FeedRef) {
        let mut subs = self.subs.lock().unwrap();
        if let Some(sub) = subs.remove(&key(&r)) {
            self.unwatch_unused(&subs, &sub.real);
        }
    }

    /// Read `dir` and send every subscriber of it what changed: the whole value to one that has
    /// none yet, a patch to the others, nothing when the value is the same.
    async fn refresh(&self, dir: &str) {
        let _one_at_a_time = self.work.lock().await;
        {
            // Re-watch under the subs lock: a project nobody reads any more is not watched again.
            let subs = self.subs.lock().unwrap();
            match subs.values().find(|s| s.dir == dir) {
                Some(s) => self.watch_project(&s.real),
                None => return,
            }
        }
        let d = dir.to_string();
        let value = match tokio::task::spawn_blocking(move || feed::value(&d)).await {
            Ok(v) => v,
            Err(e) => return self.log("error", &format!("reading {dir}: {e}")),
        };
        let mut subs = self.subs.lock().unwrap();
        for sub in subs.values_mut().filter(|s| s.dir == dir) {
            let update = match &sub.last {
                None => json!({ "feed": "beads", "params": sub.params, "value": value }),
                Some(old) if same(old, &value) => continue,
                Some(old) => {
                    let patch = serde_json::to_value(json_patch::diff(old, &value)).unwrap_or(Value::Null);
                    json!({ "feed": "beads", "params": sub.params, "patch": patch })
                }
            };
            match self.client.notify::<FeedUpdate>(update) {
                Ok(()) => sub.last = Some(value.clone()),
                Err(e) => eprintln!("wheelhouse backend: feed/update: {e}"),
            }
        }
    }

    /// The projects a batch of file events changes.
    fn affected(&self, paths: &[PathBuf], dirs: &mut BTreeSet<String>) {
        let records = records_dir();
        let subs = self.subs.lock().unwrap();
        for p in paths {
            let (Some(name), Some(parent)) = (p.file_name().and_then(|n| n.to_str()), p.parent()) else { continue };
            if name == "bead-panes.json" && records.as_deref() == Some(parent) {
                dirs.extend(subs.values().map(|s| s.dir.clone()));
                continue;
            }
            for s in subs.values() {
                let hit = match name {
                    "wheel.toml" | ".beads" => parent == s.real,
                    "last-touched" => parent == s.real.join(".beads"),
                    _ => false,
                };
                if hit {
                    dirs.insert(s.dir.clone());
                }
            }
        }
    }

    async fn invoke(self: &Arc<Self>, p: ActionInvokeParams) -> Result<Value, ResponseError> {
        let bad = |m: String| ResponseError::new(ErrorCode::INVALID_PARAMS, m);
        let failed = |m: String| ResponseError::new(ErrorCode::INTERNAL_ERROR, m);
        let arg = |k: &str| p.args.get(k).and_then(Value::as_str).map(String::from);
        let dir = arg("dir").filter(|d| d.starts_with('/')).ok_or_else(|| bad(format!("{} needs args.dir, an absolute directory", p.action)))?;
        let pane = p.caller.get("pane").and_then(Value::as_str).unwrap_or_default().to_string();
        let said = match p.action.as_str() {
            "press" => {
                let bead = arg("bead").ok_or_else(|| bad("press needs args.bead".into()))?;
                let tx = self.tx.clone();
                let changed_dir = dir.clone();
                let changed = move || {
                    let _ = tx.send(Msg::Refresh(changed_dir.clone()));
                };
                tokio::task::spawn_blocking(move || feed::press(&dir, &bead, &pane, changed)).await
            }
            "run_list" => {
                let script = arg("script").ok_or_else(|| bad("run_list needs args.script".into()))?;
                let ids: Vec<String> = match p.args.get("ids") {
                    Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    _ => return Err(bad("run_list needs args.ids, a list of bead ids".into())),
                };
                tokio::task::spawn_blocking(move || feed::run_list(&dir, &script, &ids, &pane)).await
            }
            other => {
                return Err(ResponseError::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    format!("no action {other}; its actions: {}", ACTIONS.join(", ")),
                ))
            }
        };
        match said {
            Ok(Ok(said)) => Ok(json!({ "said": said })),
            Ok(Err(e)) => Err(failed(e)),
            Err(e) => Err(failed(e.to_string())),
        }
    }
}

/// Whether two snapshots differ in anything but `now`, the time each was read.
fn same(a: &Value, b: &Value) -> bool {
    let bare = |v: &Value| {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("now");
        }
        v
    };
    bare(a) == bare(b)
}

fn key(r: &FeedRef) -> String {
    format!("{}\n{}", r.feed, r.params)
}

/// Read file events, wait for them to stop, then refresh each project they changed. Only events that
/// change a subscribed project restart the quiet wait, and a refresh comes at most `MAX_WAIT` after
/// the first of them however long events keep arriving.
async fn events(core: Arc<Core>, mut rx: UnboundedReceiver<Msg>) {
    let take = |core: &Core, m: Msg, dirs: &mut BTreeSet<String>| match m {
        Msg::Fs(paths) => core.affected(&paths, dirs),
        Msg::Refresh(d) => {
            dirs.insert(d);
        }
    };
    let mut dirs = BTreeSet::new();
    loop {
        while dirs.is_empty() {
            let Some(m) = rx.recv().await else { return };
            take(&core, m, &mut dirs);
        }
        let deadline = tokio::time::Instant::now() + MAX_WAIT;
        let mut quiet = tokio::time::Instant::now() + QUIET;
        loop {
            let until = quiet.min(deadline);
            match tokio::time::timeout_at(until, rx.recv()).await {
                Ok(Some(m)) => {
                    let mut seen = BTreeSet::new();
                    take(&core, m, &mut seen);
                    if !seen.is_empty() {
                        quiet = tokio::time::Instant::now() + QUIET;
                    }
                    dirs.extend(seen);
                }
                Ok(None) => return,
                Err(_) => break,
            }
        }
        for dir in std::mem::take(&mut dirs) {
            core.refresh(&dir).await;
        }
    }
}

pub async fn run() -> anyhow::Result<()> {
    let (tx, rx) = unbounded_channel();
    let events_tx = tx.clone();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(e) = res {
            if !matches!(e.kind, EventKind::Access(_)) {
                let _ = events_tx.send(Msg::Fs(e.paths));
            }
        }
    })?;
    let (main_loop, _) = MainLoop::new_server(|client| {
        let core = Arc::new(Core {
            client,
            subs: Mutex::default(),
            watched: Mutex::default(),
            watcher: Mutex::new(watcher),
            tx,
            work: tokio::sync::Mutex::new(()),
        });
        if let Some(dir) = records_dir() {
            core.watch(dir);
        }
        tokio::spawn(events(core.clone(), rx));
        let mut router = Router::new(core);
        router
            .request::<Initialize, _>(|_, _| async {
                Ok(InitializeResult {
                    feeds: FEEDS.map(String::from).to_vec(),
                    actions: ACTIONS.map(String::from).to_vec(),
                })
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
    // A press may still be waiting on its command; the process leaves now, as `shutdown` asks.
    if let Err(e) = &result {
        eprintln!("wheelhouse backend: {e}");
    }
    std::process::exit(if result.is_ok() { 0 } else { 1 })
}
