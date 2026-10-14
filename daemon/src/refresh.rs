//! Running an artifact's refresh command for the pane showing it.
//!
//! The manifest declares the command; the daemon runs it through `/bin/sh` in the pane's *space*
//! directory and hands it the pane's data directory in `WHEEL_DATA_DIR`, which is where an artifact
//! writes what its page reads: the artifact's own folder is read-only static content a pane serves,
//! and a project's working tree is not for generated files. That is a place to write, not a
//! sandbox — the command is a shell the person approved, and it can reach whatever they can. A run
//! happens when the pane opens, when a caller asks for one, and — when the manifest declares
//! `every` — on a timer the pane owns, which runs the same [`Refresh`] and has no way of its own to
//! run a command.
//!
//! No command runs until a person approves it at a permission request — the same request the lead's
//! tools raise, which every connected client and the `perm` CLI verbs answer. Answering "always"
//! writes the pair into a trust file in the instance directory, so a restart does not ask again;
//! the pair is the artifact's *folder* and the command, so editing the command asks afresh. A timer
//! tick never raises that request: it runs only a command remembered as approved, and skips the
//! tick rather than putting a prompt in front of someone who did nothing to ask for one.
//!
//! A write the command makes reaches the page through the data directory's watch (`web.rs`), the
//! same `pane_reload` a person editing a file there sends: the page's own load and the on-open run
//! race, so a run that wrote nothing back to the page would leave a pane showing an empty data
//! directory. Nothing in this build lets a page ask for a refresh, so no page can reload itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use wheel_proto::{ArtifactRef, PermOption, RefreshInfo, RunInfo, TimerState, TrustInfo};

use crate::artifacts::Manifest;
use crate::perms::Perms;
use crate::server::AppState;

/// How much of a run's standard error a reader gets, in bytes off the end: enough to say why a
/// command failed without putting a whole build log in every `status`.
const STDERR_TAIL: usize = 2000;

/// How long one run may take before the daemon stops it. A refresh fills a pane with current data;
/// a command still going a minute later is not doing that, and leaving it to run would hold every
/// later refresh of that pane behind it.
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// How long a run that has ended keeps reading output a process that left the command's group still
/// sends: enough for what was already written to arrive, short enough that nobody waits on it.
const PIPE_GRACE: Duration = Duration::from_secs(1);

/// Whether `command` starts by running a file inside the project: its first word (after any
/// `NAME=value` prefixes) is a path, either relative to the space's directory or under one of
/// `roots` (the artifact's folder and the space's directory). What such a command does
/// changes whenever a pull, a branch switch or an edit changes the file, so an approval of its name
/// would cover code the person never saw; it is allowed once per run and never remembered. A command
/// that names a program on the search path (`bd ready`, `make`) is remembered, because what it reads
/// is meant to change between runs. A program given a script as an argument (`sh refresh.sh`) is
/// not detected.
fn runs_a_project_file(command: &str, roots: &[&str]) -> bool {
    let is_assignment = |w: &&str| {
        w.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
    };
    let first = command
        .split_whitespace()
        .find(|w| !is_assignment(w))
        .unwrap_or("");
    first.contains('/')
        && (!first.starts_with('/') || roots.iter().any(|r| Path::new(first).starts_with(r)))
}

/// The file in the instance directory holding what a person answered "always" to. It lives there
/// rather than in the saved layout for the same reason the manifests do: a daemon that drops
/// unknown top-level keys when it rewrites the layout must not be able to lose it.
const TRUST_FILE: &str = "artifact-trust.json";

/// The trust file's contents.
#[derive(Serialize, Deserialize, Default)]
struct Trusted {
    #[serde(default)]
    trusted: Vec<TrustInfo>,
}

/// The remembered approvals: the only durable artifact state the daemon owns. Every read and every
/// write goes to the file itself, so an approval a person removes by hand takes effect at the next
/// run; one lock around the file is what makes a read-modify-write atomic.
pub(crate) struct TrustFile {
    path: PathBuf,
}

impl TrustFile {
    pub(crate) fn new(instance_dir: &Path) -> TrustFile {
        TrustFile {
            path: instance_dir.join(TRUST_FILE),
        }
    }

