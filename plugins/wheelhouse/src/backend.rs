//! `wheelhouse backend`: the plugin backend `wheeld` runs for the wheelhouse package. It speaks
//! Wheel's JSON-RPC 2.0 over standard input and output (Content-Length framing, `async-lsp`;
//! `docs/plugin-runtime.md`, "Plugin backends") as the server side.
//!
//! One process serves every project: a feed's params name the project (`{dir}`). The `beads`, `beads_graph` (the Focus or Graph scene of the beads, params `{dir, view, sel, depth, width, more, list}`, read again exactly when `beads` is), `project`,
//! `work`, `repo`, `diff`, `prs`, `pr_detail` and `issues` feeds send the whole value on every subscribe and a JSON Patch on each
//! later change, found by watching the files that can change it: `beads` the project's `.beads`, `project`
//! the project directory, its `.beads` and its `docs` directory (a `docs` directory created later is an
//! event in the project directory, and the next read watches it), `work` no file but the agents
//! `wheel agent watch` lists (followed while a `work` subscription exists, and every change of the list
//! reads each subscribed project again), `issues` no file at all (it is read on subscribe, on the clock and after each
//! issue action or list script), `repo`,
//! `prs` and `pr_detail` the git refs, and `diff` the checkout itself (every file under its top except `.git` and what git
//! ignores) plus HEAD, the index, the refs and `info/exclude`. One path is watched once, recursively
//! when any subscription needs it so (a `beads` subscription's non-recursive watch of a project
//! directory is upgraded by a `diff` subscription's recursive watch of the same checkout, and
//! downgraded again when that one leaves). `repo`, `prs` and `pr_detail` are also read again every
//! 120 s, as is `issues`, for what only `gh` knows; `diff` has no clock. A read is keyed on the feed
//! and its whole params, so `pr_detail {dir, number: 3}` and `{dir, number: 5}` are two values, while an
//! action or clock names a feed and a project and reads every subscription of that feed on it. Actions
//! are `press`, `run_list`, `issue_create`, `issue_close`, `branch_delete`, `pr_merge`, `pr_close`, `pr_draft`, `pr_checkout`,
//! `pr_worktree`, `pr_button`, `pr_refresh`, `vision_save` and `agent_start`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_lsp::router::Router;
use async_lsp::{ClientSocket, ErrorCode, MainLoop, ResponseError};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use wheel_plugin_sdk::wire::*;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::beads::feed as beads_feed;
use crate::beads::graph_feed as beads_graph_feed;
use crate::core::agents::Agents;
use crate::core::project::Origin;
use crate::diff::feed as diff_feed;
use crate::issues::feed as issues_feed;
use crate::prs::feed as prs_feed;
use crate::repo::feed as repo_feed;
use crate::status::feed as status_feed;
use crate::work::feed as work_feed;

/// The lock that orders the reads of one (feed, params).
type ReadLock = Arc<tokio::sync::Mutex<()>>;

/// The feeds that hold what `bd` and the bead panes' records say: a press and a change of `bead-panes.json` refresh each.
const BEAD_FEEDS: [&str; 2] = ["beads", "beads_graph"];
const FEEDS: [&str; 9] = ["beads", "beads_graph", "project", "work", "repo", "diff", "prs", "pr_detail", "issues"];
const ACTIONS: [&str; 14] = ["press", "run_list", "issue_create", "issue_close", "branch_delete", "pr_merge", "pr_close", "pr_draft", "pr_checkout", "pr_worktree", "pr_button", "pr_refresh", "vision_save", "agent_start"];

/// How often a `repo`, `prs`, `pr_detail` or `issues` subscription is read again whatever the files do: GitHub has no event.
const REPO_EVERY: Duration = Duration::from_secs(120);

/// How long file events must stop before the changed projects are read again.
const QUIET: Duration = Duration::from_millis(150);
/// The longest a refresh waits after the first relevant file event, however long events keep coming.
const MAX_WAIT: Duration = Duration::from_secs(1);

enum Msg {
    /// Files changed.
    Fs(Vec<PathBuf>),
    /// Read every subscription of this feed on this project again.
    RefreshFeed(String, String),
}

