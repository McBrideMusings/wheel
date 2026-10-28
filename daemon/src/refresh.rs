//! Running an artifact's refresh command for the pane showing it.
//!
//! The artifact's entry in a `wheel.toml` declares the command; the daemon runs it through `/bin/sh` in the pane's *space*
//! directory and hands it the pane's data directory in `WHEEL_DATA_DIR`, which is where an artifact
//! writes what its page reads: the artifact's own folder is read-only static content a pane serves,
//! and a project's working tree is not for generated files. That is a place to write, not a
//! sandbox — the command is a shell the entry declares, and it can reach whatever the person can. A run
//! happens when the pane opens, when a caller asks for one, and — when the entry declares
//! `every` — on a timer the pane owns, which runs the same [`Refresh`] and has no way of its own to
//! run a command.
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

use wheel_proto::{ArtifactRef, RefreshInfo, RunInfo, TimerState};

use crate::artifacts::Entry;
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

/// What a reader of a runner sees: what it would run now, and what the last run did.
struct Seen {
    command: String,
    every: Option<u64>,
    /// The folder the entry's page really sat in at the last read.
    dir: String,
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
    /// The `wheel.toml` holding the artifact's entry, read afresh every run: the command, the
    /// interval and the folder all come from it as it is now.
    entry: PathBuf,
    /// The pane's data directory: the one place the command is given to write.
    data: PathBuf,
    /// The program each run starts as its warden: `wheeld` itself.
    warden: PathBuf,
    /// The daemon's PATH, which each run's command is given as it starts.
    path: Arc<Mutex<wheel_proto::LoginPath>>,
    /// Where `wheel` reaches this daemon, and the instance directory holding its bin: what a run's
    /// command is given so it can call `wheel`.
    control: Option<crate::shell::Control>,
    instance_dir: PathBuf,
    /// One run at a time per pane, so two shells never write one data directory and an on-demand
    /// run during the on-open one waits rather than racing it.
    running: tokio::sync::Mutex<()>,
    seen: Mutex<Seen>,
    /// Why the last tick skipped, until a tick runs again: a skip is logged when this changes, so a
    /// pane left open on an entry with no command writes one line rather than one per tick.
    skipped: Mutex<Option<&'static str>>,
    /// This pane's timer task, when the entry asked for one as the pane opened. Its liveness is
    /// what a reader is told about the timer, so a timer that is gone is never reported as ticking.
    timer: std::sync::OnceLock<tokio::task::AbortHandle>,
}

/// Record why a tick skipped; true when that differs from the last skip, so it is worth a log line.
fn note_skip(last: &mut Option<&'static str>, why: &'static str) -> bool {
    last.replace(why) != Some(why)
}

impl Refresh {
    /// Run the command the entry declares now. The record it returns is the one `status` reports.
    ///
    /// The entry is read afresh every run, so editing `command` changes what runs rather than
    /// running whatever the pane opened with. An error only when there is nothing to run at all; a
    /// command that failed is a successful read of a failed run.
    pub(crate) async fn run(&self) -> Result<RefreshInfo, String> {
        let _one = self.running.lock().await;
        let (command, _) = self.declared()?;
        let started = self.start(&command);
        self.finish(&command, started).await;
        Ok(self.info().await)
    }

