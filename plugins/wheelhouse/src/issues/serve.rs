//! The Issues view: the project's tracker, and creating, updating and closing an item from the page.
//!
//! Each pane keeps its search, filter, open item and new-item draft; each subscribed project keeps
//! one snapshot, re-read when `.beads/last-touched` moves or two minutes pass.

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
use crate::core::tracker::{KINDS, STATUSES};
use crate::view::{View, MIME};

pub const NAME: &str = "issues";
const URI: &str = "a2ui://issues";
const SURFACE: &str = "issues";

struct Ui {
    q: String,
    show: String,
    sel: Option<String>,
    title: String,
    kind: String,
    priority: i64,
    notice: Option<(bool, String)>,
}

impl Default for Ui {
    fn default() -> Self {
        Ui { q: String::new(), show: "open".into(), sel: None, title: String::new(), kind: "task".into(), priority: 2, notice: None }
    }
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

fn shown(i: &Item, ui: &Ui) -> bool {
    let hay = format!("{} {} {} {}", i.id, i.title, i.labels.join(" "), i.assignee.as_deref().unwrap_or("")).to_lowercase();
    ui.q.to_lowercase().split_whitespace().all(|w| hay.contains(w))
        && match ui.show.as_str() {
            "closed" => i.status == "closed",
            "ready" => i.ready,
            "all" => true,
            _ => i.status != "closed",
        }
}

fn tone(i: &Item) -> &'static str {
    match i.status.as_str() {
        "closed" => "muted",
        "in_progress" => "ok",
        _ if i.blocked || i.status == "blocked" => "warn",
        _ if i.ready => "ok",
        _ => "muted",
    }
}

fn state(i: &Item) -> &str {
    match i.status.as_str() {
        "open" if i.blocked => "blocked",
        "open" if i.ready => "ready",
        s => s,
    }
}

fn options(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| json!({ "value": v, "text": v })).collect())
}

fn priorities() -> Value {
    Value::Array((0..5).map(|n| json!({ "value": n.to_string(), "text": format!("P{n}") })).collect())
}

fn row(i: &Item, ui: &Ui) -> Value {
    let cells = vec![
        cell(&i.id, json!({ "key": "id", "mono": true })),
        cell(&i.title, json!({ "key": "title" })),
        cell(state(i), json!({ "key": "state", "chip": true, "tone": tone(i) })),
        cell(i.priority.map(|p| format!("P{p}")).unwrap_or_default(), json!({ "key": "pri", "mono": true })),
        cell(&i.kind, json!({ "key": "type", "tone": "muted" })),
        cell(i.assignee.clone().unwrap_or_default(), json!({ "key": "who", "tone": "muted" })),
    ];
    json!({ "id": i.id, "path": i.id, "depth": 0, "kids": 0, "collapsed": false, "dim": i.status == "closed",
        "sel": ui.sel.as_deref() == Some(i.id.as_str()), "cells": cells, "menu": [] })
}

