//! An app artifact's process: the program the daemon starts, owns and stops for the web pane that
//! shows the artifact.
//!
//! The artifact's entry in a `wheel.toml` names the command in `app` and, in `restart`, when it
//! starts again. The daemon runs it through `/bin/sh` in the pane's *space* directory, in the same
//! kind of warden-led process group a refresh runs in, so closing the pane — or the daemon dying —
//! ends the shell and everything it started. One supervisor task per pane owns the process: it
//! starts it when the pane shows the artifact, restarts it by the entry's policy, and stops it when
//! the pane drops its handle.
//!
//! The program announces its port by writing it as a line of decimal digits to the descriptor named
//! in `WHEEL_APP_PORT_FD` (3): a pipe the daemon reads, so the announcement is an event rather than
//! a file polled for. A program that binds port 0 gets a port nothing else holds, which is how two
//! apps never collide. Output (standard output and error) goes to a log per run under the
//! instance directory, removed with the pane.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use wheel_proto::{AppAction, AppExit, AppInfo, AppRestart, AppState, ArtifactRef};

use crate::refresh::Group;
use crate::server::AppState as Daemon;

/// Numbers each app started, so an app that replaces another in the same pane keeps its own logs.
static NEXT_INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The descriptor number a program finds its announcement pipe at.
const PORT_FD: i32 = 3;

/// How much of a run's output one log file holds. When the file would pass this, it becomes the
/// run's `.log.old` (replacing the one before) and a new file starts, so a run keeps its last
/// `LOG_SEGMENT` to twice that in bytes and a chatty app cannot fill the disk.
const LOG_SEGMENT: u64 = 2 << 20;

/// How long a run that has ended keeps reading output a process that left its group still sends.
const PIPE_GRACE: Duration = Duration::from_secs(1);

/// A run's log: the current file and the size written to it.
struct Rotating {
    file: std::fs::File,
    path: PathBuf,
    old: PathBuf,
    size: u64,
}

impl Rotating {
    fn create(path: PathBuf, old: PathBuf) -> std::io::Result<Rotating> {
        Ok(Rotating {
            file: std::fs::File::create(&path)?,
            path,
            old,
            size: 0,
        })
    }

    fn write(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        if self.size > 0 && self.size + chunk.len() as u64 > LOG_SEGMENT {
            std::fs::rename(&self.path, &self.old)?;
            self.file = std::fs::File::create(&self.path)?;
            self.size = 0;
        }
        self.size += chunk.len() as u64;
        self.file.write_all(chunk)
    }
}

/// Copy a pipe into the run's log until it closes.
async fn copy_log(mut pipe: impl tokio::io::AsyncRead + Unpin, sink: Arc<Mutex<Rotating>>) {
    use tokio::io::AsyncReadExt;
    let mut chunk = [0u8; 8192];
    while let Ok(n) = pipe.read(&mut chunk).await {
        if n == 0 || sink.lock().unwrap().write(&chunk[..n]).is_err() {
            break;
        }
    }
}

/// A run that lasted this long was not a crash loop: the delay before the next restart starts over.
const STEADY: Duration = Duration::from_secs(10);

/// The longest the daemon waits before starting a failing app again.
const RESTART_MAX: u64 = 30;

/// How long a control request waits for the supervisor to carry it out.
const CONTROL_WAIT: Duration = Duration::from_secs(10);

/// What a control request asks the supervisor, with the channel it answers on once done.
type Request = (AppAction, oneshot::Sender<()>);

/// What a reader sees of one pane's app.
struct Seen {
    command: String,
    restart: AppRestart,
    state: AppState,
    pid: Option<u32>,
    port: Option<u16>,
    runs: u64,
    started_ms: Option<u64>,
    last_exit: Option<AppExit>,
    /// When the restart delay ends, while the state is `waiting`.
    restart_at: Option<Instant>,
}

/// One pane's app, shared between its supervisor and the readers.
pub(crate) struct App {
    pane: String,
    artifact: ArtifactRef,
    cwd: String,
    /// The `wheel.toml` holding the entry, read afresh at every start.
    entry: PathBuf,
    data: PathBuf,
    /// `app-logs/<pane>/` under the instance directory: `<run>.log` for each run.
    logs: PathBuf,
    warden: PathBuf,
    path: Arc<Mutex<wheel_proto::LoginPath>>,
    control: Option<crate::shell::Control>,
    instance_dir: PathBuf,
    requests: mpsc::UnboundedSender<Request>,
    seen: Mutex<Seen>,
}

