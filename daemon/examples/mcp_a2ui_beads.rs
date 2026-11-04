//! The Beads page as a native process: an MCP server over stdio, run in the space's directory,
//! that reads `bd` and the repo's `[beads.buttons.*]` and answers with A2UI, so a Wheel pane draws
//! the page as native controls. It keeps the tab and the query the person set, tells the pane when
//! the beads change (`notifications/resources/updated`: `.beads/last-touched` moved, or the
//! minute is up and `bd` answers differently), and runs a bead's button.
//!
//! Register it in a wheel.toml: `[artifacts.beads-native]` with
//! `mcp = "<built binary> stdio"`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, MetaObject, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::{
    schemars, service::RequestContext, tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler,
    ServiceExt,
};
use serde_json::{json, Value};

const MIME: &str = "application/a2ui+json";
const URI: &str = "a2ui://beads";
const SURFACE: &str = "beads";
const CATALOG: &str = "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json";

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
    source: String,
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
                source: "wheel.toml".into(),
            });
        }
    }
    (out, vec![])
}

fn collect() -> Snapshot {
    let path = std::env::var("WHEEL_SPACE_DIR")
        .ok()
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_default();
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

// ───────────────────────────── query (a port of artifacts/beads/query.js) ─────────────────────────────

const FIELDS: [&str; 7] = ["status", "priority", "type", "label", "title", "assignee", "owner"];

#[derive(Clone, Debug)]
enum Tok {
    Open(usize),
    Close(usize),
    Op(String, usize),
    Word { v: String, quoted: bool, at: usize },
}

impl Tok {
    fn at(&self) -> usize {
        match self {
            Tok::Open(a) | Tok::Close(a) | Tok::Op(_, a) | Tok::Word { at: a, .. } => *a,
        }
    }
    fn text(&self) -> String {
        match self {
            Tok::Open(_) => "(".into(),
            Tok::Close(_) => ")".into(),
            Tok::Op(v, _) | Tok::Word { v, .. } => v.clone(),
        }
    }
}

fn tokenize(text: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut toks = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            toks.push(Tok::Open(i));
            i += 1;
        } else if c == ')' {
            toks.push(Tok::Close(i));
            i += 1;
        } else if i + 1 < chars.len() && matches!((c, chars[i + 1]), ('!', '=') | ('>', '=') | ('<', '=')) {
            toks.push(Tok::Op(format!("{c}="), i));
            i += 2;
        } else if matches!(c, '=' | '>' | '<') {
            toks.push(Tok::Op(c.to_string(), i));
            i += 1;
        } else if c == '"' || c == '\'' {
            let Some(end) = chars[i + 1..].iter().position(|x| *x == c) else {
                return Err(format!("unterminated quote at column {}", i + 1));
            };
            toks.push(Tok::Word { v: chars[i + 1..i + 1 + end].iter().collect(), quoted: true, at: i });
            i += end + 2;
        } else {
            let mut j = i;
            while j < chars.len() && !(chars[j].is_whitespace() || "()=<>!\"'".contains(chars[j])) {
                j += 1;
            }
            if j == i {
                return Err(format!("unexpected \"{c}\" at column {}", i + 1));
            }
            toks.push(Tok::Word { v: chars[i..j].iter().collect(), quoted: false, at: i });
            i = j;
        }
    }
    Ok(toks)
}

type Pred = Box<dyn Fn(&Bead) -> bool>;

fn priority_of(v: &str) -> Result<i64, String> {
    let t = v.strip_prefix(['p', 'P']).unwrap_or(v);
    t.parse::<i64>().map_err(|_| format!("priority wants a number such as 1 or P1, not \"{v}\""))
}

fn comparison(field: &str, op: &str, value: &str) -> Result<Pred, String> {
    if field == "priority" {
        let want = priority_of(value)?;
        let op = op.to_string();
        return Ok(Box::new(move |b| match b.priority {
            None => op == "!=",
            Some(have) => match op.as_str() {
                "=" => have == want,
                "!=" => have != want,
                ">" => have > want,
                ">=" => have >= want,
                "<" => have < want,
                _ => have <= want,
            },
        }));
    }
    if op != "=" && op != "!=" {
        return Err(format!("{field} takes = or !=, not {op}"));
    }
    let want = value.to_lowercase();
    let field = field.to_string();
    let has: Pred = Box::new(move |b| match field.as_str() {
        "label" => b.labels.iter().any(|l| l.to_lowercase() == want),
        "title" => b.title.to_lowercase().contains(&want),
        "type" => b.kind.to_lowercase() == want,
        "status" => {
            b.status.to_lowercase() == want
                || (want == "blocked" && b.status == "open" && !b.ready && !b.blocked_by.is_empty())
        }
        _ => b.owner.as_deref().unwrap_or("").to_lowercase() == want,
    });
    Ok(if op == "=" { has } else { Box::new(move |b| !has(b)) })
}

