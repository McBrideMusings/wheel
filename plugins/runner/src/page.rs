//! The Runner page, laid out as A2UI components (ADR-0029: presentation data only). Two views of
//! one pane. The list: a search field over the project's commands as a tree (groups and
//! sub-groups come from the nesting of `[runner]` tables), a Running section pinned above it. A
//! row is the command and its latest run: Run starts it, Open (once it has a run) shows that run's
//! log. The log view: one run's output filling the pane, a back button to the list and a button
//! for each other running command. A command with `confirm` shows its question in its row with
//! Cancel and a Run that names the question, before anything starts.

use std::collections::{BTreeMap, BTreeSet};

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

/// What one pane has open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ui {
    /// The command whose confirm question is showing.
    pub asking: Option<String>,
    /// The run whose log fills the pane; none shows the list.
    pub open: Option<String>,
    /// What the search field holds.
    pub query: String,
    /// The groups folded shut, by their dotted path.
    pub folded: BTreeSet<String>,
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

/// The messages for `components`: one `updateComponents`, then an `updateDataModel` for each entry
/// that carries a data-model value (the search field's text) rather than a component.
pub fn update(components: Vec<Value>) -> Value {
    let (data, components): (Vec<Value>, Vec<Value>) = components.into_iter().partition(|c| c.get("updateDataModel").is_some());
    let mut messages = vec![json!({ "version": "v0.9", "updateComponents": { "surfaceId": SURFACE, "components": components } })];
    for mut m in data {
        m["version"] = json!("v0.9");
        m["updateDataModel"]["surfaceId"] = json!(SURFACE);
        messages.push(m);
    }
    Value::Array(messages)
}

#[path = "../../wheelhouse/src/core/layout.rs"]
mod layout;

struct Page {
    out: Vec<Value>,
}

impl Page {
    fn put(&mut self, v: Value) {
        self.out.push(v);
    }
    fn label(&mut self, id: &str, text: &str, extra: Value) -> String {
        layout::label(&mut self.out, id, text, &extra)
    }
    fn boxed(&mut self, id: &str, axis: &str, children: Vec<String>, extra: Value) -> String {
        layout::boxed(&mut self.out, id, axis, children, &extra)
    }
    /// A library `Button` whose label is an `Icon` (`extra.icon`), a `Text`, or the component
    /// `extra.child` already put. `extra`: `style` link or plain draws it borderless, `help` is its
    /// hover text and its accessibility label.
    fn button(&mut self, id: &str, text: &str, name: &str, context: Value, extra: Value) -> String {
        let child = if let Some(c) = extra["child"].as_str() {
            c.to_string()
        } else if let Some(i) = extra["icon"].as_str() {
            self.put(json!({ "id": format!("{id}.i"), "component": "Icon", "name": i }));
            format!("{id}.i")
        } else {
            self.put(json!({ "id": format!("{id}.t"), "component": "Text", "text": text }));
            format!("{id}.t")
        };
        let mut mods = vec![];
        if let Some(h) = extra["help"].as_str() {
            mods.push(json!({ "help": h }));
            mods.push(json!({ "accessibilityLabel": h }));
        }
        let mut v = json!({ "id": id, "component": "Button", "child": child, "action": { "event": { "name": name, "context": context } } });
        if matches!(extra["style"].as_str(), Some("link" | "plain")) {
            v["variant"] = json!("borderless");
        }
        if !mods.is_empty() {
            v["modifiers"] = json!(mods);
        }
        self.put(v);
        id.to_string()
    }
    /// An icon-only button.
    fn icon(&mut self, id: &str, icon: &str, help: &str, name: &str, context: Value) -> String {
        self.button(id, "", name, context, json!({ "icon": icon, "help": help }))
    }
}

/// A component id made of `name`: letters and digits stay, every other byte becomes `_` and its
/// two hex digits, so two names never share an id.
fn sane(name: &str) -> String {
    name.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("_{b:02x}") }).collect()
}

