//! The Diff view: the project's working-tree diff, by scope, one file's hunks at a time.
//!
//! Each subscribed project keeps one snapshot. It is re-read when HEAD, the index or a file the
//! diff names changes (the host re-reads the watched files after each refresh, so a newly changed
//! file is watched from then on) or, for a file git has not seen yet, within 30 seconds.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, MetaObject, RequestMetaObject, ResourceContents, Tool};
use rmcp::{schemars, service::RequestContext, tool, tool_router, ErrorData, RoleServer};
use serde_json::{json, Value};

use super::*;
use crate::core::page::{self, cell, Page};
use crate::core::project::Origin;
use crate::core::table;
use crate::view::{View, MIME};

pub const NAME: &str = "diff";
const URI: &str = "a2ui://diff";
const SURFACE: &str = "diff";
/// The most hunk lines drawn for one file.
const DRAWN: usize = 600;

#[derive(Default)]
struct Ui {
    scope: String,
    sel: Option<String>,
}

#[derive(Default)]
struct Shared {
    projects: HashMap<String, Snapshot>,
    panes: HashMap<String, Ui>,
}

impl Shared {
    fn project(&mut self, dir: &str) -> &mut Snapshot {
        self.projects.entry(dir.to_string()).or_insert_with(|| collect(dir))
    }
}

fn ui_meta() -> MetaObject {
    MetaObject(json!({ "ui": { "resourceUri": URI, "mimeType": MIME } }).as_object().unwrap().clone())
}

/// The scope the pane shows: its own pick when the project still has it, else the first.
fn scope_of<'a>(s: &'a Snapshot, ui: &Ui) -> Option<&'a Scope> {
    s.scopes.iter().find(|c| c.key == ui.scope).or_else(|| s.scopes.first())
}

fn file_of<'a>(scope: &'a Scope, ui: &Ui) -> Option<&'a File> {
    scope.files.iter().find(|f| Some(&f.path) == ui.sel.as_ref()).or_else(|| scope.files.first())
}

fn status_tone(c: char) -> &'static str {
    match c {
        'A' | '?' => "ok",
        'D' => "bad",
        _ => "warn",
    }
}

/// The page's padding at its left and right.
const SIDE: f64 = 16.0;
/// The page's width at which every file is shown stacked beside the file list: a 640 pt pane less the page's sides.
/// Narrower, one file is open under a fixed file table.
const WIDE: f64 = 640.0 - 2.0 * SIDE;
/// The most diff lines the stacked layout draws across the files that are not selected; later ones show their header and a
/// note. The selected file is always drawn, up to `DRAWN` lines.
const STACKED: usize = 2000;
/// The fixed-width slot the totals sit in at the right of the top row, so a longer or shorter total moves nothing.
const TOTALS: f64 = 210.0;
/// Rows the narrow layout's file table shows before it scrolls (its header row included in the height).
const TABLE_ROWS: f64 = 7.0;

/// GitHub's five squares for a file: green for its share of added lines, red for removed, grey when it has none.
fn squares(f: &File) -> Value {
    let total = f.add + f.del;
    let green = if total == 0 { 0 } else { (f.add * 5 + total / 2) / total };
    let red = if total == 0 { 0 } else { 5 - green };
    let mut runs = vec![];
    for (n, tone) in [(green, "ok"), (red, "bad"), (5 - green - red, "todo")] {
        if n > 0 {
            runs.push(json!({ "text": "■".repeat(n), "tone": tone }));
        }
    }
    json!(runs)
}

/// Gerrit's size bar: up to ten blocks, the scope's largest file filling it.
fn size_bar(f: &File, largest: usize) -> String {
    let total = f.add + f.del;
    if total == 0 || largest == 0 {
        return String::new();
    }
    "▇".repeat(((total * 10).div_ceil(largest)).clamp(1, 10))
}

/// A file's name first, then the folder it is in, so the name survives a narrow column.
fn short_name(f: &File) -> String {
    match f.path.rsplit_once('/') {
        Some((dir, name)) => format!("{name}  {dir}/"),
        None => f.path.clone(),
    }
}

