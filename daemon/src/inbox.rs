//! An orchestrator's inbox (ADR-0054): a Maildir in the orchestrator's folder. wheeld writes each
//! event about an agent routed to the orchestrator as one JSON file in `inbox/tmp` and renames it
//! into `inbox/new`, so a reader never sees a partial file; attaching it to the person's next
//! message moves it to `inbox/cur`. Nothing edits or deletes an inbox file.
//!
//! The folder's `tools/on-event` runs once per new file, with its path, and once with `start`
//! when the orchestrator registers in this wheeld; it runs on a task of its own, one call at a
//! time in the order the files landed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wheel_proto::{InboxEvent, InboxKind};

/// The longest one `tools/on-event` call may run before it is killed and logged as failed.
const SCRIPT_LIMIT: Duration = Duration::from_secs(60);

/// The most stderr a failed `tools/on-event` call keeps for its log line and inbox event.
const STDERR_KEPT: usize = 2000;

/// Orders the files this wheeld writes within one millisecond.
static SEQ: AtomicU64 = AtomicU64::new(0);

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// What `tools/on-event` is run for.
#[derive(Debug)]
pub(crate) enum Run {
    /// The orchestrator registered in this wheeld: its first run, with the argument `start`.
    Start,
    /// A new inbox file, by path.
    File(PathBuf),
}

/// How the folder's `tools/on-event` is run: the environment `wheel` reads, and who to name
/// when it fails.
pub(crate) struct Runner {
    pub env: Vec<(String, String)>,
    /// The orchestrator's agent, named on a `script_failed` event.
    pub agent: String,
}

pub(crate) struct Inbox {
    /// The orchestrator's folder; the Maildir is its `inbox/`.
    pub dir: PathBuf,
    /// How long one agent turn runs before it is reported over time, in seconds.
    turn_limit: AtomicU64,
    runs: tokio::sync::mpsc::UnboundedSender<Run>,
}

impl Inbox {
    /// The inbox in folder `dir`, its `tmp`, `new` and `cur` made when missing, with the task
    /// that runs `dir/tools/on-event` for it. The task ends when the inbox is dropped.
    pub(crate) fn open(dir: PathBuf, runner: Runner) -> Result<std::sync::Arc<Self>, String> {
        for sub in ["tmp", "new", "cur"] {
            let path = dir.join("inbox").join(sub);
            std::fs::create_dir_all(&path).map_err(|e| format!("cannot make {}: {e}", path.display()))?;
        }
        let (runs, rx) = tokio::sync::mpsc::unbounded_channel();
        let inbox = std::sync::Arc::new(Self { dir, turn_limit: AtomicU64::new(1800), runs });
        tokio::spawn(run_scripts(std::sync::Arc::downgrade(&inbox), inbox.dir.clone(), runner, rx));
        Ok(inbox)
    }

    pub(crate) fn turn_limit(&self) -> Duration {
        Duration::from_secs(self.turn_limit.load(Ordering::Relaxed))
    }

    pub(crate) fn set_turn_limit(&self, limit: Duration) {
        self.turn_limit.store(limit.as_secs().max(1), Ordering::Relaxed);
    }

    fn sub(&self, name: &str) -> PathBuf {
        self.dir.join("inbox").join(name)
    }

    /// Write `event` as a new file and queue `tools/on-event` for it; returns the file's path in
    /// `new`.
    pub(crate) fn add(&self, event: &InboxEvent) -> Result<PathBuf, String> {
        let path = self.write(event)?;
        let _ = self.runs.send(Run::File(path.clone()));
        Ok(path)
    }

    /// Queue `tools/on-event start`.
    pub(crate) fn start(&self) {
        let _ = self.runs.send(Run::Start);
    }