/// The state word, its SF Symbol and its tone for a run.
fn state_of(r: &Record) -> (String, &'static str, &'static str) {
    match (r.state, r.exit_code) {
        (State::Running, _) => ("running".into(), "circle.fill", "warn"),
        (State::Exited, Some(0)) => ("exit 0".into(), "checkmark.circle.fill", "ok"),
        (State::Exited, Some(c)) => (format!("exit {c}"), "xmark.circle.fill", "bad"),
        (State::Exited, None) => ("exit ?".into(), "xmark.circle.fill", "bad"),
        (State::Stopped, _) => ("stopped".into(), "stop.circle.fill", "muted"),
        (State::Lost, _) => ("lost".into(), "xmark.circle.fill", "bad"),
    }
}

/// A run's status as one line: `running 2:14` while it goes, else `exit 65 · 17:40:12`.
fn status_text(r: &Record, at: i64) -> String {
    let (state, _, _) = state_of(r);
    if r.state == State::Running {
        format!("{state} {}", crate::duration(at - r.started))
    } else {
        format!("{state} · {}", crate::clock(r.started))
    }
}

/// Whether `r` answers the search `q` (already lower case): its label, description, group path or
/// identifier holds it.
fn matches(r: &Runner, q: &str) -> bool {
    q.is_empty()
        || [r.label.as_str(), r.desc.as_deref().unwrap_or(""), &r.group.join(" "), &r.name].iter().any(|t| t.to_lowercase().contains(q))
}

/// One level of the tree: its commands, then its groups, each in the order the file gave them.
#[derive(Default)]
struct Node<'a> {
    commands: Vec<&'a Runner>,
    groups: Vec<(String, Node<'a>)>,
}

impl<'a> Node<'a> {
    fn insert(&mut self, path: &[String], r: &'a Runner) {
        match path.split_first() {
            None => self.commands.push(r),
            Some((head, rest)) => {
                let i = self.groups.iter().position(|(k, _)| k == head).unwrap_or_else(|| {
                    self.groups.push((head.clone(), Node::default()));
                    self.groups.len() - 1
                });
                self.groups[i].1.insert(rest, r);
            }
        }
    }
    fn count(&self) -> usize {
        self.commands.len() + self.groups.iter().map(|(_, n)| n.count()).sum::<usize>()
    }
}

/// `runners` are the commands, `runs` the project's runs newest first, `log` the last lines of
/// the open run's log (absent when its log cannot be read) and `at` the time now, in Unix seconds.
pub fn components(runners: &[Runner], runs: &[Record], ui: &Ui, log: Option<&str>, at: i64) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let mut page = vec![];

    // No page title: the pane's own bar says which page it is.
    if let Some(e) = &ui.problem {
        let l = p.label("problem.t", e, json!({ "tone": "bad" }));
        page.push(p.boxed("problem", "v", vec![l], json!({ "border": "bad" })));
    }

    let open = ui.open.as_ref().and_then(|id| runs.iter().find(|r| &r.id == id));
    match open {
        Some(run) => log_view(&mut p, &mut page, runners, runs, run, log, at),
        None => list_view(&mut p, &mut page, runners, runs, ui, at),
    }

    p.boxed("page", "v", page, json!({ "align": "start", "padding": [20, 20, 40, 20], "spacing": 14 }));
    // Every column stretches, so rows fill the pane and every button sits at the right edge.
    for c in p.out.iter_mut().filter(|c| c["component"] == "Column") {
        c["align"] = json!("stretch");
    }
    p.out
}

