//! Per-command status checks: the readout `status = { kind = … }` on a `[runner]` entry names.
//! Every check is bounded by [`TIMEOUT`], so a hung host or script delays one readout only.

use std::fs::File;
use std::io::{BufRead, Read};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::OnceLock;
use std::time::Duration;

use crate::config::StatusCheck;

pub const TIMEOUT: Duration = Duration::from_secs(5);

/// One readout: whether the check succeeded and the text to show.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Reading {
    pub up: bool,
    pub text: String,
}

fn down(text: impl ToString) -> Reading {
    Reading { up: false, text: text.to_string() }
}

pub fn run(check: &StatusCheck, cwd: &Path) -> Reading {
    match check {
        StatusCheck::Http { url } => match get(url) {
            Ok(_) => Reading { up: true, text: "up".into() },
            Err(e) => down(e),
        },
        StatusCheck::Json { url, pull } => match get(url).and_then(|body| pull_path(&body, pull)) {
            Ok(text) => Reading { up: true, text },
            Err(e) => down(e),
        },
        StatusCheck::Command { run } => command(run, cwd),
    }
}

fn get(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let code = resp.status().as_u16();
    if !(200..400).contains(&code) {
        return Err(format!("HTTP {code}"));
    }
    resp.body_mut().with_config().limit(16 << 20).read_to_string().map_err(|e| e.to_string())
}

/// The value at a dotted path (`data.0.name`) of a JSON document, as text.
fn pull_path(body: &str, path: &str) -> Result<String, String> {
    let doc: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    let mut at = &doc;
    for key in path.split('.').filter(|k| !k.is_empty()) {
        at = match at {
            serde_json::Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get(i)),
            other => other.get(key),
        }
        .ok_or_else(|| format!("no {path} in the answer"))?;
    }
    Ok(match at {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// The most of each output stream a bounded command keeps; the rest is read and dropped.
pub const OUTPUT_CAP: usize = 4 << 20;

/// What a bounded shell command left: its exit status (None when it was killed at the limit, by
/// a cancel or by a signal) and what it wrote to each stream, each cut at [`OUTPUT_CAP`] bytes.
pub struct Bounded {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// A [`Wake::Cancel`] ended it.
    pub cancelled: bool,
    /// Standard output passed [`OUTPUT_CAP`] and lost its end.
    pub cut: bool,
}

impl Bounded {
    /// The first line of standard output.
    pub fn first_line(&self) -> &str {
        self.stdout.lines().next().unwrap_or("")
    }
}

/// What ends a bounded command's wait: its exit, or a caller that no longer wants it.
pub enum Wake {
    Exited,
    Cancel,
}

/// Read a stream to its end on a thread, keeping the first [`OUTPUT_CAP`] bytes; true when it cut.
fn drain(mut from: impl Read + Send + 'static) -> std::thread::JoinHandle<(String, bool)> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut cut = false;
        let mut buf = [0u8; 8192];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            let room = OUTPUT_CAP.saturating_sub(kept.len());
            cut |= n > room;
            kept.extend_from_slice(&buf[..n.min(room)]);
        }
        (String::from_utf8_lossy(&kept).into_owned(), cut)
    })
}