fn detail(p: &mut Page, i: &Item) -> String {
    let mut c = vec![p.label("d.title", &i.title, json!({ "size": 17, "weight": "semibold" }))];
    let fact = |p: &mut Page, n: &str, name: &str, value: String| -> String {
        let l = p.label(&format!("d.f.{n}.k"), name, json!({ "tone": "muted", "size": 12 }));
        p.boxed(&format!("d.f.{n}"), "h", vec![l, value], json!({ "spacing": 12, "widths": [84, "flex"], "align": "firstTextBaseline" }))
    };
    let v = p.label("d.f.id.v", &i.id, json!({ "mono": true, "size": 12 }));
    c.push(fact(p, "id", "ID", v));
    let v = p.label("d.f.ty.v", &i.kind, json!({}));
    c.push(fact(p, "ty", "Type", v));
    if let Some(par) = &i.parent {
        let v = p.label("d.f.pa.v", par, json!({ "mono": true, "size": 12 }));
        c.push(fact(p, "pa", "Parent", v));
    }
    if !i.labels.is_empty() {
        let v = p.label("d.f.lb.v", &i.labels.join("  ·  "), json!({}));
        c.push(fact(p, "lb", "Labels", v));
    }
    if let Some(a) = &i.assignee {
        let v = p.label("d.f.as.v", a, json!({}));
        c.push(fact(p, "as", "Assignee", v));
    }
    if i.status == "closed" {
        let v = p.label("d.f.st.v", "closed", json!({ "chip": true, "tone": "muted" }));
        c.push(fact(p, "st", "Status", v));
    } else {
        p.choose("d.status", "/set_field/status", &i.status.to_string(), options(&STATUSES), false, false);
        let l = p.label("d.f.st.k", "Status", json!({ "tone": "muted", "size": 12 }));
        c.push(p.boxed("d.f.st", "h", vec![l, "d.status".into()], json!({ "spacing": 12, "widths": [84, "flex"], "align": "firstTextBaseline" })));
        p.choose("d.pri", "/set_field/priority", &i.priority.unwrap_or(2).to_string(), priorities(), false, false);
        let l = p.label("d.f.pr.k", "Priority", json!({ "tone": "muted", "size": 12 }));
        c.push(p.boxed("d.f.pr", "h", vec![l, "d.pri".into()], json!({ "spacing": 12, "widths": [84, "flex"], "align": "firstTextBaseline" })));
    }
    if !i.description.is_empty() {
        let l = p.label("d.desc", &i.description.chars().take(1500).collect::<String>(), json!({ "size": 13 }));
        c.push(p.boxed("d.desc.b", "v", vec![l], json!({ "padding": [10, 12], "fill": "soft", "radius": 8 })));
    }
    if i.status != "closed" {
        c.push(p.button("d.close", "Close issue", "close_issue", json!({}), json!({})));
    }
    let body = p.boxed("d.body", "v", c, json!({ "spacing": 12, "align": "start" }));
    p.put(json!({ "id": "detail", "component": "Modal", "presented": true, "dismissAction": "close_detail", "children": [body] }));
    "detail".into()
}

