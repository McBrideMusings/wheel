//! `wheelhouse backend`: the plugin backend `wheeld` runs for the wheelhouse package. It speaks
//! Wheel's JSON-RPC 2.0 over standard input and output (Content-Length framing, `async-lsp`;
//! `docs/plugin-runtime.md`, "Plugin backends") as the server side.
//!
//! One process serves every project: a feed's params name the project (`{dir}`). The `beads` and
//! `repo` feeds send the whole value on every subscribe and a JSON Patch on each later change,
//! found by watching the files that can change it (the `repo` feed is also read again every 120 s,
//! for the open PRs and checks only `gh` knows). Actions are `press`, `run_list` and `branch_delete`.

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

use crate::beads::feed as beads_feed;
use crate::repo::feed as repo_feed;

/// The lock that orders the reads of one (feed, project).
type ReadLock = Arc<tokio::sync::Mutex<()>>;

const FEEDS: [&str; 2] = ["beads", "repo"];
const ACTIONS: [&str; 3] = ["press", "run_list", "branch_delete"];

/// How often a `repo` subscription is read again whatever the files do: GitHub has no event.
const REPO_EVERY: Duration = Duration::from_secs(120);

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
    /// Read this one feed of this project again.
    RefreshFeed(String, String),
}

struct Sub {
    feed: String,
    /// The params as subscribed, echoed in every update.
    params: Value,
    dir: String,
    /// `dir` as the file watcher reports paths under it.
    real: PathBuf,
    /// The directories this subscription needs watched, and whether each is watched recursively.
    watch: Vec<(PathBuf, bool)>,
    /// A `repo` subscription: the files whose change matters (found by their exact path)...
    files: Vec<PathBuf>,
    /// ...and the directories under which any change matters.
    trees: Vec<PathBuf>,
    /// The value last sent; none until the first.
    last: Option<Value>,
}

/// What a subscription to `feed` for `dir` needs watched: (directories, files, trees).
fn watches(feed: &str, dir: &str, real: &std::path::Path) -> (Vec<(PathBuf, bool)>, Vec<PathBuf>, Vec<PathBuf>) {
    if feed != "repo" {
        return (vec![(real.to_path_buf(), false), (real.join(".beads"), false)], vec![], vec![]);
    }
    let Some((own, common)) = crate::core::git::dirs(dir) else { return Default::default() };
    let (own, common) = (PathBuf::from(own), PathBuf::from(common));
    let files = [own.join("logs/HEAD"), own.join("HEAD"), common.join("packed-refs"), common.join("refs/stash")];
    let trees = [common.join("refs/heads"), common.join("refs/remotes/origin"), common.join("worktrees")];
    // The watcher reports real paths, so name them that way: the nearest existing ancestor made real,
    // then the rest (a file or tree that does not exist yet).
    let real_of = |p: &PathBuf| {
        let mut tail = vec![];
        let mut at = p.clone();
        loop {
            if let Ok(real) = at.canonicalize() {
                return tail.iter().rev().fold(real, |acc, n| acc.join(n));
            }
            match (at.parent().map(|d| d.to_path_buf()), at.file_name().map(|n| n.to_os_string())) {
                (Some(d), Some(n)) => {
                    tail.push(n);
                    at = d;
                }
                _ => return p.clone(),
            }
        }
    };
    let files: Vec<PathBuf> = files.iter().map(real_of).collect();
    let trees: Vec<PathBuf> = trees.iter().map(real_of).collect();
    let mut watch: Vec<(PathBuf, bool)> = files.iter().filter_map(|f| f.parent().map(|d| (d.to_path_buf(), false))).collect();
    for t in &trees {
        if t.is_dir() {
            watch.push((t.clone(), true));
        } else {
            // A tree that does not exist yet (`refs/remotes/origin` before the first fetch): watch the
            // nearest ancestor that does, so its creation is an event; the next read watches the tree.
            if let Some(up) = t.ancestors().skip(1).find(|a| a.is_dir()) {
                watch.push((up.to_path_buf(), false));
            }
        }
    }
    (watch, files, trees)
}

struct Core {
    client: ClientSocket,
    subs: Mutex<HashMap<String, Sub>>,
    watched: Mutex<HashSet<PathBuf>>,
    watcher: Mutex<notify::RecommendedWatcher>,
    tx: UnboundedSender<Msg>,
    /// One lock per (feed, project), held while it is read and its value sent, so two reads of the same
    /// feed and project never swap order and a slow read (`gh`) delays nothing else.
    work: Mutex<HashMap<(String, String), ReadLock>>,
    /// The (feed, project) pairs a refresh is queued for and has not started reading: another event for
    /// one of them needs no second refresh.
    waiting: Mutex<HashSet<(String, String)>>,
}

