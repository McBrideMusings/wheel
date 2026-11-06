//! Ending everything a run started. Every run's command is started with `WHEEL_RUN_ID=<id>` in
//! its environment, which every child inherits through fork, exec, `setsid` and reparenting. A
//! dev server that detaches (Node's `detached: true`) escapes any process-group kill, so a run is
//! ended by finding each process that carries its id, not by walking groups or parents.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};

pub const ENV: &str = "WHEEL_RUN_ID";

/// The pids carrying run `id`, except this process and its ancestors.
pub fn tagged(sys: &mut System, id: &str) -> Vec<Pid> {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
    );
    let mut protected = HashSet::new();
    let mut at = Some(Pid::from_u32(std::process::id()));
    while let Some(p) = at {
        if !protected.insert(p) {
            break;
        }
        at = sys.process(p).and_then(|p| p.parent());
    }
    let want = format!("{ENV}={id}");
    sys.processes()
        .iter()
        .filter(|(pid, p)| !protected.contains(pid) && p.environ().iter().any(|e| e.to_str() == Some(want.as_str())))
        .map(|(pid, _)| *pid)
        .collect()
}

/// Signal process group `group` (the run's leader started it) when it is a group of its own.
fn signal_group(group: Option<u32>, signal: i32) {
    if let Some(g) = group.filter(|g| *g > 1) {
        // SAFETY: kill only sends a signal; the group is the one this run's leader made.
        if g as i32 != unsafe { libc::getpgrp() } {
            unsafe { libc::kill(-(g as i32), signal) };
        }
    }
}

fn group_alive(group: Option<u32>) -> bool {
    // SAFETY: signal 0 only checks that the group exists.
    group.filter(|g| *g > 1).is_some_and(|g| unsafe { libc::kill(-(g as i32), 0) } == 0)
}

/// End every process of run `id`: SIGTERM, up to `grace` for them to go, then SIGKILL. The run's
/// process group goes first, which reaches programs macOS withholds the environment of (its own
/// protected `sleep`, `sh`); the tag reaches what left the group. Rescans up to three times, so a
/// supervisor that respawns a child is caught. Returns the pids signalled by tag.
pub fn sweep(id: &str, group: Option<u32>, grace: Duration) -> Vec<u32> {
    let mut sys = System::new();
    let mut signalled = vec![];
    for _ in 0..3 {
        let pids = tagged(&mut sys, id);
        if pids.is_empty() && !group_alive(group) {
            break;
        }
        signal_group(group, libc::SIGTERM);
        for pid in &pids {
            if let Some(p) = sys.process(*pid) {
                p.kill_with(Signal::Term);
            }
            signalled.push(pid.as_u32());
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            if tagged(&mut sys, id).is_empty() && !group_alive(group) {
                break;
            }
        }
        signal_group(group, libc::SIGKILL);
        for pid in tagged(&mut sys, id) {
            if let Some(p) = sys.process(pid) {
                p.kill();
            }
        }
    }
    signalled
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// A copy of `sleep` outside the system folders: macOS withholds the environment of its own
    /// protected programs, so a tagged `/bin/sleep` could not be found.
    fn sleeper(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-runner-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::copy("/bin/sleep", &path).unwrap();
        // A copy of a platform binary is killed on launch until it carries its own signature.
        assert!(Command::new("codesign").args(["--force", "-s", "-"]).arg(&path).stderr(Stdio::null()).status().unwrap().success());
        path
    }

    #[test]
    fn a_tagged_process_is_found_and_ended_and_an_untagged_one_is_left() {
        let id = format!("test-{}", std::process::id());
        let mut tagged_child = Command::new(sleeper("sleep-a")).arg("31001").env(ENV, &id).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let mut other = Command::new(sleeper("sleep-b")).arg("31002").env(ENV, "other-run").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let signalled = sweep(&id, None, Duration::from_secs(2));
        assert_eq!(signalled, [tagged_child.id()]);
        assert!(tagged_child.wait().is_ok());
        assert!(other.try_wait().unwrap().is_none(), "a process of another run was ended");
        other.kill().unwrap();
        other.wait().unwrap();
    }
}
