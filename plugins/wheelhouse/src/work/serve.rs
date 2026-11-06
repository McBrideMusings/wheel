//! The Implement and Swarm views: the agent panes of the opened space's project, as passes by
//! item and stage (Implement) or as workers by state (Swarm), and starting an agent.
//!
//! Both follow the agents the core follows, so a page redraws when an agent's status or its
//! declared work changes. Each pane keeps the brief typed so far; Start sends it through the
//! core's `spawn_agent`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, MetaObject, RequestMetaObject, ResourceContents, Tool};
use rmcp::{schemars, service::RequestContext, tool, tool_router, ErrorData, RoleServer};
use serde_json::{json, Map, Value};

use super::*;
use crate::core::agents::{self, Agents};
use crate::core::page::{self, cell, Page};
use crate::core::project::{self, Origin};
use crate::core::table;
use crate::view::{View, MIME};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Implement,
    Swarm,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Implement => "implement-native",
            Mode::Swarm => "swarm-native",
        }
    }
    fn uri(self) -> String {
        format!("a2ui://{}", self.name())
    }
    fn surface(self) -> &'static str {
        match self {
            Mode::Implement => "implement",
            Mode::Swarm => "swarm",
        }
    }
}

#[derive(Default)]
struct Ui {
    brief: String,
    notice: Option<(bool, String)>,
}

#[derive(Default)]
struct Shared {
    /// The agents of each subscribed project, as of the last change.
    projects: HashMap<String, Vec<agents::Agent>>,
    panes: HashMap<String, Ui>,
}

fn tone_of(status: &str) -> &'static str {
    match status {
        "working" => "ok",
        "needs_you" => "warn",
        "stopped" => "bad",
        _ => "muted",
    }
}

fn status_cell(status: &str) -> Value {
    let words = if status == "needs_you" { "needs you" } else { status };
    cell(words, json!({ "chip": true, "tone": tone_of(status) }))
}

fn keyed(key: &str, mut c: Value) -> Value {
    c["key"] = json!(key);
    c
}

fn item_row(r: &Row, mode: Mode) -> Value {
    let who = match &r.model {
        Some(m) => format!("{} · {m}", r.agent),
        None => r.agent.clone(),
    };
    let cells = match mode {
        Mode::Implement => vec![
            keyed("item", cell(r.item.clone().unwrap_or_else(|| "no item declared".into()), json!({ "mono": r.item.is_some(), "tone": if r.item.is_some() { "fg" } else { "muted" } }))),
            keyed("stage", r.stage.as_ref().map(|s| cell(s, json!({ "chip": true, "tone": "fg" }))).unwrap_or_else(|| cell("", json!({})))),
            keyed("note", cell(&r.note, json!({ "tone": "muted" }))),
            keyed("agent", cell(who, json!({ "tone": "muted" }))),
            keyed("status", status_cell(&r.status)),
        ],
        Mode::Swarm => vec![
            keyed("pane", cell(&r.pane, json!({ "mono": true }))),
            keyed("agent", cell(who, json!({}))),
            keyed("item", cell(r.item.clone().unwrap_or_default(), json!({ "mono": true }))),
            keyed("stage", cell(r.stage.clone().unwrap_or_default(), json!({ "tone": "muted" }))),
            keyed("note", cell(&r.note, json!({ "tone": "muted" }))),
            keyed("status", status_cell(&r.status)),
        ],
    };
    json!({ "id": format!("{}:{}", r.pane, r.item.clone().unwrap_or_default()), "path": r.pane, "depth": 0, "kids": 0,
        "collapsed": false, "dim": false, "sel": false, "cells": cells, "menu": [] })
}

fn section_of(title: &str, rows: &[&Row], mode: Mode) -> Option<Value> {
    (!rows.is_empty()).then(|| table::section(title, rows.iter().map(|r| item_row(r, mode)).collect()))
}