/// A pane's app and the task supervising it. Dropping it ends the task, which drops the process
/// group with it, and removes the logs.
pub(crate) struct Supervised {
    app: Arc<App>,
    task: tokio::task::AbortHandle,
}

impl Drop for Supervised {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.app.logs);
        if let Some(pane_dir) = self.app.logs.parent() {
            let _ = std::fs::remove_dir(pane_dir);
        }
    }
}

/// The apps, keyed by pane.
pub(crate) type Apps = HashMap<String, Supervised>;

fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Seconds to wait before the restart that follows `failures` short runs in a row.
fn restart_delay(failures: u32) -> u64 {
    (1u64 << failures.saturating_sub(1).min(8)).min(RESTART_MAX)
}

/// Whether a run that ended as `exit` is started again under `policy`.
fn restarts(policy: AppRestart, exit: &AppExit) -> bool {
    match policy {
        AppRestart::Always => true,
        AppRestart::Never => false,
        AppRestart::OnFailure => exit.exit != Some(0),
    }
}

impl App {
    /// This app as a reader sees it.
    pub(crate) fn info(&self) -> AppInfo {
        let seen = self.seen.lock().unwrap();
        AppInfo {
            pane: self.pane.clone(),
            scope: self.artifact.scope(),
            name: self.artifact.name().to_string(),
            command: seen.command.clone(),
            cwd: self.cwd.clone(),
            restart: seen.restart,
            state: seen.state,
            pid: seen.pid,
            port: seen.port,
            runs: seen.runs,
            started_ms: seen.started_ms,
            last_exit: seen.last_exit.clone(),
            restart_in_secs: seen
                .restart_at
                .map(|at| at.saturating_duration_since(Instant::now()).as_secs() + 1),
            log: (seen.runs > 0).then(|| self.log_file(seen.runs).display().to_string()),
        }
    }

    fn log_file(&self, run: u64) -> PathBuf {
        self.logs.join(format!("{run}.log"))
    }

    /// The earlier half of a run's output, once it has written more than [`LOG_SEGMENT`].
    fn old_log_file(&self, run: u64) -> PathBuf {
        self.logs.join(format!("{run}.log.old"))
    }

    /// Ask the supervisor to start, stop or restart the process, and wait until it has.
    pub(crate) async fn control(&self, action: AppAction) -> Result<AppInfo, String> {
        let (done, wait) = oneshot::channel();
        self.requests
            .send((action, done))
            .map_err(|_| format!("pane {}'s app is gone", self.pane))?;
        tokio::time::timeout(CONTROL_WAIT, wait)
            .await
            .map_err(|_| {
                format!(
                    "pane {}'s app did not answer within {}s",
                    self.pane,
                    CONTROL_WAIT.as_secs()
                )
            })?
            .map_err(|_| format!("pane {}'s app is gone", self.pane))?;
        Ok(self.info())
    }

    /// The output of run `run` (the current or last when None), cut to its last `tail` lines.
    pub(crate) fn logs(
        &self,
        run: Option<u64>,
        tail: Option<usize>,
    ) -> Result<(u64, String), String> {
        let run = run.unwrap_or_else(|| self.seen.lock().unwrap().runs);
        if run == 0 {
            return Err(format!("pane {}'s app has not been started", self.pane));
        }
        let mut bytes = std::fs::read(self.old_log_file(run)).unwrap_or_default();
        bytes.extend(
            std::fs::read(self.log_file(run))
                .map_err(|e| format!("no log for run {run} of pane {}: {e}", self.pane))?,
        );
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let text = match tail {
            Some(n) => {
                let lines: Vec<&str> = text.lines().collect();
                let from = lines.len().saturating_sub(n);
                let mut out = lines[from..].join("\n");
                if !out.is_empty() {
                    out.push('\n');
                }
                out
            }
            None => text,
        };
        Ok((run, text))
    }

