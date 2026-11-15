//! The Swarm pane's run: the latest swarm run of a project, read from the run log the `swarm` tool
//! writes (`<repo-slug dir>/swarm/run.tsv`, one `<utc time>\t<event>\t<bead>\t<detail>` line per
//! event), each launched worker's agent pane (the agent whose space is the worker's worktree) and
//! each worker's Claude transcript (`<config dir>/projects/<encoded worktree>/*.jsonl`): its latest
//! actions and when it was last written. A worker in flight whose transcript has not been written
//! for [`STUCK_AFTER`] seconds is stuck, whatever its agent state says.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::core::agents::Agent;

pub mod feed;

/// How long a worker in flight may go without a transcript write before it shows as stuck.
pub const STUCK_AFTER: i64 = 600;
/// The latest actions kept per worker.
const ACTIONS: usize = 8;
/// How much of a transcript's end is read for them at first; doubled while it holds fewer than
/// [`ACTIONS`] (one tool result can be megabytes) up to [`TAIL_MAX`].
const TAIL_BYTES: u64 = 400_000;
const TAIL_MAX: u64 = 16_000_000;

/// One line of the run log.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub at: i64,
    pub event: String,
    pub bead: String,
    pub detail: String,
}

/// The events of the latest run: from the last `run` line on, or the whole log when it has none.
pub fn latest_run(log: &str) -> Vec<Event> {
    let all: Vec<Event> = log
        .lines()
        .filter_map(|l| {
            let mut c = l.splitn(4, '\t');
            let (t, e) = (c.next()?, c.next()?);
            Some(Event { at: epoch(t)?, event: e.into(), bead: c.next().unwrap_or("").into(), detail: c.next().unwrap_or("").into() })
        })
        .collect();
    let start = all.iter().rposition(|e| e.event == "run").unwrap_or(0);
    all[start..].to_vec()
}

/// Seconds since the Unix epoch of an RFC 3339 UTC time (`2026-11-14T21:41:01Z`, fractions allowed).
pub fn epoch(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[10] != b'T' || !t.ends_with('Z') {
        return None;
    }
    let n = |r: std::ops::Range<usize>| t.get(r)?.parse::<i64>().ok();
    let (mut y, mo, d, h, mi, s) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if mo <= 2 {
        y -= 1;
    }
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + mi * 60 + s)
}

/// A Unix time as `HH:MM:SS` at the UTC offset `offset` (seconds east).
pub fn clock(ts: i64, offset: i64) -> String {
    let s = (ts + offset).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// One thing a worker's transcript says it did: a tool call, or what it said.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub at: i64,
    /// The tool's name; empty for something the agent said.
    pub tool: String,
    pub text: String,
    /// A tool call: whether it has returned, and whether with an error.
    pub done: bool,
    pub error: bool,
}

fn rel<'a>(p: &'a str, worktree: &str) -> &'a str {
    p.strip_prefix(worktree).and_then(|r| r.strip_prefix('/')).unwrap_or(p)
}

/// The line a tool call is shown as.
fn summary(tool: &str, input: &Value, worktree: &str) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    let text = match tool {
        "Bash" => if s("description").is_empty() { s("command") } else { s("description") }.to_string(),
        "Read" | "Edit" | "Write" => rel(s("file_path"), worktree).to_string(),
        "NotebookEdit" => rel(s("notebook_path"), worktree).to_string(),
        "Grep" | "Glob" => s("pattern").to_string(),
        "Skill" => [s("skill"), s("args")].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" "),
        "Agent" => s("description").to_string(),
        _ if tool.ends_with("work_set") => input
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|i| [i.get("stage"), i.get("note")].iter().filter_map(|v| v.and_then(Value::as_str)).collect::<Vec<_>>().join(" "))
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default(),
        _ => input.as_object().and_then(|o| o.values().find_map(|v| v.as_str().filter(|x| !x.is_empty()))).unwrap_or("").to_string(),
    };
    cut(&text, 240)
}

