//! A bead's button: one slot whose state the host decides. The project writes the commands and
//! labels in its wheel.toml; the host keeps a record of the pane and conversation each press
//! started, and reads the state from that record and the panes that exist now.
//!
//! ```toml
//! [beads.task]                    # a leaf bead; `[beads.epic]` is the same for an epic
//! start.label  = "Implement"      # no record: runs start.run
//! start.run    = '...; echo "$pane"'
//! resume.label = "Resume"         # a record, its pane gone: runs resume.run with $CONVERSATION
//! resume.run   = '...; echo "$pane"'
//! focus.label  = "Focus"          # its pane still open: the host focuses it
//!
//! [beads.disabled]
//! closed = "Closed"               # a closed bead's label
//! ```
//!
//! The last line `start.run` or `resume.run` prints is the pane it started; the host reads that
//! pane's conversation with `wheel pane info` and records both.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// What a verb is called and what it runs; `focus` has no command, the host focuses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verb {
    pub label: Option<String>,
    pub run: Option<String>,
}

/// The three verbs of one kind of bead.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub start: Verb,
    pub resume: Verb,
    pub focus: Verb,
}

/// A project's `[beads.task]`, `[beads.epic]` and `[beads.disabled]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Buttons {
    pub task: Option<Group>,
    pub epic: Option<Group>,
    pub closed: Option<String>,
}

/// Which of its four states a bead's button is in.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Closed,
    /// Its recorded pane is open; the string is that pane.
    Live(String),
    Saved,
    Fresh,
}

/// What a bead's button shows and does now.
#[derive(Clone, Debug, PartialEq)]
pub struct Act {
    pub state: State,
    pub label: String,
    pub disabled: bool,
    pub help: String,
}

/// The pane and conversation a press started.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    pub pane: String,
    pub conversation: String,
}

fn verb(t: &toml_edit::Item, name: &str) -> Verb {
    let get = |k: &str| {
        t.get(name)
            .and_then(|v| v.get(k))
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.trim().is_empty())
    };
    Verb { label: get("label"), run: get("run") }
}

fn group(beads: &toml_edit::Item, kind: &str) -> Option<Group> {
    let t = beads.get(kind)?;
    Some(Group { start: verb(t, "start"), resume: verb(t, "resume"), focus: verb(t, "focus") })
}

/// The buttons the project's wheel.toml in `dir` writes, and what is wrong with the file.
pub fn read(dir: &str) -> (Buttons, Vec<String>) {
    let file = PathBuf::from(dir).join("wheel.toml");
    let Ok(text) = std::fs::read_to_string(&file) else { return (Buttons::default(), vec![]) };
    let doc: toml_edit::DocumentMut = match text.parse() {
        Ok(t) => t,
        Err(e) => return (Buttons::default(), vec![format!("{}: {e}", file.display())]),
    };
    let Some(beads) = doc.get("beads") else { return (Buttons::default(), vec![]) };
    let mut problems = vec![];
    if beads.get("buttons").is_some() {
        problems.push(format!(
            "{}: [beads.buttons.*] is gone; write [beads.task] and [beads.epic] with start, resume and focus",
            file.display()
        ));
    }
    let closed = beads.get("disabled").and_then(|d| d.get("closed")).and_then(|v| v.as_str()).map(String::from);
    (Buttons { task: group(beads, "task"), epic: group(beads, "epic"), closed }, problems)
}

/// The button of a bead whose kind is `epic` (a parent) or not, in the state its record and the
/// panes open now put it in. A kind the project wrote no group for has no button.
pub fn decide(b: &Buttons, status: &str, epic: bool, record: Option<&Record>, panes: &HashSet<String>) -> Option<Act> {
    if status == "closed" {
        let label = b.closed.clone().unwrap_or_else(|| "Closed".into());
        return Some(Act { state: State::Closed, label, disabled: true, help: "This bead is closed".into() });
    }
    let g = if epic { b.epic.as_ref() } else { b.task.as_ref() }?;
    let kind = if epic { "epic" } else { "task" };
    let act = |state: State, v: &Verb, fallback: &str, verb: &str, needs_run: bool| {
        let missing = needs_run && v.run.is_none();
        Act {
            state,
            label: v.label.clone().unwrap_or_else(|| fallback.into()),
            disabled: missing,
            help: if missing { format!("[beads.{kind}] has no {verb}.run") } else { String::new() },
        }
    };
    Some(match record {
        Some(r) if panes.contains(&r.pane) => act(State::Live(r.pane.clone()), &g.focus, "Focus", "focus", false),
        Some(_) => act(State::Saved, &g.resume, "Resume", "resume", true),
        None => act(State::Fresh, &g.start, "Start", "start", true),
    })
}

