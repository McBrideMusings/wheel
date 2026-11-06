//! Ending everything a run started. Every run's command is started with `WHEEL_RUN_ID=<id>` in
//! its environment, which every child inherits through fork, exec, `setsid` and reparenting. A
//! dev server that detaches (Node's `detached: true`) escapes any process-group kill, so a run is
//! ended by finding each process that carries its id, not by walking groups or parents.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};

pub const ENV: &str = "WHEEL_RUN_ID";

/// This process and its ancestors, which a sweep never signals.
fn protected(sys: &System) -> HashSet<Pid> {
    let mut protected = HashSet::new();
    let mut at = Some(Pid::from_u32(std::process::id()));
    while let Some(p) = at {
        if !protected.insert(p) {
            break;
        }
        at = sys.process(p).and_then(|p| p.parent());
    }
    protected
}

/// The pids carrying run `id`, except this process and its ancestors.
pub fn tagged(sys: &mut System, id: &str) -> Vec<Pid> {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
    );
    let protected = protected(sys);
    let want = format!("{ENV}={id}");
    sys.processes()
        .iter()
        .filter(|(pid, p)| !protected.contains(pid) && p.environ().iter().any(|e| e.to_str() == Some(want.as_str())))
        .map(|(pid, _)| *pid)
        .collect()
}

/// A process seen below a run's command while the command lived: its pid and when it started, so
/// a later process given the same pid is told from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub pid: u32,
    pub started: u64,
}

/// Every process below the run's command `root` by parent links, none when the process holding
/// that pid is not the command (it started at another time than `root.started`). macOS withholds
/// the environment of its own protected programs but not their parent, so this reaches a `sleep`
/// that later detaches into its own session, where neither the tag nor the process group finds it.
/// A process whose start time cannot be read is left out: it could not be told from a reused pid.
pub fn descendants(sys: &mut System, root: Leader) -> Vec<Seen> {
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let top = Pid::from_u32(root.pid);
    if root.started != 0 && sys.process(top).is_none_or(|p| p.start_time() != root.started) {
        return vec![];
    }
    let mut children: std::collections::HashMap<Pid, Vec<Pid>> = std::collections::HashMap::new();
    for (pid, p) in sys.processes() {
        if let Some(parent) = p.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    let mut found = vec![];
    let mut frontier = vec![top];
    while let Some(parent) = frontier.pop() {
        for pid in children.get(&parent).into_iter().flatten() {
            let started = sys.process(*pid).map(|p| p.start_time()).unwrap_or(0);
            if started != 0 && found.len() < MAX_SEEN {
                found.push(Seen { pid: pid.as_u32(), started });
                frontier.push(*pid);
            }
        }
    }
    found
}

/// The most processes noted below one run.
const MAX_SEEN: usize = 4096;

/// Whether the process `s` names is still running.
pub fn still_running(sys: &System, s: &Seen) -> bool {
    sys.process(Pid::from_u32(s.pid)).is_some_and(|p| p.start_time() == s.started)
}

/// The pids a sweep of run `id` signals: the tagged ones, and the `seen` ones still running.
fn reach(sys: &mut System, id: &str, seen: &[Seen]) -> Vec<Pid> {
    let mut pids = tagged(sys, id);
    let protected = protected(sys);
    for s in seen.iter().filter(|s| s.started != 0) {
        let pid = Pid::from_u32(s.pid);
        if !pids.contains(&pid) && !protected.contains(&pid) && still_running(sys, s) {
            pids.push(pid);
        }
    }
    pids
}

/// The process that leads a run's process group, and when it started.
#[derive(Clone, Copy, Debug)]
pub struct Leader {
    pub pid: u32,
    /// Seconds since the Unix epoch; 0 when unknown.
    pub started: u64,
}

/// When process `pid` started, in seconds since the Unix epoch; 0 when it cannot be read.
pub fn started(pid: u32) -> u64 {
    let mut sys = System::new();
    let p = Pid::from_u32(pid);
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, ProcessRefreshKind::nothing());
    sys.process(p).map(|p| p.start_time()).unwrap_or(0)
}

fn pgid_of(pid: u32) -> i32 {
    // SAFETY: getpgid only reads.
    unsafe { libc::getpgid(pid as i32) }
}

/// Whether process group `leader.pid` is still this run's. The kernel hands a process id out
/// again only once no process group has that id, so a group that exists while its leader process
/// does not is the run's own, left behind by the command. While a process with that id exists, it
/// is the run's leader when it started when the run did, or when a member of the group carries the
/// run's tag; otherwise the id was reused. (macOS withholds the tag of its own protected programs,
/// so a leader that is one has only its start time to go by.)
fn group_is_ours(sys: &mut System, id: &str, leader: Leader) -> bool {
    let g = leader.pid;
    // SAFETY: getpgrp only reads.
    if g <= 1 || g as i32 == unsafe { libc::getpgrp() } {
        return false;
    }
    let tagged_members = tagged(sys, id);
    if sys.process(Pid::from_u32(g)).is_none() {
        return true;
    }
    tagged_members.iter().any(|p| pgid_of(p.as_u32()) == g as i32)
        || (leader.started != 0 && pgid_of(g) == g as i32 && sys.process(Pid::from_u32(g)).is_some_and(|p| p.start_time() == leader.started))
}

