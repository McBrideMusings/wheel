//! `wheel-runner commands`: Wheel's command backend. `wheeld` runs it, in the package folder, for a
//! plugin package whose `wheel-plugin.toml` has a `[commands]` table, so a tool with a command line
//! gets a Luau pane with no backend of its own (`docs/plugin-runtime.md`, "Command backend").
//!
//! - A feed runs its command once per subscribed (feed, params) on subscribe, every `every` seconds,
//!   after any action of the package and when a `watch` path changes. Its value is `{value,
//!   error}`: `value` the command's standard output as JSON or lines (`output`), `error` null after
//!   a good run, else why the run failed, with `value` kept from the last good run. The whole value
//!   goes on the first run, a JSON Patch after, nothing when it did not change.
//! - An action runs its command and answers `{code, stdout, stderr}`, whatever the exit code; a
//!   run that was killed at its timeout or could not start is the action's error.
//!
//! Commands run with `/bin/sh -c` through [`crate::check::bounded_with`], in `params.dir` /
//! `args.dir` when that is an absolute directory, else the package folder, with the params or args
//! in the environment (`PARAM_<KEY>` and `WHEEL_PARAMS`, `ARG_<KEY>` and `WHEEL_ARGS`), never in
//! the command text.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};
use wheel_plugin_sdk::commands::{self as manifest, Action, Feed, Output};
use wheel_plugin_sdk::{ActionError, Backend, Call, Subscription};

use crate::check::{bounded_with, Bounded, Wake, OUTPUT_CAP};

/// How long watched-path events must stop before the feed runs again.
const QUIET: Duration = Duration::from_millis(150);
/// The longest one wait for events to stop may take, so a folder written all the time still lets
/// the feed run.
const QUIET_MAX: Duration = Duration::from_secs(1);

/// Drop watched-path events until they stop for [`QUIET`], or [`QUIET_MAX`] has passed.
async fn settle(changed: &mut mpsc::UnboundedReceiver<()>) {
    let deadline = tokio::time::Instant::now() + QUIET_MAX;
    loop {
        let until = (tokio::time::Instant::now() + QUIET).min(deadline);
        match tokio::time::timeout_at(until, changed.recv()).await {
            Ok(Some(())) if tokio::time::Instant::now() < deadline => continue,
            _ => break,
        }
    }
}

/// How many of the last lines of a failed command's output its error carries.
const ERROR_LINES: usize = 20;

/// Serve the package in the current directory until `wheeld` sends `shutdown`. Returns the exit status.
pub async fn run() -> anyhow::Result<i32> {
    let folder = std::env::current_dir()?;
    let path = folder.join("wheel-plugin.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let table = manifest::from_manifest(&text)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?
        .ok_or_else(|| anyhow::anyhow!("{} has no [commands] table", path.display()))?;
    let folder = Arc::new(folder);
    // Every action sends one; every feed subscription runs again on it.
    let (ran, _) = broadcast::channel::<()>(16);
    let mut backend = Backend::new();
    for (name, feed) in table.feeds {
        let (folder, ran, feed) = (folder.clone(), ran.clone(), Arc::new(feed));
        let label = name.clone();
        backend.feed(&name, move |sub| serve_feed(sub, label.clone(), feed.clone(), folder.clone(), ran.subscribe()));
    }
    for (name, action) in table.actions {
        let (folder, ran, action) = (folder.clone(), ran.clone(), Arc::new(action));
        backend.action(&name, move |call| invoke(call, action.clone(), folder.clone(), ran.clone()));
    }
    // The guard ends every command still running once this process is gone, however it went.
    crate::check::guard().map_err(|e| anyhow::anyhow!(e))?;
    Ok(backend.run().await)
}

/// `params.dir` (or `args.dir`) when it is an absolute directory, else the package folder.
fn cwd_of(input: &Value, folder: &Path) -> PathBuf {
    input
        .get("dir")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|d| d.is_absolute() && d.is_dir())
        .unwrap_or_else(|| folder.to_path_buf())
}

