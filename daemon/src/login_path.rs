//! The PATH the person's terminals have, as their interactive login shell sets it. launchd starts
//! wheeld with PATH=/usr/bin:/bin:/usr/sbin:/sbin, where neither an agent's program nor most tools
//! a terminal user expects are found. wheeld asks the login shell before anything else runs, and
//! holds the answer as state: every agent lookup, terminal, agent process and refresh command is
//! given it. A shell that ran out of time is asked again every [`RETRY`] until it answers, since a
//! Mac busy at login can make a shell that normally takes a second take more than [`LIMIT`].
//!
//! The shell's startup files are the person's, so they may wait on input or start something that
//! never exits, and an interactive zsh ignores SIGTERM: the shell runs in a session of its own,
//! with no terminal and stdin on /dev/null, and its whole process group is killed with SIGKILL
//! once it has printed PATH, after [`LIMIT`], or when it exits.

use std::io::Read as _;
use std::os::fd::AsRawFd as _;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long the shell has to print PATH.
pub const LIMIT: Duration = Duration::from_secs(4);
/// How long after a shell ran out of time it is asked again.
pub const RETRY: Duration = Duration::from_secs(10);
/// Startup files that print without end never reach PATH; reading stops after this much.
const MOST: usize = 256 * 1024;
/// Brackets PATH in the shell's output, so whatever its startup files print is told apart.
const MARK: u8 = 0x1e;

/// Why a shell gave no PATH, and whether that was running out of time, which asking again can fix.
#[derive(Debug)]
pub struct Failed {
    pub why: String,
    pub timed_out: bool,
}

/// The shell to ask: `$SHELL`, which launchd sets for an agent from the user's account.
pub fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

/// Ask `shell` for PATH as [`probe`] does, on the `try_`th try, and say how it went as `status`
/// reports it, with `own` (the PATH wheeld was started with) in force when the shell gave none.
/// True when it ran out of time and is worth asking again. It runs before logging starts, so it
/// logs nothing itself: the caller logs what it returns.
pub fn ask(shell: &str, own: &str, try_: u32) -> (wheel_proto::LoginPath, bool) {
    let start = Instant::now();
    let on_try = match try_ {
        1 => String::new(),
        n => format!(" on try {n}"),
    };
    match probe(shell, LIMIT) {
        Ok(path) => {
            let how = format!(
                "{shell} -il gave it in {} ms{on_try}",
                start.elapsed().as_millis()
            );
            (
                wheel_proto::LoginPath {
                    path,
                    how,
                    problem: None,
                },
                false,
            )
        }
        Err(failed) => {
            let again = match failed.timed_out {
                true => format!("; asking again every {} s", RETRY.as_secs()),
                false => String::new(),
            };
            let login_path = wheel_proto::LoginPath {
                path: own.to_string(),
                how: "the PATH wheeld was started with, because the login shell gave none".into(),
                problem: Some(format!("{}{on_try}{again}", failed.why)),
            };
            (login_path, failed.timed_out)
        }
    }
}