    /// One timer tick: the same run as any other. A tick that cannot run skips — it is not a run,
    /// so nothing about it is recorded as one — and says why, rather than ending the timer: the
    /// next tick runs as soon as the entry declares a command again.
    ///
    /// Taking the pane's run lock first is what keeps a tick from overlapping a run: a tick landing
    /// during an on-demand run waits for it, and reads the entry afresh once it has the lock.
    async fn tick(&self) {
        let _one = self.running.lock().await;
        let skip = |why: &'static str| {
            if note_skip(&mut self.skipped.lock().unwrap(), why) {
                tracing::info!(pane = %self.pane, entry = %self.entry.display(), %why, "refresh timer skipped a tick");
            }
        };
        let Ok((command, _)) = self.declared() else {
            return skip(
                "the entry declares no refresh command any more, or its page is not there",
            );
        };
        if self.seen.lock().unwrap().every.is_none() {
            return skip("the entry does not ask for a refresh on a timer any more");
        }
        *self.skipped.lock().unwrap() = None;
        let started = self.start(&command);
        self.finish(&command, started).await;
    }

    /// What this pane's timer would do if it ticked now: read from the entry and
    /// the timer task itself every time, so what a reader is told is the timer's state now and
    /// never an outcome a later edit has outgrown.
    fn timer(&self) -> TimerState {
        let Some(task) = self.timer.get() else {
            return TimerState::Off;
        };
        if task.is_finished() {
            return TimerState::Stopped;
        }
        let Ok((Entry { command, every, .. }, _)) = self.definition() else {
            return TimerState::Idle;
        };
        match (command, every) {
            (Some(_), Some(_)) => TimerState::Ticking,
            _ => TimerState::Idle,
        }
    }

    /// The artifact's entry as it is now, and the folder its page really sits in: an edited
    /// command, interval or path is what runs — and what is asked about — at the next run, rather
    /// than whatever the pane opened with.
    fn definition(&self) -> Result<(Entry, PathBuf), String> {
        let entry =
            crate::artifacts::entry(&self.entry, self.artifact.name())?.ok_or_else(|| {
                format!(
                    "{} has no entry for {} any more",
                    self.entry.display(),
                    self.artifact.name()
                )
            })?;
        let dir = entry.locate()?.root;
        Ok((entry, dir))
    }

    /// What the entry declares to run now, and the folder its page sits in, recorded for a
    /// reader along with the interval it asks for. An error when it declares no command at all, or
    /// its page is not anywhere: there is nothing to run.
    fn declared(&self) -> Result<(String, PathBuf), String> {
        let (Entry { command, every, .. }, dir) = self.definition()?;
        let Some(command) = command else {
            return Err(format!(
                "the entry for {} in {} declares no refresh command, so there is nothing to run for pane {}",
                self.artifact.name(),
                self.entry.display(),
                self.pane
            ));
        };
        let mut seen = self.seen.lock().unwrap();
        seen.command = command.clone();
        seen.every = every;
        seen.dir = dir.display().to_string();
        Ok((command, dir))
    }

    /// Mark a run as starting: when it started, for a reader, and the clock its duration comes off.
    fn start(&self, command: &str) -> (u64, Instant) {
        tracing::debug!(pane = %self.pane, scope = self.artifact.scope().name(), name = self.artifact.name(), %command, cwd = %self.cwd, data = %self.data.display(), "refresh start");
        (unix_ms(SystemTime::now()), Instant::now())
    }

    /// Run `command` and record what happened as this runner's last run.
    async fn finish(&self, command: &str, started: (u64, Instant)) {
        let (started_ms, at) = started;
        let (exit, error, stderr_tail) = self.exec(command).await;
        let last = RunInfo {
            started_ms,
            duration_ms: at.elapsed().as_millis() as u64,
            exit,
            error,
            stderr_tail,
        };
        // A run that worked is routine; only a failed one is news at info.
        if last.ok() {
            tracing::debug!(pane = %self.pane, %command, exit = ?last.exit, duration_ms = last.duration_ms, stderr_bytes = last.stderr_tail.len(), "refresh end");
        } else {
            tracing::info!(pane = %self.pane, %command, exit = ?last.exit, error = last.error.as_deref().unwrap_or("-"), duration_ms = last.duration_ms, stderr_bytes = last.stderr_tail.len(), "refresh end");
        }
        let mut seen = self.seen.lock().unwrap();
        seen.runs += 1;
        seen.last = Some(last);
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
            .envs(crate::shell::command_env(
                self.control.as_ref(),
                &self.instance_dir,
                &self.path.lock().unwrap().path,
            ))
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
        tracing::debug!(pane = %self.pane, stdout_bytes = out.total, stderr_bytes = err.total, %status, "refresh command finished");
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
    pub(crate) async fn info(&self) -> RefreshInfo {
        let timer = self.timer();
        let seen = self.seen.lock().unwrap();
        RefreshInfo {
            pane: self.pane.clone(),
            scope: self.artifact.scope(),
            name: self.artifact.name().to_string(),
            dir: seen.dir.clone(),
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
/// an entry that lost its command, an `every` that came back — is a tick that
/// skips or runs and never a timer that cannot come back.
///
/// One sequential task, so at most one tick is ever pending: a run that outlasts its own interval
/// delays the next tick rather than being overlapped by it, and ticks do not pile up behind it. The
/// interval comes from each tick's own reading of the entry, so editing `every` takes effect at
/// the tick after the edit, and the first tick is one interval after the pane's opening run.
async fn ticking(refresh: Arc<Refresh>, mut every: u64) {
    loop {
        tokio::time::sleep(Duration::from_secs(every)).await;
        refresh.tick().await;
        // The interval the entry asks for now; the one last read when it asks for none, so that
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
        tracing::debug!(pane = %pane, pgid, "refresh warden armed");
        Ok(group)
    }

    fn kill(&mut self) {
        if self.pgid <= 0 {
            return;
        }
        // The warden is one member; a group of only the warden had nothing left to kill, which is
        // every run that ended on its own.
        let left = group_members(self.pgid).saturating_sub(1);
        // SAFETY: killpg takes two integers and touches no memory of ours. The id is the warden's
        // pid, and the warden is not reaped until this group is dropped, so no other process can
        // have been given it. The warden is a member, so the group always has one to kill.
        let alive = unsafe { libc::killpg(self.pgid, libc::SIGKILL) } == 0;
        if alive && left > 0 {
            tracing::info!(pane = %self.pane, pgid = self.pgid, left, "refresh process group killed");
        } else if alive {
            tracing::debug!(pane = %self.pane, pgid = self.pgid, "refresh process group had nothing left to kill");
        }
        self.pgid = 0;
    }
}

/// How many processes are in the process group `pgid`. Where the system gives no count, a large one,
/// so a caller that only asks whether anything besides the warden is left logs the kill.
#[cfg(target_os = "macos")]
fn group_members(pgid: i32) -> usize {
    const PROC_PGRP_ONLY: u32 = 2;
    // A size-only call (null buffer) counts every process on the machine, not the group's, so the
    // list is read into a buffer; a run's group is a handful of processes.
    let mut pids = [0 as libc::pid_t; 1024];
    // SAFETY: the buffer is `pids`, and the size passed is its size in bytes.
    let bytes = unsafe {
        libc::proc_listpids(
            PROC_PGRP_ONLY,
            pgid as u32,
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(&pids) as libc::c_int,
        )
    };
    usize::try_from(bytes).map_or(usize::MAX, |bytes| {
        bytes / std::mem::size_of::<libc::pid_t>()
    })
}

#[cfg(not(target_os = "macos"))]
fn group_members(_pgid: i32) -> usize {
    usize::MAX
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
    /// opening. Replaces any earlier runner of that pane. An artifact whose entry declares no
    /// command registers nothing, so nothing to run is nothing to report. Call from inside the
    /// tokio runtime.
    pub(crate) fn start_refresh(
        &self,
        pane: &str,
        artifact: &ArtifactRef,
        cwd: &str,
        data: PathBuf,
    ) {
        // The folder the pane serves from, and nothing else: an artifact whose page is not anywhere
        // serves nothing, so its command never runs either.
        let entry = self.artifact_registry(artifact.scope(), cwd);
        let (Entry { command, every, .. }, dir) = match self.artifact_entry(artifact, cwd).and_then(
            |found| {
                let found = found.ok_or("no entry for it")?;
                let dir = found.locate()?.root;
                Ok((found, dir))
            },
        ) {
            Ok(defined) => defined,
            Err(e) => {
                tracing::warn!(%pane, scope = artifact.scope().name(), name = artifact.name(), error = %e, "the artifact's page is not anywhere; nothing refreshes this pane");
                self.stop_refresh(pane);
                return;
            }
        };
        let Some(command) = command else {
            self.stop_refresh(pane);
            return;
        };
        let refresh = Arc::new(Refresh {
            pane: pane.to_string(),
            artifact: artifact.clone(),
            cwd: cwd.to_string(),
            entry,
            data,
            warden: self.warden.clone(),
            path: self.login_path.clone(),
            control: self.control.lock().unwrap().clone(),
            instance_dir: self.instance_dir.clone(),
            running: tokio::sync::Mutex::new(()),
            seen: Mutex::new(Seen {
                command: command.clone(),
                every,
                dir: dir.display().to_string(),
                runs: 0,
                last: None,
            }),
            skipped: Mutex::new(None),
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
    ///
    /// The runner is found and the run started before this returns, so a request queued behind
    /// this one on the same connection finds the run already owned by the pane; only the wait for
    /// the result is left to the returned future.
    pub(crate) fn refresh_pane(
        &self,
        pane: &str,
    ) -> Result<impl std::future::Future<Output = Result<RefreshInfo, String>>, String> {
        let task = {
            let mut refreshers = self.refreshers.lock().unwrap();
            let refresher = refreshers.get_mut(pane).ok_or_else(|| {
                format!("pane {pane} has no refresh: it shows no artifact, or that artifact's entry declares no command")
            })?;
            let refresh = refresher.refresh.clone();
            let task = tokio::spawn(async move { refresh.run().await });
            refresher.owns(task.abort_handle());
            task
        };
        let pane = pane.to_string();
        Ok(async move {
            match task.await {
                Ok(result) => result,
                Err(e) if e.is_cancelled() => Err(format!(
                    "pane {pane} stopped showing its artifact, so its refresh was cancelled"
                )),
                Err(e) => Err(format!("pane {pane}'s refresh could not be run: {e}")),
            }
        })
    }

    /// Every refreshing pane's runner, sorted by pane. Each runner is read after the runners' lock
    /// is let go: reading one stats the files its command names, and hashes a changed one, and a pane opening or closing
    /// meanwhile must not wait on that.
    pub(crate) async fn refreshes(&self) -> Vec<RefreshInfo> {
        let runners: Vec<Arc<Refresh>> = self
            .refreshers
            .lock()
            .unwrap()
            .values()
            .map(|r| r.refresh.clone())
            .collect();
        let mut all = Vec::with_capacity(runners.len());
        for runner in &runners {
            all.push(runner.info().await);
        }
        all.sort_by(|a, b| a.pane.cmp(&b.pane));
        all
    }
}

/// The runners, keyed by pane.
pub(crate) type Refreshers = HashMap<String, Refresher>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A repeated skip reason logs once; a changed reason, or the same one after a run cleared it,
    /// logs again.
    #[test]
    fn a_skip_logs_when_its_reason_changes() {
        let mut last = None;
        assert!(note_skip(&mut last, "a"));
        assert!(!note_skip(&mut last, "a"));
        assert!(!note_skip(&mut last, "a"));
        assert!(note_skip(&mut last, "b"));
        last = None;
        assert!(note_skip(&mut last, "b"));
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
