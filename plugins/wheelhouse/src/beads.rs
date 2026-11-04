//! The Beads plugin: a project's beads read through `bd`, the page's logic (views, search,
//! filters, sort, trees) and its drawing as A2UI. The same logic serves the command line
//! (`cli`) and a host's pane over MCP (`serve`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub mod cli;
pub mod serve;

const CATALOG: &str = "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json";
const SURFACE: &str = "beads";

// ───────────────────────────── data ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
struct Bead {
    id: String,
    title: String,
    status: String,
    kind: String,
    priority: Option<i64>,
    parent: Option<String>,
    labels: Vec<String>,
    owner: Option<String>,
    updated: String,
    blocked_by: Vec<String>,
    description: String,
    ready: bool,
    epic: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Script {
    name: String,
    label: Option<String>,
    on: Vec<String>,
    run: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Snapshot {
    name: String,
    path: String,
    scripts: Vec<Script>,
    beads: Vec<Bead>,
    error: Option<String>,
    problems: Vec<String>,
    has_repo: bool,
}

fn bd(path: &str, args: &[&str]) -> Result<Vec<Value>, String> {
    let out = Command::new("bd")
        .args(args)
        .args(["--json", "--limit", "0"])
        .current_dir(path)
        .output()
        .map_err(|e| format!("bd: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).chars().take(300).collect());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    serde_json::from_str(&text).map_err(|e| format!("bd {}: not JSON: {e}", args.join(" ")))
}

fn scripts(path: &str) -> (Vec<Script>, Vec<String>) {
    let file = PathBuf::from(path).join("wheel.toml");
    let Ok(text) = std::fs::read_to_string(&file) else { return (vec![], vec![]) };
    let doc: toml_edit::DocumentMut = match text.parse() {
        Ok(t) => t,
        Err(e) => return (vec![], vec![format!("{}: {e}", file.display())]),
    };
    let mut out = vec![];
    if let Some(buttons) = doc.get("beads").and_then(|b| b.get("buttons")).and_then(|b| b.as_table_like()) {
        for (name, s) in buttons.iter() {
            out.push(Script {
                name: name.to_string(),
                label: s.get("label").and_then(|v| v.as_str()).map(String::from),
                on: s
                    .get("on")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                run: s.get("run").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            });
        }
    }
    (out, vec![])
}

/// The beads of the project in `path`: what `bd` lists, which are ready, and the buttons the
/// project writes in its wheel.toml.
fn collect(path: &str) -> Snapshot {
    let path = path.to_string();
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
    let mut snap = Snapshot { name, path: path.clone(), ..Default::default() };
    if !PathBuf::from(&path).join(".beads").is_dir() {
        return snap;
    }
    snap.has_repo = true;
    let (s, problems) = scripts(&path);
    snap.scripts = s;
    snap.problems = problems;
    let load = || -> Result<Vec<Bead>, String> {
        let all = bd(&path, &["list", "--all"])?;
        let ready: HashSet<String> =
            bd(&path, &["ready"])?.iter().filter_map(|b| b["id"].as_str().map(String::from)).collect();
        let kids: HashSet<String> = all.iter().filter_map(|b| b["parent"].as_str().map(String::from)).collect();
        let closed: HashSet<String> = all
            .iter()
            .filter(|b| b["status"] == "closed")
            .filter_map(|b| b["id"].as_str().map(String::from))
            .collect();
        Ok(all
            .iter()
            .map(|b| {
                let id = b["id"].as_str().unwrap_or("").to_string();
                let kind = b["issue_type"].as_str().unwrap_or("task").to_string();
                Bead {
                    ready: ready.contains(&id),
                    epic: kind == "epic" || kids.contains(&id),
                    blocked_by: b["dependencies"]
                        .as_array()
                        .map(|d| {
                            d.iter()
                                .filter(|d| d["type"] == "blocks")
                                .filter_map(|d| d["depends_on_id"].as_str())
                                .filter(|d| !closed.contains(*d))
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default(),
                    title: b["title"].as_str().unwrap_or("").to_string(),
                    status: b["status"].as_str().unwrap_or("open").to_string(),
                    priority: b["priority"].as_i64(),
                    parent: b["parent"].as_str().map(String::from),
                    labels: b["labels"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect())
                        .unwrap_or_default(),
                    owner: b["assignee"].as_str().or(b["owner"].as_str()).map(String::from),
                    updated: b["updated_at"].as_str().unwrap_or("").to_string(),
                    description: b["description"].as_str().unwrap_or("").chars().take(1500).collect(),
                    id,
                    kind,
                }
            })
            .collect())
    };
    match load() {
        Ok(b) => snap.beads = b,
        Err(e) => snap.error = Some(e),
    }
    snap
}

// ───────────────────────────── the page's logic (a port of the installed index.html and table.js) ─────────────────────────────

fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n.push(c);
                    x.next();
                }
                let mut m = String::new();
                while let Some(d) = y.peek().copied().filter(char::is_ascii_digit) {
                    m.push(d);
                    y.next();
                }
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

fn kind_of(b: &Bead) -> &'static str {
    match b.status.as_str() {
        "closed" => "done",
        "in_progress" => "in progress",
        "deferred" => "deferred",
        _ if b.ready => "ready",
        _ if !b.blocked_by.is_empty() => "blocked",
        _ => "open",
    }
}

const KINDS: [&str; 6] = ["in progress", "ready", "blocked", "open", "deferred", "done"];

fn krank(k: &str) -> usize {
    KINDS.iter().position(|x| *x == k).unwrap_or(3)
}

fn tone_of(k: &str) -> &'static str {
    match k {
        "done" | "ready" => "ok",
        "in progress" => "warn",
        "blocked" => "bad",
        _ => "muted",
    }
}

fn is_open(b: &Bead) -> bool {
    b.status != "closed"
}

fn groups(b: &Bead) -> bool {
    b.epic || b.kind == "epic" || b.kind == "milestone"
}

fn idle(b: &Bead) -> bool {
    b.status != "closed" && !(b.status == "open" && !b.blocked_by.is_empty())
}

fn wants(s: &Script, b: &Bead) -> bool {
    s.on.iter().any(|k| match k.as_str() {
        "ready" => b.ready && b.status == "open",
        "wip" => b.status == "in_progress",
        "epic" => groups(b) && idle(b),
        "milestone" => b.kind == "milestone" && idle(b),
        "blocked" => b.status == "open" && !b.ready,
        _ => false,
    })
}

/// Seconds since the Unix epoch of an RFC 3339 UTC time such as `2026-10-30T01:07:41Z`.
fn epoch(iso: &str) -> i64 {
    let n = |a: usize, b: usize| iso.get(a..b).and_then(|s| s.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) =
        (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19))
    else {
        return 0;
    };
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + h * 3600 + mi * 60 + s
}

fn ago(iso: &str) -> String {
    if iso.is_empty() {
        return String::new();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let m = ((now - epoch(iso)).max(0) as f64) / 60.0;
    if m < 60.0 {
        format!("{}m", m.round())
    } else if m < 1440.0 {
        format!("{}h", (m / 60.0).round())
    } else {
        format!("{}d", (m / 1440.0).round())
    }
}

struct Ix<'a> {
    snap: &'a Snapshot,
    by_id: HashMap<&'a str, &'a Bead>,
    unlocks: HashMap<&'a str, Vec<&'a Bead>>,
    kids: HashMap<&'a str, Vec<&'a Bead>>,
}

impl<'a> Ix<'a> {
    fn new(snap: &'a Snapshot) -> Self {
        let by_id = snap.beads.iter().map(|b| (b.id.as_str(), b)).collect();
        let mut unlocks: HashMap<&str, Vec<&Bead>> = HashMap::new();
        let mut kids: HashMap<&str, Vec<&Bead>> = HashMap::new();
        for b in &snap.beads {
            for id in &b.blocked_by {
                unlocks.entry(id.as_str()).or_default().push(b);
            }
            if let Some(p) = &b.parent {
                kids.entry(p.as_str()).or_default().push(b);
            }
        }
        Ix { snap, by_id, unlocks, kids }
    }
    fn blockers(&self, b: &Bead) -> Vec<&'a Bead> {
        b.blocked_by.iter().filter_map(|id| self.by_id.get(id.as_str()).copied()).collect()
    }
    fn unlocked(&self, b: &Bead) -> Vec<&'a Bead> {
        self.unlocks.get(b.id.as_str()).cloned().unwrap_or_default()
    }
    fn children(&self, b: &Bead) -> Vec<&'a Bead> {
        self.kids.get(b.id.as_str()).cloned().unwrap_or_default()
    }
    fn short<'b>(&self, id: &'b str) -> &'b str {
        id.strip_prefix(&format!("{}-", self.snap.name)).unwrap_or(id)
    }
}