/// Run `sh -c run` in `cwd` for at most `limit`, with `env` added to the environment. `wake` is a
/// channel the caller made: a [`Wake::Cancel`] sent on a clone of its sender ends the command at
/// once. Both streams are read on threads, so a command that fills a pipe cannot block, and the
/// exit is awaited by a thread blocked in `waitid`, so no poll runs. The command's whole process
/// group is killed before the command is reaped, so the group id cannot belong to another process
/// yet, and a background child holding a pipe cannot outlast the command. When [`guard`] started a
/// guard, the group is named to it from the command's own process before `sh` runs, and struck
/// from it before the command is reaped. Git never asks for credentials.
pub fn bounded_with(
    run: &str,
    cwd: &Path,
    env: &[(String, String)],
    limit: Duration,
    wake: (Sender<Wake>, Receiver<Wake>),
) -> Result<Bounded, String> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(run).current_dir(cwd).env("GIT_TERMINAL_PROMPT", "0")
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    if let Some(fd) = GUARD.get().map(|g| g.as_raw_fd()) {
        // SAFETY: runs between fork and exec, after the child has its own group, and only calls
        // getpid and write on the guard's pipe, which exec then closes (it is close-on-exec).
        unsafe { command.pre_exec(move || { tell(fd, b'+', libc::getpid()); Ok(()) }) };
    }
    let mut child = command.spawn().map_err(|e| format!("sh: {e}"))?;
    let pid = child.id();
    let pgid = pid as i32;
    let out = drain(child.stdout.take().expect("piped"));
    let err = drain(child.stderr.take().expect("piped"));
    let (tx, rx) = wake;
    std::thread::spawn(move || {
        // SAFETY: waitid with WNOWAIT only reports the child's exit and leaves it to be reaped.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
        let _ = tx.send(Wake::Exited);
    });
    let (timed_out, cancelled) = match rx.recv_timeout(limit) {
        Ok(Wake::Exited) => (false, false),
        Ok(Wake::Cancel) => (false, true),
        Err(_) => (true, false),
    };
    // SAFETY: killpg only signals the group the child leads; the child is not reaped yet.
    unsafe { libc::killpg(pgid, libc::SIGKILL) };
    if let Some(fd) = GUARD.get().map(|g| g.as_raw_fd()) {
        tell(fd, b'-', pgid);
    }
    let status = child.wait();
    let code = if timed_out || cancelled { None } else { status.ok().and_then(|s| s.code()) };
    let (stdout, cut) = out.join().unwrap_or_default();
    let (stderr, _) = err.join().unwrap_or_default();
    Ok(Bounded { code, stdout, stderr, timed_out, cancelled, cut })
}

/// [`bounded_with`] with no added environment and no way to cancel.
pub fn bounded(run: &str, cwd: &Path, limit: Duration) -> Result<Bounded, String> {
    bounded_with(run, cwd, &[], limit, channel())
}

/// The write end of the pipe the guard reads, once [`guard`] started it.
static GUARD: OnceLock<File> = OnceLock::new();

/// Start the guard, `wheel-runner __guard`, for a process that serves bounded commands. Each
/// command runs in its own process group, outside this process's group, so when `wheeld` kills
/// that group, or this process dies any other way, nothing here can end the commands. The guard
/// runs in its own session and reads a pipe only this process writes: it learns each command's
/// group as the command starts and forgets it as the command ends, and once the pipe closes it
/// kills every group it still holds.
pub fn guard() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("the guard's program: {e}"))?;
    let mut guard = Command::new(&exe);
    guard.arg("__guard").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid runs between fork and exec and touches no memory of this process.
    unsafe { guard.pre_exec(|| { libc::setsid(); Ok(()) }) };
    let mut child = guard.spawn().map_err(|e| format!("guard {}: {e}", exe.display()))?;
    let pipe = File::from(OwnedFd::from(child.stdin.take().expect("piped")));
    GUARD.set(pipe).map_err(|_| "the guard is already running".to_string())
}