/// A label whose style's `padding` becomes a padding modifier (a label's own style has none).
fn padded(p: &mut Page, id: &str, text: &str, style: Value) -> String {
    let pad = style.get("padding").cloned();
    let l = p.label(id, text, style);
    if let Some(pad) = pad {
        crate::core::layout::modify(&mut p.out, &l, json!({ "padding": pad }));
    }
    l
}

/// One drawn line of a hunk: both line numbers, the kind and the text, coloured by kind.
fn diff_line(p: &mut Page, id: &str, l: &Line) -> String {
    let n = |v: Option<u32>| v.map(|v| format!("{v:>4}")).unwrap_or_else(|| "    ".into());
    let tone = match l.kind { '+' => "ok", '-' => "bad", _ => "fg" };
    p.label(id, &format!("{} {} {} {}", n(l.old), n(l.new), l.kind, l.text), json!({ "mono": true, "size": 11, "tone": tone }))
}

/// A file's hunks as lines under `prefix`, at most `budget` diff lines; a note replaces what is left out.
fn hunk_lines(p: &mut Page, prefix: &str, f: &File, budget: usize) -> (Vec<String>, usize) {
    let mut out = vec![];
    if f.binary {
        out.push(padded(p, &format!("{prefix}.n"), "Binary file.", json!({ "tone": "muted", "padding": [6, 0, 6, 0] })));
        return (out, 0);
    }
    if f.truncated {
        out.push(padded(p, &format!("{prefix}.n"), "Too large to show here; the counts still hold.", json!({ "tone": "muted", "padding": [6, 0, 6, 0] })));
        return (out, 0);
    }
    let mut drawn = 0;
    'hunks: for (hi, h) in f.hunks.iter().enumerate() {
        out.push(padded(p, &format!("{prefix}.h{hi}"), &format!("@@ {}", h.header), json!({ "mono": true, "size": 11, "tone": "muted", "padding": [6, 0, 2, 0] })));
        for (li, l) in h.lines.iter().enumerate() {
            if drawn == budget.min(DRAWN) {
                out.push(padded(p, &format!("{prefix}.more"), &format!("{drawn} lines shown; the rest is in `wheelhouse diff --json`."), json!({ "tone": "muted", "padding": [6, 0, 6, 0] })));
                break 'hunks;
            }
            drawn += 1;
            out.push(diff_line(p, &format!("{prefix}.{hi}.{li}"), l));
        }
    }
    (out, drawn)
}

/// The words a scope with no files shows.
fn clean(scope: &Scope) -> &'static str {
    match scope.key.as_str() { "uncommitted" => "No uncommitted changes.", _ => "No changes in this scope." }
}

/// The top row: the scope switcher at the left and the totals in a fixed slot at the right. A `ViewThatFits` of the same row
/// with a segmented control and with a menu: the totals' slot is fixed, so which one fits depends on the pane's width alone.
fn top_row(p: &mut Page, s: &Snapshot, scope: &Scope) -> String {
    let options = json!(s.scopes.iter().map(|c| json!({ "value": c.key, "text": c.label })).collect::<Vec<_>>());
    p.bind("/set_scope", json!([scope.key]));
    let totals = |p: &mut Page, id: &str| {
        let n = scope.files.len();
        let count = p.label(&format!("{id}.c"), &format!("{n} {}  +{} −{}", if n == 1 { "file" } else { "files" }, scope.add(), scope.del()),
            json!({ "size": 12, "weight": "medium", "tabular": true, "align": "end" }));
        let since = match (scope.commits, &scope.fork) {
            (Some(c), Some(f)) => format!("{c} {} since {f}", if c == 1 { "commit" } else { "commits" }),
            _ => format!("working tree vs {}", scope.base),
        };
        let base = p.label(&format!("{id}.b"), &since, json!({ "tone": "muted", "size": 11, "tabular": true, "align": "end" }));
        for l in [&count, &base] {
            crate::core::layout::modify(&mut p.out, l, json!({ "lineLimit": 1 }));
        }
        p.boxed(id, "v", vec![count, base], json!({ "spacing": 1, "align": "end" }));
        crate::core::layout::modify(&mut p.out, id, json!({ "frame": { "width": TOTALS, "alignment": "trailing" } }));
        id.to_string()
    };
    let seg = p.pick("scopes.seg", "/set_scope", &options, Some("chips"), false, json!([{ "fixedSize": { "horizontal": true } }]));
    let t1 = totals(p, "totals.seg");
    let r1 = p.boxed("top.seg", "h", vec![seg, t1], json!({ "spacing": 12, "align": "center", "trailing": 1 }));
    let menu = p.pick("scopes.menu", "/set_scope", &options, Some("menu"), false, json!([{ "fixedSize": true }]));
    let t2 = totals(p, "totals.menu");
    let r2 = p.boxed("top.menu", "h", vec![menu, t2], json!({ "spacing": 12, "align": "center", "trailing": 1 }));
    p.put(json!({ "id": "top", "component": "ViewThatFits", "in": "horizontal", "children": [r1, r2] }));
    "top".into()
}

