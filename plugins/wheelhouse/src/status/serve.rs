//! The Project view: the project's status against its vision, and editing the vision.
//!
//! Each pane keeps whether it is editing the vision and the text typed so far; each subscribed
//! project keeps one snapshot, re-read when `.beads/last-touched` or `docs/CONTEXT.md` moves.

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

pub const NAME: &str = "project";
const URI: &str = "a2ui://project";
const SURFACE: &str = "project";

#[derive(Default)]
struct Ui {
    /// The vision text being edited, or `None` when the page only reads it.
    editing: Option<String>,
    notice: Option<(bool, String)>,
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

fn components(s: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let mut page = vec![];
    let t = p.runs("title", json!([{ "text": s.name, "size": 20, "weight": "bold" }]), json!({}));
    page.push(p.boxed("head", "h", vec![t], json!({ "padding": [0, 0, 8, 0] })));
    if let Some(e) = &s.error {
        let l = p.label("problem.t", e, json!({ "tone": "bad" }));
        page.push(p.boxed("problem", "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12] })));
    }
    if let Some((ok, text)) = &ui.notice {
        let l = p.label("notice.t", text, json!({ "tone": if *ok { "ok" } else { "bad" } }));
        page.push(p.boxed("notice", "v", vec![l], json!({ "border": if *ok { "ok" } else { "bad" }, "radius": 8, "padding": [8, 12], "margin": [0, 0, 8, 0] })));
    }
    match &ui.editing {
        Some(text) => {
            p.search("vision.edit", "set_vision_draft", text, "What this project is for");
            let save = p.button("vision.save", "Save", "save_vision", json!({}), json!({ "disabled": text.trim().is_empty() }));
            let cancel = p.button("vision.cancel", "Cancel", "cancel_vision", json!({}), json!({ "style": "link" }));
            page.push(p.boxed("vision.row", "h", vec!["vision.edit".into(), save, cancel], json!({ "spacing": 8, "widths": ["flex", null, null], "padding": [0, 0, 14, 0] })));
        }
        None => {
            let text = s.vision.as_deref().unwrap_or("No vision yet: the first paragraph of docs/CONTEXT.md.");
            let v = p.label("vision", text, json!({ "size": 14, "tone": if s.vision.is_some() { "fg" } else { "muted" } }));
            let edit = p.button("vision.edit", "Edit vision", "edit_vision", json!({}), json!({ "style": "link" }));
            page.push(p.boxed("vision.row", "h", vec![v, edit], json!({ "spacing": 12, "widths": ["flex", null], "padding": [0, 0, 14, 0] })));
        }
    }
    if !s.has_tracker {
        let l = p.label("none", "This project keeps no tracker (no .beads directory).", json!({ "tone": "muted" }));
        page.push(p.boxed("none.b", "v", vec![l], json!({})));
    } else if s.error.is_none() {
        let c = &s.counts;
        let stats = [("Done", c.done, "ok"), ("In progress", c.progress, "fg"), ("Ready", c.ready, "ok"),
            ("Blocked", c.blocked, "warn"), ("Open", c.open, "fg"), ("Deferred", c.deferred, "muted")];
        let cards: Vec<String> = stats.iter().enumerate().map(|(i, (l, v, tone))| {
            p.stat(&format!("kpi{i}"), &v.to_string(), &l.to_string(), if *v == 0 { "muted" } else { tone })
        }).collect();
        page.push(p.boxed("kpis", "h", cards, json!({ "spacing": 10, "widths": ["flex", "flex", "flex", "flex", "flex", "flex"] })));
        let rows: Vec<Value> = s.epics.iter().map(|e| {
            let cells = vec![
                cell(&e.id, json!({ "key": "id", "mono": true })),
                cell(&e.title, json!({ "key": "title" })),
                cell(if e.closed { "done".to_string() } else { format!("{}/{}", e.done, e.all) }, json!({ "key": "progress", "mono": true, "tabular": true })),
                cell(if e.ready > 0 { format!("{} ready", e.ready) } else { String::new() }, json!({ "key": "ready", "tone": "ok" })),
            ];
            json!({ "id": e.id, "path": e.id, "depth": 0, "kids": 0, "collapsed": false, "dim": e.closed, "sel": false, "cells": cells, "menu": [] })
        }).collect();
        let columns = vec![
            table::column("id", "Epic", json!(110), false, false), table::column("title", "Title", json!("flex"), false, true),
            table::column("progress", "Done", json!(70), false, false), table::column("ready", "Ready", json!(90), false, false),
        ];
        let mut t = table::component(columns, vec![table::section("Epics", rows)], "none", "none", "", ("id", true), "No epics.");
        t["id"] = json!("table");
        p.put(t);
        page.push(p.boxed("table.b", "v", vec!["table".into()], json!({ "padding": [18, 0, 0, 0] })));
    }
    p.boxed("page", "v", page, json!({ "align": "start", "padding": [18, 16, 48, 16] }));
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
        ui.notice = None;
        f(&snap, ui);
        answer(&snap, ui)
    }
}

#[tool_router]
impl Server {
    fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self { tool_router: Self::tool_router(), shared }
    }

    #[tool(description = "The project page", meta = ui_meta())]
    fn get_project(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, _| {})
    }