/// `<prefix>_<KEY>` for each top-level key of `input` (upper case, anything but a letter or digit
/// as `_`; a string as it is, null as empty, anything else as JSON), and `<whole>` holding `input`
/// as JSON. Keys that come to the same name: the later one in key order wins.
fn env_of(prefix: &str, whole: &str, input: &Value) -> Vec<(String, String)> {
    let mut env = vec![(whole.to_string(), if input.is_null() { "{}".into() } else { input.to_string() })];
    if let Some(map) = input.as_object() {
        for (key, value) in map {
            let name: String = key.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            env.push((format!("{prefix}_{name}"), text));
        }
    }
    env
}

/// The last [`ERROR_LINES`] lines of `text`, trimmed.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(ERROR_LINES)..].join("\n").trim().to_string()
}

/// Why a run failed, or None when it exited 0.
fn failure(b: &Bounded, timeout: f64) -> Option<String> {
    if b.timed_out {
        return Some(format!("timed out after {timeout} s"));
    }
    if b.cancelled {
        return Some("cancelled: nothing reads it any more".into());
    }
    match b.code {
        Some(0) => None,
        code => {
            let said = if b.stderr.trim().is_empty() { tail(&b.stdout) } else { tail(&b.stderr) };
            let how = code.map(|c| format!("exit {c}")).unwrap_or_else(|| "ended by a signal".into());
            Some(if said.is_empty() { how } else { format!("{how}: {said}") })
        }
    }
}

/// A feed command's standard output as the feed's value.
fn value_of(output: Output, stdout: &str) -> Result<Value, String> {
    let lines = || Value::Array(stdout.lines().map(|l| Value::String(l.to_string())).collect());
    match output {
        Output::Text => Ok(Value::String(stdout.to_string())),
        Output::Lines => Ok(lines()),
        Output::Json => serde_json::from_str(stdout).map_err(|e| format!("the output is not JSON: {e}")),
        Output::Auto => Ok(serde_json::from_str(stdout).unwrap_or_else(|_| lines())),
    }
}

/// Sends [`Wake::Cancel`] when dropped, so a run whose task is dropped (the last pane left) is
/// killed instead of running to its timeout.
struct CancelOnDrop(std::sync::mpsc::Sender<Wake>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(Wake::Cancel);
    }
}

/// Run `run` off the runtime's threads; dropping the future kills the command.
async fn run_bounded(run: String, cwd: PathBuf, env: Vec<(String, String)>, timeout: f64) -> Result<Bounded, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _cancel = CancelOnDrop(tx.clone());
    tokio::task::spawn_blocking(move || bounded_with(&run, &cwd, &env, Duration::from_secs_f64(timeout), (tx, rx)))
        .await
        .map_err(|e| format!("the run's thread failed: {e}"))?
}

/// A `watch` entry as a path: `~/…` under the home directory, a relative one under `cwd`.
fn watch_path(entry: &str, cwd: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (entry.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        (None, Some(home)) if entry == "~" => home,
        _ => cwd.join(entry),
    }
}

/// Watch each path: a directory recursively, a file (made or not yet) through its folder, so a save
/// that replaces the file is seen. Sends one message per event that touches a watched path. The
/// watcher lives as long as the returned value.
fn watch(sub: &Subscription, name: &str, paths: &[PathBuf]) -> (Option<notify::RecommendedWatcher>, mpsc::UnboundedReceiver<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    if paths.is_empty() {
        return (None, rx);
    }
    // Events name real paths, so each target is compared through its folder's real path.
    let targets: Vec<(PathBuf, bool)> = paths
        .iter()
        .map(|p| {
            let dir = p.is_dir();
            let real = if dir {
                p.canonicalize().unwrap_or_else(|_| p.clone())
            } else {
                match (p.parent().and_then(|d| d.canonicalize().ok()), p.file_name()) {
                    (Some(parent), Some(file)) => parent.join(file),
                    _ => p.clone(),
                }
            };
            (real, dir)
        })
        .collect();
    let wanted = targets.clone();
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(e) = event {
            if e.paths.iter().any(|p| wanted.iter().any(|(t, dir)| if *dir { p.starts_with(t) } else { p == t })) {
                let _ = tx.send(());
            }
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            sub.peer.log("warn", format!("feed {name}: no file watcher: {e}"));
            return (None, rx);
        }
    };
    for (target, dir) in &targets {
        let (at, mode) = if *dir { (target.clone(), RecursiveMode::Recursive) } else { (target.parent().map(Path::to_path_buf).unwrap_or_default(), RecursiveMode::NonRecursive) };
        if let Err(e) = watcher.watch(&at, mode) {
            sub.peer.log("warn", format!("feed {name}: cannot watch {}: {e}", at.display()));
        }
    }
    (Some(watcher), rx)
}