    /// Every remembered pair, sorted by folder and then command. A file that is not there yet holds
    /// none, which is not an error; a file that exists and cannot be read or parsed is an error, so
    /// that a write never replaces approvals this could not see.
    fn read(&self) -> Result<Vec<TrustInfo>, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("read {}: {e}", self.path.display())),
        };
        let mut all = serde_json::from_str::<Trusted>(&text)
            .map_err(|e| format!("{} is not a readable trust file: {e}", self.path.display()))?
            .trusted;
        all.sort_by(|a, b| (&a.dir, &a.command).cmp(&(&b.dir, &b.command)));
        Ok(all)
    }

    /// The same for a reader that has no one to report a damaged file to: it holds nothing, so a
    /// damaged file asks the person again rather than approving on its own.
    fn load(&self) -> Vec<TrustInfo> {
        self.read()
            .inspect_err(|e| {
                tracing::warn!(path = %self.path.display(), error = %e, "the artifact trust file cannot be read; nothing is trusted")
            })
            .unwrap_or_default()
    }

    /// Write the whole list through a temporary file, so a crash mid-write leaves the old one.
    fn save(&self, trusted: &[TrustInfo]) -> Result<(), String> {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            let all = Trusted {
                trusted: trusted.to_vec(),
            };
            std::fs::write(&tmp, serde_json::to_vec_pretty(&all)?)?;
            std::fs::rename(tmp, &self.path)
        };
        write().map_err(|e| format!("write {}: {e}", self.path.display()))
    }

    /// Whether this exact folder and command were approved before. Read from the file every time,
    /// a timer's ticks included: reading and parsing a 20-row trust file takes 60µs, so one tick per
    /// second per refreshing pane costs nothing worth keeping a stale copy for, and an approval
    /// removed by hand takes effect at the next tick.
    fn allows(&self, dir: &str, command: &str) -> bool {
        !runs_a_project_file(command, &[dir])
            && self
                .load()
                .iter()
                .any(|t| t.dir == dir && t.command == command)
    }

    /// Remember one pair, replacing an earlier approval of the same one rather than listing it
    /// twice. Both writers read with [`TrustFile::read`], so a file that cannot be read stops the
    /// write rather than replacing every approval in it with this one.
    fn remember(&self, entry: TrustInfo) -> Result<(), String> {
        let mut all = self.read()?;
        all.retain(|t| !(t.dir == entry.dir && t.command == entry.command));
        all.push(entry);
        all.sort_by(|a, b| (&a.dir, &a.command).cmp(&(&b.dir, &b.command)));
        self.save(&all)
    }

    /// Forget every approval remembered for artifact `name`, whatever scope it was approved in and
    /// however many commands it has had. Refused when nothing is remembered for that name, so a
    /// typo is not a silent success.
    fn forget(&self, name: &str) -> Result<Vec<TrustInfo>, String> {
        let all = self.read()?;
        let left: Vec<TrustInfo> = all.iter().filter(|t| t.name != name).cloned().collect();
        if left.len() == all.len() {
            return Err(format!("no approval is remembered for artifact {name}"));
        }
        self.save(&left)?;
        tracing::info!(%name, forgot = all.len() - left.len(), left = left.len(), "artifact approval forgotten");
        Ok(left)
    }
}

/// What a reader of a runner sees: what it would run now, and what the last run did.
struct Seen {
    command: String,
    every: Option<u64>,
    runs: u64,
    last: Option<RunInfo>,
}

/// One pane's refresh runner: it owns that pane's command and its last result, and nothing else
/// runs a command for that pane. Every input it needs is taken when the pane opens, so a run holds
/// no lock of the daemon's but its own.
pub(crate) struct Refresh {
    pane: String,
    artifact: ArtifactRef,
    /// The space's directory: the command's working directory.
    cwd: String,
    /// The artifact's folder: the manifest is read from here, and trust is keyed by it.
    dir: PathBuf,
    /// The pane's data directory: the one place the command is given to write.
    data: PathBuf,
    perms: Arc<Perms>,
    trust: Arc<Mutex<TrustFile>>,
    /// The program each run starts as its warden: `wheeld` itself.
    warden: PathBuf,
    /// One run at a time per pane, so two shells never write one data directory and an on-demand
    /// run during the on-open one waits rather than racing it.
    running: tokio::sync::Mutex<()>,
    seen: Mutex<Seen>,
    /// This pane's timer task, when the manifest asked for one as the pane opened. Its liveness is
    /// what a reader is told about the timer, so a timer that is gone is never reported as ticking.
    timer: std::sync::OnceLock<tokio::task::AbortHandle>,
}

impl Refresh {
    /// Run the command the manifest declares now, asking the person first unless the pair is
    /// already trusted. The record it returns is the one `status` reports.
    ///
    /// The manifest is read afresh every run, so editing `command` changes what runs — and what is
    /// asked about — rather than running whatever the pane opened with. An error only when there is
    /// nothing to run at all; a command that failed, or that the person refused, is a successful
    /// read of a failed run.
    pub(crate) async fn run(&self) -> Result<RefreshInfo, String> {
        let _one = self.running.lock().await;
        let command = self.declared()?;
        let started = self.start(&command);
        let allowed = self.approved(&command).await;
        self.finish(&command, started, allowed).await;
        Ok(self.info())
    }