/// The file table's rows, each file's name first: `wide` is the stacked layout's list of names; otherwise the narrow layout's
/// table also has a size bar and the counts.
fn file_rows(scope: &Scope, sel: Option<&File>, wide: bool) -> Vec<Value> {
    let largest = scope.files.iter().map(|f| f.add + f.del).max().unwrap_or(0);
    scope.files.iter().map(|f| {
        let on = sel.is_some_and(|x| x.path == f.path);
        let name = match &f.old { Some(o) => format!("{o} → {}", f.path), None => short_name(f) };
        let mut cells = vec![json!({ "key": "status", "text": f.status.to_string(), "chip": true, "tone": status_tone(f.status) }),
            cell(name, json!({ "key": "path", "mono": true, "tone": if on { "accent" } else { "fg" } }))];
        if !wide {
            cells.push(cell(size_bar(f, largest), json!({ "key": "size", "mono": true, "tone": "muted" })));
            cells.push(cell(format!("+{}", f.add), json!({ "key": "add", "mono": true, "tone": "ok" })));
            cells.push(cell(format!("−{}", f.del), json!({ "key": "del", "mono": true, "tone": "bad" })));
        }
        json!({ "id": f.path, "path": f.path, "depth": 0, "kids": 0, "collapsed": false, "dim": false, "sel": on, "cells": cells, "menu": [] })
    }).collect()
}