struct Sub {
    feed: String,
    /// The params as subscribed, echoed in every update.
    params: Value,
    /// `params` as its canonical string: with `feed`, what identifies the value read.
    pkey: String,
    dir: String,
    /// `dir` as the file watcher reports paths under it.
    real: PathBuf,
    /// The directories this subscription needs watched, and whether each is watched recursively.
    watch: Vec<(PathBuf, bool)>,
    /// A git-reading subscription: the files whose change matters (found by their exact path)...
    files: Vec<PathBuf>,
    /// ...and the directories under which any change matters.
    trees: Vec<PathBuf>,
    /// A `diff` subscription: the checkout's top, where any change matters unless it is under `.git`
    /// or one of `ignored`.
    top: Option<PathBuf>,
    /// A `diff` subscription: the paths under `top` that git ignores.
    ignored: HashSet<PathBuf>,
    /// The value last sent; none until the first.
    last: Option<Value>,
}

/// What a subscription needs watched, and how an event is matched to it.
#[derive(Default)]
struct Watches {
    watch: Vec<(PathBuf, bool)>,
    files: Vec<PathBuf>,
    trees: Vec<PathBuf>,
    top: Option<PathBuf>,
    ignored: HashSet<PathBuf>,
}

/// Whether a file event at `p` matters to a `repo` or `diff` subscription: `p` is one of `files`, is
/// inside (or is an ancestor of) one of `trees`, or is under `top` and neither under `.git` nor at or
/// under a path in `ignored`. Each ancestor of `p` up to `top` is looked up, so the cost is the depth
/// of `p`, not the length of `ignored`.
fn git_hit(files: &[PathBuf], trees: &[PathBuf], top: Option<&PathBuf>, ignored: &HashSet<PathBuf>, p: &std::path::Path) -> bool {
    files.iter().any(|f| f == p)
        || trees.iter().any(|t| p.starts_with(t) || t.starts_with(p))
        || top.is_some_and(|top| {
            p.starts_with(top) && !p.starts_with(top.join(".git")) && !p.ancestors().take_while(|a| a.starts_with(top)).any(|a| ignored.contains(a))
        })
}

/// What a path's watch needs: the strongest mode any of `wants` asks for, none when none names `p`.
fn mode_needed<'a>(wants: impl Iterator<Item = &'a [(PathBuf, bool)]>, p: &std::path::Path) -> Option<bool> {
    wants.flat_map(|w| w.iter()).filter(|(w, _)| w == p).map(|(_, rec)| *rec).reduce(|a, b| a || b)
}

/// What to do with the watch on a path.
#[derive(Debug, PartialEq)]
enum Step {
    /// Leave it as it is.
    Keep,
    /// Install a watch in this mode (replacing the installed one, if any).
    Set(bool),
    /// Remove it.
    Remove,
}

/// A request to watch a path (recursively or not) given how it is watched now (`None` for not at all).
fn on_watch(installed: Option<bool>, recursive: bool) -> Step {
    match installed {
        None => Step::Set(recursive),
        Some(false) if recursive => Step::Set(true),
        Some(_) => Step::Keep,
    }
}

/// What a path's watch becomes when the subscriptions that remain need it in mode `needed`.
fn on_unwatch(installed: bool, needed: Option<bool>) -> Step {
    match needed {
        None => Step::Remove,
        Some(n) if n != installed => Step::Set(n),
        Some(_) => Step::Keep,
    }
}

/// Whether `feed` is found by watching git's own files, so its paths come from its first refresh.
fn reads_git(feed: &str) -> bool {
    matches!(feed, "repo" | "diff" | "prs" | "pr_detail")
}

/// The feeds read again on the `REPO_EVERY` clock.
fn on_clock(feed: &str) -> bool {
    matches!(feed, "repo" | "prs" | "pr_detail" | "issues")
}

/// The params keys of the subscriptions of `feed` on project `dir`.
fn pkeys_of<'a>(subs: impl Iterator<Item = &'a Sub>, feed: &str, dir: &str) -> BTreeSet<String> {
    subs.filter(|s| s.feed == feed && s.dir == dir).map(|s| s.pkey.clone()).collect()
}