    /// One timer tick: the same run as any other, except that it never asks. A tick that cannot run
    /// skips — it is not a run, so nothing about it is recorded as one — and says why, rather than
    /// raising a request nobody is there to expect or ending the timer: the next tick runs as soon
    /// as the manifest declares a command again, or the person's answer to an on-demand refresh
    /// remembers this one.
    ///
    /// Taking the pane's run lock first is what keeps a tick from overlapping a run: a tick landing
    /// during an on-demand run — or during the wait on a person's answer to the opening one — waits
    /// for it, and reads the manifest afresh once it has the lock.
    async fn tick(&self) {
        let _one = self.running.lock().await;
        let skip = |why: &str| tracing::info!(pane = %self.pane, dir = %self.dir.display(), %why, "refresh timer skipped a tick");
        let Ok(command) = self.declared() else {
            return skip("the manifest declares no refresh command any more");
        };
        if self.seen.lock().unwrap().every.is_none() {
            return skip("the manifest does not ask for a refresh on a timer any more");
        }
        let dir = self.dir.display().to_string();
        if !self.trust.lock().unwrap().allows(&dir, &command) {
            return skip("this command is not remembered as approved, and a tick never asks");
        }
        let started = self.start(&command);
        self.finish(&command, started, Ok(true)).await;
    }

    /// What this pane's timer would do if it ticked now: read from the manifest, the trust file and
    /// the timer task itself every time, so what a reader is told is the timer's state now and
    /// never an outcome a later edit or approval has outgrown.
    fn timer(&self) -> TimerState {
        let Some(task) = self.timer.get() else {
            return TimerState::Off;
        };
        if task.is_finished() {
            return TimerState::Stopped;
        }
        let Manifest { command, every, .. } = crate::artifacts::load(&self.dir);
        let (Some(command), Some(_)) = (command, every) else {
            return TimerState::Idle;
        };
        let dir = self.dir.display().to_string();
        match self.trust.lock().unwrap().allows(&dir, &command) {
            true => TimerState::Ticking,
            false => TimerState::Untrusted,
        }
    }

    /// What the manifest declares to run now, recorded for a reader along with the interval it asks
    /// for. An error when it declares no command at all: there is nothing to run.
    fn declared(&self) -> Result<String, String> {
        let Manifest { command, every, .. } = crate::artifacts::load(&self.dir);
        let Some(command) = command else {
            return Err(format!(
                "{} declares no refresh command, so there is nothing to run for pane {}",
                self.dir.join("artifact.toml").display(),
                self.pane
            ));
        };
        let mut seen = self.seen.lock().unwrap();
        seen.command = command.clone();
        seen.every = every;
        Ok(command)
    }

    /// Mark a run as starting: when it started, for a reader, and the clock its duration comes off.
    fn start(&self, command: &str) -> (u64, Instant) {
        tracing::info!(pane = %self.pane, scope = self.artifact.scope().name(), name = self.artifact.name(), %command, cwd = %self.cwd, data = %self.data.display(), "refresh start");
        (unix_ms(SystemTime::now()), Instant::now())
    }

    /// Run `command` if `allowed` says the person allows it, and record what happened as this
    /// runner's last run either way: a refusal is a run that did not run, not a run that was lost.
    async fn finish(&self, command: &str, started: (u64, Instant), allowed: Result<bool, String>) {
        let (started_ms, at) = started;
        let (exit, error, stderr_tail) = match allowed {
            Ok(true) => self.exec(command).await,
            Ok(false) => (
                None,
                Some("the person refused this command".into()),
                String::new(),
            ),
            // The request was withdrawn: the pane stopped showing the artifact while it waited.
            Err(e) => (None, Some(e), String::new()),
        };
        let last = RunInfo {
            started_ms,
            duration_ms: at.elapsed().as_millis() as u64,
            exit,
            error,
            stderr_tail,
        };
        tracing::info!(pane = %self.pane, %command, exit = ?last.exit, error = last.error.as_deref().unwrap_or("-"), duration_ms = last.duration_ms, stderr_bytes = last.stderr_tail.len(), ok = last.ok(), "refresh end");
        let mut seen = self.seen.lock().unwrap();
        seen.runs += 1;
        seen.last = Some(last);
    }