fn records_dir() -> Option<PathBuf> {
    crate::beads::buttons::records_file().parent().map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
}

impl Core {
    fn log(&self, level: &str, message: &str) {
        eprintln!("wheelhouse backend: {message}");
        let _ = self.client.notify::<LogN>(json!({ "level": level, "message": message }));
    }

    fn watch(&self, path: PathBuf, recursive: bool) {
        let mut watched = self.watched.lock().unwrap();
        if watched.contains(&path) || !path.is_dir() {
            return;
        }
        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        match self.watcher.lock().unwrap().watch(&path, mode) {
            Ok(()) => {
                watched.insert(path);
            }
            Err(e) => {
                drop(watched);
                self.log("warn", &format!("cannot watch {}: {e}", path.display()));
            }
        }
    }

    /// Stop watching every path in `gone` that no subscription in `subs` needs; `subs` is the locked map.
    fn unwatch_unused(&self, subs: &HashMap<String, Sub>, gone: &[(PathBuf, bool)]) {
        let mut watched = self.watched.lock().unwrap();
        for (p, _) in gone {
            if !subs.values().any(|s| s.watch.iter().any(|(w, _)| w == p)) && watched.remove(p) {
                let _ = self.watcher.lock().unwrap().unwatch(p);
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
        let feed = r.feed.clone();
        {
            // Watching happens while the subscription is added, so an unsubscribe never runs between them.
            // A `repo` subscription's paths come from git, so its first refresh finds them.
            let mut subs = self.subs.lock().unwrap();
            let (watch, files, trees) = if feed == "repo" { Default::default() } else { watches(&feed, &dir, &real) };
            for (p, rec) in &watch {
                self.watch(p.clone(), *rec);
            }
            let old = subs.insert(key(&r), Sub { feed: feed.clone(), params: r.params, dir: dir.clone(), real, watch, files, trees, last: None });
            if let Some(old) = old {
                self.unwatch_unused(&subs, &old.watch);
            }
        }
        let core = self.clone();
        tokio::spawn(async move { core.refresh(&feed, &dir).await });
    }

    fn unsubscribe(&self, r: FeedRef) {
        let mut subs = self.subs.lock().unwrap();
        if let Some(sub) = subs.remove(&key(&r)) {
            self.unwatch_unused(&subs, &sub.watch);
        }
    }

    /// Read `feed` of `dir` and send every subscriber of it what changed: the whole value to one that
    /// has none yet, a patch to the others, nothing when the value is the same.
    async fn refresh(&self, feed: &str, dir: &str) {
        let lock = self.work.lock().unwrap().entry((feed.to_string(), dir.to_string())).or_default().clone();
        let _one_at_a_time = lock.lock().await;
        self.waiting.lock().unwrap().remove(&(feed.to_string(), dir.to_string()));
        let real = {
            let subs = self.subs.lock().unwrap();
            match subs.values().find(|s| s.feed == feed && s.dir == dir) {
                Some(s) => s.real.clone(),
                None => return,
            }
        };
        // Find what to watch and watch it before reading, so a change made while the read runs is an event.
        let (f, d) = (feed.to_string(), dir.to_string());
        let paths = tokio::task::spawn_blocking(move || watches(&f, &d, &real)).await;
        let (watch, files, trees) = match paths {
            Ok(v) => v,
            Err(e) => return self.log("error", &format!("reading {dir}: {e}")),
        };
        {
            // Under the subs lock, so a project nobody reads any more is not watched again.
            let subs = self.subs.lock().unwrap();
            if subs.values().any(|s| s.feed == feed && s.dir == dir) {
                for (p, rec) in &watch {
                    self.watch(p.clone(), *rec);
                }
            }
        }
        let (f, d) = (feed.to_string(), dir.to_string());
        let read = tokio::task::spawn_blocking(move || if f == "repo" { repo_feed::value(&d) } else { beads_feed::value(&d) }).await;
        let value = match read {
            Ok(v) => v,
            Err(e) => return self.log("error", &format!("reading {dir}: {e}")),
        };
        let mut subs = self.subs.lock().unwrap();
        let mut gone = vec![];
        for sub in subs.values_mut().filter(|s| s.feed == feed && s.dir == dir) {
            if !watch.is_empty() || feed != "repo" {
                gone.extend(std::mem::replace(&mut sub.watch, watch.clone()));
                sub.files = files.clone();
                sub.trees = trees.clone();
            }
            let update = match &sub.last {
                None => json!({ "feed": feed, "params": sub.params, "value": value }),
                Some(old) if same(old, &value) => continue,
                Some(old) => {
                    let patch = serde_json::to_value(json_patch::diff(old, &value)).unwrap_or(Value::Null);
                    json!({ "feed": feed, "params": sub.params, "patch": patch })
                }
            };
            match self.client.notify::<FeedUpdate>(update) {
                Ok(()) => sub.last = Some(value.clone()),
                Err(e) => eprintln!("wheelhouse backend: feed/update: {e}"),
            }
        }
        self.unwatch_unused(&subs, &gone);
    }

    /// The (feed, project) pairs a batch of file events changes.
    fn affected(&self, paths: &[PathBuf], out: &mut BTreeSet<(String, String)>) {
        let records = records_dir();
        let subs = self.subs.lock().unwrap();
        for p in paths {
            let (Some(name), Some(parent)) = (p.file_name().and_then(|n| n.to_str()), p.parent()) else { continue };
            for s in subs.values() {
                let hit = if s.feed == "repo" {
                    s.files.iter().any(|f| f == p) || s.trees.iter().any(|t| p.starts_with(t) || t.starts_with(p))
                } else if name == "bead-panes.json" && records.as_deref() == Some(parent) {
                    true
                } else {
                    match name {
                        "wheel.toml" | ".beads" => parent == s.real,
                        "last-touched" => parent == s.real.join(".beads"),
                        _ => false,
                    }
                };
                if hit {
                    out.insert((s.feed.clone(), s.dir.clone()));
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
                    let _ = tx.send(Msg::RefreshFeed("beads".into(), changed_dir.clone()));
                };
                tokio::task::spawn_blocking(move || beads_feed::press(&dir, &bead, &pane, changed)).await
            }
            "run_list" => {
                let script = arg("script").ok_or_else(|| bad("run_list needs args.script".into()))?;
                let view = arg("view").unwrap_or_else(|| "beads".into());
                if view != "beads" && view != "repo" {
                    return Err(bad(format!("run_list: no view {view}; its views: beads, repo")));
                }
                let ids: Vec<String> = match p.args.get("ids") {
                    Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    _ => return Err(bad("run_list needs args.ids, a list of ids".into())),
                };
                tokio::task::spawn_blocking(move || {
                    if view == "repo" {
                        repo_feed::run_list(&dir, &script, &ids, &pane)
                    } else {
                        beads_feed::run_list(&dir, &script, &ids, &pane)
                    }
                })
                .await
            }
            "branch_delete" => {
                let branch = arg("branch").ok_or_else(|| bad("branch_delete needs args.branch".into()))?;
                let changed_dir = dir.clone();
                let done = tokio::task::spawn_blocking(move || repo_feed::delete(&dir, &branch)).await;
                // Read the project again whether or not the delete went through.
                let _ = self.tx.send(Msg::RefreshFeed("repo".into(), changed_dir));
                done
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
    let take = |core: &Core, m: Msg, out: &mut BTreeSet<(String, String)>| match m {
        Msg::Fs(paths) => core.affected(&paths, out),
        Msg::RefreshFeed(f, d) => {
            out.insert((f, d));
        }
    };
    let mut pending = BTreeSet::new();
    loop {
        while pending.is_empty() {
            let Some(m) = rx.recv().await else { return };
            take(&core, m, &mut pending);
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
                    pending.extend(seen);
                }
                Ok(None) => return,
                Err(_) => break,
            }
        }
        // Each refresh runs on its own, so a slow one (`gh`) holds up only its own feed and project.
        for (feed, dir) in std::mem::take(&mut pending) {
            if !core.waiting.lock().unwrap().insert((feed.clone(), dir.clone())) {
                continue;
            }
            let core = core.clone();
            tokio::spawn(async move { core.refresh(&feed, &dir).await });
        }
    }
}

/// Ask for every `repo` subscription to be read again each `REPO_EVERY`.
async fn repo_clock(core: Arc<Core>) {
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + REPO_EVERY, REPO_EVERY);
    loop {
        tick.tick().await;
        let dirs: BTreeSet<String> = core.subs.lock().unwrap().values().filter(|s| s.feed == "repo").map(|s| s.dir.clone()).collect();
        for dir in dirs {
            let _ = core.tx.send(Msg::RefreshFeed("repo".into(), dir));
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
            work: Mutex::default(),
            waiting: Mutex::default(),
        });
        if let Some(dir) = records_dir() {
            core.watch(dir, false);
        }
        tokio::spawn(events(core.clone(), rx));
        tokio::spawn(repo_clock(core.clone()));
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