struct Col {
    key: &'static str,
    label: &'static str,
    width: Option<f64>,
    tree: bool,
    desc_first: bool,
    ungrouped: bool,
}

const COLS: [Col; 7] = [
    Col { key: "id", label: "ID", width: Some(64.0), tree: false, desc_first: false, ungrouped: false },
    Col { key: "title", label: "Title", width: None, tree: true, desc_first: false, ungrouped: false },
    Col { key: "type", label: "Type", width: Some(64.0), tree: false, desc_first: false, ungrouped: false },
    Col { key: "unlocks", label: "Unlocks", width: Some(60.0), tree: false, desc_first: true, ungrouped: false },
    Col { key: "status", label: "Status", width: Some(96.0), tree: false, desc_first: false, ungrouped: true },
    Col { key: "pri", label: "Priority", width: Some(56.0), tree: false, desc_first: false, ungrouped: false },
    Col { key: "updated", label: "Updated", width: Some(64.0), tree: false, desc_first: true, ungrouped: false },
];

struct Ui {
    view: String,
    q: String,
    f: BTreeMap<String, String>,
    hide: bool,
    sort: (String, bool),
    collapsed: HashSet<String>,
    detail: Option<String>,
    hist: Vec<String>,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            view: "list".into(),
            q: String::new(),
            f: BTreeMap::new(),
            hide: true,
            sort: ("status".into(), true),
            collapsed: HashSet::new(),
            detail: None,
            hist: vec![],
        }
    }
}