    /// The entry's app command and restart policy as they are now.
    fn declared(&self) -> Result<(String, AppRestart), String> {
        let entry =
            crate::artifacts::entry(&self.entry, self.artifact.name())?.ok_or_else(|| {
                format!(
                    "{} has no entry for {} any more",
                    self.entry.display(),
                    self.artifact.name()
                )
            })?;
        let command = entry.app.ok_or_else(|| {
            format!(
                "the entry for {} declares no app any more",
                self.artifact.name()
            )
        })?;
        Ok((command, entry.restart.unwrap_or_default()))
    }

    fn set_state(&self, state: AppState) {
        let mut seen = self.seen.lock().unwrap();
        seen.state = state;
        seen.restart_at = None;
        if matches!(state, AppState::Stopped | AppState::Waiting) {
            seen.pid = None;
            seen.port = None;
        }
    }
}

/// How one run ended, and what the supervisor does next.
enum Ended {
    /// The process ended on its own.
    Exited(AppExit),
    /// A stop request ended it.
    Stopped,
    /// A restart request ended it.
    Restart,
}

/// Run the app's process once and report how it ended. Acknowledges the request that asked for the
/// start, if any, as soon as the process has its pid. A request that ends the run leaves its
/// acknowledgement in `ack` for the caller, who answers it once the app's state shows the result.
async fn run_once(
    app: &Arc<App>,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    ack: &mut Option<oneshot::Sender<()>>,
) -> Result<Ended, AppExit> {
    let fail = |error: String| AppExit {
        exit: None,
        error: Some(error),
    };
    let (command, restart) = app.declared().map_err(&fail)?;
    let run = {
        let mut seen = app.seen.lock().unwrap();
        seen.command = command.clone();
        seen.restart = restart;
        seen.runs + 1
    };
    std::fs::create_dir_all(&app.logs)
        .map_err(|e| fail(format!("make {}: {e}", app.logs.display())))?;
    std::fs::create_dir_all(&app.data)
        .map_err(|e| fail(format!("make {}: {e}", app.data.display())))?;
    let sink = Arc::new(Mutex::new(
        Rotating::create(app.log_file(run), app.old_log_file(run))
            .map_err(|e| fail(format!("make the log {}: {e}", app.log_file(run).display())))?,
    ));
    let group = Group::arm(&app.pane, &app.warden).await.map_err(&fail)?;
    let (sender, receiver) =
        tokio::net::unix::pipe::pipe().map_err(|e| fail(format!("make the port pipe: {e}")))?;
    let write = sender
        .into_blocking_fd()
        .map_err(|e| fail(format!("make the port pipe: {e}")))?;
    let write_fd = write.as_raw_fd();
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(&command)
        .current_dir(&app.cwd)
        .envs(crate::shell::command_env(
            app.control.as_ref(),
            &app.instance_dir,
            &app.path.lock().unwrap().path,
        ))
        .env("WHEEL_PANE", &app.pane)
        .env("WHEEL_SPACE_DIR", &app.cwd)
        .env("WHEEL_DATA_DIR", &app.data)
        .env("WHEEL_APP_PORT_FD", PORT_FD.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(group.pgid)
        .kill_on_drop(true);
    // SAFETY: the closure runs between fork and exec and makes one async-signal-safe call on two
    // descriptors that are open in the child: the pipe's write end and the number it is given.
    unsafe {
        cmd.pre_exec(move || {
            if write_fd == PORT_FD {
                // Already in place: only the close-on-exec flag has to go.
                if libc::fcntl(PORT_FD, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(write_fd, PORT_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let spawned = cmd.spawn();
    // The daemon's copy of the write end closes now, so the pipe ends when the program's does.
    drop(write);
    let mut child = spawned.map_err(|e| fail(format!("run {command:?}: {e}")))?;
    let copies: Vec<_> = [
        child
            .stdout
            .take()
            .map(|p| tokio::spawn(copy_log(p, sink.clone()))),
        child
            .stderr
            .take()
            .map(|p| tokio::spawn(copy_log(p, sink.clone()))),
    ]
    .into_iter()
    .flatten()
    .collect();
    {
        let mut seen = app.seen.lock().unwrap();
        seen.runs = run;
        seen.pid = child.id();
        seen.port = None;
        seen.started_ms = Some(unix_ms(SystemTime::now()));
        seen.state = AppState::Starting;
        seen.restart_at = None;
    }
    tracing::info!(pane = %app.pane, name = app.artifact.name(), %command, run, pid = ?child.id(), "app started");
    if let Some(ack) = ack.take() {
        let _ = ack.send(());
    }
    let mut lines = BufReader::new(receiver).lines();
    let mut announcing = true;
    let mut group = group;
    let ended = loop {
        tokio::select! {
            status = child.wait() => {
                group.kill();
                break Ended::Exited(match status {
                    Ok(status) => match status.code() {
                        Some(code) => AppExit { exit: Some(code), error: None },
                        None => AppExit {
                            exit: None,
                            error: Some(format!(
                                "killed by signal {}",
                                std::os::unix::process::ExitStatusExt::signal(&status).unwrap_or(0)
                            )),
                        },
                    },
                    Err(e) => fail(format!("wait for {command:?}: {e}")),
                });
            }
            line = lines.next_line(), if announcing => match line {
                Ok(Some(line)) => match line.trim().parse::<u16>() {
                    Ok(port) if port > 0 => {
                        let mut seen = app.seen.lock().unwrap();
                        seen.port = Some(port);
                        seen.state = AppState::Running;
                        tracing::info!(pane = %app.pane, port, "app announced its port");
                    }
                    _ => tracing::warn!(pane = %app.pane, %line, "app wrote something other than a port to its announcement descriptor"),
                },
                _ => announcing = false,
            },
            request = requests.recv() => match request {
                // The app is gone: its pane dropped it.
                None => break Ended::Stopped,
                Some((AppAction::Start, done)) => {
                    let _ = done.send(());
                }
                Some((action, done)) => {
                    group.kill();
                    let _ = child.wait().await;
                    *ack = Some(done);
                    break if action == AppAction::Stop { Ended::Stopped } else { Ended::Restart };
                }
            },
        }
    };
    // What the program wrote before it ended is in the log before the run is reported over.
    for mut copy in copies {
        if tokio::time::timeout(PIPE_GRACE, &mut copy).await.is_err() {
            copy.abort();
        }
    }
    Ok(ended)
}

/// Answer the request waiting in `ack`, if any.
fn answer(ack: &mut Option<oneshot::Sender<()>>) {
    if let Some(ack) = ack.take() {
        let _ = ack.send(());
    }
}

/// Own the process for as long as the pane does.
async fn supervise(app: Arc<App>, mut requests: mpsc::UnboundedReceiver<Request>) {
    let mut failures = 0u32;
    let mut ack: Option<oneshot::Sender<()>> = None;
    let mut wanted = true;
    loop {
        if !wanted {
            match requests.recv().await {
                None => return,
                Some((AppAction::Stop, done)) => {
                    let _ = done.send(());
                    continue;
                }
                Some((_, done)) => {
                    wanted = true;
                    ack = Some(done);
                }
            }
        }
        let began = Instant::now();
        let ended = run_once(&app, &mut requests, &mut ack).await;
        let exit = match ended {
            Ok(Ended::Stopped) => {
                app.set_state(AppState::Stopped);
                wanted = false;
                answer(&mut ack);
                continue;
            }
            // The request that restarts is answered by the start that follows it.
            Ok(Ended::Restart) => {
                failures = 0;
                continue;
            }
            Ok(Ended::Exited(exit)) | Err(exit) => exit,
        };
        tracing::info!(pane = %app.pane, exit = ?exit.exit, error = exit.error.as_deref().unwrap_or("-"), ran_ms = began.elapsed().as_millis() as u64, "app ended");
        let policy = {
            let mut seen = app.seen.lock().unwrap();
            seen.last_exit = Some(exit.clone());
            seen.restart
        };
        // A run that never started because the entry lost its app has nothing to start again.
        let declared = app.declared().is_ok();
        if !declared || !restarts(policy, &exit) {
            app.set_state(AppState::Stopped);
            wanted = false;
            answer(&mut ack);
            continue;
        }
        failures = if began.elapsed() >= STEADY {
            1
        } else {
            failures + 1
        };
        let wait = restart_delay(failures);
        {
            let mut seen = app.seen.lock().unwrap();
            seen.state = AppState::Waiting;
            seen.pid = None;
            seen.port = None;
            seen.restart_at = Some(Instant::now() + Duration::from_secs(wait));
        }
        tracing::info!(pane = %app.pane, wait_secs = wait, "app waits to restart");
        // A start that failed to start is answered now: the app's state says why.
        answer(&mut ack);
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(wait)) => {}
            request = requests.recv() => match request {
                None => return,
                Some((AppAction::Stop, done)) => {
                    app.set_state(AppState::Stopped);
                    wanted = false;
                    let _ = done.send(());
                }
                Some((_, done)) => ack = Some(done),
            },
        }
    }
}

impl Daemon {
    /// Start the app of web pane `pane`, which shows `artifact` in a space whose directory is
    /// `cwd`, when the artifact's entry declares one. A pane already running that artifact's app
    /// keeps it; any other app of the pane is stopped. Call from inside the tokio runtime.
    pub(crate) fn start_app(&self, pane: &str, artifact: &ArtifactRef, cwd: &str, data: PathBuf) {
        let declared = self.artifact_entry(artifact, cwd).ok().flatten();
        let Some((command, restart)) = declared
            .as_ref()
            .and_then(|e| e.app.clone().map(|a| (a, e.restart.unwrap_or_default())))
        else {
            self.stop_app(pane);
            return;
        };
        if self
            .apps
            .lock()
            .unwrap()
            .get(pane)
            .is_some_and(|s| s.app.artifact == *artifact && s.app.cwd == cwd)
        {
            return;
        }
        let (requests, receiver) = mpsc::unbounded_channel();
        let app = Arc::new(App {
            pane: pane.to_string(),
            artifact: artifact.clone(),
            cwd: cwd.to_string(),
            entry: self.artifact_registry(artifact.scope(), cwd),
            data,
            logs: self.instance_dir.join("app-logs").join(pane).join(
                NEXT_INSTANCE
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    .to_string(),
            ),
            warden: self.warden.clone(),
            path: self.login_path.clone(),
            control: self.control.lock().unwrap().clone(),
            instance_dir: self.instance_dir.clone(),
            requests,
            seen: Mutex::new(Seen {
                command,
                restart,
                state: AppState::Starting,
                pid: None,
                port: None,
                runs: 0,
                started_ms: None,
                last_exit: None,
                restart_at: None,
            }),
        });
        let task = tokio::spawn(supervise(app.clone(), receiver));
        tracing::info!(%pane, scope = artifact.scope().name(), name = artifact.name(), "app artifact shown; starting its process");
        // The pane's earlier app, if it had one, drops here and its process group is killed.
        self.apps.lock().unwrap().insert(
            pane.to_string(),
            Supervised {
                app,
                task: task.abort_handle(),
            },
        );
    }

    /// Stop pane `pane`'s app and forget it: the process and its children are killed.
    pub(crate) fn stop_app(&self, pane: &str) {
        if self.apps.lock().unwrap().remove(pane).is_some() {
            tracing::info!(%pane, "stopped web pane's app");
        }
    }

    /// Pane `pane`'s app, or why it has none.
    pub(crate) fn app_of(&self, pane: &str) -> Result<Arc<App>, String> {
        self.apps
            .lock()
            .unwrap()
            .get(pane)
            .map(|s| s.app.clone())
            .ok_or_else(|| {
                format!("pane {pane} runs no app: it shows no artifact, or that artifact's entry declares no app")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run's log never holds more than two segments: the file that passed the limit becomes
    /// `.log.old`, the one before it is replaced, and the newest bytes are always kept.
    #[test]
    fn a_runs_log_keeps_its_last_two_segments() {
        let dir = std::env::temp_dir().join(format!("wheel-applog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, old) = (dir.join("1.log"), dir.join("1.log.old"));
        let mut log = Rotating::create(path.clone(), old.clone()).unwrap();
        let chunk = vec![b'a'; (LOG_SEGMENT as usize * 3) / 4];
        for n in 0..6u8 {
            log.write(&chunk).unwrap();
            log.write(&[b'0' + n]).unwrap();
        }
        let (new, older) = (
            std::fs::metadata(&path).unwrap().len(),
            std::fs::metadata(&old).unwrap().len(),
        );
        assert!(new <= LOG_SEGMENT && older <= LOG_SEGMENT, "{new} {older}");
        assert_eq!(std::fs::read(&path).unwrap().last(), Some(&b'5'));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
