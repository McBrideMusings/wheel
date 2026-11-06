//! The Runner page: the project's `[runner]` commands, its runs with their state, the status line
//! and the log of the run the pane has open, laid out as A2UI components (ADR-0029: presentation
//! data only). The page matches the HTML runner's controls: one Run button per command, grouped
//! by where the command comes from, and a command with `confirm` shows its question in its row
//! with Cancel and a Run button that names the question, before anything starts.

use serde_json::{json, Value};

use crate::config::Runner;
use crate::status;
use crate::store::{Record, State};

pub const CATALOG: &str = "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json";
pub const SURFACE: &str = "runner";
pub const URI: &str = "a2ui://runner-native";
/// MIME type of the A2UI messages a page is drawn from.
pub const MIME: &str = "application/a2ui+json";

/// How many of a log's last lines the viewer shows.
pub const LOG_LINES: usize = 300;

/// What one pane has open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ui {
    /// The command whose confirm question is showing.
    pub asking: Option<String>,
    /// The run whose log is open.
    pub open: Option<String>,
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

/// The state word and its tone for a run.
fn state_of(r: &Record) -> (String, &'static str) {
    match (r.state, r.exit_code) {
        (State::Running, _) => ("running".into(), "warn"),
        (State::Exited, Some(0)) => ("exit 0".into(), "ok"),
        (State::Exited, Some(c)) => (format!("exit {c}"), "bad"),
        (State::Exited, None) => ("exit ?".into(), "bad"),
        (State::Stopped, _) => ("stopped".into(), "muted"),
        (State::Lost, _) => ("lost".into(), "bad"),
    }
}