/// Ask `shell -il` for PATH; blocks for up to `limit`. Why not, when it gave none.
pub fn probe(shell: &str, limit: Duration) -> Result<String, Failed> {
    let failed = |why: String| Failed {
        why,
        timed_out: false,
    };
    let name = Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(shell)
        .to_string();
    let mut command = Command::new(shell);
    command
        .args(["-ilc", r#"printf '\036%s\036' "$PATH""#])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory of this process. A new session
    // makes the shell lead its own group, with no terminal to wait on.
    unsafe {
        command.pre_exec(|| match libc::setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    let start = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|e| failed(format!("could not start {shell}: {e}")))?;
    let pid = child.id() as libc::pid_t;
    let watch = Watchdog::start(pid);
    let mut out = child.stdout.take().expect("stdout is piped");

    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let answer = loop {
        let parts: Vec<&[u8]> = bytes.split(|b| *b == MARK).collect();
        if parts.len() >= 3 {
            break Ok(String::from_utf8_lossy(parts[1]).into_owned());
        }
        if bytes.len() > MOST {
            break Err(failed(format!("{name} -il printed over 256 KB without PATH, so it was stopped; something in its startup files prints without end")));
        }
        let Some(left) = limit.checked_sub(start.elapsed()).filter(|l| !l.is_zero()) else {
            break Err(Failed {
                why: format!("{name} -il did not finish within {} s, so it was stopped; something in its startup files waits for input, or the Mac was too busy", limit.as_secs()),
                timed_out: true,
            });
        };
        let mut poller = libc::pollfd {
            fd: out.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = left.as_millis().clamp(1, i32::MAX as u128) as i32;
        // SAFETY: one pollfd, owned by this frame, for a descriptor `out` keeps open.
        let ready = unsafe { libc::poll(&mut poller, 1, millis) };
        if ready < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break Err(failed(format!(
                "wheeld could not wait for {name} -il to print PATH: {e}"
            )));
        }
        if ready == 0 {
            continue;
        }
        match out.read(&mut chunk) {
            Ok(0) => break Err(failed(format!("{name} -il exited without printing PATH, so where it looks for commands is unknown"))),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => break Err(failed(format!("wheeld could not read what {name} -il printed: {e}"))),
        }
    };
    // The shell may be done while something its startup files began is not; the child is not
    // reaped until after the signal, so its pid, and the group's, cannot have been reused.
    // SAFETY: a signal to a process group this function made.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = child.wait();
    if let Some(watch) = watch {
        watch.stand_down();
    }
    answer
}

/// A forked process that kills the shell's group if wheeld dies before [`Watchdog::stand_down`]:
/// the shell leads a session of its own, so nothing else takes it down with wheeld.
struct Watchdog {
    pid: libc::pid_t,
    stop: libc::c_int,
}

impl Watchdog {
    /// Fork the watcher of process group `group`; None when a pipe or the fork failed.
    fn start(group: libc::pid_t) -> Option<Watchdog> {
        let mut ends = [0 as libc::c_int; 2];
        // SAFETY: `ends` holds the two descriptors pipe writes.
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            return None;
        }
        let [read, write] = ends;
        // Close-on-exec keeps the write end out of every process wheeld starts later, which
        // would otherwise hold the pipe open past wheeld's death.
        // SAFETY: fcntl on descriptors this function just made.
        unsafe {
            libc::fcntl(read, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(write, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        // SAFETY: the child calls only close, read, kill and _exit, which are async-signal-safe,
        // as fork in a process with other threads requires.
        match unsafe { libc::fork() } {
            -1 => {
                // SAFETY: closing the descriptors made above.
                unsafe {
                    libc::close(read);
                    libc::close(write);
                }
                None
            }
            0 => unsafe {
                libc::close(write);
                let mut byte = 0u8;
                loop {
                    match libc::read(read, (&mut byte as *mut u8).cast(), 1) {
                        1 => libc::_exit(0),
                        0 => break,
                        _ if *libc::__error() == libc::EINTR => {}
                        _ => break,
                    }
                }
                libc::kill(-group, libc::SIGKILL);
                libc::_exit(0)
            },
            pid => {
                // SAFETY: closing the read end the parent does not use.
                unsafe { libc::close(read) };
                Some(Watchdog { pid, stop: write })
            }
        }
    }

    /// The shell's group is already stopped: tell the watcher to leave it alone, and reap it.
    fn stand_down(self) {
        // SAFETY: writes one byte to, then closes, the descriptor this struct owns; waitpid
        // reaps the child this struct forked.
        unsafe {
            libc::write(self.stop, [0u8].as_ptr().cast(), 1);
            libc::close(self.stop);
            let mut status = 0;
            libc::waitpid(self.pid, &mut status, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_that_never_prints_path_is_stopped_with_its_group() {
        use std::os::unix::fs::PermissionsExt as _;
        let shell = std::env::temp_dir().join(format!("wheel-login-path-{}", std::process::id()));
        std::fs::write(&shell, "#!/bin/sh\nsleep 4731 & sleep 4732\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let start = Instant::now();
        let failed = probe(&shell.display().to_string(), Duration::from_millis(500)).unwrap_err();
        std::fs::remove_file(&shell).unwrap();
        assert!(failed.timed_out, "{failed:?}");
        assert!(start.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(100));
        let left = Command::new("pgrep")
            .args(["-f", "sleep 473[12]"])
            .output()
            .unwrap();
        assert!(left.stdout.is_empty(), "left running: {left:?}");
    }
    #[test]
    fn a_wheeld_killed_mid_probe_takes_the_shells_group_with_it() {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(shell) = std::env::var("WHEEL_LOGIN_PATH_PROBE_SHELL") {
            let _ = probe(&shell, Duration::from_secs(30));
            return;
        }
        let shell = std::env::temp_dir().join(format!("wheel-login-kill-{}", std::process::id()));
        std::fs::write(&shell, "#!/bin/sh\nsleep 4741 & sleep 4742\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut probing = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "login_path::tests::a_wheeld_killed_mid_probe_takes_the_shells_group_with_it",
            ])
            .env("WHEEL_LOGIN_PATH_PROBE_SHELL", &shell)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let running = Command::new("pgrep")
            .args(["-f", "sleep 474[12]"])
            .output()
            .unwrap();
        assert!(!running.stdout.is_empty(), "the shell never started");
        probing.kill().unwrap();
        probing.wait().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let left = Command::new("pgrep")
            .args(["-f", "sleep 474[12]"])
            .output()
            .unwrap();
        std::fs::remove_file(&shell).unwrap();
        assert!(left.stdout.is_empty(), "left running: {left:?}");
    }
}
