//! Starting, following and stopping runs.
//!
//! A run is `sh -c <command>` in the project's directory, tagged with `WHEEL_RUN_ID`, its output
//! written to the run's log one line at a time under a `HH:MM:SS` column (UTC). When the command
//! exits its children are swept, and the record gets the exit code. A watcher, a separate process
//! in its own session, holds the read end of a pipe whose write end only the owner keeps: when the
//! owner dies for any reason, SIGKILL included, the pipe closes and the watcher ends the run.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::config::Runner;
use crate::store::{self, Record, State};
use crate::sweep;

/// How long a swept process gets to end on SIGTERM before SIGKILL.
const GRACE: Duration = Duration::from_secs(3);

/// Makes an id unique among the runs this process starts in the same millisecond.
static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Called with each line of a run's output as it is logged.
pub type Mirror = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, PartialEq)]
pub enum StartError {
    /// The command asks first; start it again with `confirmed`.
    Confirm(String),
    Failed(String),
}

pub struct Started {
    pub record: Record,
    /// Ends when the run does, with its final record.
    pub done: JoinHandle<Record>,
}

/// The runs of one project.
#[derive(Clone)]
pub struct Runs {
    pub dir: String,
    folder: PathBuf,
    /// The program the watcher is: `wheel-runner`, run as `__watch`.
    watcher: PathBuf,
}

impl Runs {
    pub fn new(dir: &str) -> Self {
        Runs { dir: dir.to_string(), folder: store::folder(dir), watcher: std::env::current_exe().unwrap_or_else(|_| "wheel-runner".into()) }
    }

