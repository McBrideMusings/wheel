//! The processes a space started: one listing behind `wheel space ps` and the Mac app's process
//! viewer, and the stop both use, so a process stopped from either leaves both lists.
//!
//! Four kinds of evidence tie a process to a space. The tree walk finds what still runs under a
//! pane's shell or agent. A process that left that tree (its parent exited, or it detached on
//! purpose) is found by its `WHEEL_PANE` tag, which every pane's processes inherit and which
//! survives a server restart; by macOS's responsible process, which names wheeld for anything a
//! pane started while this wheeld ran, tag or no tag (Claude Code's Bash shells drop it); and by
//! its working directory, which places an untagged or closed-pane process in a space.

use std::collections::{HashMap, HashSet};
use std::process::Command;

use wheel_proto::{PaneKind, ProcInfo};

use crate::resume;
use crate::server::AppState;
use crate::spaces::space_panes;

/// One line of `ps`: a running process before it is tied to a pane.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    pid: u32,
    ppid: u32,
    pgid: u32,
    uid: u32,
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

/// The rows in `ps -axo pid=,ppid=,pgid=,uid=,pcpu=,rss=,etime=,command=` output.
fn parse_ps(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            let ppid = words.next()?.parse().ok()?;
            let pgid = words.next()?.parse().ok()?;
            let uid = words.next()?.parse().ok()?;
            let cpu = words.next()?.parse().ok()?;
            let mem_kb = words.next()?.parse().ok()?;
            let age_secs = parse_elapsed(words.next()?)?;
            let command = words.collect::<Vec<_>>().join(" ");
            Some(Row {
                pid,
                ppid,
                pgid,
                uid,
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
        .args(["-axo", "pid=,ppid=,pgid=,uid=,pcpu=,rss=,etime=,command="])
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

/// The command a Claude Code Bash-tool shell runs: it starts `zsh -c` (or bash) with its
/// snapshot, its own helpers, then `eval '<command>' < /dev/null && pwd -P >| <file>`.
fn claude_bash_command(command: &str) -> Option<String> {
    if !command.contains("shell-snapshots/") {
        return None;
    }
    let start = command.rfind("eval '")? + "eval '".len();
    let rest = &command[start..];
    let end = rest
        .rfind("' < /dev/null")
        .or_else(|| rest.rfind("' \\< /dev/null"))?;
    Some(rest[..end].replace("'\\''", "'"))
}

/// The agents whose children the listing names by how the agent started them.
const AGENTS: &[&str] = &["claude", "codex", "pi"];

/// How a process directly under an agent came to run: a Bash-tool command, a language server
/// the agent runs for its own use, or anything else the agent started.
fn started_by_agent(agent: &str, command: &str) -> String {
    let lower = command.to_ascii_lowercase();
    if claude_bash_command(command).is_some() {
        "Bash tool".into()
    } else if lower.contains("language-server") || lower.contains("languageserver") {
        "LSP".into()
    } else {
        format!("started by {agent}")
    }
}

/// The row a client shows for `row`: the command as run (the command inside a Claude Bash shell),
/// and how it started when its parent says.
fn info(row: &Row, pane: Option<&str>, agent: Option<&str>, how: Option<String>) -> ProcInfo {
    ProcInfo {
        pid: row.pid,
        ppid: row.ppid,
        pane: pane.map(str::to_owned),
        agent: agent.map(str::to_owned),
        name: claude_bash_command(&row.command).unwrap_or_else(|| row.command.clone()),
        command: row.command.clone(),
        cpu: row.cpu,
        mem_kb: row.mem_kb,
        age_secs: row.age_secs,
        cwd: resume::process_cwd(row.pid),
        how,
        detached: None,
    }
}

/// What the listing reads about a process outside wheeld's tree, to tie it to a space.
#[derive(Debug, Clone, Default, PartialEq)]
struct Outside {
    /// The pane in its `WHEEL_PANE`, when its `WHEEL_TOKEN_FILE` is this wheeld's.
    tag: Option<String>,
    /// macOS names this wheeld as the process responsible for it.
    ours: bool,
    cwd: Option<String>,
}

/// A space as the listing sees it.
struct SpaceRoots {
    id: String,
    dir: String,
    /// Each pane holding a shell or agent: its id, its agent, the root pid, and whether the root
    /// itself is listed (an agent pane's agent is; a terminal's shell is not).
    roots: Vec<(String, Option<String>, u32, bool)>,
    /// Every pane in the space, running anything or not.
    panes: HashSet<String>,
}

/// Whether `path` is `dir` or inside it.
fn inside(path: &str, dir: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    path == dir || path.starts_with(&format!("{dir}/"))
}

/// Why a process outside wheeld's tree belongs to `space`, or None when nothing ties it there.
fn detached_from(space: &SpaceRoots, open: &HashSet<String>, row: &Row, o: &Outside) -> Option<String> {
    let here = o.cwd.as_deref().is_some_and(|c| inside(c, &space.dir));
    let bash = claude_bash_command(&row.command).is_some();
    match &o.tag {
        Some(pane) if space.panes.contains(pane) => Some("detached from its pane".into()),
        // Another space's pane, or one that has closed: the folder decides.
        Some(pane) if !open.contains(pane) && here => {
            Some(format!("started in pane {pane}, which has closed; it runs in this folder"))
        }
        Some(_) => None,
        None if o.ours && here && bash => {
            Some("a Claude Bash command run in this folder; its pane is unknown".into())
        }
        None if o.ours && here => {
            Some("started from Wheel in this folder; its pane is unknown".into())
        }
        None => None,
    }
}

/// Every space's processes from one `ps` snapshot: the tree under each pane, then whatever left
/// it, with the evidence tying it to the space. `outside` reads a process outside wheeld's tree.
fn listing(
    rows: &[Row],
    spaces: &[SpaceRoots],
    wheeld: u32,
    uid: u32,
    mut outside: impl FnMut(&Row) -> Outside,
) -> HashMap<String, Vec<ProcInfo>> {
    let by_pid: HashMap<u32, &Row> = rows.iter().map(|r| (r.pid, r)).collect();
    let open: HashSet<String> = spaces.iter().flat_map(|s| s.panes.iter().cloned()).collect();
    let mut out: HashMap<String, Vec<ProcInfo>> = HashMap::new();
    for space in spaces {
        let list = out.entry(space.id.clone()).or_default();
        for (pane, agent, root, with_root) in &space.roots {
            for row in tree(rows, *root, *with_root) {
                let parent = by_pid.get(&row.ppid);
                let parent_name = parent.map(|p| program_name(&p.command));
                let how = if row.pid == *root {
                    None
                } else if !with_root && row.ppid == *root {
                    Some("typed in the terminal".to_string())
                } else {
                    match parent_name.as_deref() {
                        Some(name) if AGENTS.contains(&name) => {
                            Some(started_by_agent(name, &row.command))
                        }
                        _ => None,
                    }
                };
                list.push(info(row, Some(pane), agent.as_deref(), how));
            }
        }
    }
    // Everything still under wheeld is a pane's, a client's terminal or wheeld's own helper.
    let under: HashSet<u32> = tree(rows, wheeld, false).iter().map(|r| r.pid).collect();
    for row in rows {
        if row.uid != uid || row.pid == wheeld || row.pid <= 1 || under.contains(&row.pid) {
            continue;
        }
        let o = outside(row);
        if o.tag.is_none() && !o.ours {
            continue;
        }
        for space in spaces {
            if let Some(why) = detached_from(space, &open, row, &o) {
                let pane = o.tag.as_deref().filter(|p| space.panes.contains(*p));
                let mut proc = info(row, pane, None, None);
                proc.cwd = o.cwd.clone();
                proc.detached = Some(why);
                out.entry(space.id.clone()).or_default().push(proc);
                break;
            }
        }
    }
    out
}

/// The process macOS holds responsible for `pid`: the app or daemon that started its chain,
/// which a process keeps when its parent exits. Undocumented (libSystem exports it); none
/// elsewhere.
#[cfg(target_os = "macos")]
fn responsible(pid: u32) -> Option<u32> {
    unsafe extern "C" {
        fn responsibility_get_pid_responsible_for_pid(pid: libc::pid_t) -> libc::pid_t;
    }
    // SAFETY: takes a pid by value and returns a pid, -1 when it cannot say.
    let r = unsafe { responsibility_get_pid_responsible_for_pid(pid as libc::pid_t) };
    (r > 0).then_some(r as u32)
}

#[cfg(not(target_os = "macos"))]
fn responsible(_pid: u32) -> Option<u32> {
    None
}

impl AppState {
    /// Every space's processes, by space id.
    pub(crate) async fn all_procs(&self) -> Result<HashMap<String, Vec<ProcInfo>>, String> {
        let spaces: Vec<SpaceRoots> = {
            let spaces = self.spaces.lock().unwrap();
            spaces
                .spaces
                .iter()
                .map(|space| {
                    let all = space_panes(space);
                    SpaceRoots {
                        id: space.id.clone(),
                        dir: space.cwd.clone(),
                        panes: all.iter().map(|(id, _)| id.to_string()).collect(),
                        roots: all
                            .into_iter()
                            .filter_map(|(id, kind)| match kind {
                                PaneKind::Terminal { term, agent, .. } => {
                                    let pid = self.term(term)?.pid?;
                                    Some((
                                        id.to_string(),
                                        agent.as_ref().map(|a| a.name.clone()),
                                        pid,
                                        false,
                                    ))
                                }
                                PaneKind::Agent { agent, harness, .. } => {
                                    let pid = self.agent(agent).ok()?.agent_pid()?;
                                    Some((id.to_string(), Some(harness.clone()), pid, true))
                                }
                                _ => None,
                            })
                            .collect(),
                    }
                })
                .collect()
        };
        let token_file = self
            .control
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.token_file.display().to_string());
        tokio::task::spawn_blocking(move || {
            let rows = all_processes()?;
            let wheeld = std::process::id();
            // SAFETY: getuid cannot fail.
            let uid = unsafe { libc::getuid() };
            let outside = |row: &Row| {
                let env = resume::process_args(row.pid).map(|(_, env)| env).unwrap_or_default();
                let var = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
                let tag = match (var("WHEEL_PANE"), var("WHEEL_TOKEN_FILE"), &token_file) {
                    (Some(pane), Some(file), Some(ours)) if file == ours => Some(pane.to_string()),
                    _ => None,
                };
                let ours = responsible(row.pid) == Some(wheeld);
                let cwd = (tag.is_some() || ours)
                    .then(|| resume::process_cwd(row.pid))
                    .flatten();
                Outside { tag, ours, cwd }
            };
            Ok::<_, String>(listing(&rows, &spaces, wheeld, uid, outside))
        })
        .await
        .map_err(|e| format!("process listing failed: {e}"))?
    }

    /// Every process `space` started: under its panes, then detached ones.
    pub async fn space_procs(&self, space: &str) -> Result<Vec<ProcInfo>, String> {
        if !self.spaces.lock().unwrap().spaces.iter().any(|s| s.id == space) {
            return Err(format!("no space {space}"));
        }
        let procs = self.all_procs().await?.remove(space).unwrap_or_default();
        let detached = procs.iter().filter(|p| p.detached.is_some()).count();
        tracing::info!(%space, count = procs.len(), detached, "space procs");
        Ok(procs)
    }

    /// Signal a process `space_procs` lists for `space`, and every process listed below it, so a
    /// stopped shell leaves no command of its running.
    pub async fn proc_stop(&self, space: &str, pid: u32, force: bool) -> Result<(), String> {
        let listed = self.space_procs(space).await?;
        if !listed.iter().any(|p| p.pid == pid) {
            return Err(format!("process {pid} is not running under space {space}"));
        }
        let mut targets = vec![pid];
        let mut i = 0;
        while i < targets.len() {
            let parent = targets[i];
            targets.extend(listed.iter().filter(|p| p.ppid == parent && p.pid != parent).map(|p| p.pid));
            i += 1;
        }
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        // Children first, so a parent that would restart a child it saw die is gone after it.
        for target in targets.iter().rev() {
            let rc = unsafe { libc::kill(*target as libc::pid_t, signal) };
            if rc != 0 && *target == pid {
                return Err(format!(
                    "could not signal process {pid}: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        tracing::info!(%space, pid, force, below = targets.len() - 1, "process stopped");
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
            "  10     1    10 501  0.0  100 00:10 -zsh\n  11    10    11 501  2.5  200 00:05 sleep 30\n  12    11    11 501  0.0  50 00:05 /bin/sh -c x\n  13     1    13 501  0.0 10 00:01 other\n",
        );
        let pids: Vec<u32> = tree(&rows, 10, false).iter().map(|r| r.pid).collect();
        assert_eq!(pids, vec![11, 12]);
        let pids: Vec<u32> = tree(&rows, 11, true).iter().map(|r| r.pid).collect();
        assert_eq!(pids, vec![11, 12]);
        assert_eq!(rows[1].command, "sleep 30");
        assert_eq!(rows[1].uid, 501);
        assert_eq!(program_name("/bin/sh -c x"), "sh");
    }

    #[test]
    fn a_claude_bash_shell_shows_the_command_inside_it() {
        // The shape Claude Code 2.1.316 runs a Bash-tool command in (newlines as ps prints them).
        let shell = r#"/bin/zsh -c source /Users/p/.claude/shell-snapshots/snapshot-zsh-1-x.sh 2>/dev/null || true && # session-shell-env\012unset -f grep\012: && setopt NO_EXTENDED_GLOB 2>/dev/null || true && eval 'until ! pgrep -f '\''perf/m1.sh'\'' >/dev/null; do sleep 10; done' < /dev/null && pwd -P >| /tmp/claude-a340-cwd"#;
        assert_eq!(
            claude_bash_command(shell).as_deref(),
            Some("until ! pgrep -f 'perf/m1.sh' >/dev/null; do sleep 10; done")
        );
        assert_eq!(claude_bash_command("/bin/zsh -c eval 'x' < /dev/null"), None);
        assert_eq!(started_by_agent("claude", shell), "Bash tool");
        assert_eq!(
            started_by_agent("claude", "node /x/typescript-language-server --stdio"),
            "LSP"
        );
        assert_eq!(started_by_agent("claude", "docker run mcp/grafana"), "started by claude");
    }

    fn space(id: &str, dir: &str, panes: &[&str], roots: Vec<(String, Option<String>, u32, bool)>) -> SpaceRoots {
        SpaceRoots {
            id: id.into(),
            dir: dir.into(),
            panes: panes.iter().map(|p| p.to_string()).collect(),
            roots,
        }
    }

    #[test]
    fn processes_that_left_a_pane_are_tied_to_their_space_by_tag_responsibility_and_folder() {
        // wheeld 100 holds a terminal (shell 110) in space s1 (/w/a, pane p1) and an agent (120)
        // in s2 (/w/b, pane p2). 111 was typed in the terminal; 121 is the agent's Bash shell.
        // Outside wheeld's tree: 200 tagged p1, 201 tagged p9 (closed) running in /w/a, 202 an
        // untagged Bash shell wheeld is responsible for in /w/b, 203 tagged p1 by another
        // instance, 204 someone else's, 205 another user's.
        let bash = "/bin/zsh -c source /h/.claude/shell-snapshots/s.sh && eval 'sleep 9' < /dev/null && pwd -P >| /t/c";
        let ps = format!(
            "100 1 100 501 0.0 10 01:00 wheeld\n110 100 110 501 0.0 10 01:00 -zsh\n111 110 111 501 0.0 10 00:10 sleep 30\n120 100 120 501 1.0 10 01:00 claude\n121 120 120 501 0.0 10 00:05 {bash}\n200 1 200 501 0.0 10 09:00 crashpad_handler\n201 1 201 501 0.0 10 09:00 node server.js\n202 1 202 501 0.0 10 09:00 {bash}\n203 1 203 501 0.0 10 09:00 tagged-elsewhere\n204 1 204 501 0.0 10 09:00 Safari\n205 1 205 0 0.0 10 09:00 root-thing\n"
        );
        let rows = parse_ps(&ps);
        let spaces = vec![
            space("s1", "/w/a", &["p1"], vec![("p1".into(), None, 110, false)]),
            space("s2", "/w/b", &["p2"], vec![("p2".into(), Some("claude".into()), 120, true)]),
        ];
        let outside = |row: &Row| match row.pid {
            200 => Outside { tag: Some("p1".into()), ours: false, cwd: Some("/elsewhere".into()) },
            201 => Outside { tag: Some("p9".into()), ours: false, cwd: Some("/w/a/sub".into()) },
            202 => Outside { tag: None, ours: true, cwd: Some("/w/b".into()) },
            _ => Outside::default(),
        };
        let out = listing(&rows, &spaces, 100, 501, outside);
        let s1 = &out["s1"];
        let row = |list: &Vec<ProcInfo>, pid| list.iter().find(|p| p.pid == pid).cloned();
        assert_eq!(row(s1, 111).unwrap().how.as_deref(), Some("typed in the terminal"));
        assert_eq!(row(s1, 200).unwrap().detached.as_deref(), Some("detached from its pane"));
        assert_eq!(row(s1, 200).unwrap().pane.as_deref(), Some("p1"));
        let closed = row(s1, 201).unwrap();
        assert_eq!(closed.pane, None);
        assert!(closed.detached.unwrap().contains("p9, which has closed"));
        let s2 = &out["s2"];
        assert_eq!(row(s2, 120).unwrap().how, None, "the agent itself");
        let tool = row(s2, 121).unwrap();
        assert_eq!(tool.how.as_deref(), Some("Bash tool"));
        assert_eq!(tool.name, "sleep 9");
        assert_eq!(
            row(s2, 202).unwrap().detached.as_deref(),
            Some("a Claude Bash command run in this folder; its pane is unknown")
        );
        let all: Vec<u32> = out.values().flatten().map(|p| p.pid).collect();
        for pid in [100, 110, 203, 204, 205] {
            assert!(!all.contains(&pid), "{pid} is not a space's");
        }
    }
}