/// At least 640 pt across (GitHub's layout): the file list at the left with its own scroll, and every file stacked at the right
/// under a header that stays at the top while its file is read.
fn wide_layout(p: &mut Page, scope: &Scope, sel: Option<&File>) -> String {
    let columns = vec![table::column("status", "", json!(28), false, false), table::column("path", "File", Value::Null, false, false)];
    let mut t = table::component(columns, vec![table::section("", file_rows(scope, sel, true))], "select_file", "select_file", ("path", true), "No files.");
    t["id"] = json!("files.w");
    p.put(t);
    let side = p.boxed("side.w", "v", vec!["files.w".into()], json!({ "spacing": 0 }));
    crate::core::layout::modify(&mut p.out, &side, json!({ "frame": { "minWidth": 140, "idealWidth": 170, "maxWidth": 200, "maxHeight": "infinity" } }));
    let mut sections = vec![];
    let mut budget = STACKED;
    for (fi, f) in scope.files.iter().enumerate() {
        let path = p.label(&format!("w{fi}.p"), &match &f.old { Some(o) => format!("{o} → {}", f.path), None => f.path.clone() }, json!({ "mono": true, "weight": "semibold", "size": 12 }));
        crate::core::layout::modify(&mut p.out, &path, json!({ "lineLimit": 1 }));
        crate::core::layout::modify(&mut p.out, &path, json!({ "truncationMode": "head" }));
        let count = p.label(&format!("w{fi}.c"), &format!("+{} −{}", f.add, f.del), json!({ "mono": true, "size": 11, "tone": "muted", "tabular": true }));
        crate::core::layout::modify(&mut p.out, &count, json!({ "fixedSize": true }));
        let bar = p.runs(&format!("w{fi}.bar"), squares(f), json!({ "size": 9 }));
        crate::core::layout::modify(&mut p.out, &bar, json!({ "fixedSize": true }));
        let header = p.boxed(&format!("w{fi}.hd"), "h", vec![path, count, bar], json!({ "spacing": 8, "align": "center", "widths": ["flex", null, null], "padding": [6, 8], "fill": "card", "radius": 0 }));
        let selected = sel.is_some_and(|x| x.path == f.path);
        let (lines, drawn) = if budget == 0 && !selected {
            (vec![padded(p, &format!("w{fi}.n"), &format!("Not drawn: this view draws {STACKED} lines across the files; select this one to draw it."), json!({ "tone": "muted", "padding": [6, 0, 6, 0] }))], 0)
        } else {
            hunk_lines(p, &format!("w{fi}"), f, if selected { DRAWN } else { budget })
        };
        if !selected {
            budget -= drawn;
        }
        for l in &lines {
            crate::core::layout::modify(&mut p.out, l, json!({ "padding": { "horizontal": 8 } }));
        }
        // The space under a file's last line, before the next file's header.
        if let Some(last) = lines.last() {
            crate::core::layout::modify(&mut p.out, last, json!({ "padding": { "bottom": 14 } }));
        }
        let id = format!("w{fi}");
        p.put(json!({ "id": id, "component": "Section", "header": header, "children": lines }));
        sections.push(id);
    }
    if sections.is_empty() {
        sections.push(padded(p, "w.clean", clean(scope), json!({ "tone": "muted", "padding": [24, 16, 0, 16] })));
    }
    let target = sel.and_then(|x| scope.files.iter().position(|f| f.path == x.path)).map(|i| format!("w{i}"));
    let mut stream = json!({ "id": "stream.w", "component": "List", "spacing": 0, "pinnedViews": ["sectionHeaders"], "children": sections,
        "modifiers": [{ "frame": { "minWidth": 280, "maxWidth": "infinity", "maxHeight": "infinity" } }] });
    if let Some(t) = target {
        stream["scrollTo"] = json!(t);
    }
    p.put(stream);
    p.put(json!({ "id": "split.w", "component": "HSplitView", "children": [side, "stream.w"], "modifiers": [{ "frame": { "maxHeight": "infinity" } }] }));
    p.boxed("wide", "v", vec!["split.w".into()], json!({ "spacing": 0 }));
    crate::core::layout::modify(&mut p.out, "wide", json!({ "frame": { "minWidth": WIDE, "idealWidth": WIDE, "maxWidth": "infinity", "maxHeight": "infinity" } }));
    "wide".into()
}