impl Ui {
    fn filter(&self, key: &str) -> &str {
        self.f.get(key).map(String::as_str).unwrap_or("")
    }
    fn custom(&self) -> bool {
        self.view == "focus" || self.view == "graph"
    }
    fn grouped(&self) -> bool {
        self.view == "list"
    }
}

fn compare(ix: &Ix, key: &str, a: &Bead, b: &Bead) -> std::cmp::Ordering {
    match key {
        "id" => natural(&a.id, &b.id),
        "title" => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        "type" => a.kind.cmp(&b.kind),
        "unlocks" => {
            let n = |x: &Bead| ix.unlocked(x).iter().filter(|u| is_open(u)).count();
            n(a).cmp(&n(b))
        }
        "status" => krank(kind_of(a)).cmp(&krank(kind_of(b))),
        "pri" => a.priority.unwrap_or(9).cmp(&b.priority.unwrap_or(9)),
        "updated" => epoch(&a.updated).cmp(&epoch(&b.updated)),
        _ => std::cmp::Ordering::Equal,
    }
}

fn haystack(b: &Bead) -> String {
    let mut parts = vec![b.id.clone(), b.title.clone(), b.kind.clone()];
    parts.extend(b.labels.iter().cloned());
    parts.push(b.description.clone());
    parts.push(b.owner.clone().unwrap_or_default());
    parts.join(" ").to_lowercase()
}

fn matches(ui: &Ui, b: &Bead) -> bool {
    let kind = kind_of(b);
    if ui.hide && ui.filter("status").is_empty() && kind == "done" {
        return false;
    }
    if !ui.filter("status").is_empty() && kind != ui.filter("status") {
        return false;
    }
    if !ui.filter("type").is_empty() && b.kind != ui.filter("type") {
        return false;
    }
    if !ui.filter("label").is_empty() && !b.labels.iter().any(|l| l == ui.filter("label")) {
        return false;
    }
    if !ui.filter("pri").is_empty() && b.priority.map(|p| p.to_string()).as_deref() != Some(ui.filter("pri")) {
        return false;
    }
    let hay = haystack(b);
    ui.q.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

struct RowOut<'a> {
    b: &'a Bead,
    depth: usize,
    path: String,
    kids: bool,
}

struct Node<'a> {
    b: &'a Bead,
    depth: usize,
    nodes: Vec<Node<'a>>,
}

#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    b: &'a Bead,
    path: &[String],
    depth: usize,
    children: &dyn Fn(&Bead) -> Vec<&'a Bead>,
    keep: &HashSet<&str>,
    ok: &dyn Fn(&Bead) -> bool,
    by: &dyn Fn(&Bead, &Bead) -> std::cmp::Ordering,
) -> Option<Node<'a>> {
    if depth >= 60 {
        return ok(b).then(|| Node { b, depth, nodes: vec![] });
    }
    let mut nodes: Vec<Node> = children(b)
        .into_iter()
        .filter(|k| keep.contains(k.id.as_str()) && !path.contains(&k.id) && k.id != b.id)
        .filter_map(|k| {
            let mut p = path.to_vec();
            p.push(b.id.clone());
            walk(k, &p, depth + 1, children, keep, ok, by)
        })
        .collect();
    if !ok(b) && nodes.is_empty() {
        return None;
    }
    nodes.sort_by(|x, y| by(x.b, y.b));
    Some(Node { b, depth, nodes })
}

fn flat<'a>(n: &Node<'a>, parent: &str, ui: &Ui, out: &mut Vec<RowOut<'a>>) {
    if out.len() >= 20_000 {
        return;
    }
    let path = format!("{parent}>{}", n.b.id);
    out.push(RowOut { b: n.b, depth: n.depth, path: path.clone(), kids: !n.nodes.is_empty() });
    if !ui.collapsed.contains(&path) {
        for c in &n.nodes {
            flat(c, &path, ui, out);
        }
    }
}

