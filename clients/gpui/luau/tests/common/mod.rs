#![allow(dead_code)]
//! Helpers the ported WheelLuau tests share (the Swift suites' static helpers).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{Map, Value as Json, json};
pub use wheel_luau::{Failure, FeedRead, Host, Limits, PaneNode, PaneRuntime};

/// The repository root (`CARGO_MANIFEST_DIR` is `clients/gpui/luau`).
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).expect("repo root").to_path_buf()
}

pub fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `<package>/lib/*.luau` keyed `lib/<name>.luau`, as the Swift tests load them.
pub fn modules_of(package: &str) -> HashMap<String, String> {
    let lib = root().join(package).join("lib");
    let mut out = HashMap::new();
    for entry in std::fs::read_dir(&lib).unwrap_or_else(|e| panic!("{}: {e}", lib.display())) {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        if name.ends_with(".luau") {
            out.insert(format!("lib/{name}"), std::fs::read_to_string(lib.join(&name)).unwrap());
        }
    }
    out
}

/// `v` with every number as a float, the form the runtime's JSON uses (`json!(240)` is an integer
/// and would not equal `240.0`).
pub fn fl(v: Json) -> Json {
    match v {
        Json::Number(n) => n.as_f64().and_then(serde_json::Number::from_f64).map(Json::Number).unwrap_or(Json::Null),
        Json::Array(a) => Json::Array(a.into_iter().map(fl).collect()),
        Json::Object(o) => Json::Object(o.into_iter().map(|(k, v)| (k, fl(v))).collect()),
        other => other,
    }
}

pub fn same(actual: &Json, expected: Json) {
    assert_eq!(fl(actual.clone()), fl(expected));
}