/// One subscribed (feed, params): run, publish what changed, wait for the next reason to run.
/// The SDK drops this task when the last pane leaves, which drops its watcher and interval too.
async fn serve_feed(sub: Subscription, name: String, feed: Arc<Feed>, folder: Arc<PathBuf>, mut ran: broadcast::Receiver<()>) {
    let cwd = cwd_of(&sub.params, &folder);
    let env = env_of("PARAM", "WHEEL_PARAMS", &sub.params);
    let timeout = feed.timeout.unwrap_or(manifest::FEED_TIMEOUT);
    let paths: Vec<PathBuf> = feed.watch.iter().map(|w| watch_path(w, &cwd)).collect();
    let (_watcher, mut changed) = watch(&sub, &name, &paths);
    let mut every = feed.every.map(|s| {
        let mut i = tokio::time::interval(Duration::from_secs_f64(s));
        // A run that outlasts the interval is followed by one run, not a burst.
        i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        i
    });
    if let Some(i) = every.as_mut() {
        i.tick().await;
    }
    let mut sent: Option<Value> = None;
    let mut good = Value::Null;
    let mut said: Option<String> = None;
    loop {
        let (value, error) = match run_bounded(feed.run.clone(), cwd.clone(), env.clone(), timeout).await {
            Err(e) => (None, Some(e)),
            Ok(b) => match failure(&b, timeout) {
                Some(e) => (None, Some(e)),
                None if b.cut => (None, Some(format!("the output passed {} MB and was cut", OUTPUT_CAP >> 20))),
                None => match value_of(feed.output, &b.stdout) {
                    Ok(v) => (Some(v), None),
                    Err(e) => (None, Some(e)),
                },
            },
        };
        if let Some(v) = value {
            good = v;
        }
        if error != said {
            match &error {
                Some(e) => sub.peer.log("warn", format!("feed {name} {}: {e}", sub.params)),
                None if said.is_some() => sub.peer.log("info", format!("feed {name} {}: ran clean again", sub.params)),
                None => {}
            }
            said = error.clone();
        }
        let now = json!({ "value": good, "error": error });
        match &sent {
            None => sub.value(now.clone()),
            Some(last) => {
                if let Some(p) = crate::backend::patch(last, &now) {
                    sub.patch(p);
                }
            }
        }
        sent = Some(now);
        // A watched path the command itself touched (a `git status` refreshing the index under a
        // watched folder) must not run it again: drop the events of the run and of the quiet
        // time after it. A change made by someone else in that time is seen at the next reason to run.
        settle(&mut changed).await;
        tokio::select! {
            _ = async { every.as_mut().unwrap().tick().await }, if every.is_some() => {}
            Some(()) = changed.recv() => {
                // Wait for the events of one save to stop.
                settle(&mut changed).await;
            }
            r = ran.recv() => {
                if r == Err(broadcast::error::RecvError::Closed) {
                    return;
                }
            }
        }
        // Several actions or events while the last run went are one more run.
        while ran.try_recv().is_ok() {}
        while changed.try_recv().is_ok() {}
    }
}