/// The rows a view shows, as the page's table builds them: a flat sorted list, or a tree
/// flattened with a depth and cut at the collapsed paths.
fn build<'a>(ix: &Ix<'a>, ui: &Ui) -> Vec<RowOut<'a>> {
    let by = |a: &Bead, b: &Bead| {
        let o = compare(ix, &ui.sort.0, a, b);
        if ui.sort.1 { o } else { o.reverse() }
    };
    if ui.view == "list" || ui.custom() {
        let mut v: Vec<&Bead> = ix
            .snap
            .beads
            .iter()
            .filter(|b| b.kind != "epic" && b.kind != "milestone")
            .filter(|b| matches(ui, b))
            .collect();
        v.sort_by(|a, b| by(a, b));
        return v.into_iter().map(|b| RowOut { path: b.id.clone(), b, depth: 0, kids: false }).collect();
    }
    let pool: Vec<&Bead> = match ui.view.as_str() {
        "epics" => ix.snap.beads.iter().collect(),
        _ => ix.snap.beads.iter().filter(|b| ui.filter("status") == "done" || is_open(b)).collect(),
    };
    let keep: HashSet<&str> = pool.iter().map(|b| b.id.as_str()).collect();
    let children = |b: &Bead| -> Vec<&'a Bead> {
        match ui.view.as_str() {
            "epics" => ix.children(b),
            "deps" => ix.blockers(b),
            _ => ix.unlocked(b),
        }
    };
    let roots: Vec<&Bead> = match ui.view.as_str() {
        "epics" => pool
            .iter()
            .copied()
            .filter(|b| {
                b.parent.as_deref().map_or(true, |p| !ix.by_id.contains_key(p))
                    && (ix.kids.contains_key(b.id.as_str()) || b.kind == "epic" || b.kind == "milestone")
            })
            .collect(),
        "deps" => pool
            .iter()
            .copied()
            .filter(|b| !ix.blockers(b).is_empty() && !ix.unlocked(b).iter().any(|u| keep.contains(u.id.as_str())))
            .collect(),
        _ => pool
            .iter()
            .copied()
            .filter(|b| !ix.unlocked(b).is_empty() && !ix.blockers(b).iter().any(|x| keep.contains(x.id.as_str())))
            .collect(),
    };
    let ok = |b: &Bead| matches(ui, b);
    let mut tops: Vec<Node> = roots.into_iter().filter_map(|r| walk(r, &[], 0, &children, &keep, &ok, &by)).collect();
    tops.sort_by(|x, y| by(x.b, y.b));
    let mut out = vec![];
    for n in &tops {
        flat(n, "", ui, &mut out);
    }
    out
}