fn list_view(p: &mut Page, page: &mut Vec<String>, runners: &[Runner], runs: &[Record], ui: &Ui, at: i64) {
    p.bind("/query", json!(ui.query));
    p.put(json!({ "id": "search", "component": "TextField", "label": "Search commands", "value": { "path": "/query" }, "variant": "shortText" }));
    page.push("search".into());

    if runners.is_empty() {
        page.push(p.label(
            "none",
            "No runners yet. Add a [runner] table to this project's wheel.toml, such as build = \"make -j8\"; the list updates when you save.",
            json!({ "tone": "muted" }),
        ));
        return;
    }
    let q = ui.query.trim().to_lowercase();
    let shown: Vec<&Runner> = runners.iter().filter(|r| matches(r, &q)).collect();
    if shown.is_empty() {
        page.push(p.label("none", "No command matches.", json!({ "tone": "muted" })));
        return;
    }
    // The newest run of each command: runs are newest first.
    let mut latest: BTreeMap<&str, &Record> = BTreeMap::new();
    for r in runs {
        latest.entry(r.name.as_str()).or_insert(r);
    }

    let live: Vec<(&Runner, &Record)> = shown.iter().filter_map(|r| latest.get(r.name.as_str()).filter(|x| x.state == State::Running).map(|x| (*r, *x))).collect();
    if !live.is_empty() {
        page.push(heading(p, "live.h", "Running", live.len()));
        let mut rows = vec![];
        for (i, (r, run)) in live.iter().enumerate() {
            rows.push(live_row(p, i, r, run, at));
        }
        page.push(p.boxed("live", "v", rows, json!({ "border": "line", "spacing": 0, "padding": [0, 0] })));
    }

    let mut root = Node::default();
    for r in &shown {
        root.insert(&r.group, r);
    }
    page.push(heading(p, "all.h", "Commands", shown.len()));
    let tree = node(p, "t", &root, &[], &latest, ui, !q.is_empty(), at);
    page.push(tree);
}

/// A section heading: the words and, muted beside them, how many there are.
fn heading(p: &mut Page, id: &str, text: &str, n: usize) -> String {
    let t = p.label(&format!("{id}.t"), text, json!({ "weight": "semibold" }));
    let c = p.label(&format!("{id}.n"), &n.to_string(), json!({ "tone": "muted", "tabular": true }));
    p.boxed(id, "h", vec![t, c], json!({ "align": "center" }))
}

/// A row of the Running section: the command, where it sits, how long it has run, Open and Stop.
fn live_row(p: &mut Page, i: usize, r: &Runner, run: &Record, at: i64) -> String {
    let id = format!("live.{}", sane(&r.name));
    let name = p.label(&format!("{id}.name"), &r.label, json!({ "weight": "semibold", "symbol": "circle.fill", "tone": "warn" }));
    let place = p.label(&format!("{id}.place"), &r.group.join(" › "), json!({ "tone": "muted" }));
    let took = p.label(&format!("{id}.took"), &status_text(run, at), json!({ "mono": true, "tone": "muted", "tabular": true }));
    let open = p.icon(&format!("{id}.open"), "terminal", &format!("Open the log of {}", r.label), "open_run", json!({ "path": run.id }));
    let stop = p.icon(&format!("{id}.stop"), "stop.fill", &format!("Stop {}", r.label), "stop_log", json!({ "path": run.id }));
    let acts = p.boxed(&format!("{id}.acts"), "h", vec![open, stop], json!({}));
    let mut extra = json!({ "widths": [190, "flex", null, null], "align": "center", "padding": [8, 12] });
    if i > 0 {
        extra["edge"] = json!("top");
    }
    p.boxed(&id, "h", vec![name, place, took, acts], extra)
}