/// Write `+<group>` or `-<group>` and a newline to the guard's pipe in one `write`, which is
/// atomic at this size. Allocates nothing and takes no lock, so a child may call it before exec.
fn tell(fd: RawFd, sign: u8, group: i32) {
    let mut line = [0u8; 16];
    let mut digits = [0u8; 12];
    let (mut n, mut at) = (group.unsigned_abs(), 0);
    loop {
        digits[at] = b'0' + (n % 10) as u8;
        at += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    line[0] = sign;
    for k in 0..at {
        line[1 + k] = digits[at - 1 - k];
    }
    line[1 + at] = b'\n';
    // SAFETY: writes from a buffer on this stack to a descriptor this process holds open.
    unsafe { libc::write(fd, line.as_ptr().cast(), 2 + at) };
}

/// `wheel-runner __guard`: hold the groups standard input names (`+<group>` adds, `-<group>`
/// removes) until it closes, then kill each group still held.
pub fn hold(from: impl BufRead) {
    let mut groups = std::collections::BTreeSet::new();
    for line in from.lines() {
        let Ok(line) = line else { break };
        let (sign, group) = line.split_at(line.len().min(1));
        match (sign, group.parse::<i32>()) {
            ("+", Ok(g)) if g > 1 => {
                groups.insert(g);
            }
            ("-", Ok(g)) => {
                groups.remove(&g);
            }
            _ => {}
        }
    }
    for g in groups {
        // SAFETY: killpg only signals the group a command of the closed process led.
        unsafe { libc::killpg(g, libc::SIGKILL) };
    }
}

fn command(run: &str, cwd: &Path) -> Reading {
    match bounded(run, cwd, TIMEOUT) {
        Err(e) => down(e),
        Ok(b) if b.timed_out => down(format!("timed out after {}s", TIMEOUT.as_secs())),
        Ok(b) if b.code == Some(0) => Reading { up: true, text: if b.first_line().is_empty() { "up".into() } else { b.first_line().to_string() } },
        Ok(b) => down(if b.first_line().is_empty() { format!("exit {}", b.code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into())) } else { b.first_line().to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_dotted_path_reads_objects_and_arrays() {
        assert_eq!(pull_path(r#"{"a":{"b":[{"c":"x"},2]}}"#, "a.b.0.c").unwrap(), "x");
        assert_eq!(pull_path(r#"{"a":{"b":[{"c":"x"},2]}}"#, "a.b.1").unwrap(), "2");
        assert!(pull_path(r#"{"a":1}"#, "a.b").is_err());
        assert!(pull_path("nope", "a").is_err());
    }

    #[test]
    fn a_command_check_ends_at_the_limit_even_when_a_child_holds_the_pipe() {
        let tmp = std::env::temp_dir();
        let t = Instant::now();
        let b = bounded("sleep 30 & echo up", &tmp, Duration::from_secs(2)).unwrap();
        assert!(t.elapsed() < Duration::from_secs(10), "took {:?}", t.elapsed());
        assert_eq!((b.code, b.first_line()), (Some(0), "up"));
        let b = bounded("sleep 30", &tmp, Duration::from_millis(300)).unwrap();
        assert!(b.timed_out);
        let b = bounded("yes | head -c 300000; echo end", &tmp, Duration::from_secs(5)).unwrap();
        assert!(!b.timed_out && b.code == Some(0));
        let b = bounded_with("echo out; echo \"$X\" >&2; exit 3", &tmp, &[("X".into(), "it's \"quoted\"".into())], Duration::from_secs(5), channel()).unwrap();
        assert_eq!((b.code, b.stdout.as_str(), b.stderr.as_str()), (Some(3), "out\n", "it's \"quoted\"\n"));
        let b = bounded("yes | head -c 6000000", &tmp, Duration::from_secs(10)).unwrap();
        assert_eq!((b.stdout.len(), b.cut), (OUTPUT_CAP, true));
        let (tx, rx) = channel();
        let cancel = tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let _ = cancel.send(Wake::Cancel);
        });
        let t = Instant::now();
        let b = bounded_with("sleep 30", &tmp, &[], Duration::from_secs(20), (tx, rx)).unwrap();
        assert!(b.cancelled && b.code.is_none() && t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    #[test]
    fn the_guard_kills_the_groups_still_held_when_its_pipe_closes() {
        let spawn = |secs: &str| Command::new("sleep").arg(secs).process_group(0).spawn().unwrap();
        let (mut held, mut struck) = (spawn("31451"), spawn("31452"));
        let (r, w) = std::io::pipe().unwrap();
        let w = File::from(OwnedFd::from(w));
        tell(w.as_raw_fd(), b'+', held.id() as i32);
        tell(w.as_raw_fd(), b'+', struck.id() as i32);
        tell(w.as_raw_fd(), b'-', struck.id() as i32);
        drop(w);
        hold(std::io::BufReader::new(r));
        assert_eq!(held.wait().unwrap().code(), None, "the held group is killed");
        assert!(struck.try_wait().unwrap().is_none(), "the struck group runs on");
        struck.kill().unwrap();
        struck.wait().unwrap();
    }

    #[test]
    fn a_command_check_reads_exit_and_first_line() {
        let tmp = std::env::temp_dir();
        assert_eq!(run(&StatusCheck::Command { run: "echo v1.2; echo more".into() }, &tmp), Reading { up: true, text: "v1.2".into() });
        assert_eq!(run(&StatusCheck::Command { run: "echo broken; exit 3".into() }, &tmp), Reading { up: false, text: "broken".into() });
        assert_eq!(run(&StatusCheck::Command { run: "exit 4".into() }, &tmp), Reading { up: false, text: "exit 4".into() });
    }
}