// ───────────────────────────── A2UI ─────────────────────────────

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
    fn runs(&mut self, id: &str, runs: Value, extra: Value) -> String {
        let mut v = json!({ "id": id, "component": "Label", "text": "", "runs": runs });
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

fn sane(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

fn cell(text: impl Into<String>, extra: Value) -> Value {
    let mut v = json!({ "text": text.into() });
    v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    v
}

fn buttons(ix: &Ix, b: &Bead) -> Vec<Value> {
    ix.snap
        .scripts
        .iter()
        .filter(|s| wants(s, b))
        .map(|s| json!({ "label": s.label.as_deref().unwrap_or(&s.name), "script": s.name }))
        .collect()
}

fn row_json(ix: &Ix, ui: &Ui, r: &RowOut, cols: &[&Col]) -> Value {
    let b = r.b;
    let k = kind_of(b);
    let cells: Vec<Value> = cols
        .iter()
        .map(|c| match c.key {
            "id" => cell(ix.short(&b.id), json!({ "mono": true, "tone": "muted" })),
            "title" => cell(&b.title, json!({})),
            "type" => cell(&b.kind, json!({ "tone": "muted" })),
            "unlocks" => {
                let n = ix.unlocked(b).iter().filter(|u| is_open(u)).count();
                cell(if n > 0 { n.to_string() } else { String::new() }, json!({ "tone": "muted" }))
            }
            "status" => cell(k, json!({ "dot": true, "tone": tone_of(k) })),
            "pri" => cell(
                b.priority.map(|p| format!("P{p}")).unwrap_or_default(),
                json!({ "tone": if b.priority.is_some_and(|p| p <= 1) { "bad" } else { "muted" } }),
            ),
            _ => cell(ago(&b.updated), json!({ "tone": "muted" })),
        })
        .collect();
    json!({ "id": b.id, "path": r.path, "depth": r.depth, "kids": r.kids,
        "collapsed": ui.collapsed.contains(&r.path), "dim": k == "done",
        "sel": ui.detail.as_deref() == Some(b.id.as_str()),
        "cells": cells, "buttons": buttons(ix, b) })
}

fn table(ix: &Ix, ui: &Ui) -> (Value, usize, usize) {
    let rows = build(ix, ui);
    let grouped = ui.grouped();
    let cols: Vec<&Col> = COLS.iter().filter(|c| !(c.ungrouped && grouped)).collect();
    let total = ix.snap.beads.iter().filter(|b| b.kind != "epic" && b.kind != "milestone").count();
    let shown = rows.len();
    let sections: Vec<Value> = if grouped {
        let titles = [
            ("in progress", "In progress"),
            ("ready", "Ready to start"),
            ("blocked", "Blocked"),
            ("open", "Open"),
            ("deferred", "Deferred"),
            ("done", "Done"),
        ];
        titles
            .iter()
            .filter_map(|(k, title)| {
                let mine: Vec<Value> =
                    rows.iter().filter(|r| kind_of(r.b) == *k).map(|r| row_json(ix, ui, r, &cols)).collect();
                (!mine.is_empty()).then(|| json!({ "title": title, "count": mine.len(), "rows": mine }))
            })
            .collect()
    } else {
        vec![json!({ "title": "", "count": rows.len(),
            "rows": rows.iter().map(|r| row_json(ix, ui, r, &cols)).collect::<Vec<_>>() })]
    };
    let columns: Vec<Value> = cols
        .iter()
        .map(|c| json!({ "key": c.key, "label": c.label, "width": c.width, "tree": c.tree && !grouped,
            "sortable": !ui.custom() }))
        .collect();
    let v = json!({ "component": "BeadTable", "columns": columns,
        "sort": { "key": ui.sort.0, "asc": ui.sort.1 }, "sections": sections,
        "empty": if total == 0 { "No beads yet" } else { "Nothing matches the search and filters." } });
    (v, shown, total)
}

fn opts(first: &str, values: Vec<(String, String)>) -> Value {
    let mut o = vec![json!({ "value": "", "text": first })];
    o.extend(values.into_iter().map(|(v, t)| json!({ "value": v, "text": t })));
    Value::Array(o)
}

type Next<'a, 'b> = &'b dyn Fn(&Bead) -> Vec<&'a Bead>;

#[allow(clippy::too_many_arguments)]
fn branch<'a>(
    p: &mut Page,
    ix: &Ix<'a>,
    prefix: &str,
    b: &'a Bead,
    next: Next<'a, '_>,
    depth: usize,
    seen: &[String],
    budget: &mut i32,
) -> String {
    *budget -= 1;
    let id = format!("{prefix}.{}", sane(&b.id));
    let k = kind_of(b);
    let link = p.pill(
        &format!("{id}.l"),
        &format!("{}  {}", ix.short(&b.id), b.title),
        "open_bead",
        json!({ "id": b.id }),
        json!({ "style": "link" }),
    );
    let chip = p.label(&format!("{id}.c"), k, json!({ "chip": true, "size": 11, "tone": tone_of(k) }));
    let mut kids_ids = vec![p.boxed(&format!("{id}.h"), "h", vec![link, chip], json!({ "spacing": 6, "align": "center" }))];
    let kids: Vec<&Bead> = next(b).into_iter().filter(|x| !seen.contains(&x.id)).collect();
    if !kids.is_empty() {
        if depth >= 4 {
            kids_ids.push(p.label(&format!("{id}.m"), &format!("… {} more", kids.len()), json!({ "tone": "muted", "size": 12 })));
        } else {
            let mut seen2 = seen.to_vec();
            seen2.push(b.id.clone());
            let mut inner = vec![];
            for (i, k) in kids.iter().enumerate() {
                if *budget <= 0 {
                    inner.push(p.label(&format!("{id}.more"), &format!("… {} more not shown", kids.len() - i), json!({ "tone": "muted", "size": 12 })));
                    break;
                }
                inner.push(branch(p, ix, &id, k, next, depth + 1, &seen2, budget));
            }
            kids_ids.push(p.boxed(&format!("{id}.k"), "v", inner, json!({ "padding": [0, 0, 0, 16], "spacing": 2 })));
        }
    }
    p.boxed(&id, "v", kids_ids, json!({ "spacing": 2 }))
}

fn tree<'a>(p: &mut Page, ix: &Ix<'a>, prefix: &str, b: &'a Bead, next: Next<'a, '_>, budget: &mut i32) -> String {
    let ks = next(b);
    if ks.is_empty() {
        return p.label(&format!("{prefix}.none"), "None.", json!({ "tone": "muted", "size": 12 }));
    }
    let seen = vec![b.id.clone()];
    let mut out = vec![];
    for (i, k) in ks.iter().enumerate() {
        if *budget <= 0 {
            out.push(p.label(&format!("{prefix}.more"), &format!("… {} more not shown", ks.len() - i), json!({ "tone": "muted", "size": 12 })));
            break;
        }
        out.push(branch(p, ix, prefix, k, next, 1, &seen, budget));
    }
    p.boxed(&format!("{prefix}.t"), "v", out, json!({ "spacing": 2 }))
}