    /// Write `event` into `tmp`, then rename it into `new`. File names sort in the order they
    /// were written: the time, then a count within this wheeld, then its pid.
    fn write(&self, event: &InboxEvent) -> Result<PathBuf, String> {
        let name = format!(
            "{:013}.{:06}.{}.json",
            event.at_ms,
            SEQ.fetch_add(1, Ordering::Relaxed),
            std::process::id()
        );
        let tmp = self.sub("tmp").join(&name);
        let new = self.sub("new").join(&name);
        let body = serde_json::to_vec_pretty(event).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, body).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &new).map_err(|e| format!("cannot move {} into new: {e}", tmp.display()))?;
        tracing::info!(file = %new.display(), kind = ?event.kind, agent = %event.agent, "inbox event");
        Ok(new)
    }

    /// The files in `new`, oldest first, with their events. A file that does not parse is
    /// listed with no event, so it still moves to `cur` with the rest.
    pub(crate) fn pending(&self) -> Vec<(PathBuf, Option<InboxEvent>)> {
        let Ok(entries) = std::fs::read_dir(self.sub("new")) else { return Vec::new() };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let event = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
                if event.is_none() {
                    tracing::warn!(file = %path.display(), "inbox file does not parse");
                }
                (path, event)
            })
            .collect()
    }

    /// Move `paths`, files in `new`, to `cur`. A file already gone (another message took it) is
    /// passed over.
    pub(crate) fn told(&self, paths: &[PathBuf]) {
        for path in paths {
            let Some(name) = path.file_name() else { continue };
            let cur = self.sub("cur").join(name);
            match std::fs::rename(path, &cur) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(file = %path.display(), "cannot move an inbox file to cur: {e}"),
            }
        }
    }
}

/// `events` as the block attached to the person's message: marked as wheeld's, one line each.
pub(crate) fn attachment(events: &[(PathBuf, Option<InboxEvent>)]) -> String {
    let lines: Vec<String> = events
        .iter()
        .map(|(path, event)| match event {
            Some(e) => format!("- {} · {}", clock(e.at_ms), e.text),
            None => format!("- unreadable inbox file {}", path.display()),
        })
        .collect();
    format!(
        "[Automated: {} inbox event{} from wheeld since the person's last message. The person did not write this.]\n{}",
        lines.len(),
        if lines.len() == 1 { "" } else { "s" },
        lines.join("\n")
    )
}

/// `ms` since the epoch as `HH:MM UTC`.
fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{:02}:{:02} UTC", (secs / 3600) % 24, (secs / 60) % 60)
}

/// Run `dir/tools/on-event` for each `Run`, one at a time, until the inbox is dropped. A folder
/// with no script runs nothing. A failure (spawn error, non-zero exit, the time limit) goes to
/// wheeld's log and into the inbox as a `script_failed` event, for which the script is not run.
async fn run_scripts(
    inbox: std::sync::Weak<Inbox>,
    dir: PathBuf,
    runner: Runner,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Run>,
) {
    let script = dir.join("tools").join("on-event");
    while let Some(run) = rx.recv().await {
        if !script.is_file() {
            continue;
        }
        let arg = match &run {
            Run::Start => "start".into(),
            Run::File(path) => path.display().to_string(),
        };
        let Some(why) = run_once(&script, &dir, &arg, &runner.env).await else {
            tracing::info!(script = %script.display(), %arg, "on-event ran");
            continue;
        };
        tracing::warn!(script = %script.display(), %arg, %why, "on-event failed");
        let Some(inbox) = inbox.upgrade() else { break };
        let event = InboxEvent {
            kind: InboxKind::ScriptFailed,
            agent: runner.agent.clone(),
            text: format!("tools/on-event {arg} failed: {why}"),
            at_ms: now_ms(),
            perm: None,
        };
        if let Err(e) = inbox.write(&event) {
            tracing::warn!("cannot record the on-event failure: {e}");
        }
    }
}

