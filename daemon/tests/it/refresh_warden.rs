//! The refresh warden kills its process group when the process that started it exits.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// True while `pid` is a process that has not exited (a zombie counts as exited).
fn running(pid: &str) -> bool {
    let out = Command::new("ps").args(["-o", "stat=", "-p", pid]).output().unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    !stat.trim().is_empty() && !stat.trim_start().starts_with('Z')
}

/// Kills a process group when dropped, so a test that fails part-way leaves no `sleep 300` behind.
struct KillGroup(libc::pid_t);

impl Drop for KillGroup {
    fn drop(&mut self) {
        // SAFETY: a signal to the group this test made; a group already gone returns ESRCH.
        unsafe { libc::killpg(self.0, libc::SIGKILL) };
    }
}

#[test]
fn the_group_dies_with_the_wardens_parent() {
    // The shell leads a new group holding the warden and a sleep, then waits on stdin.
    let mut shell = Command::new("sh")
        .arg("-c")
        .arg(r#""$0" --refresh-warden & sleep 300 & echo "sleep $!"; read _"#)
        .arg(env!("CARGO_BIN_EXE_wheeld"))
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let _group = KillGroup(shell.id() as libc::pid_t);
    let mut lines = BufReader::new(shell.stdout.take().unwrap()).lines();
    let (mut armed, mut sleep) = (false, None);
    while !armed || sleep.is_none() {
        let line = lines.next().expect("shell output ended early").unwrap();
        match line.strip_prefix("sleep ") {
            Some(pid) => sleep = Some(pid.to_string()),
            None => armed |= line == "armed",
        }
    }
    let sleep = sleep.unwrap();
    assert!(running(&sleep));

    shell.kill().unwrap();
    shell.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while running(&sleep) {
        assert!(Instant::now() < deadline, "sleep {sleep} outlived the warden's parent");
        std::thread::sleep(Duration::from_millis(50));
    }
}