    /// Whether the person allows `command` for this artifact's folder: at once when the pair is
    /// remembered, otherwise by asking, which waits for an answer. "Always" is remembered before
    /// the command runs, so a daemon that dies mid-run does not ask again for what it was told.
    async fn approved(&self, command: &str) -> Result<bool, String> {
        let dir = self.dir.display().to_string();
        if self.trust.lock().unwrap().allows(&dir, command) {
            tracing::info!(pane = %self.pane, %dir, %command, "refresh command already trusted");
            return Ok(true);
        }
        // Quoted, so the person sees the command exactly as the manifest wrote it, and told which
        // artifact and which project directory it would run in.
        let title = format!(
            "Refresh the {} artifact {} in {}: run {command:?}",
            self.artifact.scope().name(),
            self.artifact.name(),
            self.cwd
        );
        let remembered = !runs_a_project_file(command, &[&dir, &self.cwd]);
        let options = [
            ("allow", "Allow once", "allow_once", true),
            ("always", "Always allow", "allow_always", remembered),
            ("deny", "Deny", "reject_once", true),
        ]
        .into_iter()
        .filter(|(.., offered)| *offered)
        .map(|(id, name, kind, _)| PermOption {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
        })
        .collect::<Vec<_>>();
        let choice = self.perms.ask(&self.pane, title, options).await?;
        if choice == "always" && remembered {
            let entry = TrustInfo {
                dir: dir.clone(),
                command: command.to_string(),
                scope: self.artifact.scope(),
                name: self.artifact.name().to_string(),
                approved_ms: unix_ms(SystemTime::now()),
            };
            match self.trust.lock().unwrap().remember(entry) {
                Ok(()) => {
                    tracing::info!(pane = %self.pane, %dir, %command, "refresh command trusted from now on")
                }
                // The run still happens: the person allowed it. Only the remembering failed, so
                // the next run asks again rather than the allowed run being lost.
                Err(e) => {
                    tracing::warn!(pane = %self.pane, %dir, error = %e, "could not remember the approval; the next run asks again")
                }
            }
        }
        tracing::info!(pane = %self.pane, %choice, "refresh command answered");
        // An allow-list, so an option id this does not know refuses rather than runs: the one place
        // a command gets to run is the two answers that say so. "Always" counts only when it was
        // offered.
        Ok(choice == "allow" || (choice == "always" && remembered))
    }

    /// Run `command` through `/bin/sh -c` in the space's directory, with the pane's data directory
    /// and the pane in its environment. Returns its exit status, why it has none, and the tail of
    /// its standard error.
    async fn exec(&self, command: &str) -> (Option<i32>, Option<String>, String) {
        // The command is given a directory to write into, so it is made again here: the pane's
        // data directory is removed when the pane stops showing the artifact, and a run races that.
        if let Err(e) = std::fs::create_dir_all(&self.data) {
            return (
                None,
                Some(format!("make {}: {e}", self.data.display())),
                String::new(),
            );
        }
        // Declared before the run's future, so it drops after it: a pane that closes mid-run aborts
        // the task holding this, and the limit below drops the run, and either way the whole group
        // dies rather than writing on into a directory nothing serves.
        let mut group = match Group::arm(&self.pane, &self.warden).await {
            Ok(group) => group,
            Err(e) => return (None, Some(e), String::new()),
        };
        let child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .env("WHEEL_PANE", &self.pane)
            .env("WHEEL_SPACE_DIR", &self.cwd)
            .env("WHEEL_DATA_DIR", &self.data)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The shell joins the warden's group, so everything it starts — work it backgrounds
            // included — can be killed with it rather than outliving the run.
            .process_group(group.pgid)
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => return (None, Some(format!("run {command:?}: {e}")), String::new()),
        };
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        // Outside the run, so what was read stays readable when the limit drops the run.
        let (mut out, mut err) = (Captured::default(), Captured::default());
        let run = async {
            let reading = async {
                tokio::join!(drain(stdout, &mut out), drain(stderr, &mut err));
            };
            tokio::pin!(reading);
            let mut read = false;
            // The run ends when the shell does, whoever still holds its pipes.
            let status = tokio::select! {
                status = child.wait() => status,
                () = &mut reading => {
                    read = true;
                    child.wait().await
                }
            };
            // Whatever the shell left running is killed with its group, which closes the pipes it
            // held. A process that left the group holds them still, so the rest of the output gets
            // a moment rather than the run waiting on it until the limit.
            group.kill();
            if !read {
                let _ = tokio::time::timeout(PIPE_GRACE, &mut reading).await;
            }
            status
        };
        // A command that never ends would hold this pane's runner forever, so every later refresh
        // of that pane would wait behind it rather than reading fresh data. Dropping the run kills
        // its group, so the limit ends the command as well as the wait.
        let status = tokio::time::timeout(RUN_LIMIT, run).await;
        let status = match status {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return (None, Some(format!("run {command:?}: {e}")), tail(&err.end)),
            Err(_) => {
                tracing::warn!(pane = %self.pane, %command, limit_secs = RUN_LIMIT.as_secs(), "refresh command stopped at its time limit");
                return (
                    None,
                    Some(format!(
                        "the command ran longer than {}s and was stopped",
                        RUN_LIMIT.as_secs()
                    )),
                    tail(&err.end),
                );
            }
        };
        let stderr = tail(&err.end);
        tracing::info!(pane = %self.pane, stdout_bytes = out.total, stderr_bytes = err.total, %status, "refresh command finished");
        match status.code() {
            Some(code) => (Some(code), None, stderr),
            // Killed by a signal: there is no exit status to report, so the reason goes in `error`
            // rather than being reported as some number the shell never returned.
            None => {
                let signal = std::os::unix::process::ExitStatusExt::signal(&status);
                (
                    None,
                    Some(format!(
                        "the command was killed by signal {}",
                        signal.unwrap_or(0)
                    )),
                    stderr,
                )
            }
        }
    }

    /// This runner as a reader sees it.
    pub(crate) fn info(&self) -> RefreshInfo {
        let timer = self.timer();
        let seen = self.seen.lock().unwrap();
        RefreshInfo {
            pane: self.pane.clone(),
            scope: self.artifact.scope(),
            name: self.artifact.name().to_string(),
            dir: self.dir.display().to_string(),
            cwd: self.cwd.clone(),
            data_dir: self.data.display().to_string(),
            command: seen.command.clone(),
            every: seen.every,
            timer,
            runs: seen.runs,
            last: seen.last.clone(),
        }
    }
}

