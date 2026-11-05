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

pub const NAME: &str = "diff-native";
const URI: &str = "a2ui://diff-native";
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

fn components(s: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let mut page = vec![];
    let t = p.runs("title", json!([{ "text": format!("Diff · {}", s.name), "size": 20, "weight": "bold" }]), json!({}));
    let sub = p.label("sub", &format!("{} at {}", s.branch.as_deref().unwrap_or("a detached HEAD"), s.head), json!({ "size": 12, "tone": "muted" }));
    page.push(p.boxed("head", "h", vec![t, sub], json!({ "wrap": true, "justify": "between", "spacing": 12, "rowSpacing": 4, "padding": [0, 0, 12, 0] })));
    if let Some(e) = &s.error {
        let l = p.label("problem.t", e, json!({ "tone": "bad" }));
        page.push(p.boxed("problem", "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if let Some(scope) = scope_of(s, ui) {
        let options: Vec<Value> = s.scopes.iter().map(|c| json!({ "value": c.key, "text": c.label })).collect();
        p.put(json!({ "id": "scopes", "component": "Segmented", "selected": scope.key, "action": "set_scope", "options": options }));
        let summary = format!("{} files · +{} −{}{}{}", scope.files.len(), scope.add(), scope.del(),
            scope.commits.map(|n| format!(" · {n} commits")).unwrap_or_default(),
            scope.fork.as_ref().map(|f| format!(" since {f}")).unwrap_or_default());
        let count = p.label("count", &summary, json!({ "tone": "muted", "size": 11, "tabular": true, "align": "end" }));
        page.push(p.boxed("tools", "h", vec!["scopes".into(), count], json!({ "wrap": true, "spacing": 8, "rowSpacing": 6, "padding": [12, 0, 0, 0], "widths": [null, "flex"], "trailing": 1 })));
        let sel = file_of(scope, ui);
        let rows: Vec<Value> = scope.files.iter().map(|f| {
            let name = match &f.old { Some(o) => format!("{o} → {}", f.path), None => f.path.clone() };
            json!({ "id": f.path, "path": f.path, "depth": 0, "kids": 0, "collapsed": false, "dim": false,
                "sel": sel.map_or(false, |x| x.path == f.path),
                "cells": [
                    { "key": "status", "text": f.status.to_string(), "chip": true, "tone": status_tone(f.status) },
                    cell(name, json!({ "key": "path", "mono": true })),
                    cell(format!("+{}", f.add), json!({ "key": "add", "mono": true, "tone": "ok" })),
                    cell(format!("−{}", f.del), json!({ "key": "del", "mono": true, "tone": "bad" })),
                ], "menu": [] })
        }).collect();
        let columns = vec![table::column("status", "", json!(36), false, false), table::column("path", "File", json!("flex"), false, true),
            table::column("add", "+", json!(60), false, false), table::column("del", "−", json!(60), false, false)];
        let empty = match scope.key.as_str() { "uncommitted" => "No uncommitted changes.", _ => "No changes in this scope." };
        let mut t = table::component(columns, vec![table::section("", rows)], "select_file", "select_file", ("path", true), empty);
        t["id"] = json!("table");
        p.put(t);
        page.push(p.boxed("table.b", "v", vec!["table".into()], json!({})));
        if let Some(f) = sel {
            let mut lines = vec![p.label("file.h", &f.path, json!({ "weight": "bold", "mono": true, "padding": [14, 0, 4, 0] }))];
            if f.binary {
                lines.push(p.label("file.n", "Binary file.", json!({ "tone": "muted" })));
            } else if f.truncated {
                lines.push(p.label("file.n", "Too large to show here; the counts above still hold.", json!({ "tone": "muted" })));
            }
            let mut drawn = 0;
            'hunks: for (hi, h) in f.hunks.iter().enumerate() {
                lines.push(p.label(&format!("h{hi}"), &format!("@@ {}", h.header), json!({ "mono": true, "size": 11, "tone": "muted", "padding": [6, 0, 2, 0] })));
                for (li, l) in h.lines.iter().enumerate() {
                    if drawn == DRAWN {
                        lines.push(p.label("more", &format!("{DRAWN} lines shown; the rest is in `wheelhouse diff --json`."), json!({ "tone": "muted" })));
                        break 'hunks;
                    }
                    drawn += 1;
                    let n = |v: Option<u32>| v.map(|v| format!("{v:>4}")).unwrap_or_else(|| "    ".into());
                    let tone = match l.kind { '+' => "ok", '-' => "bad", _ => "fg" };
                    lines.push(p.label(&format!("l{hi}.{li}"), &format!("{} {} {} {}", n(l.old), n(l.new), l.kind, l.text), json!({ "mono": true, "size": 11, "tone": tone })));
                }
            }
            page.push(p.boxed("file", "v", lines, json!({ "spacing": 0 })));
        }
    }
    p.put(json!({ "id": "page", "component": "Box", "axis": "v", "align": "start", "padding": [18, 16, 48, 16], "children": page }));
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
struct Path {
    /// A file's path.
    path: String,
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
    fn select_file(&self, meta: RequestMetaObject, Parameters(Path { path }): Parameters<Path>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.sel = Some(path))
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

    #[test]
    fn the_page_lists_files_and_draws_the_selected_ones_hunks() {
        let s = Snapshot { name: "t".into(), scopes: vec![Scope { key: "uncommitted".into(), label: "Uncommitted".into(), base: "HEAD".into(),
            fork: None, commits: None, files: vec![file("a.rs"), file("b.rs")] }], ..Default::default() };
        let ui = Ui { scope: String::new(), sel: Some("b.rs".into()) };
        let c = components(&s, &ui);
        let table = c.iter().find(|v| v["id"] == "table").unwrap();
        assert_eq!(table["sections"][0]["rows"].as_array().unwrap().len(), 2);
        assert!(c.iter().any(|v| v["id"] == "file.h" && v["text"] == "b.rs"));
        assert!(c.iter().any(|v| v["id"] == "l0.0" && v["tone"] == "ok"));
    }
}
