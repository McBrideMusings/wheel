//! The processes a space's panes started: one listing behind `wheel space ps` and the Mac app's
//! process viewer, and the stop both use, so a process stopped from either leaves both lists.

use std::collections::HashMap;
use std::process::Command;

use wheel_proto::{PaneKind, ProcInfo};

use crate::resume;
use crate::server::AppState;
use crate::spaces::panes;

/// One line of `ps`: a running process before it is tied to a pane.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    pid: u32,
    ppid: u32,
    pgid: u32,
    cpu: f32,
    mem_kb: u64,
    age_secs: u64,
    command: String,
}

/// Seconds in `ps`'s elapsed time, `[[dd-]hh:]mm:ss`.
fn parse_elapsed(text: &str) -> Option<u64> {
    let (days, rest) = match text.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let mut secs = 0u64;
    for part in rest.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(days * 86400 + secs)
}

/// The rows in `ps -axo pid=,ppid=,pgid=,pcpu=,rss=,etime=,command=` output.
fn parse_ps(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            let ppid = words.next()?.parse().ok()?;
            let pgid = words.next()?.parse().ok()?;
            let cpu = words.next()?.parse().ok()?;
            let mem_kb = words.next()?.parse().ok()?;
            let age_secs = parse_elapsed(words.next()?)?;
            let command = words.collect::<Vec<_>>().join(" ");
            Some(Row {
                pid,
                ppid,
                pgid,
                cpu,
                mem_kb,
                age_secs,
                command,
            })
        })
        .collect()
}

fn all_processes() -> Result<Vec<Row>, String> {
    let out = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid=,pcpu=,rss=,etime=,command="])
        .output()
        .map_err(|e| format!("could not run ps: {e}"))?;
    if !out.status.success() {
        return Err(format!("ps failed: {}", out.status));
    }
    Ok(parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

/// `root` and everything below it, in pid order. The root is left out when `with_root` is false.
fn tree(rows: &[Row], root: u32, with_root: bool) -> Vec<&Row> {
    let mut children: HashMap<u32, Vec<&Row>> = HashMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row);
    }
    let mut found = Vec::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            if child.pid != root {
                found.push(*child);
                queue.push(child.pid);
            }
        }
    }
    // An agent is started as the leader of its own process group; a pid that no longer leads one
    // is not the agent any more (the number was reused).
    if with_root {
        found.extend(rows.iter().filter(|r| r.pid == root && r.pgid == root));
    }
    found.sort_by_key(|r| r.pid);
    found.dedup_by_key(|r| r.pid);
    found
}

fn program_name(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or(command);
    first
        .rsplit('/')
        .next()
        .unwrap_or(first)
        .trim_start_matches('-')
        .to_string()
}

impl AppState {
    /// Every process under `space`'s panes. A terminal pane's are its shell's descendants; an
    /// agent pane's are the agent process and its descendants.
    pub async fn space_procs(&self, space: &str) -> Result<Vec<ProcInfo>, String> {
        let roots: Vec<(String, Option<String>, u32, bool)> = {
            let spaces = self.spaces.lock().unwrap();
            let found = spaces
                .spaces
                .iter()
                .find(|s| s.id == space)
                .ok_or_else(|| format!("no space {space}"))?;
            panes(&found.root)
                .into_iter()
                .filter_map(|(id, kind)| match kind {
                    PaneKind::Terminal { term, agent, .. } => {
                        let pid = self.term(term)?.pid?;
                        Some((id.to_string(), agent.as_ref().map(|a| a.name.clone()), pid, false))
                    }
                    PaneKind::Agent { session, agent, .. } => {
                        let pid = self.session(session).ok()?.agent_pid()?;
                        Some((id.to_string(), Some(agent.clone()), pid, true))
                    }
                    _ => None,
                })
                .collect()
        };
        let procs = tokio::task::spawn_blocking(move || {
            let rows = all_processes()?;
            let mut procs = Vec::new();
            for (pane, agent, root, with_root) in roots {
                for row in tree(&rows, root, with_root) {
                    procs.push(ProcInfo {
                        pid: row.pid,
                        ppid: row.ppid,
                        pane: pane.clone(),
                        agent: agent.clone(),
                        name: program_name(&row.command),
                        command: row.command.clone(),
                        cpu: row.cpu,
                        mem_kb: row.mem_kb,
                        age_secs: row.age_secs,
                        cwd: resume::process_cwd(row.pid),
                    });
                }
            }
            Ok::<_, String>(procs)
        })
        .await
        .map_err(|e| format!("process listing failed: {e}"))??;
        tracing::info!(%space, count = procs.len(), "space procs");
        Ok(procs)
    }

    /// Signal a process `space_procs` lists for `space`.
    pub async fn proc_stop(&self, space: &str, pid: u32, force: bool) -> Result<(), String> {
        let listed = self.space_procs(space).await?;
        if !listed.iter().any(|p| p.pid == pid) {
            return Err(format!("process {pid} is not running under space {space}"));
        }
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if rc != 0 {
            return Err(format!(
                "could not signal process {pid}: {}",
                std::io::Error::last_os_error()
            ));
        }
        tracing::info!(%space, pid, force, "process stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_forms() {
        assert_eq!(parse_elapsed("00:07"), Some(7));
        assert_eq!(parse_elapsed("02:03:04"), Some(7384));
        assert_eq!(parse_elapsed("3-01:00:00"), Some(3 * 86400 + 3600));
    }

    #[test]
    fn tree_walks_descendants() {
        let rows = parse_ps(
            "  10     1    10  0.0  100 00:10 -zsh\n  11    10    11  2.5  200 00:05 sleep 30\n  12    11    11  0.0  50 00:05 /bin/sh -c x\n  13     1    13  0.0 10 00:01 other\n",
        );
        let pids: Vec<u32> = tree(&rows, 10, false).iter().map(|r| r.pid).collect();
        assert_eq!(pids, vec![11, 12]);
        let pids: Vec<u32> = tree(&rows, 11, true).iter().map(|r| r.pid).collect();
        assert_eq!(pids, vec![11, 12]);
        assert_eq!(rows[1].command, "sleep 30");
        assert_eq!(program_name("/bin/sh -c x"), "sh");
    }
}