/// One `action/invoke`: run the command, answer `{code, stdout, stderr}` whatever its exit code,
/// then run every feed subscription of the package again. A run killed at its timeout, or one
/// that could not start, is the action's error.
async fn invoke(call: Call, action: Arc<Action>, folder: Arc<PathBuf>, ran: broadcast::Sender<()>) -> Result<Value, ActionError> {
    let cwd = cwd_of(&call.args, &folder);
    let env = env_of("ARG", "WHEEL_ARGS", &call.args);
    let timeout = action.timeout.unwrap_or(manifest::ACTION_TIMEOUT);
    let result = run_bounded(action.run.clone(), cwd, env, timeout).await;
    let _ = ran.send(());
    let b = result.map_err(ActionError::new)?;
    if let Some(why) = failure(&b, timeout) {
        call.peer.log("warn", format!("action {} {}: {why}", call.action, call.args));
        if b.code.is_none() {
            return Err(ActionError::new(why));
        }
    }
    Ok(json!({ "code": b.code, "stdout": b.stdout, "stderr": b.stderr }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_becomes_json_lines_or_text() {
        assert_eq!(value_of(Output::Auto, "{\"a\": 1}\n").unwrap(), json!({"a": 1}));
        assert_eq!(value_of(Output::Auto, "one\ntwo\n").unwrap(), json!(["one", "two"]));
        assert_eq!(value_of(Output::Auto, "123\n").unwrap(), json!(123));
        assert_eq!(value_of(Output::Lines, "123\n").unwrap(), json!(["123"]));
        assert_eq!(value_of(Output::Text, "a\nb\n").unwrap(), json!("a\nb\n"));
        assert_eq!(value_of(Output::Auto, "").unwrap(), json!([]));
        assert!(value_of(Output::Json, "nope").unwrap_err().contains("not JSON"));
    }

    #[test]
    fn params_reach_the_environment_by_name_and_whole() {
        let env = env_of("ARG", "WHEEL_ARGS", &json!({"name": "it's \"x\"", "port-no": 8080, "none": null}));
        assert!(env.contains(&("ARG_NAME".into(), "it's \"x\"".into())));
        assert!(env.contains(&("ARG_PORT_NO".into(), "8080".into())));
        assert!(env.contains(&("ARG_NONE".into(), String::new())));
        assert_eq!(env[0].0, "WHEEL_ARGS");
        assert_eq!(env_of("PARAM", "WHEEL_PARAMS", &Value::Null), vec![("WHEEL_PARAMS".into(), "{}".into())]);
    }

    #[test]
    fn a_failure_names_the_exit_and_the_end_of_stderr() {
        let tmp = std::env::temp_dir();
        let run = |cmd: &str, limit: Duration| bounded_with(cmd, &tmp, &[], limit, std::sync::mpsc::channel()).unwrap();
        let b = run("echo fine; echo broke >&2; exit 3", Duration::from_secs(5));
        assert_eq!(failure(&b, 5.0).unwrap(), "exit 3: broke");
        let b = run("echo only-stdout; exit 2", Duration::from_secs(5));
        assert_eq!(failure(&b, 5.0).unwrap(), "exit 2: only-stdout");
        let b = run("sleep 5", Duration::from_millis(200));
        assert_eq!(failure(&b, 0.2).unwrap(), "timed out after 0.2 s");
        let b = run("true", Duration::from_secs(5));
        assert_eq!(failure(&b, 5.0), None);
    }

    #[tokio::test]
    async fn settle_ends_within_its_limit_while_events_keep_coming() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let flood = tokio::spawn(async move {
            loop {
                if tx.send(()).is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let t = std::time::Instant::now();
        settle(&mut rx).await;
        assert!(t.elapsed() < QUIET_MAX + Duration::from_millis(400), "{:?}", t.elapsed());
        flood.abort();
    }

    #[tokio::test]
    async fn dropping_a_run_kills_its_command() {
        let marker = std::env::temp_dir().join(format!("wheel-cmd-drop-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let run = format!("sleep 0.4; touch {}", marker.display());
        let t = std::time::Instant::now();
        let r = tokio::time::timeout(Duration::from_millis(100), run_bounded(run, std::env::temp_dir(), vec![], 20.0)).await;
        assert!(r.is_err(), "the run should still be going at 100 ms");
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(!marker.exists(), "the dropped run went on to touch its file");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_dir_param_is_used_only_when_it_is_an_absolute_directory() {
        let folder = Path::new("/nonexistent-package");
        let tmp = std::env::temp_dir();
        assert_eq!(cwd_of(&json!({"dir": tmp.to_str().unwrap()}), folder), tmp);
        assert_eq!(cwd_of(&json!({"dir": "relative"}), folder), folder);
        assert_eq!(cwd_of(&json!({"dir": "/no/such/dir"}), folder), folder);
        assert_eq!(cwd_of(&json!({}), folder), folder);
    }
}