fn cut(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n { one } else { one.chars().take(n - 1).collect::<String>() + "…" }
}

/// The latest actions in the transcript lines `lines`, oldest first.
pub fn actions(lines: &str, worktree: &str) -> Vec<Action> {
    let mut out: Vec<Action> = vec![];
    let mut by_id: std::collections::HashMap<String, usize> = Default::default();
    for line in lines.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        let Some(content) = e.pointer("/message/content").and_then(Value::as_array) else { continue };
        let at = e.get("timestamp").and_then(Value::as_str).and_then(epoch).unwrap_or(0);
        let kind = e.get("type").and_then(Value::as_str).unwrap_or("");
        for c in content {
            match (kind, c.get("type").and_then(Value::as_str)) {
                ("assistant", Some("tool_use")) => {
                    let name = c.get("name").and_then(Value::as_str).unwrap_or("");
                    let text = summary(name, c.get("input").unwrap_or(&Value::Null), worktree);
                    // An MCP tool (`mcp__<server>__<tool>`) is shown by its own name.
                    let tool = name.rsplit("__").next().unwrap_or(name).to_string();
                    if let Some(id) = c.get("id").and_then(Value::as_str) {
                        by_id.insert(id.into(), out.len());
                    }
                    out.push(Action { at, tool, text, done: false, error: false });
                }
                ("assistant", Some("text")) => {
                    let t = c.get("text").and_then(Value::as_str).unwrap_or("").trim();
                    if !t.is_empty() {
                        out.push(Action { at, tool: String::new(), text: cut(t, 400), done: true, error: false });
                    }
                }
                ("user", Some("tool_result")) => {
                    if let Some(&i) = c.get("tool_use_id").and_then(Value::as_str).and_then(|id| by_id.get(id)) {
                        out[i].done = true;
                        out[i].error = c.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    }
                }
                _ => {}
            }
        }
    }
    let skip = out.len().saturating_sub(ACTIONS);
    out.split_off(skip)
}

/// A worktree's directory name under a Claude config dir's `projects`.
pub fn encoded(worktree: &str) -> String {
    worktree.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The Claude config dirs that hold transcripts: `~/.claude` and every `~/.claude-*` profile.
fn config_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".claude")];
    if let Ok(rd) = std::fs::read_dir(home) {
        let mut more: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(".claude-"))).collect();
        more.sort();
        dirs.extend(more);
    }
    dirs.into_iter().filter(|d| d.join("projects").is_dir()).collect()
}