struct Parser {
    toks: Vec<Tok>,
    p: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.p)
    }
    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word { v, quoted: false, .. }) if v.to_uppercase() == w)
    }
    fn describe(tok: Option<&Tok>) -> String {
        match tok {
            Some(t) => format!("\"{}\" at column {}", t.text(), t.at() + 1),
            None => "the end of the query".into(),
        }
    }
    fn or(&mut self) -> Result<Pred, String> {
        let mut left = self.and()?;
        while self.is_word("OR") {
            self.p += 1;
            let (l, r) = (left, self.and()?);
            left = Box::new(move |b| l(b) || r(b));
        }
        Ok(left)
    }
    fn and(&mut self) -> Result<Pred, String> {
        let mut left = self.not()?;
        while self.is_word("AND") {
            self.p += 1;
            let (l, r) = (left, self.not()?);
            left = Box::new(move |b| l(b) && r(b));
        }
        Ok(left)
    }
    fn not(&mut self) -> Result<Pred, String> {
        if self.is_word("NOT") {
            self.p += 1;
            let x = self.not()?;
            return Ok(Box::new(move |b| !x(b)));
        }
        self.atom()
    }
    fn atom(&mut self) -> Result<Pred, String> {
        let Some(tok) = self.peek().cloned() else {
            return Err("the query ends where a condition should be".into());
        };
        if let Tok::Open(_) = tok {
            self.p += 1;
            let x = self.or()?;
            if !matches!(self.peek(), Some(Tok::Close(_))) {
                return Err("missing \")\" to close \"(\"".into());
            }
            self.p += 1;
            return Ok(x);
        }
        let Tok::Word { v, quoted, .. } = &tok else {
            return Err(format!("expected a field name, found {}", Self::describe(Some(&tok))));
        };
        if !quoted && matches!(v.to_uppercase().as_str(), "AND" | "OR" | "NOT") {
            return Err(format!("expected a field name, found {}", Self::describe(Some(&tok))));
        }
        let field = v.to_lowercase();
        if !FIELDS.contains(&field.as_str()) {
            return Err(format!("unknown field \"{v}\"; the fields are {}", FIELDS.join(", ")));
        }
        self.p += 1;
        let op = match self.peek() {
            Some(Tok::Op(o, _)) => o.clone(),
            other => {
                return Err(format!(
                    "{field} needs an operator (=, !=, >, >=, <, <=), found {}",
                    Self::describe(other)
                ))
            }
        };
        self.p += 1;
        let val = match self.peek() {
            Some(Tok::Word { v, .. }) => v.clone(),
            other => return Err(format!("{field}{op} needs a value, found {}", Self::describe(other))),
        };
        self.p += 1;
        comparison(&field, &op, &val)
    }
}

fn parse_query(text: &str) -> Result<Pred, String> {
    let toks = tokenize(text)?;
    if toks.is_empty() {
        return Ok(Box::new(|_| true));
    }
    let mut p = Parser { toks, p: 0 };
    let r = p.or()?;
    if p.p < p.toks.len() {
        return Err(format!("unexpected {}", Parser::describe(p.toks.get(p.p))));
    }
    Ok(r)
}

// ───────────────────────────── grouping (a port of the page's script) ─────────────────────────────

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
                let o = n.trim_start_matches('0').len().cmp(&m.trim_start_matches('0').len())
                    .then_with(|| n.trim_start_matches('0').cmp(m.trim_start_matches('0')));
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

fn rank(s: &str) -> u8 {
    match s {
        "in_progress" => 0,
        "open" => 1,
        "deferred" => 2,
        "closed" => 3,
        _ => 1,
    }
}

fn by_work(a: &Bead, b: &Bead) -> std::cmp::Ordering {
    rank(&a.status)
        .cmp(&rank(&b.status))
        .then(b.ready.cmp(&a.ready))
        .then(a.priority.unwrap_or(9).cmp(&b.priority.unwrap_or(9)))
        .then_with(|| natural(&a.id, &b.id))
}

#[derive(Clone, Copy, Default)]
struct Counts {
    closed: usize,
    wip: usize,
    open: usize,
    total: usize,
}

fn counts(bs: &[Bead]) -> Counts {
    let mut c = Counts { total: bs.len(), ..Default::default() };
    for b in bs {
        match b.status.as_str() {
            "closed" => c.closed += 1,
            "in_progress" => c.wip += 1,
            _ => c.open += 1,
        }
    }
    c
}

fn pct(c: Counts) -> usize {
    if c.total == 0 {
        0
    } else {
        (100.0 * c.closed as f64 / c.total as f64).round() as usize
    }
}