/// `p` as the file watcher reports it: the nearest existing ancestor made real, then the rest (a file
/// or tree that does not exist yet).
fn real_of(p: &std::path::Path) -> PathBuf {
    let mut tail = vec![];
    let mut at = p.to_path_buf();
    loop {
        if let Ok(real) = at.canonicalize() {
            return tail.iter().rev().fold(real, |acc, n| acc.join(n));
        }
        match (at.parent().map(|d| d.to_path_buf()), at.file_name().map(|n| n.to_os_string())) {
            (Some(d), Some(n)) => {
                tail.push(n);
                at = d;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// What a subscription to `feed` for `dir` needs watched.
fn watches(feed: &str, dir: &str, real: &std::path::Path) -> Watches {
    match feed {
        "work" | "issues" => return Watches::default(),
        "project" => return Watches { watch: vec![(real.to_path_buf(), false), (real.join(".beads"), false), (real.join("docs"), false)], ..Default::default() },
        _ if !reads_git(feed) => return Watches { watch: vec![(real.to_path_buf(), false), (real.join(".beads"), false)], ..Default::default() },
        _ => {}
    }
    let Some((own, common)) = crate::core::git::dirs(dir) else { return Default::default() };
    let (own, common) = (PathBuf::from(own), PathBuf::from(common));
    let (files, trees) = if feed != "diff" {
        (
            vec![own.join("logs/HEAD"), own.join("HEAD"), common.join("packed-refs"), common.join("refs/stash")],
            vec![common.join("refs/heads"), common.join("refs/remotes/origin"), common.join("worktrees")],
        )
    } else {
        (
            vec![own.join("HEAD"), own.join("index"), own.join("logs/HEAD"), common.join("packed-refs"), common.join("info/exclude")],
            vec![common.join("refs")],
        )
    };
    let files: Vec<PathBuf> = files.iter().map(|p| real_of(p)).collect();
    let trees: Vec<PathBuf> = trees.iter().map(|p| real_of(p)).collect();
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
    let (mut top, mut ignored) = (None, HashSet::new());
    if feed == "diff" {
        if let Ok(t) = crate::core::git::toplevel(dir) {
            let t = real_of(std::path::Path::new(&t));
            watch.push((t.clone(), true));
            ignored = crate::core::git::run(&t.to_string_lossy(), &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory"])
                .map(|out| out.lines().map(|l| real_of(&t.join(l.trim_end_matches('/')))).collect())
                .unwrap_or_default();
            top = Some(t);
        }
    }
    Watches { watch, files, trees, top, ignored }
}

struct Core {
    client: ClientSocket,
    subs: Mutex<HashMap<String, Sub>>,
    /// The paths watched, each with whether recursively.
    watched: Mutex<HashMap<PathBuf, bool>>,
    watcher: Mutex<notify::RecommendedWatcher>,
    tx: UnboundedSender<Msg>,
    /// One lock per (feed, params key), held while it is read and its value sent, so two reads of the same
    /// value never swap order and a slow read (`gh`) delays nothing else.
    work: Mutex<HashMap<(String, String), ReadLock>>,
    /// The (feed, params key) pairs a refresh is queued for and has not started reading: another event for
    /// one of them needs no second refresh.
    waiting: Mutex<HashSet<(String, String)>>,
    /// The agents `wheel agent watch` lists, followed while a `work` subscription exists.
    agents: Agents,
}

fn records_dir() -> Option<PathBuf> {
    crate::beads::buttons::records_file().parent().map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
}

impl Core {
    fn log(&self, level: &str, message: &str) {
        eprintln!("wheelhouse backend: {message}");
        let _ = self.client.notify::<LogMessage>(json!({ "level": level, "message": message }));
    }

    /// Install `mode` on `path`, replacing the watch it has; false when the watcher refused.
    fn install(&self, path: &std::path::Path, recursive: bool, replace: bool) -> bool {
        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        let mut w = self.watcher.lock().unwrap();
        if replace {
            let _ = w.unwatch(path);
        }
        match w.watch(path, mode) {
            Ok(()) => true,
            Err(e) => {
                drop(w);
                self.log("warn", &format!("cannot watch {}: {e}", path.display()));
                false
            }
        }
    }

    /// Watch `path`; a recursive request on a path watched without recursion upgrades it.
    fn watch(&self, path: PathBuf, recursive: bool) {
        if !path.is_dir() {
            return;
        }
        let mut watched = self.watched.lock().unwrap();
        let installed = watched.get(&path).copied();
        if let Step::Set(rec) = on_watch(installed, recursive) {
            if self.install(&path, rec, installed.is_some()) {
                watched.insert(path, rec);
            } else if installed.is_some() {
                watched.remove(&path);
            }
        }
    }

    /// Change the watch of every path in `gone` to what the subscriptions in `subs` (the locked map)
    /// still need of it: nothing removes it, a weaker mode than is installed downgrades it.
    fn unwatch_unused(&self, subs: &HashMap<String, Sub>, gone: &[(PathBuf, bool)]) {
        let mut watched = self.watched.lock().unwrap();
        for (p, _) in gone {
            let Some(&installed) = watched.get(p) else { continue };
            match on_unwatch(installed, mode_needed(subs.values().map(|s| s.watch.as_slice()), p)) {
                Step::Keep => {}
                Step::Remove => {
                    let _ = self.watcher.lock().unwrap().unwatch(p);
                    watched.remove(p);
                }
                Step::Set(rec) => {
                    if self.install(p, rec, true) {
                        watched.insert(p.clone(), rec);
                    } else {
                        watched.remove(p);
                    }
                }
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
        let pkey = r.params.to_string();
        {
            // Watching happens while the subscription is added, so an unsubscribe never runs between them.
            // A `repo` or `diff` subscription's paths come from git, so its first refresh finds them.
            let mut subs = self.subs.lock().unwrap();
            let w = if reads_git(&feed) { Watches::default() } else { watches(&feed, &dir, &real) };
            for (p, rec) in &w.watch {
                self.watch(p.clone(), *rec);
            }
            let k = key(&r);
            let sub = Sub { feed: feed.clone(), pkey: pkey.clone(), params: r.params, dir: dir.clone(), real, watch: w.watch, files: w.files, trees: w.trees, top: w.top, ignored: w.ignored, last: None };
            let old = subs.insert(k, sub);
            // One acquire per `work` subscription: a re-subscribe of the same key replaces the old one.
            if feed == "work" && old.is_none() {
                self.agents.acquire();
            }
            if let Some(old) = old {
                self.unwatch_unused(&subs, &old.watch);
            }
        }
        let core = self.clone();
        tokio::spawn(async move { core.refresh(&feed, &pkey).await });
    }

    fn unsubscribe(&self, r: FeedRef) {
        let mut subs = self.subs.lock().unwrap();
        if let Some(sub) = subs.remove(&key(&r)) {
            if sub.feed == "work" {
                self.agents.release();
            }
            self.unwatch_unused(&subs, &sub.watch);
        }
    }

    /// Read `feed` with the params whose canonical string is `pkey` and send every subscriber of it what
    /// changed: the whole value to one that has none yet, a patch to the others, nothing when the value
    /// is the same.
    async fn refresh(&self, feed: &str, pkey: &str) {
        let lock = self.work.lock().unwrap().entry((feed.to_string(), pkey.to_string())).or_default().clone();
        let _one_at_a_time = lock.lock().await;
        self.waiting.lock().unwrap().remove(&(feed.to_string(), pkey.to_string()));
        let (real, dir, params) = {
            let subs = self.subs.lock().unwrap();
            match subs.values().find(|s| s.feed == feed && s.pkey == pkey) {
                Some(s) => (s.real.clone(), s.dir.clone(), s.params.clone()),
                None => return,
            }
        };
        let dir = dir.as_str();
        // Find what to watch and watch it before reading, so a change made while the read runs is an event.
        let (f, d) = (feed.to_string(), dir.to_string());
        let paths = tokio::task::spawn_blocking(move || watches(&f, &d, &real)).await;
        let found = match paths {
            Ok(v) => v,
            Err(e) => return self.log("error", &format!("reading {dir}: {e}")),
        };
        {
            // Under the subs lock, so a project nobody reads any more is not watched again. The paths
            // that match an event are set here too, before the read, so an event that arrives during it
            // is a hit; a git-reading feed whose paths came back empty keeps the ones it has.
            let mut subs = self.subs.lock().unwrap();
            if subs.values().any(|s| s.feed == feed && s.pkey == pkey) {
                for (p, rec) in &found.watch {
                    self.watch(p.clone(), *rec);
                }
                let mut gone = vec![];
                for sub in subs.values_mut().filter(|s| s.feed == feed && s.pkey == pkey) {
                    if !found.watch.is_empty() || !reads_git(feed) {
                        gone.extend(std::mem::replace(&mut sub.watch, found.watch.clone()));
                        sub.files = found.files.clone();
                        sub.trees = found.trees.clone();
                        sub.top = found.top.clone();
                        sub.ignored = found.ignored.clone();
                    }
                }
                self.unwatch_unused(&subs, &gone);
            }
        }
        let (f, d, agents) = (feed.to_string(), dir.to_string(), self.agents.clone());
        let read = tokio::task::spawn_blocking(move || match f.as_str() {
            "project" => status_feed::value(&d),
            "work" => work_feed::value(&d, &agents.all(), agents.loaded(), agents.error()),
            "repo" => repo_feed::value(&d),
            "diff" => diff_feed::value(&d),
            "prs" => prs_feed::value(&d),
            "issues" => issues_feed::value(&d),
            "pr_detail" => prs_feed::detail_value(&d, crate::core::number_of(&params["number"]).unwrap_or(0)),
            "beads_graph" => beads_graph_feed::value(&d, &params),
            _ => beads_feed::value(&d),
        })
        .await;
        let value = match read {
            Ok(v) => v,
            Err(e) => return self.log("error", &format!("reading {dir}: {e}")),
        };
        let mut subs = self.subs.lock().unwrap();
        for sub in subs.values_mut().filter(|s| s.feed == feed && s.pkey == pkey) {
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
    }

    /// The (feed, params key) pairs a batch of file events changes.
    fn affected(&self, paths: &[PathBuf], out: &mut BTreeSet<(String, String)>) {
        let records = records_dir();
        let subs = self.subs.lock().unwrap();
        for p in paths {
            let (Some(name), Some(parent)) = (p.file_name().and_then(|n| n.to_str()), p.parent()) else { continue };
            for s in subs.values() {
                let hit = if s.feed == "work" || s.feed == "issues" {
                    false
                } else if reads_git(&s.feed) {
                    git_hit(&s.files, &s.trees, s.top.as_ref(), &s.ignored, p)
                } else if BEAD_FEEDS.contains(&s.feed.as_str()) && name == "bead-panes.json" && records.as_deref() == Some(parent) {
                    true
                } else {
                    match name {
                        "wheel.toml" | ".beads" => parent == s.real,
                        "last-touched" => parent == s.real.join(".beads"),
                        "docs" => s.feed == "project" && parent == s.real,
                        "CONTEXT.md" => s.feed == "project" && parent == s.real.join("docs"),
                        _ => false,
                    }
                };
                if hit {
                    out.insert((s.feed.clone(), s.pkey.clone()));
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
                    for feed in BEAD_FEEDS {
                        let _ = tx.send(Msg::RefreshFeed(feed.into(), changed_dir.clone()));
                    }
                };
                tokio::task::spawn_blocking(move || beads_feed::press(&dir, &bead, &pane, changed)).await
            }
            "run_list" => {
                let script = arg("script").ok_or_else(|| bad("run_list needs args.script".into()))?;
                let view = arg("view").unwrap_or_else(|| "beads".into());
                if !["beads", "repo", "prs", "issues"].contains(&view.as_str()) {
                    return Err(bad(format!("run_list: no view {view}; its views: beads, repo, prs, issues")));
                }
                let ids: Vec<String> = match p.args.get("ids") {
                    Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    _ => return Err(bad("run_list needs args.ids, a list of ids".into())),
                };
                let changed_dir = dir.clone();
                let issues = view == "issues";
                let done = tokio::task::spawn_blocking(move || {
                    if view == "repo" {
                        repo_feed::run_list(&dir, &script, &ids, &pane)
                    } else if view == "prs" {
                        prs_feed::run_list(&dir, &script, &ids, &pane)
                    } else if view == "issues" {
                        issues_feed::run_list(&dir, &script, &ids, &pane)
                    } else {
                        beads_feed::run_list(&dir, &script, &ids, &pane)
                    }
                })
                .await;
                // A list script may change GitHub, which has no event: read the issues again whether or not it went through.
                if issues {
                    let _ = self.tx.send(Msg::RefreshFeed("issues".into(), changed_dir));
                }
                done
            }
            "issue_create" => {
                let title = arg("title").unwrap_or_default();
                let changed_dir = dir.clone();
                let done = tokio::task::spawn_blocking(move || {
                    crate::issues::create(&dir, &title).map(|n| format!("Opened #{n}"))
                })
                .await;
                // Read the issues again whether or not it went through.
                let _ = self.tx.send(Msg::RefreshFeed("issues".into(), changed_dir));
                done
            }
            "issue_close" => {
                let number = p.args.get("number").and_then(crate::core::number_of).ok_or_else(|| bad("issue_close needs args.number, an issue number".into()))?;
                let changed_dir = dir.clone();
                let done = tokio::task::spawn_blocking(move || {
                    crate::issues::close(&dir, number).map(|_| format!("Closed #{number}"))
                })
                .await;
                let _ = self.tx.send(Msg::RefreshFeed("issues".into(), changed_dir));
                done
            }
            "branch_delete" => {
                let branch = arg("branch").ok_or_else(|| bad("branch_delete needs args.branch".into()))?;
                let changed_dir = dir.clone();
                let done = tokio::task::spawn_blocking(move || repo_feed::delete(&dir, &branch)).await;
                // Read the project again whether or not the delete went through.
                let _ = self.tx.send(Msg::RefreshFeed("repo".into(), changed_dir));
                done
            }
            "pr_merge" | "pr_close" | "pr_draft" | "pr_checkout" | "pr_worktree" | "pr_button" | "pr_refresh" => {
                let number = p.args.get("number").and_then(crate::core::number_of);
                if number.is_none() && p.action != "pr_refresh" {
                    return Err(bad(format!("{} needs args.number, a PR number", p.action)));
                }
                let (action, args, changed_dir) = (p.action.clone(), p.args.clone(), dir.clone());
                let done = tokio::task::spawn_blocking(move || {
                    let said = prs_feed::act(&action, &dir, &pane, &args);
                    // Whether or not it went through, the next read asks GitHub again.
                    crate::prs::forget(&dir);
                    said
                })
                .await;
                for feed in ["prs", "pr_detail"] {
                    let _ = self.tx.send(Msg::RefreshFeed(feed.into(), changed_dir.clone()));
                }
                done
            }
            "vision_save" => {
                let text = arg("text").ok_or_else(|| bad("vision_save needs args.text".into()))?;
                let changed_dir = dir.clone();
                let done = tokio::task::spawn_blocking(move || status_feed::save_vision(&dir, &text)).await;
                // Read the project again whether or not the write went through.
                let _ = self.tx.send(Msg::RefreshFeed("project".into(), changed_dir));
                done
            }
            "agent_start" => {
                let brief = arg("brief").filter(|b| !b.is_empty()).ok_or_else(|| bad("agent_start needs args.brief".into()))?;
                let agents = self.agents.clone();
                tokio::task::spawn_blocking(move || {
                    let origin = Origin { dir, pane: if pane.is_empty() { "default".into() } else { pane } };
                    agents.start(&origin, &brief, None, None).map(|pane| format!("Started the agent in pane {pane}"))
                })
                .await
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
            for k in pkeys_of(core.subs.lock().unwrap().values(), &f, &d) {
                out.insert((f.clone(), k));
            }
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
        for (feed, pkey) in std::mem::take(&mut pending) {
            if !core.waiting.lock().unwrap().insert((feed.clone(), pkey.clone())) {
                continue;
            }
            let core = core.clone();
            tokio::spawn(async move { core.refresh(&feed, &pkey).await });
        }
    }
}

/// Ask for every `work` subscription's project to be read again whenever the agent list changes.
async fn work_changes(core: Arc<Core>) {
    let mut changes = core.agents.changes();
    while changes.changed().await.is_ok() {
        let dirs: BTreeSet<String> = core.subs.lock().unwrap().values().filter(|s| s.feed == "work").map(|s| s.dir.clone()).collect();
        for dir in dirs {
            let _ = core.tx.send(Msg::RefreshFeed("work".into(), dir));
        }
    }
}

/// Ask for every `repo`, `prs`, `pr_detail` and `issues` subscription to be read again each `REPO_EVERY`.
async fn repo_clock(core: Arc<Core>) {
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + REPO_EVERY, REPO_EVERY);
    loop {
        tick.tick().await;
        let units: BTreeSet<(String, String)> = core.subs.lock().unwrap().values().filter(|s| on_clock(&s.feed)).map(|s| (s.feed.clone(), s.dir.clone())).collect();
        for (feed, dir) in units {
            let _ = core.tx.send(Msg::RefreshFeed(feed, dir));
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
    let agents = Agents::new("wheel");
    let core_agents = agents.clone();
    let (main_loop, _) = MainLoop::new_server(|client| {
        let core = Arc::new(Core {
            client,
            subs: Mutex::default(),
            watched: Mutex::default(),
            watcher: Mutex::new(watcher),
            tx,
            work: Mutex::default(),
            waiting: Mutex::default(),
            agents: core_agents,
        });
        if let Some(dir) = records_dir() {
            core.watch(dir, false);
        }
        tokio::spawn(events(core.clone(), rx));
        tokio::spawn(repo_clock(core.clone()));
        tokio::spawn(work_changes(core.clone()));
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
    agents.shutdown();
    if let Err(e) = &result {
        eprintln!("wheelhouse backend: {e}");
    }
    std::process::exit(if result.is_ok() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn hit_rule_matches_files_trees_and_the_checkout() {
        let top = p("/r");
        let ignored: HashSet<PathBuf> = ["/r/target", "/r/a/b.log"].iter().map(|s| p(s)).collect();
        let files = vec![p("/r/.git/HEAD"), p("/r/.git/info/exclude")];
        let trees = vec![p("/r/.git/refs")];
        let hit = |x: &str| git_hit(&files, &trees, Some(&top), &ignored, &p(x));
        assert!(hit("/r/src/a.rs"));
        assert!(hit("/r/.git/HEAD"));
        assert!(hit("/r/.git/info/exclude"));
        assert!(hit("/r/.git/refs/heads/main"));
        assert!(hit("/r/.git"), "an ancestor of a tree");
        assert!(!hit("/r/.git/objects/ab/cd"));
        assert!(!hit("/r/target"));
        assert!(!hit("/r/target/debug/x"));
        assert!(!hit("/r/a/b.log"));
        assert!(hit("/r/a/b.txt"));
        assert!(!hit("/elsewhere/x"));
        assert!(!git_hit(&files, &trees, None, &ignored, &p("/r/src/a.rs")));
    }

    #[test]
    fn a_request_upgrades_a_weaker_watch_and_never_downgrades() {
        assert_eq!(on_watch(None, false), Step::Set(false));
        assert_eq!(on_watch(None, true), Step::Set(true));
        assert_eq!(on_watch(Some(false), true), Step::Set(true));
        assert_eq!(on_watch(Some(true), false), Step::Keep);
        assert_eq!(on_watch(Some(true), true), Step::Keep);
        assert_eq!(on_watch(Some(false), false), Step::Keep);
    }

    #[test]
    fn leaving_subscriptions_remove_or_downgrade_the_watch() {
        let top = p("/r");
        let beads = vec![(top.clone(), false)];
        let diff = vec![(top.clone(), true)];
        let both = || [beads.as_slice(), diff.as_slice()].into_iter();
        assert_eq!(mode_needed(both(), &top), Some(true));
        assert_eq!(mode_needed([beads.as_slice()].into_iter(), &top), Some(false));
        assert_eq!(mode_needed([].into_iter(), &top), None);
        assert_eq!(mode_needed(both(), &p("/other")), None);
        assert_eq!(on_unwatch(true, mode_needed([beads.as_slice()].into_iter(), &top)), Step::Set(false));
        assert_eq!(on_unwatch(true, mode_needed(both(), &top)), Step::Keep);
        assert_eq!(on_unwatch(false, mode_needed([beads.as_slice()].into_iter(), &top)), Step::Keep);
        assert_eq!(on_unwatch(true, None), Step::Remove);
    }

    fn sub(feed: &str, params: Value) -> (String, Sub) {
        let dir = params["dir"].as_str().unwrap().to_string();
        let r = FeedRef { feed: feed.into(), params: params.clone() };
        let s = Sub { feed: feed.into(), pkey: params.to_string(), params, dir: dir.clone(), real: p(&dir), watch: vec![], files: vec![], trees: vec![], top: None, ignored: HashSet::new(), last: None };
        (key(&r), s)
    }

    #[test]
    fn two_pr_numbers_in_one_directory_are_two_values_and_a_feed_refresh_names_both() {
        let subs: HashMap<String, Sub> = [
            sub("pr_detail", json!({ "dir": "/r", "number": 3 })),
            sub("pr_detail", json!({ "number": 5, "dir": "/r" })),
            sub("pr_detail", json!({ "dir": "/other", "number": 3 })),
            sub("prs", json!({ "dir": "/r" })),
        ]
        .into_iter()
        .collect();
        assert_eq!(subs.len(), 4, "each (feed, params) is its own subscription");
        let keys = pkeys_of(subs.values(), "pr_detail", "/r");
        assert_eq!(keys, BTreeSet::from([json!({ "dir": "/r", "number": 3 }).to_string(), json!({ "dir": "/r", "number": 5 }).to_string()]));
        assert_eq!(pkeys_of(subs.values(), "prs", "/r").len(), 1);
        assert!(pkeys_of(subs.values(), "pr_detail", "/nowhere").is_empty());
        assert!(pkeys_of(subs.values(), "repo", "/r").is_empty());
    }

    #[test]
    fn beads_graph_is_a_feed_that_watches_what_beads_watches_and_is_one_of_the_feeds_a_press_refreshes() {
        assert!(FEEDS.contains(&"beads_graph"));
        assert_eq!(BEAD_FEEDS, ["beads", "beads_graph"]);
        assert!(BEAD_FEEDS.iter().all(|f| FEEDS.contains(f)));
        let top = p("/r");
        let (a, b) = (watches("beads", "/r", &top), watches("beads_graph", "/r", &top));
        assert_eq!(a.watch, b.watch);
        assert!(!b.watch.is_empty() && !reads_git("beads_graph") && !on_clock("beads_graph"));
        let params = json!({ "dir": "/r", "view": "focus", "sel": "x" });
        let subs: HashMap<String, Sub> = [sub("beads", json!({ "dir": "/r" })), sub("beads_graph", params.clone())].into_iter().collect();
        assert_eq!(pkeys_of(subs.values(), "beads_graph", "/r"), BTreeSet::from([params.to_string()]));
    }

    #[test]
    fn project_watches_its_directory_beads_and_docs_and_work_watches_no_file() {
        let top = p("/r");
        let w = watches("project", "/r", &top);
        assert_eq!(w.watch, vec![(p("/r"), false), (p("/r/.beads"), false), (p("/r/docs"), false)]);
        let w = watches("work", "/r", &top);
        assert!(w.watch.is_empty() && w.files.is_empty() && w.trees.is_empty() && w.top.is_none());
        assert!(!reads_git("work") && !reads_git("project") && !on_clock("work") && !on_clock("project"));
        let w = watches("issues", "/r", &top);
        assert!(w.watch.is_empty() && w.files.is_empty() && w.trees.is_empty() && w.top.is_none());
        assert!(FEEDS.contains(&"issues") && on_clock("issues") && !reads_git("issues"));
        assert!(ACTIONS.contains(&"issue_create") && ACTIONS.contains(&"issue_close"));
        assert!(FEEDS.contains(&"project") && FEEDS.contains(&"work") && ACTIONS.contains(&"vision_save") && ACTIONS.contains(&"agent_start"));
    }
}