/// Under 640 pt across (Gerrit's layout): a file table with a size bar fixed at the top, scrolling inside itself, and the one
/// open file under it with previous, next and its place in the list.
fn narrow_layout(p: &mut Page, scope: &Scope, sel: Option<&File>) -> String {
    let columns = vec![table::column("status", "", json!(28), false, false), table::column("path", "File", Value::Null, false, false),
        table::column("size", "Size", json!(78), false, false), table::column("add", "+", json!(44), false, false), table::column("del", "−", json!(44), false, false)];
    let mut t = table::component(columns, vec![table::section("", file_rows(scope, sel, false))], "select_file", "select_file", ("path", true), clean(scope));
    t["id"] = json!("files.n");
    // Seven rows of 24 pt under a 28 pt header.
    t["modifiers"] = json!([{ "frame": { "height": 28.0 + 24.0 * TABLE_ROWS } }]);
    p.put(t);
    let at = sel.and_then(|x| scope.files.iter().position(|f| f.path == x.path));
    let total = scope.files.len();
    let step = |p: &mut Page, id: &str, icon: &str, help: &str, to: Option<&File>| {
        let ctx = json!({ "id": to.map(|f| f.path.clone()).unwrap_or_default() });
        p.button(id, "", "select_file", ctx, json!({ "icon": icon, "style": "plain", "disabled": to.is_none(), "help": help }))
    };
    let prev = step(p, "prev.n", "chevron.left", "Previous file", at.and_then(|i| i.checked_sub(1)).and_then(|i| scope.files.get(i)));
    let next = step(p, "next.n", "chevron.right", "Next file", at.and_then(|i| scope.files.get(i + 1)));
    let place = p.label("place.n", &format!("{} / {total}", at.map_or(0, |i| i + 1)), json!({ "size": 11, "tone": "muted", "tabular": true }));
    crate::core::layout::modify(&mut p.out, &place, json!({ "frame": { "minWidth": 48 } }));
    let name = p.label("name.n", &sel.map(|f| f.path.clone()).unwrap_or_default(), json!({ "mono": true, "weight": "semibold", "size": 12 }));
    crate::core::layout::modify(&mut p.out, &name, json!({ "lineLimit": 1 }));
    crate::core::layout::modify(&mut p.out, &name, json!({ "truncationMode": "head" }));
    p.boxed("nav.n", "h", vec![prev, place, next, name], json!({ "spacing": 6, "align": "center", "widths": [null, null, null, "flex"], "padding": [4, 0, 4, 0] }));
    let lines = match sel {
        Some(f) => hunk_lines(p, &format!("n{}", at.unwrap_or(0)), f, DRAWN).0,
        None => vec![],
    };
    let mut list = json!({ "id": "lines.n", "component": "List", "spacing": 0, "children": lines, "modifiers": [{ "frame": { "maxHeight": "infinity" } }] });
    // A file's first line, so a newly opened file starts at its top (a binary file's first line is its note).
    if let Some(first) = list["children"].get(0).cloned() {
        list["scrollTo"] = first;
    }
    p.put(list);
    p.boxed("narrow", "v", vec!["files.n".into(), "nav.n".into(), "lines.n".into()], json!({ "spacing": 4 }));
    crate::core::layout::modify(&mut p.out, "narrow", json!({ "frame": { "maxHeight": "infinity" } }));
    "narrow".into()
}

fn components(s: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let mut page = vec![];
    // No page title or subtitle: the pane's own bar says which page it is.
    if let Some(e) = &s.error {
        let l = p.label("problem.t", e, json!({ "tone": "bad" }));
        page.push(p.boxed("problem", "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if let Some(scope) = scope_of(s, ui) {
        page.push(top_row(&mut p, s, scope));
        let sel = file_of(scope, ui);
        let wide = wide_layout(&mut p, scope, sel);
        let narrow = narrow_layout(&mut p, scope, sel);
        p.put(json!({ "id": "layout", "component": "ViewThatFits", "in": "horizontal", "children": [wide, narrow] }));
        page.push("layout".into());
    }
    // Only the file list and the diff text scroll: the file table without a frame makes the page fill the pane.
    p.boxed("page", "v", page, json!({ "align": "start", "spacing": 10, "padding": [12.0, SIDE, 10.0, SIDE] }));
    p.out
}

fn answer(s: &Snapshot, ui: &Ui) -> CallToolResult {
    let messages = page::update(SURFACE, components(s, ui));
    CallToolResult::success(vec![ContentBlock::resource(ResourceContents::TextResourceContents {
        uri: URI.into(), mime_type: Some(MIME.into()), text: messages.to_string(), meta: None })])
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Value1 {
    /// The new value.
    value: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct FileId {
    /// A file's path: a file table row's id, or the context of a previous or next button.
    id: String,
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
    shared: Arc<Mutex<Shared>>,
}

impl Server {
    fn with(&self, meta: &RequestMetaObject, f: impl FnOnce(&Snapshot, &mut Ui)) -> CallToolResult {
        let o = Origin::of(meta);
        let mut s = self.shared.lock().unwrap();
        let snap = s.project(&o.dir).clone();
        let ui = s.panes.entry(o.pane).or_default();
        f(&snap, ui);
        answer(&snap, ui)
    }
}

#[tool_router]
impl Server {
    fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self { tool_router: Self::tool_router(), shared }
    }

    #[tool(description = "The diff page", meta = ui_meta())]
    fn get_diff(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, _| {})
    }

    #[tool(description = "Show a scope: uncommitted, default, base0 or base1", meta = ui_meta())]
    fn set_scope(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |s, ui| {
            if s.scopes.iter().any(|c| c.key == value) {
                ui.scope = value;
                ui.sel = None;
            }
        })
    }

    #[tool(description = "Show a file's hunks", meta = ui_meta())]
    fn select_file(&self, meta: RequestMetaObject, Parameters(FileId { id }): Parameters<FileId>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.sel = Some(id))
    }
}