fn summary(text: &str) -> String {
    let mut stripped = String::new();
    let mut in_fence = false;
    for line in text.split('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            stripped.push_str(line);
            stripped.push('\n');
        }
    }
    let para = stripped
        .split("\n\n")
        .map(|p| p.trim())
        .find(|p| !p.is_empty() && !p.starts_with('#'))
        .unwrap_or("");
    let plain: Vec<String> = para
        .lines()
        .map(|l| {
            let t = l.trim_start();
            let t = t
                .strip_prefix(['-', '*', '+'])
                .filter(|r| r.starts_with(' '))
                .map(|r| r.trim_start())
                .unwrap_or(t);
            t.to_string()
        })
        .collect();
    let plain = plain.join(" ").replace("**", "").replace("__", "").replace('`', "");
    if plain.chars().count() > 240 {
        let cut: String = plain.chars().take(240).collect();
        let at = cut.rfind(' ').unwrap_or(cut.len());
        format!("{}…", &cut[..at])
    } else {
        plain
    }
}

struct Feature {
    root: Bead,
    single: bool,
    beads: Vec<Bead>,
    done: bool,
}

fn group(beads: &[Bead]) -> (Vec<Feature>, Vec<Bead>) {
    let ids: HashSet<&str> = beads.iter().map(|b| b.id.as_str()).collect();
    let mut kids: HashMap<&str, Vec<&Bead>> = HashMap::new();
    for b in beads {
        if let Some(p) = b.parent.as_deref().filter(|p| ids.contains(p)) {
            kids.entry(p).or_default().push(b);
        }
    }
    fn below<'a>(id: &str, kids: &HashMap<&str, Vec<&'a Bead>>) -> Vec<&'a Bead> {
        kids.get(id)
            .map(|ks| ks.iter().flat_map(|k| std::iter::once(*k).chain(below(&k.id, kids))).collect())
            .unwrap_or_default()
    }
    let top = |b: &Bead| !b.parent.as_deref().is_some_and(|p| ids.contains(p));
    let mut claimed: HashSet<String> = HashSet::new();
    let mut features: Vec<Feature> = beads
        .iter()
        .filter(|b| top(b) && b.kind != "milestone" && (b.kind == "epic" || b.kind == "feature" || kids.contains_key(b.id.as_str())))
        .map(|r| {
            let ds = below(&r.id, &kids);
            claimed.insert(r.id.clone());
            ds.iter().for_each(|d| {
                claimed.insert(d.id.clone());
            });
            let single = ds.is_empty();
            let mut members: Vec<Bead> = if single { vec![r.clone()] } else { ds.iter().map(|d| (*d).clone()).collect() };
            members.sort_by(by_work);
            let done = r.status == "closed" || (!single && ds.iter().all(|d| d.status == "closed"));
            Feature { root: r.clone(), single, beads: members, done }
        })
        .collect();
    features.sort_by(|a, b| {
        let (ca, cb) = (counts(&a.beads), counts(&b.beads));
        a.done
            .cmp(&b.done)
            .then(b.beads.iter().any(|x| x.ready).cmp(&a.beads.iter().any(|x| x.ready)))
            .then(pct(cb).cmp(&pct(ca)))
    });
    let mut loose: Vec<Bead> =
        beads.iter().filter(|b| !claimed.contains(&b.id) && b.kind != "milestone").cloned().collect();
    loose.sort_by(by_work);
    (features, loose)
}

fn kind_of(b: &Bead) -> &'static str {
    match b.status.as_str() {
        "closed" => "closed",
        "in_progress" => "in_progress",
        "deferred" => "deferred",
        _ if b.ready => "ready",
        _ if !b.blocked_by.is_empty() => "blocked",
        _ => "open",
    }
}

fn status_word(k: &str) -> &'static str {
    match k {
        "closed" => "done",
        "in_progress" => "in progress",
        "ready" => "ready",
        "blocked" => "blocked",
        "deferred" => "deferred",
        _ => "open",
    }
}

fn wants(s: &Script, b: &Bead) -> bool {
    s.on.iter().any(|k| match k.as_str() {
        "ready" => b.ready && b.status == "open",
        "wip" => b.status == "in_progress",
        "epic" => b.epic && b.status != "closed",
        "blocked" => b.status == "open" && !b.ready,
        _ => false,
    })
}

// ───────────────────────────── A2UI components ─────────────────────────────

fn sane(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

/// The page's components, built with stable ids so a redraw keeps what the pane holds for them.
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
    fn button(&mut self, id: &str, label: &str, src: Option<&str>, name: &str, context: Value, selected: bool) -> String {
        let mut v = json!({ "id": id, "component": "Pill", "text": label, "name": name, "context": context, "selected": selected });
        if let Some(src) = src {
            v["src"] = json!(src);
        }
        self.put(v);
        id.to_string()
    }
}

struct View {
    tab: String,
    q: String,
}

struct Ctx<'a> {
    snap: &'a Snapshot,
}

