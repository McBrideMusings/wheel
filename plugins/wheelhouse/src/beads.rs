//! The Beads plugin: a project's beads read through `bd`, the page's logic (views, search,
//! filters, sort, trees) and its drawing as A2UI. The same logic serves the command line
//! (`cli`) and a host's pane over MCP (`serve`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::core::buttons::Script;
use crate::core::page::{cell, Page};
use crate::core::table;

pub mod cli;
pub mod graph;
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
    crate::core::run::bd(path, args)
}

/// The beads of the project in `path`: what `bd` lists, which are ready, and the buttons the
/// project writes in its wheel.toml.
fn collect(path: &str) -> Snapshot {
    let path = path.to_string();
    let name = crate::core::project::name_of(&path);
    let mut snap = Snapshot { name, path: path.clone(), ..Default::default() };
    if !PathBuf::from(&path).join(".beads").is_dir() {
        return snap;
    }
    snap.has_repo = true;
    let (s, problems) = crate::core::buttons::scripts(&path, "beads");
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
pub fn epoch(iso: &str) -> i64 {
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
    /// Shown only when the pane is wide.
    wide: bool,
}

const COLS: [Col; 7] = [
    Col { key: "id", label: "ID", width: Some(56.0), tree: false, wide: true },
    Col { key: "title", label: "Title", width: None, tree: true, wide: false },
    Col { key: "type", label: "Type", width: Some(56.0), tree: false, wide: true },
    Col { key: "unlocks", label: "Unlocks", width: Some(28.0), tree: false, wide: true },
    Col { key: "status", label: "Status", width: Some(84.0), tree: false, wide: false },
    Col { key: "pri", label: "Priority", width: Some(24.0), tree: false, wide: false },
    Col { key: "updated", label: "Updated", width: Some(30.0), tree: false, wide: true },
];

struct Ui {
    view: String,
    q: String,
    f: BTreeMap<String, String>,
    /// The filters (by key) whose values are excluded rather than matched.
    not: HashSet<String>,
    hide: bool,
    collapsed: HashSet<String>,
    detail: Option<String>,
    hist: Vec<String>,
    /// What the last button run did, shown in the open bead.
    ran: Option<String>,
    /// The bead Focus centres on and Graph highlights.
    sel: Option<String>,
    /// How many hops Focus shows on each side.
    depth: usize,
    /// The beads Focus centred on before, most recent last.
    fhist: Vec<String>,
    /// The "+ N more" list that is open in Focus, by key.
    more: Option<String>,
    /// The width the client reported for the Focus canvas.
    cw: f64,
    /// Graph should centre on the selected bead when it is drawn.
    centre: bool,
    /// The column the flat list is sorted by, and whether ascending; a header click sets it.
    sort: (String, bool),
    /// What the last list button did, shown once at the top of the page.
    said: Option<String>,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            view: "list".into(),
            q: String::new(),
            f: BTreeMap::new(),
            not: HashSet::new(),
            hide: true,
            collapsed: HashSet::new(),
            detail: None,
            hist: vec![],
            ran: None,
            sel: None,
            depth: 2,
            fhist: vec![],
            more: None,
            cw: 900.0,
            centre: false,
            sort: ("status".into(), true),
            said: None,
        }
    }
}