fn components(s: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let mut page = vec![];
    let t = p.runs("title", json!([{ "text": format!("Issues · {}", s.name), "size": 20, "weight": "bold" }]), json!({}));
    page.push(p.boxed("head", "h", vec![t], json!({ "padding": [0, 0, 12, 0] })));
    for (n, e) in s.error.iter().enumerate() {
        let l = p.label(&format!("problem{n}.t"), e, json!({ "tone": "bad" }));
        page.push(p.boxed(&format!("problem{n}"), "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if let Some((ok, text)) = &ui.notice {
        let l = p.label("notice.t", text, json!({ "tone": if *ok { "ok" } else { "bad" } }));
        page.push(p.boxed("notice", "v", vec![l], json!({ "border": if *ok { "ok" } else { "bad" }, "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if !s.has_tracker {
        let l = p.label("none", "This project keeps no tracker (no .beads directory).", json!({ "tone": "muted" }));
        page.push(p.boxed("none.b", "v", vec![l], json!({ "padding": [12, 0] })));
    } else if s.error.is_none() {
        p.search("new.title", "set_draft", &ui.title, "New issue title");
        p.choose("new.kind", "/set_new/kind", &ui.kind.to_string(), options(&KINDS), false, false);
        p.choose("new.pri", "/set_new/priority", &ui.priority.to_string(), priorities(), false, false);
        let create = p.button("new.create", "Create", "create_issue", json!({}), json!({ "disabled": ui.title.trim().is_empty() }));
        page.push(p.boxed("new", "h", vec!["new.title".into(), "new.kind".into(), "new.pri".into(), create],
            json!({ "spacing": 8, "widths": ["flex", null, null, null], "padding": [0, 0, 12, 0] })));
        p.search("search", "set_query", &ui.q, "Search");
        p.choose("views", "/set_show", &ui.show, vec![
            json!({ "value": "open", "text": "Open" }), json!({ "value": "ready", "text": "Ready" }),
            json!({ "value": "closed", "text": "Closed" }), json!({ "value": "all", "text": "All" })], true, false);
        let rows: Vec<&Item> = s.items.iter().filter(|i| shown(i, ui)).collect();
        let count = p.label("count", &format!("{} of {}", rows.len(), s.items.len()), json!({ "tone": "muted", "size": 11, "tabular": true, "align": "end" }));
        page.push(p.boxed("tools", "h", vec!["search".into(), "views".into(), count],
            json!({ "spacing": 8, "align": "center", "widths": ["flex", null, 96], "trailing": 1 })));
        let mut sorted = rows.clone();
        sorted.sort_by(|a, b| (a.priority.unwrap_or(9), &a.id).cmp(&(b.priority.unwrap_or(9), &b.id)));
        let columns = vec![
            table::column("id", "ID", json!(110), false, false), table::column("title", "Title", json!("flex"), false, true),
            table::column("state", "State", json!(96), false, false), table::column("pri", "Priority", json!(64), false, false),
            table::column("type", "Type", json!(80), false, false), table::column("who", "Assignee", json!(110), false, true),
        ];
        let sections = vec![table::section("", sorted.iter().map(|i| row(i, ui)).collect())];
        let mut t = table::component(columns, sections, "select_issue", "select_issue", ("pri", true),
            if s.items.is_empty() { "No issues yet." } else { "Nothing matches the search." });
        t["id"] = json!("table");
        p.put(t);
        page.push(p.boxed("table.b", "v", vec!["table".into()], json!({})));
        match ui.sel.as_deref().and_then(|id| s.items.iter().find(|i| i.id == id)) {
            Some(i) => page.push(detail(&mut p, i)),
            None => {
                p.put(json!({ "id": "detail", "component": "Modal", "presented": false, "dismissAction": "close_detail" }));
            }
        }
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Field {
    /// The field: status or priority (kind or priority for a new issue).
    key: String,
    /// Its new value.
    value: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct IssueId {
    /// The issue's id.
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
        ui.notice = None;
        f(&snap, ui);
        answer(&snap, ui)
    }

    /// Run a write against the project, re-read the tracker, and answer with what it said.
    fn write(&self, meta: &RequestMetaObject, f: impl FnOnce(&str, &mut Ui) -> Result<String, String>) -> CallToolResult {
        let o = Origin::of(meta);
        let mut s = self.shared.lock().unwrap();
        let ui = s.panes.entry(o.pane.clone()).or_default();
        let result = f(&o.dir, ui);
        let ui = s.panes.get_mut(&o.pane).unwrap();
        ui.notice = Some(match result { Ok(t) => (true, t), Err(e) => (false, e) });
        let fresh = collect(&o.dir);
        s.projects.insert(o.dir.clone(), fresh.clone());
        answer(&fresh, s.panes.get(&o.pane).unwrap())
    }
}

#[tool_router]
impl Server {
    fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self { tool_router: Self::tool_router(), shared }
    }

    #[tool(description = "The issues page", meta = ui_meta())]
    fn get_issues(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, _| {})
    }

    #[tool(description = "Search the issues", meta = ui_meta())]
    fn set_query(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.q = value)
    }

    #[tool(description = "Show open, ready, closed or all issues", meta = ui_meta())]
    fn set_show(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.show = value)
    }

    #[tool(description = "Open an issue's panel, or close it by choosing it again", meta = ui_meta())]
    fn select_issue(&self, meta: RequestMetaObject, Parameters(IssueId { id }): Parameters<IssueId>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.sel = if ui.sel.as_deref() == Some(&id) { None } else { Some(id) })
    }

    #[tool(description = "Close the open issue's panel", meta = ui_meta())]
    fn close_detail(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| ui.sel = None)
    }

    #[tool(description = "Type the new issue's title", meta = ui_meta())]
    fn set_draft(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.title = value)
    }

    #[tool(description = "Choose the new issue's type or priority", meta = ui_meta())]
    fn set_new(&self, meta: RequestMetaObject, Parameters(Field { key, value }): Parameters<Field>) -> CallToolResult {
        self.with(&meta, |_, ui| match key.as_str() {
            "kind" => ui.kind = value,
            "priority" => ui.priority = value.parse().unwrap_or(2),
            _ => {}
        })
    }

    #[tool(description = "Create the issue from the title, type and priority typed on the page", meta = ui_meta())]
    fn create_issue(&self, meta: RequestMetaObject) -> CallToolResult {
        self.write(&meta, |dir, ui| {
            let id = crate::core::tracker::create(dir, &ui.title, &ui.kind, ui.priority, "")?;
            ui.title.clear();
            Ok(format!("Created {id}"))
        })
    }

    #[tool(description = "Set the open issue's status or priority", meta = ui_meta())]
    fn set_field(&self, meta: RequestMetaObject, Parameters(Field { key, value }): Parameters<Field>) -> CallToolResult {
        self.write(&meta, |dir, ui| {
            let id = ui.sel.clone().ok_or("no issue is open")?;
            match key.as_str() {
                "status" => crate::core::tracker::update(dir, &id, Some(&value), None, None)?,
                "priority" => crate::core::tracker::update(dir, &id, None, Some(value.parse().map_err(|_| "priority is a number 0 to 4")?), None)?,
                _ => return Err(format!("no field {key}")),
            }
            Ok(format!("Updated {id}"))
        })
    }

    #[tool(description = "Close the open issue", meta = ui_meta())]
    fn close_issue(&self, meta: RequestMetaObject) -> CallToolResult {
        self.write(&meta, |dir, ui| {
            let id = ui.sel.take().ok_or("no issue is open")?;
            crate::core::tracker::close(dir, &id, "")?;
            Ok(format!("Closed {id}"))
        })
    }
}

/// The Issues view.
pub struct IssuesView {
    server: Server,
}

impl IssuesView {
    pub fn new() -> Self {
        IssuesView { server: Server::new(Arc::new(Mutex::new(Shared::default()))) }
    }
}

#[async_trait]
impl View for IssuesView {
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
        vec![PathBuf::from(dir).join(".beads/last-touched")]
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

    fn item(id: &str, status: &str, ready: bool) -> Item {
        Item { id: id.into(), title: format!("title {id}"), status: status.into(), kind: "task".into(), priority: Some(1), parent: None,
            labels: vec![], assignee: None, description: String::new(), updated: String::new(), ready, blocked: false }
    }

    #[test]
    fn the_filter_hides_closed_items_until_asked_for() {
        let mut ui = Ui::default();
        assert!(shown(&item("a", "open", false), &ui));
        assert!(!shown(&item("a", "closed", false), &ui));
        ui.show = "closed".into();
        assert!(shown(&item("a", "closed", false), &ui));
        ui.show = "ready".into();
        assert!(!shown(&item("a", "open", false), &ui));
    }

    #[test]
    fn the_page_lists_items_and_an_open_item_gets_a_panel_with_close() {
        let s = Snapshot { name: "t".into(), has_tracker: true, items: vec![item("t-1", "open", true)], ..Default::default() };
        let mut ui = Ui::default();
        let c = components(&s, &ui);
        let table = c.iter().find(|v| v["id"] == "table").unwrap();
        assert_eq!(table["sections"][0]["rows"][0]["cells"][0]["text"], "t-1");
        assert!(c.iter().any(|v| v["id"] == "detail" && v["presented"] == false), "a closed panel is sent as a Modal that is not presented");
        ui.sel = Some("t-1".into());
        let c = components(&s, &ui);
        assert!(c.iter().any(|v| v["id"] == "d.close" && v["action"]["event"]["name"] == "close_issue"));
        assert!(c.iter().any(|v| v["updateDataModel"]["path"] == "/set_field/status" && v["updateDataModel"]["value"][0] == "open"));
    }
}