impl Ctx<'_> {
    fn buttons(&self, p: &mut Page, scope: &str, b: &Bead) -> Option<String> {
        let ids: Vec<String> = self
            .snap
            .scripts
            .iter()
            .filter(|s| wants(s, b))
            .map(|s| {
                p.button(
                    &format!("btn.{}.{}.{}", sane(scope), sane(&b.id), sane(&s.name)),
                    s.label.as_deref().unwrap_or(&s.name),
                    Some(&s.source),
                    "run_script",
                    json!({ "bead": b.id, "script": s.name }),
                    false,
                )
            })
            .collect();
        if ids.is_empty() {
            return None;
        }
        Some(p.boxed(
            &format!("acts.{}.{}", sane(scope), sane(&b.id)),
            "h",
            ids,
            json!({ "spacing": 6, "controlSize": "small" }),
        ))
    }

    fn bead_row(&self, p: &mut Page, scope: &str, b: &Bead, last: bool) -> String {
        let id = format!("row.{}.{}", sane(scope), sane(&b.id));
        let k = kind_of(b);
        let mut head: Vec<String> = vec![];
        let mut widths: Vec<Value> = vec![];
        if b.kind != "task" {
            head.push(p.label(&format!("{id}.ty"), &b.kind, json!({ "size": 10, "tone": "muted", "chip": true })));
            widths.push(Value::Null);
        }
        for (i, l) in b.labels.iter().enumerate() {
            head.push(p.label(&format!("{id}.lb{i}"), l, json!({ "size": 10, "tone": "muted", "chip": true })));
            widths.push(Value::Null);
        }
        head.push(p.label(
            &format!("{id}.ti"),
            &b.title,
            json!({ "tone": if k == "closed" { "muted" } else { "fg" } }),
        ));
        widths.push(json!("flex"));
        let mut kids = vec![p.boxed(&format!("{id}.h"), "h", head, json!({ "spacing": 5, "align": "firstTextBaseline", "widths": widths }))];
        if k == "blocked" {
            kids.push(p.label(&format!("{id}.why"), &format!("blocked by {}", b.blocked_by.join(", ")), json!({ "size": 11, "tone": "muted" })));
        }
        let mut meta = vec![p.label(&format!("{id}.id"), &b.id, json!({ "mono": true, "size": 11, "tone": "muted" }))];
        let (tone, dot, weight) = match k {
            "closed" => ("ok", "fill", "semibold"),
            "in_progress" => ("warn", "fill", "semibold"),
            "ready" => ("ok", "ring", "semibold"),
            "blocked" => ("bad", "fill", "semibold"),
            _ => ("muted", "hollow", "regular"),
        };
        meta.push(p.label(&format!("{id}.st"), status_word(k), json!({ "size": 11, "tone": tone, "dot": dot, "weight": weight })));
        if let Some(pr) = b.priority {
            meta.push(p.label(
                &format!("{id}.pr"),
                &format!("P{pr}"),
                json!({ "mono": true, "size": 11, "tone": if pr <= 1 { "bad" } else { "muted" } }),
            ));
        }
        let acts = self.buttons(p, scope, b);
        let trailing = usize::from(acts.is_some());
        meta.extend(acts);
        kids.push(p.boxed(
            &format!("{id}.m"),
            "h",
            meta,
            json!({ "spacing": 12, "align": "firstTextBaseline", "trailing": trailing }),
        ));
        let mut extra = json!({ "spacing": 4, "padding": [8, 14] });
        if !last {
            extra["edge"] = json!("bottom");
            extra["dash"] = json!(true);
        }
        if k == "ready" {
            extra["fill"] = json!("ready");
        }
        p.boxed(&id, "v", kids, extra)
    }

    fn rows(&self, p: &mut Page, scope: &str, beads: &[Bead]) -> Vec<String> {
        beads.iter().enumerate().map(|(i, b)| self.bead_row(p, scope, b, i + 1 == beads.len())).collect()
    }

    fn card(&self, p: &mut Page, id: &str, child: String) -> String {
        p.boxed(id, "v", vec![child], json!({ "border": "line", "fill": "card", "radius": 10, "margin": [0, 0, 8, 0] }))
    }

    /// A feature, a label or a type: a row with a bar and its beads under it.
    #[allow(clippy::too_many_arguments)]
    fn section(&self, p: &mut Page, key: &str, title: &str, tag: &str, beads: &[Bead], done: bool, acts: Option<String>, desc: &str) -> String {
        let id = format!("sec.{}", sane(key));
        let c = counts(beads);
        let mut runs = vec![json!({ "text": title, "weight": "semibold", "size": 14, "tone": "fg" })];
        if done {
            runs.push(json!({ "text": " ✓", "weight": "semibold", "size": 14, "tone": "ok" }));
        }
        runs.push(json!({ "text": format!("  {tag}"), "size": 11, "mono": true, "tone": "muted", "weight": "regular" }));
        let t = p.runs(&format!("{id}.t0"), Value::Array(runs), json!({}));
        let t = p.boxed(&format!("{id}.t"), "v", vec![t], json!({}));
        p.put(json!({ "id": format!("{id}.bar"), "component": "ProgressBar",
            "closed": c.closed, "wip": c.wip, "total": c.total }));
        let pc = p.label(&format!("{id}.pc"), &format!("{}%", pct(c)), json!({ "weight": "semibold", "align": "end", "size": 14 }));
        let wip = if c.wip > 0 { format!(" · {} wip", c.wip) } else { String::new() };
        let ps = p.label(&format!("{id}.ps"), &format!("{}/{}{wip}", c.closed, c.total), json!({ "size": 11, "tone": "muted", "align": "end" }));
        let pcb = p.boxed(&format!("{id}.pcb"), "v", vec![pc, ps], json!({ "align": "end" }));
        let row = p.boxed(
            &format!("{id}.row"),
            "h",
            vec![t, format!("{id}.bar"), pcb],
            json!({ "spacing": 14, "align": "firstTextBaseline", "widths": ["flex", 160, 64] }),
        );
        let mut head = vec![row];
        if let Some(a) = acts {
            head.push(a);
        }
        if !desc.is_empty() {
            let d = p.label(&format!("{id}.d"), desc, json!({ "size": 12, "tone": "muted" }));
            head.push(p.boxed(&format!("{id}.db"), "h", vec![d], json!({ "widths": ["flex"] })));
        }
        let head = p.boxed(&format!("{id}.head"), "v", head, json!({ "spacing": 6, "padding": [10, 14] }));
        let rows = self.rows(p, key, beads);
        let body = p.boxed(&format!("{id}.body"), "v", rows, json!({ "edge": "top" }));
        p.put(json!({ "id": id, "component": "Disclosure", "children": [head, body], "chevron": "overlay", "expanded": false }));
        self.card(p, &format!("{id}.card"), id.clone())
    }

    fn list(&self, p: &mut Page, key: &str, beads: &[Bead]) -> String {
        let rows = self.rows(p, key, beads);
        let body = p.boxed(&format!("list.{}", sane(key)), "v", rows, json!({}));
        self.card(p, &format!("list.{}.card", sane(key)), body)
    }
}