impl State {
    /// The state as the command line prints it.
    pub fn word(&self) -> String {
        match self {
            State::Closed => "closed".into(),
            State::Live(p) => format!("live {p}"),
            State::Saved => "saved".into(),
            State::Fresh => "fresh".into(),
        }
    }
}

/// The command a press runs, when it runs one.
pub fn command<'a>(b: &'a Buttons, epic: bool, state: &State) -> Option<&'a str> {
    let g = if epic { b.epic.as_ref() } else { b.task.as_ref() }?;
    match state {
        State::Fresh => g.start.run.as_deref(),
        State::Saved => g.resume.run.as_deref(),
        State::Closed | State::Live(_) => None,
    }
}

/// `~/.wheel/bead-panes.json`: every project's records, in one machine-wide file.
pub fn records_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".wheel").join("bead-panes.json")
}

fn key(project: &str, bead: &str) -> String {
    format!("{project}\n{bead}")
}

/// The records in `file`: none when it does not exist, an error when it exists but cannot be read, so a
/// damaged file is never replaced by one that holds a single record.
fn load(file: &Path) -> Result<BTreeMap<String, Record>, String> {
    match std::fs::read_to_string(file) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The beads whose button a press has started and not yet recorded, as `project\nbead`.
static PRESSED: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);

/// Claim `bead` in `project` for a press; false when a press already holds it.
pub fn claim(project: &str, bead: &str) -> bool {
    PRESSED.lock().unwrap().get_or_insert_with(HashSet::new).insert(key(project, bead))
}

/// Let go of a press's claim.
pub fn release(project: &str, bead: &str) {
    if let Some(s) = PRESSED.lock().unwrap().as_mut() {
        s.remove(&key(project, bead));
    }
}

/// Whether a press holds `bead` in `project` now.
pub fn pressed(project: &str, bead: &str) -> bool {
    PRESSED.lock().unwrap().as_ref().is_some_and(|s| s.contains(&key(project, bead)))
}

/// The record of `bead` in `project`.
pub fn recorded(file: &Path, project: &str, bead: &str) -> Option<Record> {
    load(file).ok()?.remove(&key(project, bead))
}

/// Every bead's record in `project`, by bead id.
pub fn recorded_all(file: &Path, project: &str) -> BTreeMap<String, Record> {
    let prefix = format!("{project}\n");
    load(file).unwrap_or_default().into_iter().filter_map(|(k, v)| k.strip_prefix(&prefix).map(|b| (b.to_string(), v))).collect()
}

/// Replace the record of `bead` in `project`.
pub fn record(file: &Path, project: &str, bead: &str, r: Record) -> Result<(), String> {
    let _writing = WRITING.lock().unwrap();
    let mut all = load(file)?;
    all.insert(key(project, bead), r);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = file.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))
}

/// The panes `wheel pane ls --json` lists, or why it could not.
pub fn open_panes(dir: &str) -> Result<HashSet<String>, String> {
    match crate::core::run::json("wheel", dir, &["pane", "ls", "--json"])? {
        serde_json::Value::Array(rows) => Ok(rows.iter().filter_map(|r| r["pane"].as_str().map(String::from)).collect()),
        other => Err(format!("wheel pane ls: expected a list, got {other}")),
    }
}

