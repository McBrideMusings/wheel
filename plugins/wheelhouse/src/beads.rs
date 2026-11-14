//! A project's beads read through `bd`, and the list logic (views, search, filters, sort, trees)
//! the `wheelhouse beads` command prints with; the plugin's feeds are in `feed` and `graph_feed`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::core::buttons::Script;
use crate::core::list::{Filter, Item, List, Spec};

pub mod buttons;
pub mod cli;
pub mod feed;
pub mod graph;
pub mod graph_feed;

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
    buttons: buttons::Buttons,
    /// The list buttons, `[beads.list.<name>]`, over every bead the page lists.
    scripts: Vec<Script>,
    /// Each bead's button now, by bead id; `fill_acts` sets it.
    acts: HashMap<String, buttons::Act>,
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
    // The per-bead button (`[beads.task]`, `[beads.epic]`) and the list buttons (`[beads.list.<name>]`).
    let (b, problems) = buttons::read(&path);
    snap.buttons = b;
    let (s, more) = crate::core::buttons::scripts(&path, "beads");
    snap.scripts = s;
    snap.problems = problems;
    snap.problems.extend(more);
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

// ───────────────────────────── the list logic ─────────────────────────────

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

/// Set each bead's button from the records and the panes open now. When the panes cannot be listed, a bead
/// with a record shows its button disabled and says so, rather than guessing its pane is gone.
fn fill_acts(snap: &mut Snapshot) {
    let records = buttons::recorded_all(&buttons::records_file(), &snap.path);
    let panes = buttons::open_panes(&snap.path);
    let live = panes.clone().unwrap_or_default();
    snap.acts = snap
        .beads
        .iter()
        .filter_map(|b| {
            let mut act = buttons::decide(&snap.buttons, &b.status, groups(b), records.get(&b.id), &live)?;
            if let (Err(e), Some(_)) = (&panes, records.get(&b.id)) {
                if act.state != buttons::State::Closed {
                    act.disabled = true;
                    act.help = e.clone();
                }
            }
            if buttons::pressed(&snap.path, &b.id) {
                act.disabled = true;
                act.help = "Starting; wait for the pane".into();
            }
            Some((b.id.clone(), act))
        })
        .collect();
}

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

/// What the list a command prints narrows and orders by: the view, the search, filters and sort.
struct Ui {
    view: String,
    list: List,
}

impl Default for Ui {
    fn default() -> Self {
        Ui { view: "list".into(), list: List::new("status", true) }
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

/// What the Beads list narrows by: status (done hidden until asked for), type, label and priority.
pub(crate) const SPEC: Spec = Spec {
    filters: &[
        Filter { key: "status", hidden: Some("done") },
        Filter { key: "type", hidden: None },
        Filter { key: "label", hidden: None },
        Filter { key: "pri", hidden: None },
    ],
};

impl Item for Bead {
    fn haystack(&self) -> String {
        let mut parts = vec![self.id.clone(), self.title.clone(), self.kind.clone()];
        parts.extend(self.labels.iter().cloned());
        parts.push(self.description.clone());
        parts.push(self.owner.clone().unwrap_or_default());
        parts.join(" ")
    }
    fn has(&self, key: &str, value: &str) -> bool {
        match key {
            "status" => kind_of(self) == value,
            "type" => self.kind == value,
            "label" => self.labels.iter().any(|l| l == value),
            "pri" => self.priority.map(|p| p.to_string()).as_deref() == Some(value),
            _ => false,
        }
    }
}

fn matches(list: &List, b: &Bead) -> bool {
    list.keeps(&SPEC, b)
}

struct RowOut<'a> {
    b: &'a Bead,
    depth: usize,
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

fn flat<'a>(n: &Node<'a>, out: &mut Vec<RowOut<'a>>) {
    if out.len() >= 20_000 {
        return;
    }
    out.push(RowOut { b: n.b, depth: n.depth });
    for c in &n.nodes {
        flat(c, out);
    }
}

/// The rows a view shows: a flat sorted list, or a tree
/// flattened with a depth.
fn build<'a>(ix: &Ix<'a>, ui: &Ui) -> Vec<RowOut<'a>> {
    // Trees keep their own order; the flat list follows the sort.
    let by = |a: &Bead, b: &Bead| compare(ix, "status", a, b);
    if ui.view == "list" {
        let (key, asc) = (&ui.list.sort.0, ui.list.sort.1);
        let by = |a: &Bead, b: &Bead| {
            let o = compare(ix, key, a, b).then_with(|| compare(ix, "status", a, b));
            if asc { o } else { o.reverse() }
        };
        let mut v: Vec<&Bead> = ix
            .snap
            .beads
            .iter()
            .filter(|b| b.kind != "epic" && b.kind != "milestone")
            .filter(|b| matches(&ui.list, b))
            .collect();
        v.sort_by(|a, b| by(a, b));
        return v.into_iter().map(|b| RowOut { b, depth: 0 }).collect();
    }
    let pool: Vec<&Bead> = match ui.view.as_str() {
        "epics" => ix.snap.beads.iter().collect(),
        _ => ix.snap.beads.iter().filter(|b| ui.list.values("status").iter().any(|v| v == "done") || is_open(b)).collect(),
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
    let ok = |b: &Bead| matches(&ui.list, b);
    let mut tops: Vec<Node> = roots.into_iter().filter_map(|r| walk(r, &[], 0, &children, &keep, &ok, &by)).collect();
    tops.sort_by(|x, y| by(x.b, y.b));
    let mut out = vec![];
    for n in &tops {
        flat(n, &mut out);
    }
    out
}

pub(crate) fn sane(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
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
        ui.list.f.insert("status".into(), vec!["done".into()]);
        assert_eq!(build(&ix, &ui).len(), 1);
        ui.list.f.insert("status".into(), KINDS.iter().map(|k| k.to_string()).collect());
        assert_eq!(build(&ix, &ui).len(), 2);
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
    fn a_filter_of_several_values_matches_any_of_them_and_priority_groups_work() {
        let b1 = bead("a", |b| { b.kind = "bug".into(); b.labels = vec!["client".into()]; b.priority = Some(1); });
        let b2 = bead("b", |b| { b.kind = "task".into(); b.labels = vec!["mac".into()]; b.priority = Some(3); });
        let mut ui = Ui::default();
        ui.list.f.insert("type".into(), vec!["bug".into(), "feature".into()]);
        assert!(matches(&ui.list, &b1) && !matches(&ui.list, &b2));
        ui.list.f.insert("type".into(), vec![]);
        ui.list.f.insert("label".into(), vec!["client".into(), "mac".into()]);
        assert!(matches(&ui.list, &b1) && matches(&ui.list, &b2));
        ui.list.f.insert("label".into(), vec![]);
        ui.list.f.insert("pri".into(), vec!["0".into(), "1".into(), "2".into()]);
        assert!(matches(&ui.list, &b1) && !matches(&ui.list, &b2));
        ui.list.f.insert("pri".into(), vec!["3".into()]);
        assert!(!matches(&ui.list, &b1) && matches(&ui.list, &b2));
        ui.list.not.insert("pri".into());
        assert!(matches(&ui.list, &b1) && !matches(&ui.list, &b2), "an excluded filter keeps what has none of its values");
        ui.list.f.insert("pri".into(), vec![]);
        ui.list.not.clear();
        ui.list.f.insert("label".into(), vec!["client".into()]);
        ui.list.not.insert("label".into());
        assert!(!matches(&ui.list, &b1) && matches(&ui.list, &b2));
    }
}