/// One group's contents: its commands, then its sub-groups, each under a heading that folds it.
#[allow(clippy::too_many_arguments)]
fn node(p: &mut Page, id: &str, n: &Node, path: &[String], latest: &BTreeMap<&str, &Record>, ui: &Ui, searching: bool, at: i64) -> String {
    let mut kids = vec![];
    for r in &n.commands {
        kids.push(command_row(p, r, latest.get(r.name.as_str()).copied(), ui, at));
    }
    for (key, sub) in &n.groups {
        let mut sub_path = path.to_vec();
        sub_path.push(key.clone());
        let dotted = sub_path.join(".");
        let gid = format!("g.{}", sane(&dotted));
        let folded = !searching && ui.folded.contains(&dotted);
        let chevron = p.put_icon(&format!("{gid}.c"), if folded { "chevron.right" } else { "chevron.down" });
        let name = p.label(&format!("{gid}.name"), key, json!({ "weight": "semibold" }));
        let count = p.label(&format!("{gid}.n"), &sub.count().to_string(), json!({ "tone": "muted", "tabular": true }));
        let row = p.boxed(&format!("{gid}.row"), "h", vec![chevron, name, count], json!({ "align": "center", "spacing": 6 }));
        let head = p.button(&format!("{gid}.fold"), "", "toggle_group", json!({ "path": dotted }), json!({ "child": row, "style": "plain", "help": format!("{} {key}", if folded { "Show" } else { "Fold" }) }));
        let mut parts = vec![head];
        if !folded {
            parts.push(node(p, &format!("{gid}.body"), sub, &sub_path, latest, ui, searching, at));
        }
        kids.push(p.boxed(&gid, "v", parts, json!({ "spacing": 4, "padding": { "leading": if path.is_empty() { 0 } else { 14 } } })));
    }
    p.boxed(id, "v", kids, json!({ "spacing": 10, "align": "stretch", "padding": { "leading": if path.is_empty() { 0 } else { 14 } } }))
}

impl Page {
    fn put_icon(&mut self, id: &str, name: &str) -> String {
        self.put(json!({ "id": id, "component": "Icon", "name": name, "modifiers": [{ "font": { "size": 10 } }] }));
        id.to_string()
    }
    fn bind(&mut self, path: &str, value: Value) {
        self.put(json!({ "updateDataModel": { "path": path, "value": value } }));
    }
}

/// A command's row: its name and latest run's status, its description or confirm question under
/// them, and its buttons: Run, or Open and Stop once it has a run that is going; Open alone when
/// its latest run has ended.
fn command_row(p: &mut Page, r: &Runner, run: Option<&Record>, ui: &Ui, at: i64) -> String {
    let id = format!("r.{}", sane(&r.name));
    let name = p.label(&format!("{id}.name"), &r.label, json!({ "weight": "semibold" }));
    let mut head = vec![name];
    if let Some(run) = run {
        let (_, symbol, tone) = state_of(run);
        head.push(p.label(&format!("{id}.state"), &status_text(run, at), json!({ "symbol": symbol, "tone": tone, "mono": true, "tabular": true, "size": 12 })));
    }
    let asking = ui.asking.as_deref() == Some(r.name.as_str()) && r.confirm.is_some();
    let (second, acts) = if asking {
        let q = r.confirm.clone().unwrap_or_default();
        let cancel = p.icon(&format!("{id}.cancel"), "xmark", "Cancel", "cancel_ask", json!({}));
        let go = p.icon(&format!("{id}.go"), "checkmark", &format!("Run {}: {q}", r.label), "press_run", json!({ "name": r.name, "confirmed": r.command }));
        (Some(p.label(&format!("{id}.say"), &q, json!({ "tone": "bad" }))), vec![cancel, go])
    } else {
        let desc = r.desc.as_ref().map(|d| p.label(&format!("{id}.desc"), d, json!({ "tone": "muted" })));
        let mut acts = vec![];
        match run {
            Some(run) if run.state == State::Running => {
                acts.push(p.icon(&format!("{id}.open"), "terminal", &format!("Open the log of {}", r.label), "open_run", json!({ "path": run.id })));
                acts.push(p.icon(&format!("{id}.stop"), "stop.fill", &format!("Stop {}", r.label), "stop_log", json!({ "path": run.id })));
            }
            Some(run) => {
                acts.push(p.icon(&format!("{id}.open"), "terminal", &format!("Open the last log of {}", r.label), "open_run", json!({ "path": run.id })));
                acts.push(p.icon(&format!("{id}.run"), "play.fill", &format!("Run {}", r.label), "press_run", json!({ "name": r.name })));
            }
            None => acts.push(p.icon(&format!("{id}.run"), "play.fill", &format!("Run {}", r.label), "press_run", json!({ "name": r.name }))),
        }
        (desc, acts)
    };
    let first = p.boxed(&format!("{id}.head"), "h", head, json!({ "align": "center", "spacing": 10 }));
    let mut lines = vec![first];
    lines.extend(second);
    let text = p.boxed(&format!("{id}.text"), "v", lines, json!({ "spacing": 2, "align": "start" }));
    let buttons = p.boxed(&format!("{id}.acts"), "h", acts, json!({ "spacing": 6 }));
    p.boxed(&id, "h", vec![text, buttons], json!({ "widths": ["flex", null], "align": "center" }))
}