fn h3(p: &mut Page, id: &str, text: &str) -> String {
    let l = p.label(id, text, json!({ "size": 11, "weight": "semibold", "tone": "muted", "upper": true, "tracking": 0.44 }));
    p.boxed(&format!("{id}.b"), "v", vec![l], json!({ "padding": [18, 0, 6, 0] }))
}

fn empty(p: &mut Page, id: &str, text: &str) -> String {
    let l = p.label(id, text, json!({ "tone": "muted" }));
    p.boxed(&format!("{id}.b"), "v", vec![l], json!({ "padding": [12, 0, 0, 0] }))
}

fn error_box(p: &mut Page, id: &str, text: &str) -> String {
    let l = p.label(&format!("{id}.t"), text, json!({ "tone": "bad" }));
    p.boxed(id, "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] }))
}

fn bucket(c: &Ctx, p: &mut Page, beads: &[Bead], key_of: &dyn Fn(&Bead) -> Vec<String>, prefix: &str, out: &mut Vec<String>) {
    let mut m: Vec<(String, Vec<Bead>)> = vec![];
    for b in beads {
        for k in key_of(b) {
            match m.iter_mut().find(|(x, _)| *x == k) {
                Some((_, v)) => v.push(b.clone()),
                None => m.push((k, vec![b.clone()])),
            }
        }
    }
    m.sort_by(|x, y| y.1.len().cmp(&x.1.len()).then_with(|| x.0.cmp(&y.0)));
    for (k, mut bs) in m {
        bs.sort_by(by_work);
        let n = bs.len().to_string();
        out.push(c.section(p, &format!("{prefix}:{k}"), &k, &n, &bs, false, None, ""));
    }
}

fn page_components(snap: &Snapshot, view: &View) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let c = Ctx { snap };
    let (matcher, err) = match parse_query(&view.q) {
        Ok(m) => (m, String::new()),
        Err(e) => (Box::new(|_: &Bead| true) as Pred, e),
    };
    let everything: Vec<&Bead> = snap.beads.iter().filter(|b| b.kind != "milestone").collect();
    let shown: Vec<&Bead> = everything.iter().copied().filter(|b| matcher(b)).collect();
    let n = |f: &dyn Fn(&Bead) -> bool| shown.iter().filter(|b| b.kind != "epic" && f(b)).count();
    let repos = usize::from(snap.has_repo);
    let sub = format!(
        "{repos} repos · {} of {} beads{}",
        shown.len(),
        everything.len(),
        if !view.q.is_empty() && err.is_empty() { " match the query" } else { "" }
    );

    let mut page: Vec<String> = vec![];
    page.push(p.runs("title", json!([{ "text": "Beads", "size": 20, "weight": "bold" }]), json!({})));
    page.push(p.label("sub", &sub, json!({ "size": 12, "tone": "muted" })));
    p.put(json!({ "id": "search", "component": "SearchField", "value": view.q, "action": "set_query",
        "placeholder": "type=bug AND NOT status=closed", "error": !err.is_empty() }));
    page.push(p.boxed("search.b", "v", vec!["search".into()], json!({ "padding": [14, 0, 0, 0] })));
    if !err.is_empty() {
        let l = p.label("qerr", &err, json!({ "size": 12, "tone": "bad" }));
        page.push(p.boxed("qerr.b", "v", vec![l], json!({ "padding": [4, 2, 0, 2] })));
    }
    let l = p.label("qhelp", "fields: status priority type label title assignee owner · = != > >= < <= · AND OR NOT ( )", json!({ "size": 11, "tone": "muted" }));
    page.push(p.boxed("qhelp.b", "v", vec![l], json!({ "padding": [4, 2, 0, 2] })));

    p.put(json!({ "id": "tabs.seg", "component": "Segmented", "selected": view.tab, "action": "set_tab",
        "options": [
            { "value": "features", "text": "Features" }, { "value": "labels", "text": "Labels" },
            { "value": "types", "text": "Types" }, { "value": "status", "text": "Status" }] }));
    page.push(p.boxed("tabs", "h", vec!["tabs.seg".into()], json!({ "padding": [14, 0, 0, 0] })));

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
            let hot = *hot && *v > 0;
            let num = p.runs(&format!("kpi{i}.n"), json!([{ "text": v.to_string(), "size": 24, "weight": "bold", "tone": if hot { "ok" } else { "fg" } }]), json!({}));
            let lab = p.label(&format!("kpi{i}.l"), l, json!({ "size": 12, "tone": "muted" }));
            p.boxed(&format!("kpi{i}"), "v", vec![num, lab], json!({ "padding": [10, 14], "border": if hot { "ok" } else { "line" }, "fill": "card", "radius": 10 }))
        })
        .collect();
    page.push(p.boxed("kpis", "h", cards, json!({ "spacing": 10, "padding": [14, 0, 0, 0], "widths": ["flex", "flex", "flex", "flex"] })));

    for (i, pr) in snap.problems.iter().enumerate() {
        page.push(error_box(&mut p, &format!("problem{i}"), pr));
    }

    if snap.has_repo {
        let runs = json!([
            { "text": snap.name, "size": 15, "weight": "semibold" },
            { "text": format!("  {} · {} scripts", snap.path, snap.scripts.len()), "size": 12, "tone": "muted", "weight": "regular" }
        ]);
        let h = p.runs("repo.h", runs, json!({}));
        page.push(p.boxed("repo.hb", "v", vec![h], json!({ "padding": [28, 0, 8, 0] })));
        if let Some(e) = &snap.error {
            page.push(error_box(&mut p, "repo.err", e));
        }
        if snap.beads.is_empty() && snap.error.is_none() {
            page.push(empty(&mut p, "repo.none", "No beads in this repo."));
        } else {
            let bs: Vec<Bead> = snap.beads.iter().filter(|b| b.kind != "milestone" && matcher(b)).cloned().collect();
            match view.tab.as_str() {
                "features" => {
                    let (features, loose) = group(&snap.beads);
                    let keep: Vec<Feature> = features
                        .into_iter()
                        .map(|mut f| {
                            if !view.q.is_empty() && !f.single {
                                f.beads.push(f.root.clone());
                            }
                            f.beads.retain(|b| b.kind != "milestone" && matcher(b));
                            f
                        })
                        .filter(|f| !f.beads.is_empty())
                        .collect();
                    let loose: Vec<Bead> = loose.into_iter().filter(|b| matcher(b)).collect();
                    let (done, active): (Vec<&Feature>, Vec<&Feature>) = keep.iter().partition(|f| f.done);
                    let row = |p: &mut Page, f: &Feature, out: &mut Vec<String>| {
                        let r = &f.root;
                        let ready = f.beads.iter().filter(|b| b.ready && !b.epic).count();
                        let tag = if ready > 0 { format!("{} · {ready} ready", r.id) } else { r.id.clone() };
                        let acts = if f.single { None } else { c.buttons(p, &format!("sec.{}", r.id), r) };
                        out.push(c.section(p, &r.id, &r.title, &tag, &f.beads, f.done, acts, &summary(&r.description)));
                    };
                    page.push(h3(&mut p, "h3.active", &format!("In progress — {} features", active.len())));
                    if active.is_empty() {
                        page.push(empty(&mut p, "none.active", "None."));
                    }
                    for f in &active {
                        row(&mut p, f, &mut page);
                    }
                    if !loose.is_empty() {
                        page.push(h3(&mut p, "h3.loose", &format!("Loose — {} beads under no feature", loose.len())));
                        page.push(c.list(&mut p, "loose", &loose));
                    }
                    page.push(h3(&mut p, "h3.done", &format!("Complete — {} features", done.len())));
                    if done.is_empty() {
                        page.push(empty(&mut p, "none.done", "None."));
                    }
                    for f in &done {
                        row(&mut p, f, &mut page);
                    }
                }
                _ if bs.is_empty() => page.push(empty(&mut p, "none.match", "No bead matches.")),
                "labels" => bucket(&c, &mut p, &bs, &|b| if b.labels.is_empty() { vec!["(no label)".into()] } else { b.labels.clone() }, "label", &mut page),
                "types" => bucket(&c, &mut p, &bs, &|b| vec![b.kind.clone()], "type", &mut page),
                _ => {
                    for (k, label) in [("open", "Open"), ("in_progress", "In progress"), ("blocked", "Blocked"), ("closed", "Closed")] {
                        let mut group: Vec<Bead> = bs
                            .iter()
                            .filter(|b| {
                                let kk = kind_of(b);
                                (if kk == "ready" || kk == "deferred" { "open" } else { kk }) == k
                            })
                            .cloned()
                            .collect();
                        group.sort_by(by_work);
                        page.push(h3(&mut p, &format!("h3.{k}"), &format!("{label} — {}", group.len())));
                        if group.is_empty() {
                            page.push(empty(&mut p, &format!("none.{k}"), "None."));
                        } else {
                            page.push(c.list(&mut p, &format!("status.{k}"), &group));
                        }
                    }
                }
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

// ───────────────────────────── the server ─────────────────────────────

struct Shared {
    snap: Snapshot,
    tab: String,
    q: String,
}

fn ui_meta() -> MetaObject {
    let value = json!({ "ui": { "resourceUri": URI, "mimeType": MIME } });
    MetaObject(value.as_object().unwrap().clone())
}

fn answer(shared: &Shared) -> CallToolResult {
    let view = View { tab: shared.tab.clone(), q: shared.q.clone() };
    let messages = json!([{ "version": "v0.9", "updateComponents": {
        "surfaceId": SURFACE, "components": page_components(&shared.snap, &view) } }]);
    CallToolResult::success(vec![ContentBlock::resource(ResourceContents::TextResourceContents {
        uri: URI.into(),
        mime_type: Some(MIME.into()),
        text: messages.to_string(),
        meta: None,
    })])
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SetTab {
    /// features, labels, types or status.
    tab: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SetQuery {
    /// The query text.
    q: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RunScript {
    /// The bead's id.
    bead: String,
    /// The button's name in `[beads.buttons.<name>]`.
    script: String,
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
    shared: Arc<Mutex<Shared>>,
}

#[tool_router]
impl Server {
    fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self { tool_router: Self::tool_router(), shared }
    }

    #[tool(description = "The beads page", meta = ui_meta())]
    fn get_beads(&self) -> CallToolResult {
        answer(&self.shared.lock().unwrap())
    }

    #[tool(description = "Show a tab", meta = ui_meta())]
    fn set_tab(&self, Parameters(SetTab { tab }): Parameters<SetTab>) -> CallToolResult {
        let mut s = self.shared.lock().unwrap();
        if ["features", "labels", "types", "status"].contains(&tab.as_str()) {
            s.tab = tab;
        }
        answer(&s)
    }

    #[tool(description = "Filter the beads with a query", meta = ui_meta())]
    fn set_query(&self, Parameters(SetQuery { q }): Parameters<SetQuery>) -> CallToolResult {
        let mut s = self.shared.lock().unwrap();
        s.q = q;
        answer(&s)
    }

    #[tool(description = "Run a bead's button", meta = ui_meta())]
    fn run_script(&self, Parameters(RunScript { bead, script }): Parameters<RunScript>) -> CallToolResult {
        let s = self.shared.lock().unwrap();
        let Some(b) = s.snap.beads.iter().find(|b| b.id == bead) else {
            return CallToolResult::error(vec![ContentBlock::text(format!("no bead {bead}"))]);
        };
        let Some(sc) = s.snap.scripts.iter().find(|x| x.name == script) else {
            return CallToolResult::error(vec![ContentBlock::text(format!("no button {script}"))]);
        };
        let log = std::env::temp_dir().join(format!("wheel-beads-{}-{}.log", sane(&b.id), sane(&sc.name)));
        let file = match std::fs::File::create(&log) {
            Ok(f) => f,
            Err(e) => return CallToolResult::error(vec![ContentBlock::text(format!("{}: {e}", log.display()))]),
        };
        let spawned = Command::new("/bin/sh")
            .arg("-c")
            .arg(&sc.run)
            .current_dir(&s.snap.path)
            .env("BEAD_ID", &b.id)
            .env("BEAD_TITLE", &b.title)
            .env("BEAD_TYPE", &b.kind)
            .env("BEAD_REPO", &s.snap.name)
            .env("BEAD_PATH", &s.snap.path)
            .stdin(std::process::Stdio::null())
            .stdout(file.try_clone().unwrap())
            .stderr(file)
            .spawn();
        match spawned {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                answer(&s)
            }
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{script}: {e}"))]),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri != URI {
            return Err(ErrorData::resource_not_found(format!("no resource {}", request.uri), None));
        }
        Ok(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
            uri: request.uri,
            mime_type: Some(MIME.into()),
            text: skeleton().to_string(),
            meta: None,
        }])
        .into())
    }
}

fn touched(path: &str) -> Option<SystemTime> {
    std::fs::metadata(PathBuf::from(path).join(".beads/last-touched")).and_then(|m| m.modified()).ok()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() != Some("stdio") {
        anyhow::bail!("usage: mcp_a2ui_beads stdio");
    }
    let shared = Arc::new(Mutex::new(Shared { snap: collect(), tab: "features".into(), q: String::new() }));
    let running = Server::new(shared.clone()).serve(rmcp::transport::stdio()).await?;
    let peer = running.peer().clone();
    let path = shared.lock().unwrap().snap.path.clone();
    tokio::spawn(async move {
        let mut seen = touched(&path);
        let mut since = Duration::ZERO;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            since += Duration::from_secs(1);
            let now = touched(&path);
            if now == seen && since < Duration::from_secs(60) {
                continue;
            }
            seen = now;
            since = Duration::ZERO;
            let fresh = tokio::task::spawn_blocking(collect).await.unwrap_or_default();
            let changed = {
                let mut s = shared.lock().unwrap();
                let changed = s.snap != fresh;
                s.snap = fresh;
                changed
            };
            if changed && peer.notify_resource_updated(rmcp::model::ResourceUpdatedNotificationParam::new(URI)).await.is_err() {
                break;
            }
        }
    });
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bead(id: &str, f: impl Fn(&mut Bead)) -> Bead {
        let mut b = Bead {
            id: id.into(), title: id.into(), status: "open".into(), kind: "task".into(), priority: Some(2),
            parent: None, labels: vec![], owner: None, blocked_by: vec![], description: String::new(),
            ready: false, epic: false,
        };
        f(&mut b);
        b
    }

    fn ids(q: &str) -> Vec<String> {
        let fixture = vec![
            bead("b1", |b| { b.kind = "bug".into(); b.labels = vec!["mac".into()]; b.priority = Some(1); b.ready = true }),
            bead("b2", |b| { b.kind = "bug".into(); b.status = "closed".into(); b.labels = vec!["mac".into()]; b.priority = Some(1) }),
            bead("b3", |b| { b.kind = "bug".into(); b.status = "in_progress".into(); b.labels = vec!["ios".into()]; b.priority = Some(0); b.owner = Some("pierce".into()) }),
            bead("b4", |b| { b.labels = vec!["ios".into()]; b.priority = Some(3); b.blocked_by = vec!["b1".into()] }),
            bead("b5", |b| { b.kind = "feature".into(); b.labels = vec!["daemon".into()]; b.title = "Query box".into() }),
            bead("b6", |b| { b.kind = "bug".into(); b.labels = vec!["cli".into()]; b.priority = Some(4) }),
        ];
        let m = parse_query(q).unwrap();
        fixture.iter().filter(|b| m(b)).map(|b| b.id.clone()).collect()
    }

    // The cases of artifacts/beads/query.test.js.
    #[test]
    fn queries_match_the_page() {
        assert_eq!(ids("type=bug AND NOT status=closed"), ["b1", "b3", "b6"]);
        assert_eq!(ids("label=mac OR label=ios"), ["b1", "b2", "b3", "b4"]);
        assert_eq!(ids("label=mac OR label=ios AND type=bug"), ["b1", "b2", "b3"]);
        assert_eq!(ids("(label=mac OR label=ios) AND type=bug"), ["b1", "b2", "b3"]);
        assert_eq!(ids(""), ["b1", "b2", "b3", "b4", "b5", "b6"]);
        assert_eq!(ids("priority<=1"), ["b1", "b2", "b3"]);
        assert_eq!(ids("priority>P2"), ["b4", "b6"]);
        assert_eq!(ids("priority!=1"), ["b3", "b4", "b5", "b6"]);
        assert_eq!(ids("status=blocked"), ["b4"]);
        assert_eq!(ids("status!=open"), ["b2", "b3"]);
        assert_eq!(ids("title=\"query box\""), ["b5"]);
        assert_eq!(ids("assignee=pierce"), ["b3"]);
        assert_eq!(ids("not type=bug"), ["b4", "b5"]);
        assert_eq!(ids("TYPE=Bug and label=CLI"), ["b6"]);
    }

    #[test]
    fn bad_queries_do_not_parse() {
        for bad in ["type=", "colour=red", "type=bug AND", "(type=bug", "type=bug)", "title>x", "priority=high", "type bug", "AND type=bug", "title=\"x"] {
            assert!(parse_query(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ids_sort_by_number() {
        assert_eq!(natural("wheel-9", "wheel-10"), std::cmp::Ordering::Less);
        assert_eq!(natural("wheel-h3z5.2", "wheel-h3z5.10"), std::cmp::Ordering::Less);
    }

    #[test]
    fn a_feature_groups_its_descendants() {
        let beads = vec![
            bead("e", |b| b.kind = "epic".into()),
            bead("e.1", |b| b.parent = Some("e".into())),
            bead("e.1.1", |b| { b.parent = Some("e.1".into()); b.status = "closed".into() }),
            bead("loose", |_| {}),
        ];
        let (f, loose) = group(&beads);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].beads.len(), 2);
        assert_eq!(loose.len(), 1);
    }
}