/// Tick `refresh` every `every` seconds for as long as the pane owns this task: the pane dropping
/// it is the only thing that ends the timer, so every other condition — a command nobody has
/// approved for good, a manifest that lost its command, an `every` that came back — is a tick that
/// skips or runs and never a timer that cannot come back.
///
/// One sequential task, so at most one tick is ever pending: a run that outlasts its own interval
/// delays the next tick rather than being overlapped by it, and ticks do not pile up behind it. The
/// interval comes from each tick's own reading of the manifest, so editing `every` takes effect at
/// the tick after the edit, and the first tick is one interval after the pane's opening run.
async fn ticking(refresh: Arc<Refresh>, mut every: u64) {
    loop {
        tokio::time::sleep(Duration::from_secs(every)).await;
        refresh.tick().await;
        // The interval the manifest asks for now; the one last read when it asks for none, so that
        // re-adding `every` is picked up rather than needing the pane reopened.
        every = refresh.seen.lock().unwrap().every.unwrap_or(every);
    }
}

/// A pane's runner and every task running for it: the run the pane's opening asks for, each run a
/// caller asks for, and its timer. Dropping it stops all of them, so a pane that
/// stops showing its artifact stops refreshing — the command of a run in flight is killed with the
/// future holding it, rather than writing on into a data directory that is about to be removed.
pub(crate) struct Refresher {
    pub(crate) refresh: Arc<Refresh>,
    runs: Vec<tokio::task::AbortHandle>,
}

impl Refresher {
    /// Take a run's task under this runner, so the pane owns it. The handles of runs that have
    /// finished are dropped here, rather than collecting for as long as the pane is open.
    fn owns(&mut self, task: tokio::task::AbortHandle) {
        self.runs.retain(|t| !t.is_finished());
        self.runs.push(task);
    }
}

impl Drop for Refresher {
    fn drop(&mut self) {
        for task in &self.runs {
            task.abort();
        }
    }
}

/// The process group a refresh runs in: the shell and everything it starts, led by the run's
/// warden. Killing it once, when the shell exits or when the run is dropped first, is what leaves
/// nothing of a run alive after it ends, whether it finished, hit its time limit or was cancelled by
/// its pane.
///
/// All of that runs inside wheeld, so a daemon killed with SIGKILL or crashing would leave the
/// group running with nothing to stop it — macOS has no signal that tells a child its parent died.
/// The warden covers that. It is wheeld itself, started as [`WARDEN_ARG`] before the shell exists,
/// leading the group the shell then joins; it asks the kernel to wake it when its parent exits
/// (kqueue's `NOTE_EXIT`) and kills its own group when that happens, at that moment and with no
/// restart needed. It holds nothing of wheeld's — no pipe, no descriptor — so no other process
/// wheeld started can keep it from seeing the exit. Killing the group kills the warden with it.
struct Group<'a> {
    pane: &'a str,
    /// The warden's pid, which is the group's id; 0 once killed.
    pgid: i32,
    /// The warden, never waited on while the group lives: an unreaped warden keeps its pid, and so
    /// the group's id, from being handed to any other process.
    _warden: tokio::process::Child,
}

/// The argument that starts `wheeld` as a refresh's warden rather than as the daemon.
pub const WARDEN_ARG: &str = "--refresh-warden";

/// What a warden prints once it is watching its parent, so a run never starts unwatched.
const ARMED: &str = "armed";

/// How long a warden has to say it is watching. It is one exec and two system calls.
const WARDEN_START: Duration = Duration::from_secs(5);