/// `runners` are the commands, `runs` the project's runs newest first, `log` the open run's last
/// lines (when one is open and readable) and `at` the time now, in Unix seconds.
pub fn components(runners: &[Runner], runs: &[Record], ui: &Ui, log: Option<&str>, at: i64) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let mut page = vec![];

    let title = p.label("title", "Runner", json!({ "size": 20, "weight": "bold" }));
    let count = match runners.len() {
        1 => "1 runner".to_string(),
        n => format!("{n} runners"),
    };
    let sub = p.label("sub", &count, json!({ "size": 12, "tone": "muted" }));
    page.push(p.boxed("head", "h", vec![title, sub], json!({ "wrap": true, "justify": "between", "spacing": 12, "rowSpacing": 4, "padding": [0, 0, 6, 0] })));

    let line = status::line(runs, at);
    let latest = runs.first().map(|r| state_of(r).1).unwrap_or("muted");
    page.push(p.label("status", &line, json!({ "mono": true, "size": 12, "tone": latest, "padding": [0, 0, 10, 0] })));

    if let Some(e) = &ui.problem {
        let l = p.label("problem.t", e, json!({ "tone": "bad" }));
        page.push(p.boxed("problem", "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [0, 0, 10, 0] })));
    }

    if runners.is_empty() {
        page.push(p.label(
            "none",
            "No runners yet. Add a [runner] table to this project's wheel.toml, such as build = \"make -j8\"; the list updates when you save.",
            json!({ "tone": "muted" }),
        ));
    }
    for (machine, heading) in [(false, "This project"), (true, "This machine")] {
        let rows: Vec<&Runner> = runners.iter().filter(|r| (r.origin == "machine") == machine).collect();
        if rows.is_empty() {
            continue;
        }
        let key = if machine { "machine" } else { "project" };
        let h = p.label(&format!("g.{key}.h"), &format!("{heading}  {}", rows.len()), json!({ "size": 12, "weight": "bold", "tone": "muted", "padding": [10, 0, 4, 0] }));
        let mut group = vec![h];
        for r in rows {
            let id = format!("r.{key}.{}", sane(&r.name));
            let name = p.label(&format!("{id}.name"), &r.name, json!({ "weight": "semibold" }));
            let asking = ui.asking.as_deref() == Some(r.name.as_str()) && r.confirm.is_some();
            let (what, acts) = if asking {
                let q = r.confirm.clone().unwrap_or_default();
                let cancel = p.pill(&format!("{id}.cancel"), "Cancel", "cancel_ask", json!({}), json!({}));
                let go = p.pill(&format!("{id}.go"), &format!("Run {}: {q}", r.name), "press_run", json!({ "name": r.name, "confirmed": true }), json!({}));
                (p.label(&format!("{id}.cmd"), &q, json!({ "tone": "bad" })), vec![cancel, go])
            } else {
                let run = p.pill(&format!("{id}.run"), &format!("Run {}", r.name), "press_run", json!({ "name": r.name }), json!({}));
                (p.label(&format!("{id}.cmd"), &r.command, json!({ "mono": true, "size": 12, "tone": "muted" })), vec![run])
            };
            let buttons = p.boxed(&format!("{id}.acts"), "h", acts, json!({ "spacing": 6 }));
            group.push(p.boxed(&id, "h", vec![name, what, buttons], json!({ "spacing": 12, "widths": [150, "flex", null], "padding": [4, 0], "align": "center", "trailing": 1 })));
        }
        page.push(p.boxed(&format!("g.{key}"), "v", group, json!({ "spacing": 2 })));
    }

    let h = p.label("runs.h", &format!("Runs  {}", runs.len()), json!({ "size": 12, "weight": "bold", "tone": "muted", "padding": [14, 0, 4, 0] }));
    page.push(h);
    let rows: Vec<Value> = runs.iter().map(|r| {
        let (state, tone) = state_of(r);
        let took = crate::duration(r.ended.unwrap_or(at) - r.started);
        let cells = vec![
            cell(&r.name, json!({ "key": "name", "tone": "fg" })),
            cell(state, json!({ "key": "state", "tone": tone, "chip": true })),
            cell(crate::clock(r.started), json!({ "key": "started", "mono": true, "tone": "muted", "tabular": true })),
            cell(took, json!({ "key": "took", "mono": true, "tone": "muted", "tabular": true })),
        ];
        json!({ "id": r.id, "path": r.id, "depth": 0, "kids": 0, "collapsed": false, "dim": false,
            "sel": ui.open.as_deref() == Some(r.id.as_str()), "cells": cells, "menu": [] })
    }).collect();
    let column = |key: &str, label: &str, width: Value, wide: bool| json!({ "key": key, "label": label, "width": width, "tree": false, "wide": wide });
    p.put(json!({ "id": "runs", "component": "Table", "rowAction": "open_log", "foldAction": "open_log",
        "columns": [column("name", "Command", json!("flex"), true), column("state", "State", json!(90), false),
            column("started", "Started (UTC)", json!(110), false), column("took", "Took", json!(70), false)],
        "sort": { "key": "started", "asc": false },
        "sections": [{ "title": "", "count": rows.len(), "rows": rows }], "empty": "No runs yet." }));
    page.push("runs".into());

    if let Some(r) = ui.open.as_deref().and_then(|id| runs.iter().find(|r| r.id == id)) {
        let (state, tone) = state_of(r);
        let head = p.label("log.h", &format!("{} · {state}", r.name), json!({ "weight": "bold", "tone": tone }));
        let mut bar = vec![head];
        if r.state == State::Running {
            bar.push(p.pill("log.stop", "Stop", "stop_open", json!({}), json!({})));
        }
        bar.push(p.pill("log.close", "Close log", "close_log", json!({}), json!({})));
        let top = p.boxed("log.bar", "h", bar, json!({ "spacing": 10, "padding": [14, 0, 4, 0], "align": "center", "trailing": 2 }));
        let cmd = p.label("log.cmd", &r.command, json!({ "mono": true, "size": 11, "tone": "muted" }));
        let text = match log {
            Some(t) if !t.is_empty() => t.to_string(),
            Some(_) => "(no output yet)".into(),
            None => "(the log cannot be read)".into(),
        };
        let body = p.label("log.text", &text, json!({ "mono": true, "size": 11 }));
        let frame = p.boxed("log.frame", "v", vec![body], json!({ "border": "line", "radius": 8, "padding": [8, 10] }));
        page.push(p.boxed("log", "v", vec![top, cmd, frame], json!({ "spacing": 4 })));
    }

    p.put(json!({ "id": "page", "component": "Box", "axis": "v", "align": "start", "padding": [18, 16, 48, 16], "children": page }));
    p.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner(name: &str, confirm: Option<&str>, origin: &'static str) -> Runner {
        Runner { name: name.into(), command: format!("make {name}"), confirm: confirm.map(String::from), origin }
    }

    fn run(id: &str, state: State, code: Option<i32>) -> Record {
        Record {
            id: id.into(), name: "deploy".into(), command: "make deploy".into(), dir: "/p".into(), state, started: 100,
            ended: (state != State::Running).then_some(160), exit_code: code, pid: 2, owner_pid: 1, stop_requested: false, log: "/l".into(),
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
    fn each_command_has_a_run_button_under_where_it_comes_from() {
        let rs = [runner("build", None, "project"), runner("tidy", None, "machine")];
        let c = components(&rs, &[], &Ui::default(), None, 200);
        assert_eq!(find(&c, "g.project.h")["text"], "This project  1");
        assert_eq!(find(&c, "g.machine.h")["text"], "This machine  1");
        let b = find(&c, "r.project.build.run");
        assert_eq!((b["name"].clone(), b["context"].clone(), b["text"].clone()), (json!("press_run"), json!({ "name": "build" }), json!("Run build")));
        assert_eq!(find(&c, "r.project.build.cmd")["text"], "make build");
        assert_eq!(find(&c, "status")["text"], "no runs yet");
    }

    #[test]
    fn a_confirm_command_shows_its_question_with_cancel_and_a_run_that_confirms() {
        let rs = [runner("restart", Some("Ends every agent."), "project")];
        let idle = components(&rs, &[], &Ui::default(), None, 0);
        assert_eq!(find(&idle, "r.project.restart.run")["context"], json!({ "name": "restart" }));
        let ui = Ui { asking: Some("restart".into()), ..Ui::default() };
        let c = components(&rs, &[], &ui, None, 0);
        assert_eq!(find(&c, "r.project.restart.cmd")["text"], "Ends every agent.");
        assert_eq!(find(&c, "r.project.restart.cancel")["name"], "cancel_ask");
        let go = find(&c, "r.project.restart.go");
        assert_eq!(go["text"], "Run restart: Ends every agent.");
        assert_eq!(go["context"], json!({ "name": "restart", "confirmed": true }));
        assert!(c.iter().all(|v| v["id"] != "r.project.restart.run"), "the plain Run is replaced while asking");
    }

    #[test]
    fn runs_show_state_and_the_open_one_shows_its_log_and_a_stop_while_running() {
        let rs = [runner("deploy", None, "project")];
        let runs = [run("b", State::Running, None), run("a", State::Exited, Some(2))];
        let ui = Ui { open: Some("b".into()), ..Ui::default() };
        let c = components(&rs, &runs, &ui, Some("12:00:01  hi\n"), 142);
        let rows = &find(&c, "runs")["sections"][0]["rows"];
        assert_eq!(rows[0]["cells"][1]["text"], "running");
        assert_eq!(rows[0]["sel"], true);
        assert_eq!(rows[1]["cells"][1]["text"], "exit 2");
        assert_eq!(rows[1]["cells"][3]["text"], "1:00");
        assert_eq!(find(&c, "log.text")["text"], "12:00:01  hi\n");
        assert_eq!(find(&c, "log.stop")["name"], "stop_open");
        let done = components(&rs, &runs, &Ui { open: Some("a".into()), ..Ui::default() }, Some(""), 142);
        assert_eq!(find(&done, "log.text")["text"], "(no output yet)");
        assert!(done.iter().all(|v| v["id"] != "log.stop"), "a finished run has no Stop");
    }
}