    #[tool(description = "Start editing the vision", meta = ui_meta())]
    fn edit_vision(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |s, ui| ui.editing = Some(s.vision.clone().unwrap_or_default()))
    }

    #[tool(description = "Type the vision", meta = ui_meta())]
    fn set_vision_draft(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.editing = Some(value))
    }

    #[tool(description = "Stop editing the vision and keep the file as it is", meta = ui_meta())]
    fn cancel_vision(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| ui.editing = None)
    }

    #[tool(description = "Write the typed vision to docs/CONTEXT.md", meta = ui_meta())]
    fn save_vision(&self, meta: RequestMetaObject) -> CallToolResult {
        let o = Origin::of(&meta);
        let mut s = self.shared.lock().unwrap();
        let ui = s.panes.entry(o.pane.clone()).or_default();
        let text = ui.editing.clone().unwrap_or_default();
        let result = crate::core::tracker::set_vision(&o.dir, &text);
        ui.notice = Some(match &result { Ok(()) => (true, "Vision saved".into()), Err(e) => (false, e.clone()) });
        if result.is_ok() {
            ui.editing = None;
        }
        let fresh = collect(&o.dir);
        s.projects.insert(o.dir.clone(), fresh.clone());
        answer(&fresh, s.panes.get(&o.pane).unwrap())
    }
}

/// The Project view.
pub struct ProjectView {
    server: Server,
}

impl ProjectView {
    pub fn new() -> Self {
        ProjectView { server: Server::new(Arc::new(Mutex::new(Shared::default()))) }
    }
}

#[async_trait]
impl View for ProjectView {
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
        vec![PathBuf::from(dir).join(".beads/last-touched"), PathBuf::from(dir).join("docs/CONTEXT.md")]
    }

    fn every(&self) -> u64 {
        120
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

    #[test]
    fn the_page_shows_the_vision_and_swaps_it_for_a_field_while_editing() {
        let s = Snapshot { name: "t".into(), has_tracker: true, vision: Some("Why.".into()), ..Default::default() };
        let c = components(&s, &Ui::default());
        assert!(c.iter().any(|v| v["id"] == "vision" && v["text"] == "Why."));
        assert!(c.iter().any(|v| v["id"] == "vision.edit" && v["action"]["event"]["name"] == "edit_vision"));
        let c = components(&s, &Ui { editing: Some("New".into()), notice: None });
        assert!(c.iter().any(|v| v["updateDataModel"]["path"] == "/set_vision_draft" && v["updateDataModel"]["value"] == "New"));
        assert!(c.iter().any(|v| v["id"] == "vision.save"));
    }
}