impl<'a> Group<'a> {
    /// Start a run's warden, `program` with [`WARDEN_ARG`], as the leader of a new group, and wait
    /// for it to say it is watching wheeld.
    async fn arm(pane: &'a str, program: &Path) -> Result<Group<'a>, String> {
        let mut warden = tokio::process::Command::new(program)
            .arg(WARDEN_ARG)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("start the refresh warden {}: {e}", program.display()))?;
        let pgid = warden.id().map_or(0, |id| id as i32);
        let mut said = String::new();
        if let Some(out) = warden.stdout.take() {
            let mut out = tokio::io::BufReader::new(out);
            let _ = tokio::time::timeout(
                WARDEN_START,
                tokio::io::AsyncBufReadExt::read_line(&mut out, &mut said),
            )
            .await;
        }
        // Built before the check, so a warden that said the wrong thing is killed on the way out.
        let group = Group {
            pane,
            pgid,
            _warden: warden,
        };
        if pgid <= 0 || said.trim_end() != ARMED {
            return Err(format!(
                "the refresh warden {} did not say it was watching wheeld (said {said:?})",
                program.display()
            ));
        }
        tracing::info!(pane = %pane, pgid, "refresh warden armed");
        Ok(group)
    }

    fn kill(&mut self) {
        if self.pgid <= 0 {
            return;
        }
        // SAFETY: killpg takes two integers and touches no memory of ours. The id is the warden's
        // pid, and the warden is not reaped until this group is dropped, so no other process can
        // have been given it. The warden is a member, so the group always has one to kill.
        let alive = unsafe { libc::killpg(self.pgid, libc::SIGKILL) } == 0;
        if alive {
            tracing::info!(pane = %self.pane, pgid = self.pgid, "refresh process group killed");
        }
        self.pgid = 0;
    }
}

impl Drop for Group<'_> {
    fn drop(&mut self) {
        // The warden goes with the group; tokio reaps it in the background once it is dropped.
        self.kill();
    }
}

/// Run as a refresh's warden: watch the parent that started this process, and when it exits, kill
/// this process's group — the refresh command and everything it started — and with it this process.
/// `wheeld` calls this for [`WARDEN_ARG`] before it starts a runtime, a thread or anything else.
pub fn warden() -> ! {
    // SAFETY: each call takes integers or a pointer to a `kevent` on this stack, valid for the call.
    unsafe {
        let parent = libc::getppid();
        let kq = libc::kqueue();
        let change = libc::kevent {
            ident: parent as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // The watch names a pid; the parent check after it is what says that pid is still the
        // process that started this one, and not one handed the pid after wheeld already died.
        let watching = parent > 1
            && kq >= 0
            && libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0
            && libc::getppid() == parent;
        if watching {
            println!("{ARMED}");
            let mut fired: libc::kevent = std::mem::zeroed();
            while libc::kevent(kq, std::ptr::null(), 0, &mut fired, 1, std::ptr::null()) < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
            }
        }
        libc::killpg(libc::getpgrp(), libc::SIGKILL);
        libc::_exit(1)
    }
}

/// The end of a stream and how long the whole stream was. It keeps one byte more than
/// `STDERR_TAIL`, so `tail` can tell output that was cut from output that just fit, and holds no
/// more however much the command writes.
#[derive(Default)]
struct Captured {
    end: Vec<u8>,
    total: u64,
}

impl Captured {
    fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        self.end.extend_from_slice(chunk);
        let keep = STDERR_TAIL + 1;
        if self.end.len() > keep {
            self.end.drain(..self.end.len() - keep);
        }
    }
}

/// Read `pipe` until it closes, keeping the end of it in `seen`. A read error ends it with what was
/// read so far: the output is for a reader's tail, and the exit status is what says whether the run
/// worked.
async fn drain(pipe: Option<impl tokio::io::AsyncRead + Unpin>, seen: &mut Captured) {
    if let Some(mut pipe) = pipe {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = tokio::io::AsyncReadExt::read(&mut pipe, &mut chunk).await {
            if n == 0 {
                break;
            }
            seen.push(&chunk[..n]);
        }
    }
}

/// The last `STDERR_TAIL` bytes of `bytes` as text, cut at a character boundary and marked when
/// anything was left off, so a reader is never shown a tail that looks like the whole output.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= STDERR_TAIL {
        return text.into_owned();
    }
    let mut from = text.len() - STDERR_TAIL;
    while from < text.len() && !text.is_char_boundary(from) {
        from += 1;
    }
    format!("…{}", &text[from..])
}