    /// Start `runner` unless it asks first and `confirmed` is false.
    pub fn start(&self, runner: &Runner, confirmed: bool, mirror: Option<Mirror>) -> Result<Started, StartError> {
        if let (Some(question), false) = (&runner.confirm, confirmed) {
            return Err(StartError::Confirm(question.clone()));
        }
        let failed = StartError::Failed;
        let started = crate::now();
        let safe: String = runner.name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let id = format!("{}-{}-{}-{}", crate::now_ms(), safe, std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let log = store::log_path(&self.folder, &id);
        std::fs::create_dir_all(&self.folder).map_err(|e| failed(e.to_string()))?;
        let log_file = File::create(&log).map_err(|e| failed(format!("{}: {e}", log.display())))?;
        let mut record = Record {
            id: id.clone(), name: runner.name.clone(), command: runner.command.clone(), dir: self.dir.clone(),
            state: State::Running, started, ended: None, exit_code: None, pid: 0, owner_pid: std::process::id(),
            stop_requested: false, log: log.to_string_lossy().into_owned(),
        };
        store::write(&self.folder, &record).map_err(failed)?;

        let mut watcher = Command::new(&self.watcher);
        watcher.arg("__watch").arg(&id).arg(&self.folder).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        // SAFETY: setsid runs between fork and exec and touches no memory of this process.
        unsafe { watcher.pre_exec(|| { libc::setsid(); Ok(()) }) };
        let mut watcher = match watcher.spawn() {
            Ok(w) => w,
            Err(e) => {
                self.end_unstarted(&mut record, 127);
                return Err(failed(format!("watcher {}: {e}", self.watcher.display())));
            }
        };

        let mut child = Command::new("sh");
        child.arg("-c").arg(&runner.command).current_dir(&self.dir).env(sweep::ENV, &id).env("WHEEL_RUNNER_NAME", &runner.name)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
        let mut child = match child.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.end_unstarted(&mut record, 127);
                drop(watcher.stdin.take());
                let _ = watcher.wait();
                return Err(failed(format!("sh: {e}")));
            }
        };
        record.pid = child.id();
        let _ = store::write(&self.folder, &record);
        let log_file = Arc::new(Mutex::new(log_file));
        let (pumped, pump_ends) = std::sync::mpsc::channel();
        let pump = |from: Box<dyn Read + Send>| {
            let (log, mirror, ended) = (log_file.clone(), mirror.clone(), pumped.clone());
            std::thread::spawn(move || {
                let mut lines = BufReader::new(from);
                let mut buf = Vec::new();
                while matches!(lines.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
                    let text = String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_string();
                    buf.clear();
                    let line = format!("{}  {text}", crate::clock(crate::now()));
                    // A panic in a mirror (a closed terminal) must not poison the log for the other pump.
                    let _ = writeln!(log.lock().unwrap_or_else(|e| e.into_inner()), "{line}");
                    if let Some(m) = &mirror {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m(&line)));
                    }
                }
                let _ = ended.send(());
            })
        };
        pump(Box::new(child.stdout.take().unwrap()));
        pump(Box::new(child.stderr.take().unwrap()));
        drop(pumped);

        let folder = self.folder.clone();
        let done_record = record.clone();
        let done = std::thread::spawn(move || {
            let mut record = done_record;
            let status = child.wait();
            // The command is over; whatever it started goes with it. The sweep closes the pipes a
            // detached child still held, so the pumps end.
            sweep::sweep(&record.id, Some(record.pid), GRACE);
            // Both pumps end once every holder of the pipes is gone. A holder the sweep could not
            // find (it hides its environment and left the group) must not hold the record open.
            for _ in 0..2 {
                if pump_ends.recv_timeout(Duration::from_secs(2)).is_err() {
                    break;
                }
            }
            let stopped = stop_marker(&folder, &record.id).exists();
            record.ended = Some(crate::now());
            record.exit_code = status.ok().map(|s| s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)));
            record.stop_requested = stopped;
            record.state = if stopped { State::Stopped } else { State::Exited };
            let _ = store::write(&folder, &record);
            // Closing the pipe tells the watcher the run is over; it ends, and is collected.
            drop(watcher.stdin.take());
            let _ = watcher.wait();
            record
        });
        Ok(Started { record, done })
    }

    /// A run that never started: its record says it ended with `code`.
    fn end_unstarted(&self, record: &mut Record, code: i32) {
        record.state = State::Exited;
        record.ended = Some(crate::now());
        record.exit_code = Some(code);
        let _ = store::write(&self.folder, record);
    }

    /// Every run, newest first.
    pub fn list(&self) -> Vec<Record> {
        store::all(&self.folder)
    }

    pub fn get(&self, id: &str) -> Result<Record, String> {
        store::read(&self.folder, id)
    }

    /// The log of run `id` from byte `offset` (or its last `tail` lines): the text and the offset
    /// to ask from next.
    pub fn log(&self, id: &str, offset: u64, tail: Option<usize>) -> Result<(String, u64), String> {
        let record = self.get(id)?;
        let bytes = std::fs::read(&record.log).map_err(|e| format!("{}: {e}", record.log))?;
        let end = bytes.len() as u64;
        let text = String::from_utf8_lossy(&bytes[(offset.min(end)) as usize..]).into_owned();
        Ok(match tail {
            Some(n) => {
                let lines: Vec<&str> = text.lines().collect();
                let from = lines.len().saturating_sub(n);
                (lines[from..].iter().map(|l| format!("{l}\n")).collect(), end)
            }
            None => (text, end),
        })
    }

    /// Ask run `id` to stop: its processes are ended, and the record says it was stopped.
    pub fn stop(&self, id: &str) -> Result<Record, String> {
        let record = self.get(id)?;
        if record.state != State::Running {
            return Err(format!("run {id} is not running"));
        }
        // A marker file, not a rewrite of the record: the run's owner is the only writer of that.
        File::create(stop_marker(&self.folder, id)).map_err(|e| e.to_string())?;
        sweep::sweep(id, Some(record.pid), GRACE);
        Ok(record)
    }
}

fn stop_marker(folder: &Path, id: &str) -> PathBuf {
    folder.join(format!("{id}.stop"))
}

/// `wheel-runner __watch <id> <folder>`: block until stdin closes, then end run `id` if its record
/// still says it is running.
pub fn watch(id: &str, folder: &Path) {
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
    if let Ok(record) = store::read(folder, id) {
        if record.state == State::Running {
            sweep::sweep(id, Some(record.pid), GRACE);
            // Read again: a stop, or the run itself, may have finished while the sweep ran.
            if let Ok(mut record) = store::read(folder, id).map(|mut r| { r.stop_requested |= stop_marker(folder, id).exists(); r }) {
                if record.state == State::Running {
                    record.state = if record.stop_requested { State::Stopped } else { State::Lost };
                    record.ended = Some(crate::now());
                    let _ = store::write(folder, &record);
                }
            }
        }
    }
}