pub fn parse(text: &str) -> Json {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

pub fn err<T>(r: Result<T, Failure>) -> Failure {
    match r {
        Err(e) => e,
        Ok(_) => panic!("expected a Failure, got Ok"),
    }
}

pub fn new(source: &str) -> PaneRuntime {
    PaneRuntime::new(source, HashMap::new(), "pane", Limits::default(), Host::default()).unwrap()
}

pub fn new_with(source: &str, limits: Limits) -> PaneRuntime {
    PaneRuntime::new(source, HashMap::new(), "pane", limits, Host::default()).unwrap()
}

pub fn try_new(source: &str) -> Result<PaneRuntime, Failure> {
    PaneRuntime::new(source, HashMap::new(), "pane", Limits::default(), Host::default())
}

pub fn modules(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

pub type Rec = Rc<RefCell<Vec<(String, Json)>>>;

pub fn rec() -> Rec {
    Rc::new(RefCell::new(Vec::new()))
}

pub fn host(feed: impl Fn(&str, &Json) -> Json + 'static, actions: Rec) -> Host {
    Host {
        feed: Box::new(move |n, p| FeedRead::new(feed(n, p))),
        action: Box::new(move |n, a| actions.borrow_mut().push((n.to_string(), a.clone()))),
        ..Host::default()
    }
}

pub fn host_feed(feed: impl Fn(&str, &Json) -> Json + 'static) -> Host {
    host(feed, rec())
}

pub fn prop(node: &PaneNode, key: &str) -> Json {
    node.props.get(key).cloned().unwrap_or(Json::Null)
}

pub fn has_prop(node: &PaneNode, key: &str) -> bool {
    node.props.contains_key(key)
}

pub fn text(node: &PaneNode) -> Option<String> {
    node.props.get("text").and_then(|t| t.as_str()).map(String::from)
}

pub fn s(v: Option<&Json>) -> Option<String> {
    v.and_then(|v| v.as_str()).map(String::from)
}

/// Every `text` property of `node` and its children.
pub fn texts_children(node: &PaneNode) -> Vec<String> {
    let mut out: Vec<String> = text(node).into_iter().collect();
    for child in &node.children {
        out.extend(texts_children(child));
    }
    out
}

/// Like [`texts_children`], and into a `child` property that holds a node.
pub fn texts_child(node: &PaneNode) -> Vec<String> {
    let mut out: Vec<String> = text(node).into_iter().collect();
    if let Some(n) = node.props.get("child").and_then(|c| PaneNode::from_json(c.clone())) {
        out.extend(texts_child(&n));
    }
    for child in &node.children {
        out.extend(texts_child(child));
    }
    out
}

/// Every node of `component` below `node`, a `child` property followed.
pub fn nodes_of(node: &PaneNode, component: &str) -> Vec<PaneNode> {
    let mut out = if node.component == component { vec![node.clone()] } else { vec![] };
    if let Some(n) = node.props.get("child").and_then(|c| PaneNode::from_json(c.clone())) {
        out.extend(nodes_of(&n, component));
    }
    for child in &node.children {
        out.extend(nodes_of(child, component));
    }
    out
}

/// A Button's `help` and whether `disabled`, from its modifiers.
pub fn button_mods(props: &Map<String, Json>) -> (String, bool) {
    let (mut help, mut disabled) = (String::new(), false);
    if let Some(Json::Array(mods)) = props.get("modifiers") {
        for m in mods {
            if let Json::Object(m) = m {
                if let Some(Json::String(h)) = m.get("help") {
                    help = h.clone();
                }
                if m.get("disabled") == Some(&Json::Bool(true)) {
                    disabled = true;
                }
            }
        }
    }
    (help, disabled)
}

// The wheelhouse harness (WheelhousePaneTests' statics).

pub const WHEELHOUSE: &str = "plugins/wheelhouse";

pub fn wh_data() -> Json {
    json!({"pane": "p1", "space": "s1", "cwd": "/p"})
}

/// A pane over `panes/<name>.luau` answering `wheelhouse.<feed>` with `value` and `wheel.panes` with `panes`.
pub fn wh_pane(name: &str, feed: &str, value: Json, panes: Json, actions: Rec) -> PaneRuntime {
    let feed_name = format!("wheelhouse.{feed}");
    let h = host(move |n, _| if n == feed_name { value.clone() } else if n == "wheel.panes" { panes.clone() } else { Json::Null }, actions);
    PaneRuntime::new(&read(&format!("{WHEELHOUSE}/panes/{name}.luau")), modules_of(WHEELHOUSE), "pane", Limits::default(), h).unwrap()
}

pub fn wh_msg(kind: &str, fields: Json) -> Json {
    let mut o = match fields {
        Json::Object(o) => o,
        _ => Map::new(),
    };
    o.insert("type".into(), json!(kind));
    Json::Object(o)
}

/// The tree as JSON: each node an object with its props, its `component` and its `children`.
pub fn jsonof(n: &PaneNode) -> Json {
    let mut o = n.props.clone();
    o.insert("component".into(), json!(n.component));
    o.insert("children".into(), Json::Array(n.children.iter().map(jsonof).collect()));
    Json::Object(o)
}

/// Every object below `v` (v included) that names a component, depth first, keys sorted.
pub fn nodes(v: &Json, out: &mut Vec<Map<String, Json>>) {
    match v {
        Json::Object(o) => {
            if matches!(o.get("component"), Some(Json::String(_))) {
                out.push(o.clone());
            }
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            for k in keys {
                nodes(&o[k], out);
            }
        }
        Json::Array(a) => a.iter().for_each(|x| nodes(x, out)),
        _ => {}
    }
}

pub fn all(tree: &PaneNode, component: &str) -> Vec<Map<String, Json>> {
    let mut out = vec![];
    nodes(&jsonof(tree), &mut out);
    out.into_iter().filter(|o| o.get("component") == Some(&json!(component))).collect()
}

pub fn texts_json(v: &Json) -> Vec<String> {
    let mut flat = vec![];
    nodes(v, &mut flat);
    flat.iter().filter_map(|o| o.get("text").and_then(|t| t.as_str()).map(String::from)).collect()
}

pub struct Row {
    pub id: String,
    pub title: String,
    pub row: Map<String, Json>,
}

/// The listed table's rows: each row's id and its title cell.
pub fn rows(tree: &PaneNode) -> Vec<Row> {
    let tables = all(tree, "Table");
    let table = tables.first().expect("no table");
    let Some(Json::Array(sections)) = table.get("sections") else { panic!("no table") };
    let Some(Json::Object(sec)) = sections.first() else { panic!("no table") };
    let Some(Json::Array(rows)) = sec.get("rows") else { panic!("no table") };
    rows.iter()
        .filter_map(|r| {
            let Json::Object(o) = r else { return None };
            let id = s(o.get("id"))?;
            let mut title = String::new();
            if let Some(Json::Array(cells)) = o.get("cells") {
                for c in cells {
                    if let Json::Object(c) = c {
                        if s(c.get("key")).as_deref() == Some("title") {
                            title = s(c.get("text")).unwrap_or_default();
                        }
                    }
                }
            }
            Some(Row { id, title, row: o.clone() })
        })
        .collect()
}

pub fn ids(rt: &mut PaneRuntime) -> Vec<String> {
    rows(&rt.view(&wh_data()).unwrap()).into_iter().map(|r| r.id).collect()
}

#[derive(Clone, Debug)]
pub struct Btn {
    pub text: String,
    pub action: Json,
    pub disabled: bool,
    pub help: String,
}

/// Every Button below `tree` (a Modal's footer and a Popover's body included).
pub fn buttons(tree: &PaneNode) -> Vec<Btn> {
    all(tree, "Button")
        .into_iter()
        .map(|b| {
            let (help, disabled) = button_mods(&b);
            let text = b.get("child").map(|c| texts_json(c).into_iter().next().unwrap_or_default()).unwrap_or_default();
            Btn { text, action: b.get("action").cloned().unwrap_or(Json::Null), disabled, help }
        })
        .collect()
}

pub fn action_type(b: &Btn) -> Option<String> {
    if let Json::Object(o) = &b.action { s(o.get("type")) } else { None }
}

// The beads fixture.

pub struct Bead<'a> {
    pub id: &'a str,
    pub status: &'a str,
    pub kind: &'a str,
    pub pri: Option<i64>,
    pub parent: Option<&'a str>,
    pub labels: Vec<&'a str>,
    pub blocked: Vec<&'a str>,
    pub ready: bool,
    pub epic: bool,
    pub desc: String,
}

pub fn bead_spec(id: &str) -> Bead<'_> {
    Bead { id, status: "open", kind: "task", pri: Some(2), parent: None, labels: vec![], blocked: vec![], ready: false, epic: false, desc: String::new() }
}

impl Bead<'_> {
    pub fn json(&self) -> Json {
        json!({"id": self.id, "title": format!("Bead {}", self.id), "status": self.status, "kind": self.kind, "priority": self.pri,
               "parent": self.parent, "labels": self.labels, "owner": "pierce", "updated": "2026-11-01T00:00:00Z",
               "blocked_by": self.blocked, "description": self.desc, "ready": self.ready, "epic": self.epic})
    }
}

pub fn snapshot(beads: Vec<Json>) -> Json {
    snapshot_with(beads, json!({"task": null, "epic": null, "closed": null}), json!({}), json!([]))
}

pub fn snapshot_with(beads: Vec<Json>, buttons: Json, records: Json, pressing: Json) -> Json {
    json!({"now": 1793500000, "name": "wheel", "path": "/p", "has_repo": true, "error": null, "problems": [], "buttons": buttons,
           "scripts": [], "records": records, "pressing": pressing, "beads": beads})
}