/// Signal the run's process group when it is still the run's.
fn signal_group(sys: &mut System, id: &str, group: Option<Leader>, signal: i32) {
    if let Some(l) = group.filter(|l| group_is_ours(sys, id, *l)) {
        // SAFETY: kill only sends a signal; the group was just checked to hold this run.
        unsafe { libc::kill(-(l.pid as i32), signal) };
    }
}

fn group_alive(sys: &mut System, id: &str, group: Option<Leader>) -> bool {
    // SAFETY: signal 0 only checks that the group exists.
    group.is_some_and(|l| group_is_ours(sys, id, l) && unsafe { libc::kill(-(l.pid as i32), 0) } == 0)
}

/// End every process of run `id`: SIGTERM, up to `grace` for them to go, then SIGKILL. The run's
/// process group goes first, which reaches programs macOS withholds the environment of (its own
/// protected `sleep`, `sh`); the tag reaches what left the group. Rescans up to three times, so a
/// supervisor that respawns a child is caught. Returns the pids signalled by tag.
pub fn sweep(id: &str, group: Option<Leader>, seen: &[Seen], grace: Duration) -> Vec<u32> {
    let mut sys = System::new();
    let mut signalled = vec![];
    for _ in 0..3 {
        let pids = reach(&mut sys, id, seen);
        if pids.is_empty() && !group_alive(&mut sys, id, group) {
            break;
        }
        signal_group(&mut sys, id, group, libc::SIGTERM);
        for pid in &pids {
            if let Some(p) = sys.process(*pid) {
                p.kill_with(Signal::Term);
            }
            signalled.push(pid.as_u32());
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            if reach(&mut sys, id, seen).is_empty() && !group_alive(&mut sys, id, group) {
                break;
            }
        }
        signal_group(&mut sys, id, group, libc::SIGKILL);
        for pid in reach(&mut sys, id, seen) {
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
        let signalled = sweep(&id, None, &[], Duration::from_secs(2));
        assert_eq!(signalled, [tagged_child.id()]);
        assert!(tagged_child.wait().is_ok());
        assert!(other.try_wait().unwrap().is_none(), "a process of another run was ended");
        other.kill().unwrap();
        other.wait().unwrap();
    }
    /// A recorded group whose id now belongs to someone else's process is left alone; the same
    /// group is ended once its leader is the process that started with the run.
    #[test]
    fn a_group_that_is_not_the_runs_is_not_signalled() {
        use std::os::unix::process::CommandExt;
        let id = format!("test-group-{}", std::process::id());
        let mut stranger = Command::new(sleeper("sleep-c")).arg("31003").process_group(0).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let pid = stranger.id();
        // The pid was recorded for a run that is gone: no member carries its tag and the process
        // that leads the group started at another time than the run did.
        let stale = Leader { pid, started: started(pid) + 1000, };
        sweep(&id, Some(stale), &[], Duration::from_secs(1));
        assert!(stranger.try_wait().unwrap().is_none(), "a group of another process was ended");
        let unknown = Leader { pid, started: 0, };
        sweep(&id, Some(unknown), &[], Duration::from_secs(1));
        assert!(stranger.try_wait().unwrap().is_none(), "a group with no start time on record was ended");
        let ours = Leader { pid, started: started(pid), };
        sweep(&id, Some(ours), &[], Duration::from_secs(2));
        assert!(stranger.wait().is_ok());
    }

    /// What a command left in its group after its leader exited is ended, though nothing in it
    /// carries the run's tag.
    #[test]
    fn a_group_left_behind_by_an_exited_leader_is_ended() {
        use std::os::unix::process::CommandExt;
        let id = format!("test-left-{}", std::process::id());
        let sleep = sleeper("sleep-d");
        let mut leader = Command::new("sh").arg("-c").arg(format!("{} 31004 & exit 0", sleep.display()))
            .process_group(0).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let (pid, at) = (leader.id(), started(leader.id()));
        leader.wait().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let left = |n: &str| Command::new("pgrep").args(["-f", n]).stdout(Stdio::null()).status().unwrap().success();
        assert!(left("sleep-d 31004"), "the command left nothing behind to end");
        sweep(&id, Some(Leader { pid, started: at }), &[], Duration::from_secs(2));
        assert!(!left("sleep-d 31004"), "the group the exited leader left behind was not ended");
    }
}
