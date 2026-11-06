//! The Runner page: the project's `[runner]` commands, its runs with their state, the status line
//! and the log of the run the pane has open, laid out as A2UI components (ADR-0029: presentation
//! data only). The page matches the HTML runner's controls: one Run button per command, grouped
//! by where the command comes from, and a command with `confirm` shows its question in its row
//! with Cancel and a Run button that names the question, before anything starts.

use serde_json::{json, Value};

use crate::config::Runner;
use crate::store::{Record, State};

pub const CATALOG: &str = "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json";
pub const SURFACE: &str = "runner";
pub const URI: &str = "a2ui://runner-native";
/// MIME type of the A2UI messages a page is drawn from.
pub const MIME: &str = "application/a2ui+json";

/// How many of a log's last lines the viewer shows.
pub const LOG_LINES: usize = 300;
/// The runs shown before "Show older runs", and how many each press adds.
const BATCH: usize = 5;

/// The runs the page shows a block for: the newest not closed, `BATCH` plus a batch per press.
pub fn shown<'a>(runs: &'a [Record], ui: &Ui) -> Vec<&'a Record> {
    runs.iter().filter(|r| !ui.closed.contains(&r.id)).take(BATCH * (1 + ui.more)).collect()
}

/// What one pane has open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ui {
    /// The command whose confirm question is showing.
    pub asking: Option<String>,
    /// The runs whose block the person closed.
    pub closed: std::collections::BTreeSet<String>,
    /// How many more batches of older runs the person asked for.
    pub more: usize,
    /// What went wrong with the last press, shown until the next one.
    pub problem: Option<String>,
}

/// The messages a read of the page answers with before the first call draws it.
pub fn skeleton() -> Value {
    json!([
        { "version": "v0.9", "createSurface": { "surfaceId": SURFACE, "catalogId": CATALOG } },
        { "version": "v0.9", "updateComponents": { "surfaceId": SURFACE, "components": [
            { "id": "root", "component": "Column", "align": "stretch", "children": ["page"] },
            { "id": "page", "component": "Column", "children": [] }
        ] } }
    ])
}

/// The `updateComponents` message for `components`.
pub fn update(components: Vec<Value>) -> Value {
    json!([{ "version": "v0.9", "updateComponents": { "surfaceId": SURFACE, "components": components } }])
}

struct Page {
    out: Vec<Value>,
}

impl Page {
    fn put(&mut self, v: Value) {
        self.out.push(v);
    }
    fn label(&mut self, id: &str, text: &str, extra: Value) -> String {
        let mut v = json!({ "id": id, "component": "Label", "text": text });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.put(v);
        id.to_string()
    }
    fn boxed(&mut self, id: &str, axis: &str, children: Vec<String>, extra: Value) -> String {
        let mut v = json!({ "id": id, "component": "Box", "axis": axis, "children": children });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.put(v);
        id.to_string()
    }
    fn pill(&mut self, id: &str, text: &str, name: &str, context: Value, extra: Value) -> String {
        let mut v = json!({ "id": id, "component": "Pill", "text": text, "name": name, "context": context });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.put(v);
        id.to_string()
    }
}

/// A component id made of `name`: letters and digits stay, every other byte becomes `_` and its
/// two hex digits, so two names never share an id.
fn sane(name: &str) -> String {
    name.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("_{b:02x}") }).collect()
}

fn cell(text: impl Into<String>, extra: Value) -> Value {
    let mut v = json!({ "text": text.into() });
    v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    v
}

/// The state word, its SF Symbol (`spinner` while running) and its tone for a run.
fn state_of(r: &Record) -> (String, &'static str, &'static str) {
    match (r.state, r.exit_code) {
        (State::Running, _) => ("running".into(), "spinner", "warn"),
        (State::Exited, Some(0)) => ("exit 0".into(), "checkmark.circle.fill", "ok"),
        (State::Exited, Some(c)) => (format!("exit {c}"), "xmark.circle.fill", "bad"),
        (State::Exited, None) => ("exit ?".into(), "xmark.circle.fill", "bad"),
        (State::Stopped, _) => ("stopped".into(), "stop.circle.fill", "muted"),
        (State::Lost, _) => ("lost".into(), "xmark.circle.fill", "bad"),
    }
}