fn components(dir: &str, agents: &[agents::Agent], ui: &Ui, mode: Mode) -> Vec<Value> {
    let rows = rows(agents);
    let mut p = Page { out: vec![] };
    let mut page = vec![];
    let title = match mode {
        Mode::Implement => "Implement",
        Mode::Swarm => "Swarm",
    };
    let t = p.runs("title", json!([{ "text": format!("{title} · {}", project::name_of(dir)), "size": 20, "weight": "bold" }]), json!({}));
    page.push(p.boxed("head", "h", vec![t], json!({ "padding": [0, 0, 12, 0] })));

    let count = |f: &dyn Fn(&Row) -> bool| rows.iter().filter(|r| f(r)).count();
    let kpis: Vec<(String, usize, &str)> = match mode {
        Mode::Implement => {
            let mut k = vec![("Items".to_string(), count(&|r| r.item.is_some()), "fg")];
            for s in STAGES {
                let name = format!("{}{}", s[..1].to_uppercase(), &s[1..]);
                k.push((name, count(&|r| r.stage.as_deref() == Some(s) && r.item.is_some()), "fg"));
            }
            k
        }
        Mode::Swarm => vec![
            ("Workers".into(), agents.len(), "fg"),
            ("Working".into(), count(&|r| r.status == "working"), "ok"),
            ("Needs you".into(), count(&|r| r.status == "needs_you"), "warn"),
            ("Idle".into(), count(&|r| r.status == "idle"), "fg"),
        ],
    };
    let cards: Vec<String> = kpis.iter().enumerate().map(|(i, (l, v, tone))| {
        p.stat(&format!("kpi{i}"), &v.to_string(), &l.to_string(), if *v == 0 { "muted" } else { tone })
    }).collect();
    let widths: Vec<&str> = cards.iter().map(|_| "flex").collect();
    page.push(p.boxed("kpis", "h", cards, json!({ "spacing": 10, "widths": widths })));

    if let Some((ok, text)) = &ui.notice {
        let l = p.label("notice.t", text, json!({ "tone": if *ok { "ok" } else { "bad" } }));
        page.push(p.boxed("notice", "v", vec![l], json!({ "border": if *ok { "ok" } else { "bad" }, "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }

    p.put(json!({ "id": "brief", "component": "SearchField", "value": ui.brief, "action": "set_brief", "placeholder": "Brief for a new agent" }));
    let start = p.button("start", "Start agent", "start_agent", json!({}), json!({ "disabled": ui.brief.trim().is_empty() }));
    page.push(p.boxed("starter", "h", vec!["brief".into(), start], json!({ "spacing": 8, "widths": ["flex", null], "trailing": 1, "padding": [12, 0, 0, 0] })));

    let sections: Vec<Value> = match mode {
        Mode::Implement => {
            let mut s: Vec<Value> = STAGES.iter().filter_map(|st| {
                let rs: Vec<&Row> = rows.iter().filter(|r| r.item.is_some() && r.stage.as_deref() == Some(st)).collect();
                section_of(&format!("{}{}", st[..1].to_uppercase(), &st[1..]), &rs, mode)
            }).collect();
            let no_stage: Vec<&Row> = rows.iter().filter(|r| r.item.is_some() && !r.stage.as_deref().is_some_and(|x| STAGES.contains(&x))).collect();
            let none: Vec<&Row> = rows.iter().filter(|r| r.item.is_none()).collect();
            s.extend(section_of("No stage declared", &no_stage, mode));
            s.extend(section_of("Agents with no item", &none, mode));
            s
        }
        Mode::Swarm => ["needs_you", "working", "idle", "stopped", "unknown"].iter().filter_map(|st| {
            let rs: Vec<&Row> = rows.iter().filter(|r| r.status == *st).collect();
            section_of(match *st { "needs_you" => "Needs you", "working" => "Working", "idle" => "Idle", "stopped" => "Stopped", _ => "Unknown" }, &rs, mode)
        }).collect(),
    };
    let columns = match mode {
        Mode::Implement => vec![
            table::column("item", "Item", json!("flex"), false, true), table::column("stage", "Stage", json!(90), false, false),
            table::column("note", "Doing", json!("flex"), false, true), table::column("agent", "Agent", json!(180), false, true),
            table::column("status", "State", json!(96), false, false),
        ],
        Mode::Swarm => vec![
            table::column("pane", "Pane", json!(64), false, false), table::column("agent", "Agent", json!(180), false, true),
            table::column("item", "Item", json!(170), false, true), table::column("stage", "Stage", json!(80), false, false),
            table::column("note", "Doing", json!("flex"), false, true), table::column("status", "State", json!(96), false, false),
        ],
    };
    let mut t = table::component(columns, sections, "noop", "noop", ("status", true),
        match mode { Mode::Implement => "No agent is running in this project.", Mode::Swarm => "No workers: start an agent above." });
    t["id"] = json!("table");
    p.put(t);
    page.push(p.boxed("table.b", "v", vec!["table".into()], json!({ "padding": [12, 0, 0, 0] })));
    p.put(json!({ "id": "page", "component": "Box", "axis": "v", "align": "start", "padding": [18, 16, 48, 16], "children": page }));
    p.out
}

fn answer(dir: &str, agents: &[agents::Agent], ui: &Ui, mode: Mode) -> CallToolResult {
    let messages = page::update(mode.surface(), components(dir, agents, ui, mode));
    CallToolResult::success(vec![ContentBlock::resource(ResourceContents::TextResourceContents {
        uri: mode.uri(), mime_type: Some(MIME.into()), text: messages.to_string(), meta: None })])
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Value1 {
    /// The new value.
    value: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Nothing {}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
    shared: Arc<Mutex<Shared>>,
    agents: Agents,
    mode: Mode,
}

impl Server {
    fn draw(&self, o: &Origin, f: impl FnOnce(&mut Ui)) -> CallToolResult {
        let mut s = self.shared.lock().unwrap();
        let list = s.projects.get(&o.dir).cloned().unwrap_or_default();
        let ui = s.panes.entry(o.pane.clone()).or_default();
        f(ui);
        answer(&o.dir, &list, ui, self.mode)
    }
}

#[tool_router]
impl Server {
    fn build(shared: Arc<Mutex<Shared>>, agents: Agents, mode: Mode) -> Self {
        Self { tool_router: Self::tool_router(), shared, agents, mode }
    }

    #[tool(description = "The page")]
    fn get_page(&self, meta: RequestMetaObject) -> CallToolResult {
        self.draw(&Origin::of(&meta), |_| {})
    }

    #[tool(description = "Type the brief of a new agent")]
    fn set_brief(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.draw(&Origin::of(&meta), |ui| ui.brief = value)
    }

    #[tool(description = "Row clicks have no action")]
    fn noop(&self, meta: RequestMetaObject, Parameters(_): Parameters<Nothing>) -> CallToolResult {
        self.draw(&Origin::of(&meta), |_| {})
    }

    #[tool(description = "Start an agent in a new pane with the typed brief")]
    async fn start_agent(&self, meta: RequestMetaObject) -> CallToolResult {
        let o = Origin::of(&meta);
        let brief = self.shared.lock().unwrap().panes.entry(o.pane.clone()).or_default().brief.clone();
        let (agents, origin) = (self.agents.clone(), o.clone());
        let mut args = Map::new();
        args.insert("brief".into(), brief.into());
        let result = tokio::task::spawn_blocking(move || agents.spawn(&origin, &args)).await;
        let outcome = match result {
            Ok(r) => {
                let text = serde_json::to_value(&r.content[0]).ok().and_then(|v| v["text"].as_str().map(String::from)).unwrap_or_default();
                if r.is_error == Some(true) { (false, text) } else { (true, format!("Started the agent in pane {text}")) }
            }
            Err(e) => (false, e.to_string()),
        };
        self.draw(&o, |ui| {
            if outcome.0 {
                ui.brief.clear();
            }
            ui.notice = Some(outcome);
        })
    }
}

/// The Implement view or the Swarm view.
pub struct WorkView {
    server: Server,
}

impl WorkView {
    pub fn new(mode: Mode, agents: Agents) -> Self {
        WorkView { server: Server::build(Arc::new(Mutex::new(Shared::default())), agents, mode) }
    }
}

#[async_trait]
impl View for WorkView {
    fn name(&self) -> &str {
        self.server.mode.name()
    }

    fn tools(&self) -> Vec<Tool> {
        let ui = MetaObject(json!({ "ui": { "resourceUri": self.server.mode.uri(), "mimeType": MIME } }).as_object().unwrap().clone());
        let mut tools = self.server.tool_router.list_all();
        for t in &mut tools {
            t.meta = Some(ui.clone());
        }
        tools.push(agents::spawn_tool());
        tools
    }

    async fn call(&self, params: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        if params.name == "spawn_agent" {
            let (agents, origin) = (self.server.agents.clone(), Origin::of(&context.meta));
            let args = params.arguments.unwrap_or_default();
            return Ok(tokio::task::spawn_blocking(move || agents.spawn(&origin, &args)).await.unwrap().into());
        }
        self.server.tool_router.call(ToolCallContext::new(&self.server, params, context)).await
    }

    fn skeleton(&self) -> Value {
        page::skeleton(self.server.mode.surface())
    }

    fn watches(&self, _dir: &str) -> Vec<PathBuf> {
        vec![]
    }

    fn open(&self, dir: &str) {
        self.server.shared.lock().unwrap().projects.entry(dir.to_string()).or_default();
    }

    fn close(&self, dir: &str) {
        self.server.shared.lock().unwrap().projects.remove(dir);
    }

    fn follows_agents(&self) -> bool {
        true
    }

    fn agents_changed(&self, dir: &str, agents: &[agents::Agent]) -> bool {
        let mut s = self.server.shared.lock().unwrap();
        match s.projects.get_mut(dir) {
            Some(list) if list != agents => {
                *list = agents.to_vec();
                true
            }
            _ => false,
        }
    }

    fn refresh(&self, _dir: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::agents::{Work, WorkItem};

    fn agent(pane: &str, status: &str, stage: Option<&str>) -> agents::Agent {
        agents::Agent { space: "s".into(), cwd: "/p".into(), pane: pane.into(), agent: "w1".into(), harness: "claude".into(), model: Some("sonnet".into()),
            status: Some(status.into()),
            work: stage.map(|s| Work { items: vec![WorkItem { kind: "bead".into(), id: format!("b-{pane}") }], stage: Some(s.into()), note: Some("doing".into()) }) }
    }

    #[test]
    fn implement_groups_items_by_stage_and_counts_them() {
        let list = [agent("p1", "working", Some("edit")), agent("p2", "idle", Some("gate")), agent("p3", "idle", None)];
        let c = components("/p", &list, &Ui::default(), Mode::Implement);
        let table = c.iter().find(|v| v["id"] == "table").unwrap();
        let titles: Vec<&str> = table["sections"].as_array().unwrap().iter().map(|s| s["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Edit", "Gate", "Agents with no item"]);
        assert_eq!(table["sections"][0]["rows"][0]["cells"][0]["text"], "bead b-p1");
        assert_eq!(c.iter().find(|v| v["id"] == "kpi0.v").unwrap()["text"], "2");
    }

    #[test]
    fn swarm_groups_workers_by_state_and_start_needs_a_brief() {
        let list = [agent("p1", "needs_you", Some("verify")), agent("p2", "working", None)];
        let c = components("/p", &list, &Ui::default(), Mode::Swarm);
        let table = c.iter().find(|v| v["id"] == "table").unwrap();
        assert_eq!(table["sections"][0]["title"], "Needs you");
        assert_eq!(table["sections"][1]["title"], "Working");
        assert!(c.iter().find(|v| v["id"] == "start").unwrap()["modifiers"].as_array().unwrap().iter().any(|m| m["disabled"] == true));
        let typed = Ui { brief: "go".into(), notice: None };
        let c = components("/p", &list, &typed, Mode::Swarm);
        assert!(c.iter().find(|v| v["id"] == "start").unwrap()["modifiers"].is_null(), "an enabled Start has no disabled modifier");
    }
}