/// The Diff view.
pub struct DiffView {
    server: Server,
}

impl DiffView {
    pub fn new() -> Self {
        DiffView { server: Server::new(Arc::new(Mutex::new(Shared::default()))) }
    }
}

#[async_trait]
impl View for DiffView {
    fn name(&self) -> &str {
        NAME
    }

    fn tools(&self) -> Vec<Tool> {
        self.server.tool_router.list_all()
    }

    async fn call(&self, params: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        self.server.tool_router.call(ToolCallContext::new(&self.server, params, context)).await
    }

    fn skeleton(&self) -> Value {
        page::skeleton(SURFACE)
    }

    fn watches(&self, dir: &str) -> Vec<PathBuf> {
        let Some((own, common)) = crate::core::git::dirs(dir) else { return vec![] };
        let (own, common) = (PathBuf::from(own), PathBuf::from(common));
        let mut files = vec![own.join("logs/HEAD"), own.join("HEAD"), own.join("index"), common.join("refs/heads")];
        files.extend(git::quiet(dir, &["diff", "--name-only", "HEAD", "--"]).lines().map(|f| PathBuf::from(dir).join(f)));
        files
    }

    fn every(&self) -> u64 {
        30
    }

    fn open(&self, dir: &str) {
        self.server.shared.lock().unwrap().project(dir);
    }

    fn close(&self, dir: &str) {
        self.server.shared.lock().unwrap().projects.remove(dir);
    }