/// Run `script arg` in `dir`; None when it exited 0, else why it failed.
async fn run_once(script: &Path, dir: &Path, arg: &str, env: &[(String, String)]) -> Option<String> {
    let child = tokio::process::Command::new(script)
        .arg(arg)
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(e) => return Some(format!("cannot start it: {e}")),
    };
    match tokio::time::timeout(SCRIPT_LIMIT, child.wait_with_output()).await {
        Err(_) => Some(format!("still running after {} seconds, killed", SCRIPT_LIMIT.as_secs())),
        Ok(Err(e)) => Some(format!("cannot wait on it: {e}")),
        Ok(Ok(out)) if out.status.success() => None,
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr = stderr.trim();
            let cut = stderr.char_indices().nth(STDERR_KEPT).map_or(stderr.len(), |(i, _)| i);
            let code = out.status.code().map_or("a signal".to_string(), |c| format!("exit {c}"));
            Some(if stderr.is_empty() { code } else { format!("{code}: {}", &stderr[..cut]) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(agent: &str, kind: InboxKind, text: &str, at_ms: u64) -> InboxEvent {
        InboxEvent { kind, agent: agent.into(), text: text.into(), at_ms, perm: None }
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-inbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn files_land_in_new_in_order_and_move_to_cur_once_told() {
        let dir = folder("order");
        let inbox = Inbox::open(dir.clone(), Runner { env: Vec::new(), agent: "w1".into() }).unwrap();
        // Same millisecond: the count keeps their order.
        inbox.add(&event("w5", InboxKind::Permission, "w5 needs permission: Bash", 1_700_000_000_000)).unwrap();
        inbox.add(&event("w5", InboxKind::Finished, "w5 finished its turn", 1_700_000_000_000)).unwrap();
        assert!(std::fs::read_dir(dir.join("inbox/tmp")).unwrap().next().is_none(), "nothing is left in tmp");
        let pending = inbox.pending();
        let texts: Vec<_> = pending.iter().map(|(_, e)| e.as_ref().unwrap().text.as_str()).collect();
        assert_eq!(texts, ["w5 needs permission: Bash", "w5 finished its turn"]);
        let paths: Vec<_> = pending.iter().map(|(p, _)| p.clone()).collect();
        inbox.told(&paths);
        inbox.told(&paths); // a second move of the same files is passed over
        assert!(inbox.pending().is_empty());
        assert_eq!(std::fs::read_dir(dir.join("inbox/cur")).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_attachment_is_marked_as_wheelds() {
        let one = [(PathBuf::from("/x"), Some(event("w5", InboxKind::Finished, "w5 finished its turn", 3_600_000 * 20 + 60_000 * 25)))];
        assert_eq!(
            attachment(&one),
            "[Automated: 1 inbox event from wheeld since the person's last message. The person did not write this.]\n- 20:25 UTC · w5 finished its turn"
        );
        let bad = [(PathBuf::from("/i/new/a.json"), None)];
        assert!(attachment(&bad).ends_with("- unreadable inbox file /i/new/a.json"));
    }

    #[tokio::test]
    async fn a_failing_script_is_an_event_and_is_not_run_for_it() {
        let dir = folder("fail");
        std::fs::create_dir_all(dir.join("tools")).unwrap();
        let script = dir.join("tools/on-event");
        let log = dir.join("ran.log");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$1\" >> {}\necho broken >&2\nexit 3\n", log.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let inbox = Inbox::open(dir.clone(), Runner { env: Vec::new(), agent: "w1".into() }).unwrap();
        inbox.start();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while inbox.pending().len() < 1 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pending = inbox.pending();
        assert_eq!(pending.len(), 1, "{pending:?}");
        let failed = pending[0].1.as_ref().unwrap();
        assert_eq!((failed.kind, failed.agent.as_str()), (InboxKind::ScriptFailed, "w1"));
        assert_eq!(failed.text, "tools/on-event start failed: exit 3: broken");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "start\n", "the failure event ran no script");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