fn mtime(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    m.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

/// The worker's transcript: the newest one any config dir holds for `worktree` written since a minute
/// before `launched`, with its last write time.
pub fn transcript(home: &Path, worktree: &str, launched: i64) -> Option<(PathBuf, i64)> {
    let enc = encoded(worktree);
    config_dirs(home)
        .into_iter()
        .filter_map(|c| std::fs::read_dir(c.join("projects").join(&enc)).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| mtime(&p).map(|m| (p, m)))
        .filter(|(_, m)| *m >= launched - 60)
        .max_by_key(|(_, m)| *m)
}

/// The last `bytes` of the file at `p`, its first (cut) line dropped when the file is longer, and
/// whether that reached the start of the file.
fn tail(p: &Path, bytes: u64) -> (String, bool) {
    let Ok(mut f) = std::fs::File::open(p) else { return (String::new(), true) };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = size.saturating_sub(bytes);
    let mut buf = vec![];
    if f.seek(SeekFrom::Start(from)).is_err() || f.read_to_end(&mut buf).is_err() {
        return (String::new(), true);
    }
    let s = String::from_utf8_lossy(&buf).into_owned();
    if from > 0 { (s.split_once('\n').map(|(_, r)| r.to_string()).unwrap_or_default(), false) } else { (s, true) }
}

/// The latest actions of the transcript at `p`, reading further back while the end holds fewer than [`ACTIONS`].
fn transcript_actions(p: &Path, worktree: &str) -> Vec<Action> {
    let mut bytes = TAIL_BYTES;
    loop {
        let (text, whole) = tail(p, bytes);
        let found = actions(&text, worktree);
        if found.len() >= ACTIONS || whole || bytes >= TAIL_MAX {
            return found;
        }
        bytes *= 2;
    }
}

/// A worker of the run, as the log, its pane and its transcript say.
#[derive(Clone, Debug, PartialEq)]
pub struct Worker {
    pub bead: String,
    pub model: String,
    pub worktree: String,
    pub launched: i64,
    /// The last of `finished`, `landed`, `parked`, `failed`, `reopened`, `ready`, `rejected`, `signed`,
    /// `declined` logged for it, with its time and detail; a later verdict replaces `finished`.
    pub outcome: Option<(String, i64, String)>,
    /// No `finished`, `landed`, `parked`, `failed` or `reopened` after its launch (what `swarm status`
    /// lists as in flight: a `rejected` report goes back to the worker), and no `end` of the run.
    pub in_flight: bool,
}

/// The workers launched in `events`, in launch order.
pub fn workers(events: &[Event]) -> Vec<Worker> {
    let mut out: Vec<Worker> = vec![];
    for e in events {
        if e.event == "launched" {
            let (model, wt) = e.detail.split_once(' ').unwrap_or((&e.detail, ""));
            out.retain(|w| w.bead != e.bead);
            out.push(Worker { bead: e.bead.clone(), model: model.into(), worktree: wt.into(), launched: e.at, outcome: None, in_flight: true });
            continue;
        }
        if e.event == "end" {
            out.iter_mut().for_each(|w| w.in_flight = false);
            continue;
        }
        let Some(w) = out.iter_mut().find(|w| w.bead == e.bead) else { continue };
        if matches!(e.event.as_str(), "finished" | "landed" | "parked" | "failed" | "reopened") {
            w.in_flight = false;
        }
        if matches!(e.event.as_str(), "finished" | "landed" | "parked" | "failed" | "reopened" | "ready" | "rejected" | "signed" | "declined") {
            if e.event != "finished" || w.outcome.is_none() {
                w.outcome = Some((e.event.clone(), e.at, e.detail.clone()));
            }
        }
    }
    out
}

/// The run log of the repo holding `dir`: `/private/tmp/claude/<repo-slug of its main checkout>/swarm/run.tsv`,
/// the main checkout found as the `swarm` tool finds it (the parent of the common `.git`) and the slug
/// from `repo-slug` (`~/.claude/tools/repo-slug`, the one definition the tool uses; without `--path`,
/// which would create the directory on every read).
pub fn run_log(home: &Path, dir: &str) -> Result<PathBuf, String> {
    let (_, common) = crate::core::git::dirs(dir).ok_or_else(|| format!("{dir} is in no git repository"))?;
    let common = PathBuf::from(common);
    let main = common
        .parent()
        .filter(|_| common.file_name().is_some_and(|n| n == ".git"))
        .ok_or_else(|| format!("{dir} has no main checkout (its git dir is {})", common.display()))?
        .to_string_lossy()
        .into_owned();
    let tool = home.join(".claude/tools/repo-slug");
    let slug = crate::core::run::text(&tool.to_string_lossy(), &main, &[&main])?;
    Ok(PathBuf::from("/private/tmp/claude").join(slug.trim()).join("swarm").join("run.tsv"))
}

/// The run's value, given its events, the agents of the project, the clock and how to find a worker's
/// transcript and the lines of its end.
pub fn value_of(
    events: &[Event],
    agents: &[Agent],
    now: i64,
    offset: i64,
    find: impl Fn(&str, i64) -> Option<(PathBuf, i64)>,
    acts_of: impl Fn(&Path, &str) -> Vec<Action>,
) -> Value {
    let run = events.first().filter(|e| e.event == "run");
    let end = events.iter().rev().find(|e| e.event == "end");
    let lead_pid = events.iter().rev().filter(|e| e.event == "run" || e.event == "lead").find_map(|e| {
        e.detail.split_whitespace().find_map(|w| w.strip_prefix("pid=")).and_then(|p| p.parse::<i32>().ok())
    });
    let workers: Vec<Value> = workers(events)
        .into_iter()
        .map(|w| {
            let pane = agents.iter().find(|a| a.cwd.trim_end_matches('/') == w.worktree.trim_end_matches('/') && !w.worktree.is_empty());
            let found = if w.worktree.is_empty() { None } else { find(&w.worktree, w.launched) };
            let last_write = found.as_ref().map(|(_, m)| *m);
            let quiet_since = last_write.unwrap_or(w.launched);
            let stuck = w.in_flight && now - quiet_since >= STUCK_AFTER;
            let acts: Vec<Value> = match (&found, w.in_flight) {
                (Some((p, _)), true) => acts_of(p, &w.worktree)
                    .into_iter()
                    .map(|a| json!({ "at": a.at, "clock": clock(a.at, offset), "tool": a.tool, "text": a.text, "done": a.done, "error": a.error }))
                    .collect(),
                _ => vec![],
            };
            let work = pane.and_then(|a| a.work.as_ref());
            json!({
                "bead": w.bead,
                "model": w.model,
                "worktree": w.worktree,
                "launched": w.launched,
                "launched_clock": clock(w.launched, offset),
                "in_flight": w.in_flight,
                "outcome": w.outcome.as_ref().map(|o| o.0.clone()),
                "outcome_clock": w.outcome.as_ref().map(|o| clock(o.1, offset)),
                "outcome_detail": w.outcome.as_ref().map(|o| o.2.clone()),
                "pane": pane.map(|a| a.pane.clone()),
                "status": pane.and_then(|a| a.status.clone()),
                "stage": work.and_then(|w| w.stage.clone()),
                "note": work.and_then(|w| w.note.clone()),
                "transcript": found.as_ref().map(|(p, _)| p.to_string_lossy().into_owned()),
                "last_write": last_write,
                "last_write_clock": last_write.map(|m| clock(m, offset)),
                "stuck": stuck,
                "stuck_since_clock": if stuck { Some(clock(quiet_since, offset)) } else { None },
                "actions": acts,
            })
        })
        .collect();
    json!({
        "state": if events.is_empty() { "no-run" } else if end.is_some() { "ended" } else { "live" },
        "started": run.map(|e| e.at),
        "started_clock": run.map(|e| clock(e.at, offset)),
        "space": run.and_then(|e| e.detail.split_whitespace().find_map(|w| w.strip_prefix("space=")).map(String::from)),
        "lead_pid": lead_pid,
        "ended_clock": end.map(|e| clock(e.at, offset)),
        "end_note": end.map(|e| e.detail.clone()),
        "events": events.iter().map(|e| json!({ "at": e.at, "clock": clock(e.at, offset), "event": e.event, "bead": e.bead, "detail": e.detail })).collect::<Vec<_>>(),
        "workers": workers,
    })
}

/// What the run log of `dir` holds and how its transcripts are found, for the feed.
pub fn read(home: &Path, log: &Path, agents: &[Agent], now: i64, offset: i64) -> Value {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let events = latest_run(&text);
    value_of(&events, agents, now, offset, |wt, launched| transcript(home, wt, launched), transcript_actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::agents::{Work, WorkItem};

    const LOG: &str = "2026-11-14T20:00:00Z\trun\t\tpid=1 space=old\n\
        2026-11-14T20:01:00Z\tlaunched\tw-old\tsonnet /wt/w-old\n\
        2026-11-14T21:00:00Z\trun\t\tpid=42 space=wheel\n\
        2026-11-14T21:01:00Z\tlaunched\tw-a\tsonnet /wt/w-a\n\
        2026-11-14T21:02:00Z\tlaunched\tw-b\topus /wt/w-b\n\
        2026-11-14T21:03:00Z\tlaunched\tw-c\thaiku /wt/w-c\n\
        2026-11-14T21:20:00Z\tfinished\tw-c\texit 0\n\
        2026-11-14T21:20:30Z\tlanded\tw-c\tabc123\n";

    fn agent(cwd: &str, pane: &str, status: &str) -> Agent {
        Agent { space: "s".into(), cwd: cwd.into(), pane: pane.into(), agent: "w1".into(), harness: "claude".into(), model: None,
            status: Some(status.into()), work: Some(Work { items: vec![WorkItem { kind: "bead".into(), id: "w-a".into() }], stage: Some("build".into()), note: Some("cargo".into()) }) }
    }

    #[test]
    fn times_parse_and_print() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2026-11-14T21:01:00Z"), Some(1_794_690_060));
        assert_eq!(epoch("2026-11-14T21:01:00.512Z"), Some(1_794_690_060));
        assert_eq!(epoch("garbage"), None);
        assert_eq!(clock(1_794_690_060, 0), "21:01:00");
        assert_eq!(clock(1_794_690_060, -5 * 3600), "16:01:00");
    }

    #[test]
    fn the_latest_run_starts_at_its_run_line_and_workers_follow_their_verdicts() {
        let ev = latest_run(LOG);
        assert_eq!(ev.len(), 6);
        assert_eq!(ev[0].detail, "pid=42 space=wheel");
        let ws = workers(&ev);
        assert_eq!(ws.iter().map(|w| w.bead.as_str()).collect::<Vec<_>>(), ["w-a", "w-b", "w-c"]);
        assert!(ws[0].in_flight && !ws[2].in_flight);
        assert_eq!(ws[2].outcome.as_ref().map(|o| o.0.as_str()), Some("landed"));
        assert_eq!((ws[1].model.as_str(), ws[1].worktree.as_str()), ("opus", "/wt/w-b"));
    }

    // Case 1: two workers in flight each show their latest actions with times. Case 2: one whose
    // transcript has not been written for 10 minutes is stuck, though its agent says `working`.
    #[test]
    fn workers_in_flight_carry_their_actions_and_a_quiet_one_is_stuck() {
        let ev = latest_run(LOG);
        let now = epoch("2026-11-14T21:30:00Z").unwrap();
        let agents = [agent("/wt/w-a", "p1", "working"), agent("/wt/w-b/", "p2", "working")];
        let lines = r#"{"type":"assistant","timestamp":"2026-11-14T21:25:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build","description":"Build wheelhouse"}}]}}
{"type":"user","timestamp":"2026-11-14T21:25:30Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true}]}}
{"type":"assistant","timestamp":"2026-11-14T21:26:00Z","message":{"content":[{"type":"text","text":"Build   failed;\nfixing."},{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/wt/w-a/src/lib.rs"}}]}}"#;
        let find = |wt: &str, _: i64| match wt {
            "/wt/w-a" => Some((PathBuf::from("/t/a.jsonl"), epoch("2026-11-14T21:26:00Z").unwrap())),
            "/wt/w-b" => Some((PathBuf::from("/t/b.jsonl"), epoch("2026-11-14T21:19:59Z").unwrap())),
            _ => None,
        };
        let v = value_of(&ev, &agents, now, 0, find, |_, wt| actions(lines, wt));
        assert_eq!((v["state"].as_str(), v["lead_pid"].as_i64(), v["started_clock"].as_str()), (Some("live"), Some(42), Some("21:00:00")));
        assert_eq!(v["space"], "wheel");
        let w = v["workers"].as_array().unwrap();
        assert_eq!(w.len(), 3);
        assert_eq!((w[0]["pane"].as_str(), w[0]["stage"].as_str(), w[0]["stuck"].as_bool()), (Some("p1"), Some("build"), Some(false)));
        let a = w[0]["actions"].as_array().unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(a[0], json!({ "at": 1_794_691_500, "clock": "21:25:00", "tool": "Bash", "text": "Build wheelhouse", "done": true, "error": true }));
        assert_eq!((a[1]["tool"].as_str(), a[1]["text"].as_str()), (Some(""), Some("Build failed; fixing.")));
        assert_eq!((a[2]["text"].as_str(), a[2]["done"].as_bool()), (Some("src/lib.rs"), Some(false)));
        assert_eq!((w[1]["pane"].as_str(), w[1]["status"].as_str()), (Some("p2"), Some("working")));
        assert_eq!((w[1]["stuck"].as_bool(), w[1]["stuck_since_clock"].as_str()), (Some(true), Some("21:19:59")));
        assert_eq!(w[1]["actions"].as_array().unwrap().len(), 3);
        // A landed worker is out of flight: never stuck, no actions.
        assert_eq!((w[2]["stuck"].as_bool(), w[2]["outcome"].as_str(), w[2]["actions"].as_array().unwrap().len()), (Some(false), Some("landed"), 0));
    }

    #[test]
    fn a_worker_with_no_transcript_is_stuck_ten_minutes_after_its_launch() {
        let ev = latest_run(LOG);
        let at = |ev: &[Event], t: &str| value_of(ev, &[], epoch(t).unwrap(), 0, |_, _| None, |_, _| vec![]);
        assert_eq!(at(&ev, "2026-11-14T21:10:59Z")["workers"][0]["stuck"], false);
        assert_eq!(at(&ev, "2026-11-14T21:11:00Z")["workers"][0]["stuck"], true);
        assert_eq!(value_of(&[], &[], 0, 0, |_, _| None, |_, _| vec![])["state"], "no-run");
        // A rejected report goes back to the worker, which stays in flight; an ended run has none in flight.
        let rejected = latest_run(&format!("{LOG}2026-11-14T21:21:00Z\trejected\tw-a\tno Run: line\n"));
        let v = at(&rejected, "2026-11-14T21:22:00Z");
        assert_eq!((v["workers"][0]["outcome"].as_str(), v["workers"][0]["in_flight"].as_bool()), (Some("rejected"), Some(true)));
        let ended = latest_run(&format!("{LOG}2026-11-14T21:40:00Z\tend\t\t1 landed\n"));
        let v = at(&ended, "2026-11-14T22:40:00Z");
        assert_eq!(v["state"], "ended");
        assert!(v["workers"].as_array().unwrap().iter().all(|w| w["in_flight"] == false && w["stuck"] == false));
    }

    #[test]
    fn the_transcript_is_the_newest_since_the_launch_and_only_the_last_actions_are_kept() {
        assert_eq!(encoded("/Users/p/.worktrees/wheel/wheel-pmf2.2.8"), "-Users-p--worktrees-wheel-wheel-pmf2-2-8");
        let many: String = (0..20)
            .map(|i| format!(r#"{{"type":"assistant","timestamp":"2026-11-14T21:25:{i:02}Z","message":{{"content":[{{"type":"tool_use","id":"t{i}","name":"Grep","input":{{"pattern":"p{i}"}}}}]}}}}"#) + "\n")
            .collect();
        let a = actions(&many, "/wt");
        assert_eq!((a.len(), a[0].text.as_str(), a[7].text.as_str()), (8, "p12", "p19"));
        let ws = r#"{"type":"assistant","timestamp":"2026-11-14T21:25:00Z","message":{"content":[{"type":"tool_use","id":"w","name":"mcp__work-board__work_set","input":{"items":[{"kind":"bead","id":"a","stage":"build","note":"cargo test"}]}}]}}"#;
        let a = actions(ws, "/wt");
        assert_eq!((a[0].tool.as_str(), a[0].text.as_str()), ("work_set", "build cargo test"));
    }
}