    fn refresh(&self, dir: &str) -> bool {
        if !self.server.shared.lock().unwrap().projects.contains_key(dir) {
            return false;
        }
        let fresh = collect(dir);
        let mut s = self.server.shared.lock().unwrap();
        match s.projects.get_mut(dir) {
            Some(p) if *p != fresh => {
                *p = fresh;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> File {
        File { path: path.into(), old: None, status: 'M', add: 1, del: 1, binary: false, truncated: false,
            hunks: vec![Hunk { header: "f".into(), lines: vec![Line { kind: '+', old: None, new: Some(1), text: "x".into() }] }] }
    }

    fn snap(files: Vec<File>) -> Snapshot {
        Snapshot { name: "t".into(), scopes: vec![Scope { key: "uncommitted".into(), label: "Uncommitted".into(), base: "HEAD".into(),
            fork: None, commits: None, files }], ..Default::default() }
    }

    fn get<'a>(c: &'a [Value], id: &str) -> &'a Value {
        c.iter().find(|v| v["id"] == id).unwrap_or_else(|| panic!("no component {id}"))
    }

    #[test]
    fn the_page_sends_both_layouts_in_one_view_that_fits_wide_first() {
        let c = components(&snap(vec![file("a.rs"), file("b.rs")]), &Ui { scope: String::new(), sel: Some("b.rs".into()) });
        let layout = get(&c, "layout");
        assert_eq!(layout["component"], "ViewThatFits");
        assert_eq!(layout["children"], json!(["wide", "narrow"]));
        // The wide layout fits only from 640 pt: its ideal width is the page's width at that pane width.
        assert!(get(&c, "wide")["modifiers"].as_array().unwrap().iter().any(|m| m["frame"]["idealWidth"] == 608.0 && m["frame"]["minWidth"] == 608.0));
        assert_eq!(get(&c, "page")["modifiers"][0]["padding"], json!([12.0, 16.0, 10.0, 16.0]), "608 is a 640 pt pane less these sides");
    }

    #[test]
    fn the_wide_layout_stacks_every_file_under_a_pinned_header_and_scrolls_to_the_selected_one() {
        let c = components(&snap(vec![file("a.rs"), file("b.rs")]), &Ui { scope: String::new(), sel: Some("b.rs".into()) });
        let stream = get(&c, "stream.w");
        assert_eq!(stream["children"], json!(["w0", "w1"]));
        assert_eq!(stream["pinnedViews"], json!(["sectionHeaders"]));
        assert_eq!(stream["scrollTo"], "w1");
        assert_eq!(get(&c, "w1")["header"], "w1.hd");
        assert_eq!(get(&c, "w1.p")["text"], "b.rs");
        assert!(get(&c, "w0.0.0")["modifiers"].as_array().unwrap().iter().any(|x| x["foregroundStyle"] == "green"));
        assert_eq!(get(&c, "files.w")["sections"][0]["rows"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn the_narrow_layout_opens_one_file_with_its_place_and_neighbours() {
        let c = components(&snap(vec![file("a.rs"), file("b.rs")]), &Ui { scope: String::new(), sel: Some("b.rs".into()) });
        assert_eq!(get(&c, "place.n")["text"], "2 / 2");
        assert_eq!(get(&c, "name.n")["text"], "b.rs");
        assert_eq!(get(&c, "lines.n")["children"][0], "n1.h0");
        assert_eq!(get(&c, "prev.n")["action"]["event"]["context"]["id"], "a.rs");
        // The last file's next button stays in its place, disabled.
        assert!(get(&c, "next.n")["modifiers"].as_array().unwrap().iter().any(|m| m["disabled"] == true));
        assert!(get(&c, "files.n")["modifiers"][0]["frame"]["height"].is_number());
    }

    #[test]
    fn the_top_row_keeps_its_totals_in_a_fixed_slot_and_picks_the_switcher_by_fit() {
        let c = components(&snap(vec![file("a.rs")]), &Ui::default());
        assert_eq!(get(&c, "top")["component"], "ViewThatFits");
        assert_eq!(get(&c, "scopes.seg")["displayStyle"], "chips");
        assert_eq!(get(&c, "scopes.menu")["displayStyle"], "menu");
        for t in ["totals.seg", "totals.menu"] {
            assert!(get(&c, t)["modifiers"].as_array().unwrap().iter().any(|m| m["frame"]["width"] == TOTALS));
        }
        assert_eq!(get(&c, "totals.seg.c")["text"], "1 file  +1 −1");
        assert_eq!(get(&c, "totals.seg.b")["text"], "working tree vs HEAD");
    }

    #[test]
    fn a_clean_scope_says_so_in_both_layouts() {
        let c = components(&snap(vec![]), &Ui::default());
        assert_eq!(get(&c, "w.clean")["text"], "No uncommitted changes.");
        assert_eq!(get(&c, "files.n")["empty"], "No uncommitted changes.");
        assert_eq!(get(&c, "place.n")["text"], "0 / 0");
    }

    #[test]
    fn the_selected_file_is_drawn_after_the_stacked_budget_runs_out() {
        // Each big file draws DRAWN lines, so four of them use up STACKED.
        let big = |p: &str| File { hunks: vec![Hunk { header: "f".into(), lines: (0..DRAWN as u32).map(|n| Line { kind: '+', old: None, new: Some(n), text: "x".into() }).collect() }], ..file(p) };
        let files = vec![big("a0"), big("a1"), big("a2"), big("a3"), file("b.rs"), file("c.rs")];
        let c = components(&snap(files), &Ui { scope: String::new(), sel: Some("b.rs".into()) });
        assert!(c.iter().any(|v| v["id"] == "w4.0.0"), "the selected file draws its lines");
        assert!(c.iter().any(|v| v["id"] == "w5.n"), "a later file that is not selected gets the note");
    }
}