/// The conversation `wheel pane info <pane>` prints.
pub fn conversation_of(dir: &str, pane: &str) -> Result<String, String> {
    let info = crate::core::run::text("wheel", dir, &["pane", "info", pane])?;
    info.lines()
        .find_map(|l| l.strip_prefix("Conversation:"))
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .ok_or_else(|| format!("pane {pane} has no conversation"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group_of(prefix: &str) -> Group {
        let v = |n: &str, run: bool| Verb {
            label: Some(format!("{prefix} {n}")),
            run: run.then(|| format!("{n}.sh")),
        };
        Group { start: v("start", true), resume: v("resume", true), focus: v("focus", false) }
    }

    fn buttons() -> Buttons {
        Buttons { task: Some(group_of("task")), epic: Some(group_of("epic")), closed: Some("Done".into()) }
    }

    fn rec(p: &str) -> Record {
        Record { pane: p.into(), conversation: "c1".into() }
    }

    fn open(p: &[&str]) -> HashSet<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_bead_with_no_record_offers_start() {
        let a = decide(&buttons(), "open", false, None, &open(&[])).unwrap();
        assert_eq!((a.state, a.label.as_str(), a.disabled), (State::Fresh, "task start", false));
    }

    #[test]
    fn a_record_whose_pane_is_gone_offers_resume() {
        let a = decide(&buttons(), "in_progress", false, Some(&rec("p1")), &open(&["p2"])).unwrap();
        assert_eq!((a.state, a.label.as_str(), a.disabled), (State::Saved, "task resume", false));
    }

    #[test]
    fn a_record_whose_pane_is_open_offers_focus_and_runs_nothing() {
        let a = decide(&buttons(), "in_progress", false, Some(&rec("p1")), &open(&["p1"])).unwrap();
        assert_eq!((a.state.clone(), a.label.as_str(), a.disabled), (State::Live("p1".into()), "task focus", false));
        assert_eq!(command(&buttons(), false, &a.state), None);
    }

    #[test]
    fn a_closed_bead_is_disabled_whatever_its_record_says() {
        let a = decide(&buttons(), "closed", false, Some(&rec("p1")), &open(&["p1"])).unwrap();
        assert_eq!((a.state, a.label.as_str(), a.disabled), (State::Closed, "Done", true));
    }

    #[test]
    fn an_epic_uses_its_own_group() {
        let a = decide(&buttons(), "open", true, None, &open(&[])).unwrap();
        assert_eq!(a.label, "epic start");
        assert_eq!(command(&buttons(), true, &a.state), Some("start.sh"));
    }

    #[test]
    fn a_verb_without_a_run_is_disabled_and_says_which() {
        let mut b = buttons();
        b.task.as_mut().unwrap().resume.run = None;
        let a = decide(&b, "open", false, Some(&rec("p1")), &open(&[])).unwrap();
        assert!(a.disabled);
        assert_eq!(a.help, "[beads.task] has no resume.run");
    }

    #[test]
    fn a_kind_the_project_wrote_nothing_for_has_no_button() {
        let b = Buttons { task: None, ..buttons() };
        assert_eq!(decide(&b, "open", false, None, &open(&[])), None);
    }

    #[test]
    fn the_toml_reads_into_groups_and_flags_the_old_tables() {
        let dir = std::env::temp_dir().join(format!("wheel-buttons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("wheel.toml"),
            "[beads.task]\nstart.label = \"Implement\"\nstart.run = 'echo p1'\nfocus.label = \"Focus\"\n\n[beads.disabled]\nclosed = \"Closed\"\n\n[beads.buttons.old]\nrun = 'x'\n",
        )
        .unwrap();
        let (b, problems) = read(dir.to_str().unwrap());
        let t = b.task.unwrap();
        assert_eq!(t.start, Verb { label: Some("Implement".into()), run: Some("echo p1".into()) });
        assert_eq!(t.resume, Verb::default());
        assert_eq!(t.focus.label.as_deref(), Some("Focus"));
        assert_eq!(b.epic, None);
        assert_eq!(b.closed.as_deref(), Some("Closed"));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("[beads.buttons.*] is gone"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_press_holds_its_bead_until_it_lets_go() {
        assert!(claim("/p", "b-1"));
        assert!(!claim("/p", "b-1"), "a second press on the same bead is refused");
        assert!(claim("/p", "b-2") && claim("/q", "b-1"));
        assert!(pressed("/p", "b-1"));
        release("/p", "b-1");
        assert!(!pressed("/p", "b-1") && claim("/p", "b-1"));
        for (p, b) in [("/p", "b-1"), ("/p", "b-2"), ("/q", "b-1")] {
            release(p, b);
        }
    }

    #[test]
    fn a_damaged_record_file_is_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("wheel-damaged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bead-panes.json");
        std::fs::write(&file, "{not json").unwrap();
        assert!(record(&file, "/a", "x", rec("p1")).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{not json");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_record_is_kept_per_project_and_bead_and_replaced() {
        let dir = std::env::temp_dir().join(format!("wheel-records-{}", std::process::id()));
        let file = dir.join("bead-panes.json");
        record(&file, "/a", "x-1", rec("p1")).unwrap();
        record(&file, "/b", "x-1", rec("p9")).unwrap();
        record(&file, "/a", "x-1", rec("p2")).unwrap();
        assert_eq!(recorded(&file, "/a", "x-1"), Some(rec("p2")));
        assert_eq!(recorded(&file, "/b", "x-1"), Some(rec("p9")));
        assert_eq!(recorded(&file, "/a", "x-2"), None);
        assert_eq!(recorded_all(&file, "/a").len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