fn detail<'a>(p: &mut Page, ix: &Ix<'a>, ui: &Ui, id: &str) -> Option<String> {
    let b: &'a Bead = ix.by_id.get(id).copied()?;
    let k = kind_of(b);
    let mut c: Vec<String> = vec![];
    if !ui.hist.is_empty() {
        c.push(p.pill("d.back", "← Back", "back", json!({}), json!({ "style": "link" })));
    }
    c.push(p.label("d.title", &b.title, json!({ "size": 17, "weight": "semibold" })));
    let fact = |p: &mut Page, n: &str, name: &str, value: String| -> String {
        let l = p.label(&format!("d.f.{n}.k"), name, json!({ "tone": "muted", "size": 12 }));
        p.boxed(&format!("d.f.{n}"), "h", vec![l, value], json!({ "spacing": 12, "widths": [84, "flex"], "align": "firstTextBaseline" }))
    };
    let v = p.label("d.f.id.v", &b.id, json!({ "mono": true, "size": 12 }));
    c.push(fact(p, "id", "ID", v));
    let v = p.label("d.f.st.v", k, json!({ "chip": true, "size": 11, "tone": tone_of(k) }));
    c.push(fact(p, "st", "Status", v));
    let v = p.label("d.f.pr.v", &b.priority.map(|x| format!("P{x}")).unwrap_or_else(|| "—".into()), json!({}));
    c.push(fact(p, "pr", "Priority", v));
    let v = p.label("d.f.ty.v", &b.kind, json!({}));
    c.push(fact(p, "ty", "Type", v));
    if !b.labels.is_empty() {
        let v = p.label("d.f.lb.v", &b.labels.join("  ·  "), json!({}));
        c.push(fact(p, "lb", "Labels", v));
    }
    if let Some(par) = b.parent.as_deref().and_then(|x| ix.by_id.get(x)) {
        let v = p.pill("d.f.pa.v", &format!("{}  {}", ix.short(&par.id), par.title), "open_bead", json!({ "id": par.id }), json!({ "style": "link" }));
        c.push(fact(p, "pa", "Parent", v));
    }
    if let Some(o) = &b.owner {
        let v = p.label("d.f.ow.v", o, json!({}));
        c.push(fact(p, "ow", "Owner", v));
    }
    let v = p.label("d.f.up.v", &format!("{} ago", ago(&b.updated)), json!({}));
    c.push(fact(p, "up", "Updated", v));
    if !b.description.is_empty() {
        let l = p.label("d.desc", &b.description, json!({ "size": 13 }));
        c.push(p.boxed("d.desc.b", "v", vec![l], json!({ "padding": [10, 12], "fill": "soft", "radius": 8 })));
    }
    let section = |p: &mut Page, key: &str, title: &str, next: Next<'a, '_>| -> String {
        let n = next(b).len();
        let h = p.runs(
            &format!("d.{key}.h"),
            json!([{ "text": title, "weight": "semibold", "size": 13 }, { "text": format!("  {n}"), "tone": "muted", "size": 13 }]),
            json!({}),
        );
        let mut budget = 40;
        let t = tree(p, ix, &format!("d.{key}"), b, next, &mut budget);
        p.boxed(&format!("d.{key}"), "v", vec![h, t], json!({ "spacing": 4, "padding": [8, 0, 0, 0] }))
    };
    c.push(section(p, "dep", "Depends on", &|x| ix.blockers(x)));
    c.push(section(p, "unl", "Unlocks", &|x| ix.unlocked(x)));
    if ix.kids.contains_key(b.id.as_str()) {
        c.push(section(p, "kid", "Children", &|x| ix.children(x)));
    }
    let acts: Vec<String> = ix
        .snap
        .scripts
        .iter()
        .filter(|s| wants(s, b))
        .map(|s| p.pill(&format!("d.act.{}", sane(&s.name)), s.label.as_deref().unwrap_or(&s.name), "run_script", json!({ "bead": b.id, "script": s.name }), json!({})))
        .collect();
    if !acts.is_empty() {
        c.push(p.boxed("d.acts", "h", acts, json!({ "spacing": 6, "padding": [12, 0, 0, 0] })));
    }
    let body = p.boxed("d.body", "v", c, json!({ "spacing": 8, "align": "start" }));
    p.put(json!({ "id": "detail", "component": "Sheet", "presented": true, "action": "close_detail", "children": [body] }));
    Some("detail".into())
}