/// `at` in milliseconds since the Unix epoch; 0 for a clock before it.
fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl AppState {
    /// Start refreshing for web pane `pane`, which shows `artifact` in a space whose directory is
    /// `cwd` and whose data directory is `data`: register the runner and run it once, now, for the
    /// opening. Replaces any earlier runner of that pane. An artifact whose manifest declares no
    /// command registers nothing, so nothing to run is nothing to report. Call from inside the
    /// tokio runtime.
    pub(crate) fn start_refresh(
        &self,
        pane: &str,
        artifact: &ArtifactRef,
        cwd: &str,
        data: PathBuf,
    ) {
        // The folder the pane serves from, and nothing else: a root or a folder a symlink moved out
        // of the directory its scope belongs to serves nothing and is written to by nothing, so its
        // manifest is not read here either and its command never runs.
        let Some(dir) = self.artifact_dir_inside(artifact, cwd) else {
            tracing::warn!(%pane, scope = artifact.scope().name(), name = artifact.name(), "the artifact's folder is not inside its scope's root, or is not there; nothing refreshes this pane");
            self.stop_refresh(pane);
            return;
        };
        let Manifest { command, every, .. } = crate::artifacts::load(&dir);
        let Some(command) = command else {
            self.stop_refresh(pane);
            return;
        };
        let refresh = Arc::new(Refresh {
            pane: pane.to_string(),
            artifact: artifact.clone(),
            cwd: cwd.to_string(),
            dir,
            data,
            perms: self.perms.clone(),
            trust: self.trust.clone(),
            warden: self.warden.clone(),
            running: tokio::sync::Mutex::new(()),
            seen: Mutex::new(Seen {
                command: command.clone(),
                every,
                runs: 0,
                last: None,
            }),
            timer: std::sync::OnceLock::new(),
        });
        let opening = refresh.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = opening.run().await {
                tracing::warn!(pane = %opening.pane, error = %e, "the pane's opening refresh ran nothing");
            }
        });
        tracing::info!(%pane, scope = artifact.scope().name(), name = artifact.name(), %command, ?every, "refreshing web pane");
        let mut refresher = Refresher {
            refresh,
            runs: Vec::new(),
        };
        refresher.owns(task.abort_handle());
        // The timer is one more task the pane owns, so a pane that closes or changes kind stops
        // ticking the same way it cancels a run: by this runner being dropped. The runner keeps the
        // handle as well as the pane, which is how a reader is told the timer is still there.
        if let Some(every) = every {
            let timer = tokio::spawn(ticking(refresher.refresh.clone(), every));
            let _ = refresher.refresh.timer.set(timer.abort_handle());
            refresher.owns(timer.abort_handle());
            tracing::info!(%pane, name = artifact.name(), every_secs = every, "refresh timer armed");
        }
        // The pane's earlier runner, if it had one, drops here: whatever was running for the
        // artifact it was showing is cancelled before the new one's first run can start.
        self.refreshers
            .lock()
            .unwrap()
            .insert(pane.to_string(), refresher);
    }

    /// Stop refreshing for pane `pane`: a run in flight is cancelled and its command killed.
    pub(crate) fn stop_refresh(&self, pane: &str) {
        if self.refreshers.lock().unwrap().remove(pane).is_some() {
            tracing::info!(%pane, "stopped refreshing web pane");
        }
    }

    /// Run web pane `pane`'s refresh now and give back the run. Refused when that pane has no
    /// runner: it shows no artifact, or its artifact declares no command.
    ///
    /// The run happens in a task the pane's runner owns, not on the caller's: a pane that closes
    /// or changes kind while this waits — on a person's answer, or on the command itself — cancels
    /// it, and this says so rather than reporting a run that no longer belongs to anything.
    pub(crate) async fn refresh_pane(&self, pane: &str) -> Result<RefreshInfo, String> {
        let task = {
            let mut refreshers = self.refreshers.lock().unwrap();
            let refresher = refreshers.get_mut(pane).ok_or_else(|| {
                format!("pane {pane} has no refresh: it shows no artifact, or that artifact's artifact.toml declares no command")
            })?;
            let refresh = refresher.refresh.clone();
            let task = tokio::spawn(async move { refresh.run().await });
            refresher.owns(task.abort_handle());
            task
        };
        match task.await {
            Ok(result) => result,
            Err(e) if e.is_cancelled() => Err(format!(
                "pane {pane} stopped showing its artifact, so its refresh was cancelled"
            )),
            Err(e) => Err(format!("pane {pane}'s refresh could not be run: {e}")),
        }
    }

    /// Every refreshing pane's runner, sorted by pane.
    pub(crate) fn refreshes(&self) -> Vec<RefreshInfo> {
        let mut all: Vec<RefreshInfo> = self
            .refreshers
            .lock()
            .unwrap()
            .values()
            .map(|r| r.refresh.info())
            .collect();
        all.sort_by(|a, b| a.pane.cmp(&b.pane));
        all
    }

    /// Every remembered approval.
    pub(crate) fn trusted(&self) -> Vec<TrustInfo> {
        self.trust.lock().unwrap().load()
    }

    /// Forget artifact `name`'s approvals, so its command asks again; gives back what is left.
    pub(crate) fn trust_forget(&self, name: &str) -> Result<Vec<TrustInfo>, String> {
        self.trust.lock().unwrap().forget(name)
    }
}