/// One run's output filling the pane, with a way back and a button for every other running command.
fn log_view(p: &mut Page, page: &mut Vec<String>, runners: &[Runner], runs: &[Record], run: &Record, log: Option<&str>, at: i64) {
    let back_icon = p.put_icon("back.i", "chevron.left");
    let back_text = p.label("back.t", "Commands", json!({}));
    let back_row = p.boxed("back.row", "h", vec![back_icon, back_text], json!({ "align": "center", "spacing": 4 }));
    let back = p.button("back", "", "close_log", json!({}), json!({ "child": back_row, "help": "Back to the commands" }));
    let mut bar = vec![back];
    for other in runs.iter().filter(|r| r.state == State::Running && r.id != run.id) {
        let label = runners.iter().find(|r| r.name == other.name).map(|r| r.label.as_str()).unwrap_or(other.name.as_str());
        let id = format!("also.{}", sane(&other.id));
        let t = p.label(&format!("{id}.t"), &format!("{label} {}", crate::duration(at - other.started)), json!({ "symbol": "circle.fill", "tone": "warn", "tabular": true }));
        bar.push(p.button(&id, "", "open_run", json!({ "path": other.id }), json!({ "child": t, "help": format!("Open the log of {label}") })));
    }
    page.push(p.boxed("bar", "h", bar, json!({ "align": "center", "spacing": 10 })));

    let runner = runners.iter().find(|r| r.name == run.name);
    let title = runner.map(|r| r.label.as_str()).unwrap_or(run.name.as_str());
    if let Some(r) = runner.filter(|r| !r.group.is_empty()) {
        page.push(p.label("place", &r.group.join(" › "), json!({ "tone": "muted", "size": 12 })));
    }
    let name = p.label("title", title, json!({ "weight": "semibold", "size": 17 }));
    let (_, symbol, tone) = state_of(run);
    let took = crate::duration(run.ended.unwrap_or(at) - run.started);
    let state = p.label("state", &format!("{} · {} · {took}", state_of(run).0, crate::clock(run.started)), json!({ "symbol": symbol, "tone": tone, "mono": true, "tabular": true }));
    let mut head = vec![name, state];
    head.push(if run.state == State::Running {
        p.icon("stop", "stop.fill", &format!("Stop {title}"), "stop_log", json!({ "path": run.id }))
    } else {
        p.icon("again", "play.fill", &format!("Run {title} again"), "press_run", json!({ "name": run.name }))
    });
    page.push(p.boxed("head", "h", head, json!({ "align": "center", "spacing": 12, "widths": [null, "flex", null], "trailing": 1 })));
    if let Some(d) = runner.and_then(|r| r.desc.as_ref()) {
        page.push(p.label("desc", d, json!({ "tone": "muted" })));
    }
    page.push(p.label("cmd", &run.command, json!({ "mono": true, "tone": "muted", "size": 12 })));

    let text = match log {
        Some(t) if !t.is_empty() => t.trim_end().to_string(),
        Some(_) => "(no output yet)".into(),
        None => "(the log cannot be read)".into(),
    };
    let body = p.label("log", &text, json!({ "mono": true }));
    page.push(p.boxed("out", "v", vec![body], json!({ "border": "line", "padding": [12, 12] })));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner(name: &str, group: &[&str], confirm: Option<&str>) -> Runner {
        Runner {
            name: name.into(), label: name.rsplit('.').next().unwrap().into(), group: group.iter().map(|g| g.to_string()).collect(), desc: Some(format!("does {name}")),
            command: format!("make {name}"), confirm: confirm.map(String::from), origin: "project", requires: vec![], status: None, worktree: false,
        }
    }

    fn run(id: &str, name: &str, state: State, code: Option<i32>) -> Record {
        Record {
            id: id.into(), name: name.into(), command: format!("make {name}"), dir: "/p".into(), state, started: 100,
            ended: (state != State::Running).then_some(160), exit_code: code, pid: 2, pid_started: 0, owner_pid: 1, stop_requested: false, log: "/l".into(),
        }
    }

    fn find<'a>(c: &'a [Value], id: &str) -> &'a Value {
        c.iter().find(|v| v["id"] == id).unwrap_or_else(|| panic!("no component {id}"))
    }
    fn has(c: &[Value], id: &str) -> bool {
        c.iter().any(|v| v["id"] == id)
    }
    fn action(c: &[Value], id: &str) -> (Value, Value) {
        let e = &find(c, id)["action"]["event"];
        (e["name"].clone(), e["context"].clone())
    }
    fn help(c: &[Value], id: &str) -> Value {
        find(c, id)["modifiers"].as_array().unwrap().iter().find_map(|m| m.get("help").cloned()).unwrap_or(Value::Null)
    }

    fn rs() -> Vec<Runner> {
        vec![
            runner("wheel.dev.deploy", &["wheel", "dev"], None),
            runner("wheel.dev.restart", &["wheel", "dev"], Some("Ends every agent.")),
            runner("prototypes.terrarium.dev", &["prototypes", "terrarium"], None),
            runner("hello", &[], None),
        ]
    }

    #[test]
    fn names_that_differ_only_in_punctuation_get_different_ids() {
        assert_ne!(sane("a.b"), sane("a-b"));
        assert_eq!(sane("build"), "build");
    }

    #[test]
    fn the_tree_nests_by_group_with_counts_and_a_run_button_per_command() {
        let c = components(&rs(), &[], &Ui::default(), None, 200);
        assert_eq!(find(&c, "search")["component"], "TextField");
        assert_eq!(find(&c, "all.h.n")["text"], "4");
        assert_eq!(find(&c, "g.wheel.name")["text"], "wheel");
        assert_eq!(find(&c, "g.wheel.n")["text"], "2");
        assert_eq!(find(&c, "g.wheel_2edev.name")["text"], "dev");
        assert_eq!(find(&c, "g.prototypes_2eterrarium.n")["text"], "1");
        assert_eq!(find(&c, "r.hello.name")["text"], "hello");
        assert_eq!(find(&c, "r.wheel_2edev_2edeploy.desc")["text"], "does wheel.dev.deploy");
        assert_eq!(action(&c, "r.wheel_2edev_2edeploy.run"), (json!("press_run"), json!({ "name": "wheel.dev.deploy" })));
        assert_eq!(find(&c, "r.wheel_2edev_2edeploy.run.i")["name"], "play.fill");
        assert!(!has(&c, "live.h"), "nothing runs, so there is no Running section");
    }

    #[test]
    fn a_running_command_is_pinned_and_swaps_run_for_open_and_stop() {
        let runs = [run("a", "wheel.dev.deploy", State::Running, None), run("b", "hello", State::Exited, Some(0))];
        let c = components(&rs(), &runs, &Ui::default(), None, 142);
        assert_eq!(find(&c, "live.h.n")["text"], "1");
        assert_eq!(action(&c, "live.wheel_2edev_2edeploy.open"), (json!("open_run"), json!({ "path": "a" })));
        assert_eq!(find(&c, "live.wheel_2edev_2edeploy.open.i")["name"], "terminal");
        assert_eq!(find(&c, "live.wheel_2edev_2edeploy.stop.i")["name"], "stop.fill");
        assert_eq!(action(&c, "r.wheel_2edev_2edeploy.stop"), (json!("stop_log"), json!({ "path": "a" })));
        assert!(!has(&c, "r.wheel_2edev_2edeploy.run"), "a running command has no Run");
        assert_eq!(find(&c, "r.wheel_2edev_2edeploy.state.t")["text"], "running 0:42");
        // A finished run: Open its log, or Run again.
        assert_eq!(action(&c, "r.hello.open").1, json!({ "path": "b" }));
        assert!(has(&c, "r.hello.run"));
        assert_eq!(find(&c, "r.hello.state.t")["text"], format!("exit 0 · {}", crate::clock(100)));
    }

    #[test]
    fn a_confirm_command_asks_in_its_row_before_it_runs() {
        let ui = Ui { asking: Some("wheel.dev.restart".into()), ..Ui::default() };
        let c = components(&rs(), &[], &ui, None, 0);
        assert_eq!(find(&c, "r.wheel_2edev_2erestart.say")["text"], "Ends every agent.");
        assert_eq!(action(&c, "r.wheel_2edev_2erestart.cancel").0, "cancel_ask");
        assert_eq!(help(&c, "r.wheel_2edev_2erestart.go"), "Run restart: Ends every agent.");
        assert_eq!(action(&c, "r.wheel_2edev_2erestart.go").1, json!({ "name": "wheel.dev.restart", "confirmed": "make wheel.dev.restart" }));
        assert!(!has(&c, "r.wheel_2edev_2erestart.run"));
    }

    #[test]
    fn a_folded_group_hides_its_contents_and_a_search_opens_every_group_with_a_match() {
        let ui = Ui { folded: ["wheel".to_string()].into(), ..Ui::default() };
        let c = components(&rs(), &[], &ui, None, 0);
        assert_eq!(find(&c, "g.wheel.c")["name"], "chevron.right");
        assert!(!has(&c, "r.wheel_2edev_2edeploy.name") && has(&c, "r.hello.name"));
        assert_eq!(action(&c, "g.wheel.fold"), (json!("toggle_group"), json!({ "path": "wheel" })));
        let ui = Ui { folded: ["wheel".to_string()].into(), query: "terra".into(), ..Ui::default() };
        let c = components(&rs(), &[], &ui, None, 0);
        assert!(has(&c, "r.prototypes_2eterrarium_2edev.name"));
        assert!(!has(&c, "r.hello.name") && !has(&c, "g.wheel.name"), "only matches and the groups holding them show");
        let ui = Ui { query: "DOES wheel.dev.d".into(), ..Ui::default() };
        assert!(has(&components(&rs(), &[], &ui, None, 0), "r.wheel_2edev_2edeploy.name"), "the description is searched, in any case");
        let ui = Ui { query: "zzz".into(), ..Ui::default() };
        assert_eq!(find(&components(&rs(), &[], &ui, None, 0), "none")["text"], "No command matches.");
    }

    #[test]
    fn an_open_run_fills_the_pane_with_its_log_a_way_back_and_the_other_running_commands() {
        let runs = [run("a", "wheel.dev.deploy", State::Running, None), run("b", "prototypes.terrarium.dev", State::Running, None)];
        let ui = Ui { open: Some("a".into()), ..Ui::default() };
        let c = components(&rs(), &runs, &ui, Some("12:00:01  hi\n"), 142);
        assert!(!has(&c, "search"), "the list is not drawn");
        assert_eq!(action(&c, "back").0, "close_log");
        assert_eq!(find(&c, "place")["text"], "wheel › dev");
        assert_eq!(find(&c, "title")["text"], "deploy");
        assert_eq!(find(&c, "log")["text"], "12:00:01  hi");
        assert_eq!(action(&c, "stop"), (json!("stop_log"), json!({ "path": "a" })));
        assert_eq!(action(&c, "also.b"), (json!("open_run"), json!({ "path": "b" })));
        assert!(!has(&c, "also.a"), "the open run is not offered again");
        let done = [run("a", "wheel.dev.deploy", State::Exited, Some(2))];
        let c = components(&rs(), &done, &ui, Some(""), 142);
        assert_eq!(find(&c, "log")["text"], "(no output yet)");
        assert_eq!(action(&c, "again").0, "press_run");
        assert!(!has(&c, "stop"));
    }
}