fn page_components(snap: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let ix = Ix::new(snap);
    let all: Vec<&Bead> = snap.beads.iter().filter(|b| b.kind != "epic" && b.kind != "milestone").collect();
    let mut page: Vec<String> = vec![];

    let name = if snap.has_repo { format!("Beads · {}", snap.name) } else { "Beads".into() };
    let t = p.runs("title", json!([{ "text": name, "size": 20, "weight": "bold" }]), json!({}));
    let sub = p.label("sub", &format!("{} beads, not counting epics and milestones", all.len()), json!({ "size": 12, "tone": "muted" }));
    page.push(p.boxed("head", "v", vec![t, sub], json!({ "spacing": 2 })));

    let n = |f: &dyn Fn(&Bead) -> bool| all.iter().filter(|b| f(b)).count();
    let kpis = [
        ("Ready to start", n(&|b| b.ready && b.status == "open"), true),
        ("In progress", n(&|b| b.status == "in_progress"), false),
        ("Blocked", n(&|b| b.status == "open" && !b.ready && !b.blocked_by.is_empty()), false),
        ("Closed", n(&|b| b.status == "closed"), false),
    ];
    let cards: Vec<String> = kpis
        .iter()
        .enumerate()
        .map(|(i, (l, v, hot))| {
            let tone = if *hot && *v > 0 { "ok" } else if *v == 0 { "muted" } else { "fg" };
            p.put(json!({ "id": format!("kpi{i}"), "component": "Stat", "value": v.to_string(), "label": l, "tone": tone }));
            format!("kpi{i}")
        })
        .collect();
    page.push(p.boxed("kpis", "h", cards, json!({ "spacing": 10, "padding": [14, 0, 0, 0], "widths": ["flex", "flex", "flex", "flex"] })));

    for (i, pr) in snap.problems.iter().chain(snap.error.iter()).enumerate() {
        let l = p.label(&format!("problem{i}.t"), pr, json!({ "tone": "bad" }));
        page.push(p.boxed(&format!("problem{i}"), "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }

    if !snap.has_repo {
        page.push(p.label("none", "This space has no .beads directory, so there is nothing to list.", json!({ "tone": "muted" })));
    } else {
        // toolbar: search and views, then the filters, Hide done, sort, Clear and the count
        p.put(json!({ "id": "search", "component": "SearchField", "value": ui.q, "action": "set_query", "placeholder": "Search" }));
        p.put(json!({ "id": "views", "component": "Segmented", "selected": ui.view, "action": "set_view",
            "options": [
                { "value": "list", "text": "List" }, { "value": "epics", "text": "Epics" },
                { "value": "deps", "text": "Depends on" }, { "value": "unlocks", "text": "Unlocks" },
                { "value": "focus", "text": "Focus" }, { "value": "graph", "text": "Graph" }] }));
        page.push(p.boxed("tools1", "h", vec!["search".into(), "views".into()], json!({ "spacing": 10, "padding": [14, 0, 0, 0], "widths": ["flex", Value::Null], "align": "center" })));

        let uniq = |f: &dyn Fn(&Bead) -> Vec<String>| -> Vec<(String, String)> {
            let s: BTreeSet<String> = snap.beads.iter().flat_map(f).collect();
            s.into_iter().map(|v| (v.clone(), v)).collect()
        };
        let sel = |p: &mut Page, key: &str, first: &str, values: Vec<(String, String)>| -> String {
            p.put(json!({ "id": format!("f.{key}"), "component": "Select", "key": key, "action": "set_filter",
                "options": opts(first, values), "selected": ui.filter(key) }));
            format!("f.{key}")
        };
        let mut row2 = vec![
            sel(&mut p, "status", "Any status", KINDS.iter().map(|k| (k.to_string(), k.to_string())).collect()),
            sel(&mut p, "type", "Any type", uniq(&|b| vec![b.kind.clone()])),
            sel(&mut p, "label", "Any label", uniq(&|b| b.labels.clone())),
            sel(&mut p, "pri", "Any priority", (0..5).map(|i| (i.to_string(), format!("P{i}"))).collect()),
        ];
        p.put(json!({ "id": "hide", "component": "Check", "text": "Hide done", "value": ui.hide, "action": "set_hide" }));
        row2.push("hide".into());
        let sorts: Vec<Value> =
            COLS.iter().map(|c| json!({ "value": c.key, "text": format!("Sort: {}", c.label) })).collect();
        p.put(json!({ "id": "sort", "component": "Select", "key": "sort", "action": "set_sort", "options": sorts,
            "selected": ui.sort.0, "disabled": ui.custom() }));
        row2.push("sort".into());
        let dirty = !ui.q.is_empty() || ui.f.values().any(|v| !v.is_empty());
        row2.push(p.pill("clear", "Clear", "clear", json!({}), json!({ "disabled": !dirty })));
        let (tbl, shown, total) = table(&ix, ui);
        let count = if ui.view == "list" || ui.custom() { format!("{shown} of {total}") } else { format!("{shown} rows") };
        row2.push(p.label("count", &count, json!({ "tone": "muted", "size": 12 })));
        page.push(p.boxed("tools2", "h", row2, json!({ "spacing": 8, "padding": [8, 0, 0, 0], "align": "center", "trailing": 1 })));

        if ui.custom() {
            page.push(p.label("graph.todo", "The Focus and Graph views are not drawn natively yet.", json!({ "tone": "muted", "size": 13 })));
        } else {
            let mut t = tbl;
            t["id"] = json!("table");
            p.put(t);
            page.push(p.boxed("table.b", "v", vec!["table".into()], json!({ "padding": [10, 0, 0, 0] })));
        }
        if let Some(d) = ui.detail.as_deref() {
            if let Some(id) = detail(&mut p, &ix, ui, d) {
                page.push(id);
            }
        }
    }
    p.put(json!({ "id": "page", "component": "Box", "axis": "v", "align": "start", "padding": [0, 0, 48, 0], "children": page }));
    p.out
}

fn skeleton() -> Value {
    json!([
        { "version": "v0.9", "createSurface": { "surfaceId": SURFACE, "catalogId": CATALOG } },
        { "version": "v0.9", "updateComponents": { "surfaceId": SURFACE, "components": [
            { "id": "root", "component": "Column", "align": "stretch", "children": ["page"] },
            { "id": "page", "component": "Column", "children": [] }
        ] } }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bead(id: &str, f: impl Fn(&mut Bead)) -> Bead {
        let mut b = Bead {
            id: id.into(), title: id.into(), status: "open".into(), kind: "task".into(), priority: Some(2),
            parent: None, labels: vec![], owner: None, updated: String::new(), blocked_by: vec![],
            description: String::new(), ready: false, epic: false,
        };
        f(&mut b);
        b
    }

    fn snap(beads: Vec<Bead>) -> Snapshot {
        Snapshot { name: "t".into(), path: "/t".into(), beads, has_repo: true, ..Default::default() }
    }

    #[test]
    fn ids_sort_by_number() {
        assert_eq!(natural("wheel-9", "wheel-10"), std::cmp::Ordering::Less);
        assert_eq!(natural("wheel-h3z5.2", "wheel-h3z5.10"), std::cmp::Ordering::Less);
    }

    #[test]
    fn epoch_matches_unix_time() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), 0);
        assert_eq!(epoch("2026-10-30T01:07:41Z"), 1793322461);
    }

    #[test]
    fn done_is_hidden_until_asked_for() {
        let s = snap(vec![bead("a", |_| {}), bead("b", |b| b.status = "closed".into())]);
        let ix = Ix::new(&s);
        let mut ui = Ui::default();
        assert_eq!(build(&ix, &ui).len(), 1);
        ui.hide = false;
        assert_eq!(build(&ix, &ui).len(), 2);
        ui.hide = true;
        ui.f.insert("status".into(), "done".into());
        assert_eq!(build(&ix, &ui).len(), 1);
    }

    #[test]
    fn deps_view_is_a_tree_of_what_a_bead_waits_for() {
        let s = snap(vec![
            bead("a", |b| b.blocked_by = vec!["b".into()]),
            bead("b", |b| b.blocked_by = vec!["c".into()]),
            bead("c", |_| {}),
        ]);
        let ix = Ix::new(&s);
        let ui = Ui { view: "deps".into(), ..Default::default() };
        let rows = build(&ix, &ui);
        let ids: Vec<(&str, usize)> = rows.iter().map(|r| (r.b.id.as_str(), r.depth)).collect();
        assert_eq!(ids, [("a", 0), ("b", 1), ("c", 2)]);
    }

    #[test]
    fn the_page_builds_every_view() {
        let s = snap(vec![
            bead("e", |b| b.kind = "epic".into()),
            bead("a", |b| b.parent = Some("e".into())),
            bead("b", |b| { b.parent = Some("e".into()); b.blocked_by = vec!["a".into()] }),
        ]);
        for v in ["list", "epics", "deps", "unlocks", "focus", "graph"] {
            let ui = Ui { view: v.into(), detail: Some("a".into()), ..Default::default() };
            let c = page_components(&s, &ui);
            assert!(c.iter().any(|x| x["id"] == "page"), "{v}");
        }
    }
}