/// The runners, keyed by pane.
pub(crate) type Refreshers = HashMap<String, Refresher>;

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::ArtifactScope;

    fn folder(case: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wheel-trust-{}-{case}-{}",
            std::process::id(),
            unix_ms(SystemTime::now())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(dir: &str, command: &str, name: &str) -> TrustInfo {
        TrustInfo {
            dir: dir.into(),
            command: command.into(),
            scope: ArtifactScope::Shared,
            name: name.into(),
            approved_ms: 1,
        }
    }

    /// What the trust file answers: the pair as approved, nothing else, and an approval that can be
    /// withdrawn. The pair is the folder *and* the command, so editing the command asks again.
    #[test]
    fn trust_is_keyed_by_the_folder_and_the_command() {
        let instance = folder("pairs");
        let trust = TrustFile::new(&instance);
        assert!(trust.load().is_empty(), "nothing is trusted to start with");
        assert!(!trust.allows("/a/dash", "bd ready"));

        trust
            .remember(entry("/a/dash", "bd ready", "dash"))
            .unwrap();
        assert!(trust.allows("/a/dash", "bd ready"));
        assert!(
            !trust.allows("/a/dash", "bd ready --json"),
            "the command changed in the manifest"
        );
        assert!(
            !trust.allows("/b/dash", "bd ready"),
            "the same command in another artifact's folder"
        );

        // Approving the same pair again leaves one row; another command for the same folder is its
        // own row, so a manifest that goes back to an approved command does not ask again.
        trust
            .remember(entry("/a/dash", "bd ready", "dash"))
            .unwrap();
        trust
            .remember(entry("/a/dash", "bd ready --json", "dash"))
            .unwrap();
        assert_eq!(
            trust
                .load()
                .iter()
                .map(|t| t.command.clone())
                .collect::<Vec<_>>(),
            ["bd ready", "bd ready --json"]
        );

        // Forgetting is by artifact name, and takes every command remembered for it.
        trust
            .remember(entry("/c/viewer", "epg pull", "viewer"))
            .unwrap();
        assert_eq!(trust.forget("dash").unwrap().len(), 1);
        assert!(!trust.allows("/a/dash", "bd ready"));
        assert!(trust.allows("/c/viewer", "epg pull"));
        assert!(
            trust.forget("dash").is_err(),
            "nothing is remembered for it now"
        );

        // A file nothing can read trusts nothing, rather than everything — and a write refuses
        // rather than replacing approvals it could not see with the one it was given.
        let damaged = "{ not json";
        std::fs::write(instance.join(TRUST_FILE), damaged).unwrap();
        assert!(trust.load().is_empty());
        assert!(!trust.allows("/c/viewer", "epg pull"));
        assert!(trust.remember(entry("/d/new", "x", "new")).is_err());
        assert!(trust.forget("viewer").is_err());
        assert_eq!(
            std::fs::read_to_string(instance.join(TRUST_FILE)).unwrap(),
            damaged,
            "a file this cannot read is left alone"
        );
        std::fs::remove_dir_all(&instance).unwrap();
    }

    /// A command that runs a file in the project is never remembered, so an edit to the file asks
    /// again; a program on the search path is.
    #[test]
    fn a_command_running_a_project_file_is_not_remembered() {
        let dir = "/a/dash";
        for path in [
            "./refresh.sh",
            "scripts/pull.sh --all",
            "../x",
            "/a/dash/run.sh",
            "/proj/scripts/pull.sh",
            "FOO=1 ./refresh.sh",
        ] {
            assert!(runs_a_project_file(path, &[dir, "/proj"]), "{path}");
        }
        for program in [
            "bd ready",
            "make",
            "/usr/bin/env bd ready",
            "printf ./x",
            "/a/dashboard/x",
            "FOO=1 bd ready",
            "",
        ] {
            assert!(!runs_a_project_file(program, &[dir, "/proj"]), "{program}");
        }
        let instance = folder("script");
        let trust = TrustFile::new(&instance);
        trust.remember(entry(dir, "./refresh.sh", "dash")).unwrap();
        assert!(
            !trust.allows(dir, "./refresh.sh"),
            "a stored script approval is not honoured"
        );
        trust.remember(entry(dir, "bd ready", "dash")).unwrap();
        assert!(trust.allows(dir, "bd ready"));
        std::fs::remove_dir_all(&instance).unwrap();
    }

    /// A tail is the end of the output, and says when it left something off.
    #[test]
    fn the_error_tail_keeps_the_end_and_says_so() {
        assert_eq!(tail(b""), "");
        assert_eq!(tail(b"boom"), "boom");
        let long = "x".repeat(STDERR_TAIL) + "the end";
        let cut = tail(long.as_bytes());
        assert!(cut.starts_with('…'), "{cut:.20}");
        assert!(cut.ends_with("the end"));
        assert_eq!(cut.len(), STDERR_TAIL + '…'.len_utf8());
        // A cut landing inside a character takes the next whole one instead of a broken byte.
        let wide = "é".repeat(STDERR_TAIL);
        let cut = tail(wide.as_bytes());
        assert!(cut.ends_with("é"), "{cut:.20}");
        assert!(cut.chars().count() < STDERR_TAIL);
    }
}