/// A section heading: the words and, muted beside them, how many there are.
fn heading(p: &mut Page, id: &str, text: &str, n: usize) -> String {
    let t = p.label(&format!("{id}.t"), text, json!({ "weight": "semibold" }));
    let c = p.label(&format!("{id}.n"), &n.to_string(), json!({ "tone": "muted", "tabular": true }));
    p.boxed(id, "h", vec![t, c], json!({ "align": "center" }))
}

/// `runners` are the commands, `runs` the project's runs newest first, `logs` the last lines of
/// each shown run's log (a run absent from it has an unreadable log) and `at` the time now, in Unix seconds.
pub fn components(runners: &[Runner], runs: &[Record], ui: &Ui, logs: &std::collections::HashMap<String, String>, at: i64) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let mut page = vec![];

    let title = p.label("title", "Runner", json!({ "size": 22, "weight": "bold" }));
    let count = match runners.len() {
        1 => "1 runner".to_string(),
        n => format!("{n} runners"),
    };
    let sub = p.label("sub", &count, json!({ "tone": "muted" }));
    page.push(p.boxed("head", "h", vec![title, sub], json!({ "wrap": true, "justify": "between" })));

    if let Some(e) = &ui.problem {
        let l = p.label("problem.t", e, json!({ "tone": "bad" }));
        page.push(p.boxed("problem", "v", vec![l], json!({ "border": "bad" })));
    }

    if runners.is_empty() {
        page.push(p.label(
            "none",
            "No runners yet. Add a [runner] table to this project's wheel.toml, such as build = \"make -j8\"; the list updates when you save.",
            json!({ "tone": "muted" }),
        ));
    }
    for (machine, title) in [(false, "This project"), (true, "This machine")] {
        let rows: Vec<&Runner> = runners.iter().filter(|r| (r.origin == "machine") == machine).collect();
        if rows.is_empty() {
            continue;
        }
        let key = if machine { "machine" } else { "project" };
        page.push(heading(&mut p, &format!("g.{key}.h"), title, rows.len()));
        let mut lines = vec![];
        for (i, r) in rows.iter().enumerate() {
            let id = format!("r.{key}.{}", sane(&r.name));
            let name = p.label(&format!("{id}.name"), &r.name, json!({ "weight": "semibold" }));
            let asking = ui.asking.as_deref() == Some(r.name.as_str()) && r.confirm.is_some();
            let (what, acts) = if asking {
                let q = r.confirm.clone().unwrap_or_default();
                let cancel = p.pill(&format!("{id}.cancel"), "", "cancel_ask", json!({}), json!({ "icon": "xmark", "help": "Cancel" }));
                let go = p.pill(&format!("{id}.go"), &format!("Run {}: {q}", r.name), "press_run", json!({ "name": r.name, "confirmed": r.command }), json!({ "icon": "checkmark", "help": format!("Run {}: {q}", r.name) }));
                (p.label(&format!("{id}.cmd"), &q, json!({ "tone": "bad" })), vec![cancel, go])
            } else {
                let run = p.pill(&format!("{id}.run"), "", "press_run", json!({ "name": r.name }), json!({ "icon": "play.fill", "help": format!("Run {}", r.name) }));
                (p.label(&format!("{id}.cmd"), &r.command, json!({ "mono": true, "tone": "muted" })), vec![run])
            };
            let buttons = p.boxed(&format!("{id}.acts"), "h", acts, json!({}));
            let edge = if i + 1 < rows.len() { json!({ "edge": "bottom" }) } else { json!({}) };
            let mut extra = json!({ "widths": [210, "flex", null], "align": "center", "trailing": 1 });
            extra.as_object_mut().unwrap().extend(edge.as_object().unwrap().clone());
            lines.push(p.boxed(&id, "h", vec![name, what, buttons], extra));
        }
        page.push(p.boxed(&format!("g.{key}"), "v", lines, json!({ "border": "line", "spacing": 0, "padding": [0, 0] })));
    }

    let show = shown(runs, ui);
    page.push(heading(&mut p, "runs.h", "Output", runs.iter().filter(|r| !ui.closed.contains(&r.id)).count()));
    if show.is_empty() {
        page.push(p.label("runs.none", "No runs yet.", json!({ "tone": "muted" })));
    }
    let mut blocks = vec![];
    for r in &show {
        let (state, symbol, tone) = state_of(r);
        let took = crate::duration(r.ended.unwrap_or(at) - r.started);
        let id = format!("run.{}", sane(&r.id));
        let name = p.label(&format!("{id}.name"), &r.name, json!({ "weight": "semibold" }));
        let st = p.label(&format!("{id}.state"), &state, json!({ "symbol": symbol, "tone": tone }));
        let when = p.label(&format!("{id}.when"), &format!("{} · {took}", crate::clock(r.started)), json!({ "mono": true, "tone": "muted", "tabular": true }));
        let mut bar = vec![name, st, when];
        if r.state == State::Running {
            bar.push(p.pill(&format!("{id}.stop"), "Stop", "stop_log", json!({ "path": r.id }), json!({})));
        }
        bar.push(p.pill(&format!("{id}.close"), "", "close_log", json!({ "path": r.id }), json!({ "icon": "xmark", "help": "Close", "style": "plain" })));
        let trailing = if r.state == State::Running { 2 } else { 1 };
        let head = p.boxed(&format!("{id}.head"), "h", bar, json!({ "align": "center", "trailing": trailing, "edge": "bottom" }));
        let text = match logs.get(&r.id) {
            Some(t) if !t.is_empty() => t.trim_end().to_string(),
            Some(_) => "(no output yet)".into(),
            None => "(the log cannot be read)".into(),
        };
        let body = p.label(&format!("{id}.log"), &text, json!({ "mono": true }));
        let body = p.boxed(&format!("{id}.out"), "v", vec![body], json!({ "padding": [12, 12] }));
        blocks.push(p.boxed(&id, "v", vec![head, body], json!({ "border": "line", "spacing": 0, "padding": [0, 0] })));
    }
    let later = runs.iter().filter(|r| !ui.closed.contains(&r.id)).count().saturating_sub(show.len());
    if later > 0 {
        blocks.push(p.pill("runs.more", &format!("Show older runs · {later}"), "show_older", json!({}), json!({ "style": "link" })));
    }
    if !blocks.is_empty() {
        page.push(p.boxed("runs", "v", blocks, json!({ "spacing": 16 })));
    }

    p.put(json!({ "id": "page", "component": "Box", "axis": "v", "align": "start", "padding": [28, 28, 56, 28], "spacing": 24, "children": page }));
    p.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner(name: &str, confirm: Option<&str>, origin: &'static str) -> Runner {
        Runner { name: name.into(), command: format!("make {name}"), confirm: confirm.map(String::from), origin, requires: vec![], status: None, worktree: false }
    }

    fn run(id: &str, state: State, code: Option<i32>) -> Record {
        Record {
            id: id.into(), name: "deploy".into(), command: "make deploy".into(), dir: "/p".into(), state, started: 100,
            ended: (state != State::Running).then_some(160), exit_code: code, pid: 2, pid_started: 0, owner_pid: 1, stop_requested: false, log: "/l".into(),
        }
    }

    #[test]
    fn names_that_differ_only_in_punctuation_get_different_ids() {
        assert_ne!(sane("a.b"), sane("a-b"));
        assert_eq!(sane("build"), "build");
    }

    fn find<'a>(c: &'a [Value], id: &str) -> &'a Value {
        c.iter().find(|v| v["id"] == id).unwrap_or_else(|| panic!("no component {id}"))
    }

    #[test]
    fn each_command_has_an_icon_run_button_under_where_it_comes_from() {
        let rs = [runner("build", None, "project"), runner("tidy", None, "machine")];
        let c = components(&rs, &[], &Ui::default(), &Default::default(), 200);
        assert_eq!(find(&c, "g.project.h.t")["text"], "This project");
        assert_eq!(find(&c, "g.project.h.n")["text"], "1");
        assert_eq!(find(&c, "g.machine.h.t")["text"], "This machine");
        assert_eq!(find(&c, "g.project")["border"], "line");
        assert_eq!(find(&c, "r.project.build.name")["text"], "build");
        assert_eq!(find(&c, "r.project.build.cmd")["text"], "make build");
        let b = find(&c, "r.project.build.run");
        assert_eq!((b["name"].clone(), b["context"].clone(), b["icon"].clone(), b["help"].clone()),
            (json!("press_run"), json!({ "name": "build" }), json!("play.fill"), json!("Run build")));
        assert_eq!(find(&c, "runs.none")["text"], "No runs yet.");
        assert!(c.iter().all(|v| v["id"] != "status"), "the page has no status line");
    }

    #[test]
    fn a_confirm_command_shows_its_question_with_cancel_and_a_run_that_confirms() {
        let rs = [runner("restart", Some("Ends every agent."), "project")];
        let idle = components(&rs, &[], &Ui::default(), &Default::default(), 0);
        assert_eq!(find(&idle, "r.project.restart.run")["context"], json!({ "name": "restart" }));
        let ui = Ui { asking: Some("restart".into()), ..Ui::default() };
        let c = components(&rs, &[], &ui, &Default::default(), 0);
        assert_eq!(find(&c, "r.project.restart.cmd")["text"], "Ends every agent.");
        assert_eq!(find(&c, "r.project.restart.cancel")["name"], "cancel_ask");
        let go = find(&c, "r.project.restart.go");
        assert_eq!(go["help"], "Run restart: Ends every agent.");
        assert_eq!(go["context"], json!({ "name": "restart", "confirmed": "make restart" }));
        assert!(c.iter().all(|v| v["id"] != "r.project.restart.run"), "the plain Run is replaced while asking");
    }

    #[test]
    fn every_run_has_a_block_with_its_log_and_a_stop_while_running() {
        let rs = [runner("deploy", None, "project")];
        let runs = [run("b", State::Running, None), run("a", State::Exited, Some(2))];
        let logs = std::collections::HashMap::from([("b".to_string(), "12:00:01  hi\n".to_string()), ("a".to_string(), String::new())]);
        let c = components(&rs, &runs, &Ui::default(), &logs, 142);
        assert_eq!(find(&c, "run.b")["border"], "line");
        assert_eq!(find(&c, "run.b.state")["text"], "running");
        assert_eq!(find(&c, "run.b.state")["symbol"], "spinner");
        assert_eq!(find(&c, "run.a.state")["text"], "exit 2");
        assert_eq!(find(&c, "run.a.state")["symbol"], "xmark.circle.fill");
        assert_eq!(find(&c, "run.a.when")["text"], "00:01:40 · 1:00".replace("00:01:40", &crate::clock(100)));
        assert_eq!(find(&c, "run.b.log")["text"], "12:00:01  hi");
        assert_eq!(find(&c, "run.a.log")["text"], "(no output yet)");
        let stop = find(&c, "run.b.stop");
        assert_eq!((stop["name"].clone(), stop["context"].clone()), (json!("stop_log"), json!({ "path": "b" })));
        assert!(c.iter().all(|v| v["id"] != "run.a.stop"), "a finished run has no Stop");
        assert_eq!(find(&c, "run.a.close")["context"], json!({ "path": "a" }));
    }

    #[test]
    fn closed_runs_drop_out_and_older_ones_come_in_batches_of_five() {
        let runs: Vec<Record> = (0..12).map(|i| run(&format!("r{i:02}"), State::Exited, Some(0))).collect();
        let c = components(&[], &runs, &Ui::default(), &Default::default(), 200);
        assert_eq!(c.iter().filter(|v| v["border"] == "line" && v["id"].as_str().unwrap().starts_with("run.r")).count(), 5);
        assert_eq!(find(&c, "runs.more")["text"], "Show older runs · 7");
        let ui = Ui { more: 1, closed: ["r00".to_string()].into(), ..Ui::default() };
        let c = components(&[], &runs, &ui, &Default::default(), 200);
        assert!(c.iter().all(|v| v["id"] != "run.r00"));
        assert_eq!(c.iter().filter(|v| v["border"] == "line" && v["id"].as_str().unwrap().starts_with("run.r")).count(), 10);
        assert_eq!(find(&c, "runs.more")["text"], "Show older runs · 1");
    }
}