impl Ui {
    /// The values a filter holds: one, or several joined by commas.
    fn values(&self, key: &str) -> Vec<&str> {
        self.filter(key).split(',').filter(|s| !s.is_empty()).collect()
    }
    fn filter(&self, key: &str) -> &str {
        self.f.get(key).map(String::as_str).unwrap_or("")
    }
    fn custom(&self) -> bool {
        self.view == "focus" || self.view == "graph"
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
    // A filter holds some values: the bead matches when it has one of them, or, for an excluded filter, none of them.
    let test = |key: &str, f: &dyn Fn(&str) -> bool| {
        let v = ui.values(key);
        v.is_empty() || v.iter().any(|x| f(x)) != ui.not.contains(key)
    };
    let pri = b.priority.map(|p| p.to_string());
    if !test("status", &|s| kind == s) || !test("type", &|t| b.kind == t) || !test("label", &|l| b.labels.iter().any(|x| x == l)) || !test("pri", &|d| pri.as_deref() == Some(d)) {
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
    // Trees keep the page's own order; the flat list follows the sort a header click chose, so the
    // page, the copied ids and a list button all see the rows the screen shows.
    let by = |a: &Bead, b: &Bead| compare(ix, "status", a, b);
    if ui.view == "list" || ui.custom() {
        let (key, asc) = (&ui.sort.0, ui.sort.1);
        let by = |a: &Bead, b: &Bead| {
            let o = compare(ix, key, a, b).then_with(|| compare(ix, "status", a, b));
            if asc { o } else { o.reverse() }
        };
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

/// The ids of the beads the page lists now, in the order shown: what the search, filters, view and
/// sort leave, without epics and milestones (an id list is for working leaves, and an epic's id
/// would stand for its children twice).
fn ids(ix: &Ix, ui: &Ui) -> Vec<String> {
    build(ix, ui).into_iter().filter(|r| r.b.kind != "epic" && r.b.kind != "milestone").map(|r| r.b.id.clone()).collect()
}

// ───────────────────────────── A2UI ─────────────────────────────

pub(crate) fn sane(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

fn menu(ix: &Ix, b: &Bead) -> Vec<Value> {
    ix.snap
        .scripts
        .iter()
        .filter(|s| wants(s, b))
        .map(|s| json!({ "label": s.label.as_deref().unwrap_or(&s.name), "action": "run_script",
            "context": { "bead": b.id, "script": s.name } }))
        .collect()
}

fn row_json(ix: &Ix, ui: &Ui, r: &RowOut, cols: &[&Col]) -> Value {
    let b = r.b;
    let k = kind_of(b);
    let cells: Vec<Value> = cols
        .iter()
        .map(|c| {
            let mut v = match c.key {
                "id" => cell(ix.short(&b.id), json!({ "mono": true, "tone": "muted" })),
                "title" => cell(&b.title, json!({})),
                "type" => cell(&b.kind, json!({ "tone": "muted" })),
                "unlocks" => {
                    let n = ix.unlocked(b).iter().filter(|u| is_open(u)).count();
                    cell(if n > 0 { n.to_string() } else { String::new() }, json!({ "tone": "muted", "sort": n }))
                }
                "status" => cell(k, json!({ "chip": true, "tone": tone_of(k), "sort": krank(k) })),
                "pri" => cell(
                    b.priority.map(|p| format!("P{p}")).unwrap_or_default(),
                    match b.priority {
                        Some(p) if p <= 1 => json!({ "mono": true, "tone": "bad", "sort": p }),
                        Some(p) if p >= 3 => json!({ "mono": true, "tone": "muted", "sort": p }),
                        Some(p) => json!({ "mono": true, "sort": p }),
                        None => json!({ "mono": true, "sort": 9 }),
                    },
                ),
                _ => cell(ago(&b.updated), json!({ "tone": "muted", "sort": epoch(&b.updated) })),
            };
            v["key"] = json!(c.key);
            v
        })
        .collect();
    json!({ "id": b.id, "path": r.path, "depth": r.depth, "kids": r.kids,
        "collapsed": ui.collapsed.contains(&r.path), "dim": k == "done",
        "sel": ui.detail.as_deref() == Some(b.id.as_str()),
        "cells": cells, "menu": menu(ix, b) })
}

fn table(ix: &Ix, ui: &Ui) -> (Value, usize, usize) {
    let rows = build(ix, ui);
    let cols: Vec<&Col> = COLS.iter().collect();
    let total = ix.snap.beads.iter().filter(|b| b.kind != "epic" && b.kind != "milestone").count();
    let shown = rows.len();
    let sections: Vec<Value> = vec![table::section("", rows.iter().map(|r| row_json(ix, ui, r, &cols)).collect())];
    let columns: Vec<Value> = cols
        .iter()
        .map(|c| table::column(c.key, c.label, json!(c.width), c.tree, c.wide))
        // Only the flat list sorts: a header click re-orders every row, which would tear a tree apart.
        .map(|c| if ui.view == "list" || ui.custom() { table::sortable(c) } else { c })
        .collect();
    let v = table::component(
        columns,
        sections,
        "select_bead",
        "toggle_path",
        // A tree has no sortable column, so it carries no sort action and no header arrow.
        if ui.view == "list" || ui.custom() { "set_sort" } else { "" },
        (&ui.sort.0, ui.sort.1),
        if total == 0 { "No beads yet" } else { "Nothing matches the search and filters." },
    );
    (v, shown, total)
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
    let link = p.button(
        &format!("{id}.l"),
        &format!("{}  {}", ix.short(&b.id), b.title),
        "open_bead",
        json!({ "id": b.id }),
        json!({ "style": "link" }),
    );
    let chip = p.label(&format!("{id}.c"), k, json!({ "chip": true, "size": 11, "tone": tone_of(k) }));
    // The title takes the line and the status chip keeps one column, so the chips of a list line up whatever the titles.
    crate::core::layout::modify(&mut p.out, &format!("{id}.l.t"), json!({ "lineLimit": 1 }));
    let mut kids_ids = vec![p.boxed(&format!("{id}.h"), "h", vec![link, chip], json!({ "spacing": 6, "align": "center", "widths": ["flex", 76] }))];
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
        c.push(p.button("d.back", "← Back", "back", json!({}), json!({ "style": "link" })));
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
        let v = p.button("d.f.pa.v", &format!("{}  {}", ix.short(&par.id), par.title), "open_bead", json!({ "id": par.id }), json!({ "style": "link" }));
        c.push(fact(p, "pa", "Parent", v));
    }
    if let Some(o) = &b.owner {
        let v = p.label("d.f.ow.v", o, json!({}));
        c.push(fact(p, "ow", "Owner", v));
    }
    let v = p.label("d.f.up.v", &format!("{} ago", ago(&b.updated)), json!({}));
    c.push(fact(p, "up", "Updated", v));
    if !b.description.is_empty() {
        // Flat: a rule above and below the text, no raised box.
        p.put(json!({ "id": "d.desc.t", "component": "Divider" }));
        c.push("d.desc.t".into());
        c.push(p.label("d.desc", &b.description, json!({ "size": 13 })));
        p.put(json!({ "id": "d.desc.u", "component": "Divider" }));
        c.push("d.desc.u".into());
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
        .map(|s| p.button(&format!("d.act.{}", sane(&s.name)), s.label.as_deref().unwrap_or(&s.name), "run_script", json!({ "bead": b.id, "script": s.name }), json!({ "style": "primary" })))
        .collect();
    let body = p.boxed("d.body", "v", c, json!({ "spacing": 12, "align": "start" }));
    // The buttons sit in the card's footer beside Close, with the last run's result before them.
    let mut footer = vec![];
    if !acts.is_empty() {
        if let Some(r) = &ui.ran {
            footer.push(p.label("d.ran", r, json!({ "tone": "ok", "weight": "semibold" })));
        }
        footer.extend(acts);
    }
    p.put(json!({ "id": "detail", "component": "Modal", "presented": true, "dismissAction": "close_detail", "children": [body], "actions": footer }));
    Some("detail".into())
}

fn page_components(snap: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let ix = Ix::new(snap);
    let all: Vec<&Bead> = snap.beads.iter().filter(|b| b.kind != "epic" && b.kind != "milestone").collect();
    let mut page: Vec<String> = vec![];


    let n = |f: &dyn Fn(&Bead) -> bool| all.iter().filter(|b| f(b)).count();
    // The four boxes are the status filter: a tap picks one, a tap on the chosen one clears it.
    let kpis = [
        ("Ready", "ready", n(&|b| b.ready && b.status == "open"), true),
        ("Active", "in progress", n(&|b| b.status == "in_progress"), false),
        ("Blocked", "blocked", n(&|b| b.status == "open" && !b.ready && !b.blocked_by.is_empty()), false),
        ("Closed", "done", n(&|b| b.status == "closed"), false),
    ];
    let cards: Vec<String> = kpis
        .iter()
        .enumerate()
        .map(|(i, (label, kind, count, hot))| {
            let on = ui.values("status") == [*kind];
            let tone = if *hot && *count > 0 { "ok" } else if *count == 0 { "muted" } else { "fg" };
            let v = p.label(&format!("kpi{i}.v"), &count.to_string(), json!({ "size": 20, "weight": "semibold", "tabular": true, "tone": tone }));
            let l = p.label(&format!("kpi{i}.l"), label, json!({ "tone": "muted", "size": 11 }));
            crate::core::layout::modify(&mut p.out, &l, json!({ "lineLimit": 1 }));
            let inner = p.boxed(&format!("kpi{i}.c"), "v", vec![v, l], json!({ "spacing": 2, "align": "start" }));
            let mut mods = vec![json!({ "frame": { "maxWidth": "infinity" } }), json!({ "padding": [7, 8] }),
                json!({ "overlay": { "stroke": if on { "tint" } else { "separator" }, "cornerRadius": 8 } })];
            if on {
                mods.push(json!({ "background": "controlBackground" }));
            }
            let value = if on { "" } else { kind };
            p.put(json!({ "id": format!("kpi{i}"), "component": "Button", "child": inner, "variant": "borderless",
                "action": { "event": { "name": "set_filter", "context": { "key": "status", "value": value } } }, "modifiers": mods }));
            format!("kpi{i}")
        })
        .collect();
    page.push(p.boxed("kpis", "h", cards, json!({ "spacing": 8, "widths": ["flex", "flex", "flex", "flex"] })));

    for (i, pr) in snap.problems.iter().chain(snap.error.iter()).enumerate() {
        let l = p.label(&format!("problem{i}.t"), pr, json!({ "tone": "bad" }));
        page.push(p.boxed(&format!("problem{i}"), "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if let Some(said) = &ui.said {
        let l = p.label("said.t", said, json!({ "tone": "ok" }));
        page.push(p.boxed("said", "v", vec![l], json!({ "border": "ok", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }

    if !snap.has_repo {
        page.push(p.label("none", "This space has no .beads directory, so there is nothing to list.", json!({ "tone": "muted" })));
    } else {
        // Search with the views choice beside it, one Filters button opening a sheet, and the active filters as chips.
        p.search("search", "set_query", &ui.q, "Search");
        p.choose("views", "/set_view", &ui.view, vec![
                json!({ "value": "list", "text": "List" }), json!({ "value": "epics", "text": "Epics" }),
                json!({ "value": "deps", "text": "Depends on" }), json!({ "value": "unlocks", "text": "Unlocks" }),
                json!({ "value": "focus", "text": "Focus" }), json!({ "value": "graph", "text": "Graph" })], true, false);
        crate::core::layout::modify(&mut p.out, "search", json!({ "frame": { "minWidth": 120, "maxWidth": "infinity" } }));
        // The views choice is offered its width before the search field, so the field shrinks to make room for the segmented control.
        crate::core::layout::modify(&mut p.out, "views", json!({ "layoutPriority": 1 }));
        p.boxed("find", "h", vec!["search".into(), "views".into()], json!({ "spacing": 8, "align": "center", "widths": ["flex", Value::Null], "padding": [8, 0, 0, 0] }));
        // The row takes the page's full width, so the search field grows to the views choice.
        crate::core::layout::modify(&mut p.out, "find", json!({ "frame": { "maxWidth": "infinity" } }));
        page.push("find".to_string());

        // The filters are one row of menus, each starting at "Any": status, type, label and priority, then the count.
        // Each filter is a popover of ticks; its title says what it holds. "Is not" excludes the ticked values.
        let menu = |p: &mut Page, key: &str, name: &str, opts: Vec<(String, String)>| -> String {
            let held = ui.values(key);
            let neg = ui.not.contains(key);
            let shown = |v: &str| if key == "pri" { format!("P{v}") } else { v.to_string() };
            let title = if held.is_empty() { format!("Any {name}") } else {
                format!("{}{}: {}", name[..1].to_uppercase() + &name[1..], if neg { " not" } else { "" }, held.iter().map(|v| shown(v)).collect::<Vec<_>>().join(", "))
            };
            let mut kids = vec![];
            p.bind(&format!("/set_not/{key}"), json!(neg));
            p.put(json!({ "id": format!("filters.{key}.not"), "component": "CheckBox", "label": "Is not", "value": { "path": format!("/set_not/{key}") } }));
            p.put(json!({ "id": format!("filters.{key}.div"), "component": "Divider" }));
            kids.push(format!("filters.{key}.not"));
            kids.push(format!("filters.{key}.div"));
            for (v, text) in opts {
                let path = format!("/toggle_filter/{key}:{v}");
                let id = format!("filters.{key}.{v}");
                p.bind(&path, json!(held.contains(&v.as_str())));
                p.put(json!({ "id": id, "component": "CheckBox", "label": text, "value": { "path": path } }));
                kids.push(id);
            }
            // A Mac menu closes after each tick, so the ticks sit in a popover, which stays open until it is dismissed.
            let id = format!("filters.{key}");
            let trigger = p.label(&format!("filters.{key}.title"), &format!("{title} ⌄"), json!({ "size": 12 }));
            let content = p.boxed(&format!("filters.{key}.ticks"), "v", kids, json!({ "spacing": 6, "align": "start" }));
            p.put(json!({ "id": id, "component": "Popover", "children": [trigger, content] }));
            id
        };
        let counted = |f: &dyn Fn(&Bead) -> Vec<String>| -> Vec<(String, String)> {
            let mut c: BTreeMap<String, usize> = BTreeMap::new();
            for v in all.iter().flat_map(|b| f(b)) {
                *c.entry(v).or_default() += 1;
            }
            let mut v: Vec<(String, usize)> = c.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            v.into_iter().map(|(k, n)| (k.clone(), format!("{k}  {n}"))).collect()
        };
        let status = menu(&mut p, "status", "status", KINDS.iter().map(|k| (k.to_string(), k.to_string())).collect());
        let kind = menu(&mut p, "type", "type", counted(&|b| vec![b.kind.clone()]));
        let label = menu(&mut p, "label", "label", counted(&|b| b.labels.clone()));
        let pri_opts = (0..=4).map(|n| (n.to_string(), format!("P{n}  {}", all.iter().filter(|b| b.priority == Some(n)).count()))).collect();
        let pri = menu(&mut p, "pri", "priority", pri_opts);
        let (tbl, shown, total) = table(&ix, ui);
        let count = if ui.view == "list" || ui.custom() { format!("{shown} of {total}") } else { format!("{shown} rows") };
        let count_id = p.label("count", &count, json!({ "tone": "muted", "size": 11, "tabular": true, "align": "end" }));
        // Two groups, by proximity: what narrows the list (status, type, label, priority) and the count. Inside a group the gap
        // is 6; the gap between groups is whatever width is left, never under 16. Sorting is the column headers' job.
        let narrow = p.boxed("bar.narrow", "h", vec![status, kind, label, pri], json!({ "spacing": 6, "align": "center" }));
        // Clear keeps its place whether or not a filter is on: it is hidden, not removed, so the count never moves.
        let active = !ui.q.is_empty() || !ui.values("status").is_empty() || !ui.values("type").is_empty() || !ui.values("label").is_empty() || !ui.values("pri").is_empty();
        let clear = p.button("filters.clear", "Clear", "clear", json!({}), json!({ "style": "link", "disabled": !active, "hideOff": true }));
        // The listed ids as one string, and the project's list buttons over the same ids. Each keeps its slot: with no rows
        // the buttons are disabled, not removed.
        let listed = ids(&ix, ui);
        let n = listed.len();
        let mut acts = vec![clear];
        acts.push(p.copy_button("list.copy", "Copy ids", &listed.join(" "),
            json!({ "style": "link", "disabled": n == 0, "help": format!("Copy the {n} listed ids, space separated") })));
        for s in snap.scripts.iter().filter(|s| s.on_list()) {
            acts.push(p.button(&format!("list.{}", s.name), s.label(), "run_list_script", json!({ "script": s.name }),
                json!({ "style": "link", "disabled": n == 0, "help": format!("{} over the {n} listed beads", s.label()) })));
        }
        acts.push(count_id);
        let shape = p.boxed("bar.shape", "h", acts, json!({ "spacing": 10, "align": "center" }));
        page.push(p.boxed("bar", "h", vec![narrow, shape], json!({ "spacing": 16, "align": "center", "widths": ["flex", Value::Null], "padding": [4, 0, 0, 0] })));
        crate::core::layout::modify(&mut p.out, "bar", json!({ "frame": { "maxWidth": "infinity" } }));

        if ui.custom() {
            let parts = graph::Model::new(snap, ui).components(&mut p, ui);
            page.push(p.boxed("graph.b", "v", parts, json!({ "padding": [0, 0, 0, 0] })));
        } else {
            let mut t = tbl;
            t["id"] = json!("table");
            p.put(t);
            page.push(p.boxed("table.b", "v", vec!["table".into()], json!({})));
        }
        // A closed panel is sent as a Modal that is not presented: the client keeps every component it was sent, so only that
        // replaces the open one.
        match ui.detail.as_deref().and_then(|d| detail(&mut p, &ix, ui, d)) {
            Some(id) => page.push(id),
            None => {
                p.put(json!({ "id": "detail", "component": "Modal", "presented": false, "dismissAction": "close_detail" }));
            }
        }
    }
    p.boxed("page", "v", page, json!({ "align": "start", "padding": [18, 16, 48, 16] }));
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

    #[test]
    fn a_filter_of_several_values_matches_any_of_them_and_priority_groups_work() {
        let b1 = bead("a", |b| { b.kind = "bug".into(); b.labels = vec!["client".into()]; b.priority = Some(1); });
        let b2 = bead("b", |b| { b.kind = "task".into(); b.labels = vec!["mac".into()]; b.priority = Some(3); });
        let mut ui = Ui::default();
        ui.f.insert("type".into(), "bug,feature".into());
        assert!(matches(&ui, &b1) && !matches(&ui, &b2));
        ui.f.insert("type".into(), "".into());
        ui.f.insert("label".into(), "client,mac".into());
        assert!(matches(&ui, &b1) && matches(&ui, &b2));
        ui.f.insert("label".into(), "".into());
        ui.f.insert("pri".into(), "0,1,2".into());
        assert!(matches(&ui, &b1) && !matches(&ui, &b2));
        ui.f.insert("pri".into(), "3".into());
        assert!(!matches(&ui, &b1) && matches(&ui, &b2));
        ui.not.insert("pri".into());
        assert!(matches(&ui, &b1) && !matches(&ui, &b2), "an excluded filter keeps what has none of its values");
        ui.f.insert("pri".into(), "".into());
        ui.not.clear();
        ui.f.insert("label".into(), "client".into());
        ui.not.insert("label".into());
        assert!(!matches(&ui, &b1) && matches(&ui, &b2));
    }

    #[test]
    fn copy_ids_holds_the_listed_ids_in_the_shown_order_and_list_buttons_follow_them() {
        let mut s = snap(vec![
            bead("t-1", |b| { b.priority = Some(3); b.title = "c".into(); }),
            bead("t-2", |b| { b.priority = Some(1); b.title = "a".into(); }),
            bead("t-3", |b| { b.priority = Some(2); b.title = "b".into(); b.kind = "epic".into(); }),
            bead("t-4", |b| { b.priority = Some(0); b.title = "d".into(); b.status = "closed".into(); }),
        ]);
        s.scripts = vec![
            Script { name: "iterate-list".into(), label: Some("Iterate".into()), on: vec!["list".into()], run: "true".into() },
            Script { name: "implement".into(), label: None, on: vec!["ready".into()], run: "true".into() },
        ];
        let copied = |c: &[Value]| c.iter().find(|x| x["id"] == "list.copy").unwrap()["action"]["copy"]["text"].as_str().unwrap().to_string();
        let disabled = |c: &[Value], id: &str| c.iter().find(|x| x["id"] == id).unwrap()["modifiers"].as_array().is_some_and(|m| m.iter().any(|x| x["disabled"] == true));
        let mut ui = Ui::default();
        let c = page_components(&s, &ui);
        assert_eq!(copied(&c), "t-1 t-2", "the open leaves in the page's order; the epic and the closed bead are not listed");
        let shape = c.iter().find(|x| x["id"] == "bar.shape").unwrap();
        assert_eq!(shape["children"], json!(["filters.clear", "list.copy", "list.iterate-list", "count"]), "only the list button joins Copy ids");
        assert!(!disabled(&c, "list.copy") && !disabled(&c, "list.iterate-list"));
        let table = c.iter().find(|x| x["id"] == "table").unwrap();
        assert_eq!((table["sortAction"].as_str(), &table["sort"]), (Some("set_sort"), &json!({ "key": "status", "asc": true })), "a header click reaches the page");
        ui.sort = ("title".into(), false);
        let c = page_components(&s, &ui);
        assert_eq!(copied(&c), "t-1 t-2");
        ui.sort = ("title".into(), true);
        let c = page_components(&s, &ui);
        assert_eq!(copied(&c), "t-2 t-1", "the sort a header chose orders the ids");
        ui.view = "epics".into();
        let c = page_components(&s, &ui);
        assert_eq!(c.iter().find(|x| x["id"] == "table").unwrap()["sortAction"], "", "a tree view has no sort action");
        ui.view = "list".into();
        let c = page_components(&s, &ui);
        let rows = c.iter().find(|x| x["id"] == "table").unwrap()["sections"][0]["rows"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(rows, ["t-2", "t-1"], "and the table's rows come in the same order");
        ui.q = "nothing-here".into();
        let c = page_components(&s, &ui);
        assert_eq!(copied(&c), "");
        assert!(disabled(&c, "list.copy") && disabled(&c, "list.iterate-list"), "an empty list keeps the buttons in place, disabled");
        let ix = Ix::new(&s);
        assert!(menu(&ix, &s.beads[0]).iter().all(|m| m["context"]["script"] != "iterate-list"), "a list button is never a row's button");
    }

    #[test]
    fn a_beads_buttons_sit_in_the_detail_footer_not_its_body() {
        let mut s = snap(vec![bead("a", |b| b.status = "in_progress".into())]);
        s.scripts = vec![Script { name: "resume".into(), label: Some("Resume".into()), on: vec!["wip".into()], run: "true".into() }];
        let mut ui = Ui::default();
        ui.detail = Some("a".into());
        let c = page_components(&s, &ui);
        let modal = c.iter().find(|x| x["id"] == "detail").unwrap();
        assert_eq!(modal["actions"], json!(["d.act.resume"]), "the button is a footer action beside Close");
        assert_eq!(modal["children"], json!(["d.body"]), "the card's body holds only the facts");
    }

    #[test]
    fn a_filter_is_a_popover_of_ticks_titled_with_what_it_holds() {
        let s = snap(vec![bead("a", |_| {})]);
        let mut ui = Ui::default();
        ui.f.insert("pri".into(), "0,1".into());
        ui.not.insert("pri".into());
        let c = page_components(&s, &ui);
        let menu = c.iter().find(|x| x["id"] == "filters.pri").expect("the priority menu");
        assert_eq!(menu["component"], "Popover");
        let title = c.iter().find(|x| x["id"] == "filters.pri.title").expect("the trigger text");
        assert_eq!(title["text"], "Priority not: P0, P1 ⌄");
        let tick = c.iter().find(|x| x["id"] == "filters.pri.1").expect("a tick for P1");
        assert_eq!(tick["value"]["path"], "/toggle_filter/pri:1");
        assert!(c.iter().all(|x| x["id"] != "chips"), "the menus show their own state, so there are no chips");
        let clear = c.iter().find(|x| x["id"] == "filters.clear").unwrap();
        assert!(!clear.to_string().contains("\"opacity\":0"), "Clear is shown while a filter is on");
    }

    #[test]
    fn the_page_has_status_boxes_a_search_row_filter_menus_and_a_table() {
        let s = snap(vec![bead("a", |_| {})]);
        let c = page_components(&s, &Ui::default());
        let page = c.iter().find(|x| x["id"] == "page").unwrap()["children"].as_array().unwrap().clone();
        assert_eq!(page[..4], ["kpis", "find", "bar", "table.b"].map(Value::from), "boxes, search with the views choice, the filter menus with Clear and the count, the table");
        let find = c.iter().find(|x| x["id"] == "find").unwrap();
        assert_eq!(find["children"], json!(["search", "views"]));
        let views = c.iter().find(|x| x["id"] == "views").unwrap();
        assert_eq!((views["component"].as_str(), &views["children"]), (Some("ViewThatFits"), &json!(["views.seg", "views.menu"])), "the views choice is the segmented control, then the menu");
        let style = |id: &str| c.iter().find(|x| x["id"] == id).map(|x| x["displayStyle"].clone());
        assert_eq!((style("views.seg"), style("views.menu")), (Some(json!("chips")), Some(json!("menu"))));
        assert_eq!(c.iter().find(|x| x["id"] == "filters.pri").map(|x| x["component"].clone()), Some(json!("Popover")), "a filter is a popover of ticks");
        let bar = c.iter().find(|x| x["id"] == "bar").unwrap();
        assert_eq!(bar["children"], json!(["bar.narrow", "bar.shape"]));
        let narrow = c.iter().find(|x| x["id"] == "bar.narrow").unwrap();
        assert_eq!(narrow["children"], json!(["filters.status", "filters.type", "filters.label", "filters.pri"]), "what narrows the list sits together");
        let shape = c.iter().find(|x| x["id"] == "bar.shape").unwrap();
        assert_eq!(shape["children"], json!(["filters.clear", "list.copy", "count"]), "Clear, the list actions and the count sit together");
        let boxes: Vec<&Value> = c.iter().filter(|x| x["id"].as_str().is_some_and(|i| i.starts_with("kpi") && !i.contains('.') && i != "kpis")).collect();
        assert_eq!(boxes.len(), 4);
        assert!(boxes.iter().all(|b| b["component"] == "Button" && b["action"]["event"]["name"] == "set_filter"), "each box is a status filter");
        let values: Vec<&str> = boxes.iter().map(|b| b["action"]["event"]["context"]["value"].as_str().unwrap()).collect();
        assert_eq!(values, ["ready", "in progress", "blocked", "done"]);
        assert!(values.iter().all(|v| KINDS.contains(v)), "a box filters by a status the rows are grouped under");
        let mut ui = Ui::default();
        ui.f.insert("status".into(), "ready".into());
        let filtered = page_components(&s, &ui);
        let on = filtered.iter().find(|x| x["id"] == "kpi0").unwrap();
        assert_eq!(on["action"]["event"]["context"]["value"], "", "a tap on the chosen box clears it");
        let table = c.iter().find(|x| x["id"] == "table").expect("the table");
        let first = &table["sections"][0]["rows"][0];
        let keys: Vec<&str> = first["cells"].as_array().unwrap().iter().map(|x| x["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["id", "title", "type", "unlocks", "status", "pri", "updated"]);
        let wide: Vec<&str> = table["columns"].as_array().unwrap().iter().filter(|x| x["wide"] == true).map(|x| x["key"].as_str().unwrap()).collect();
        assert_eq!(wide, ["id", "type", "unlocks", "updated"]);
    }
}
