//! The PRs view: the project's open PRs in four buckets, with their checks and review state.
//!
//! Each subscribed project keeps one snapshot, re-read when its git state moves (a fetch or push
//! updates `refs/remotes/origin`) or two minutes pass, since GitHub itself cannot be watched.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, MetaObject, RequestMetaObject, ResourceContents, Tool};
use rmcp::{schemars, service::RequestContext, tool, tool_router, ErrorData, RoleServer};
use serde_json::{json, Value};

use super::*;
use crate::core::page::{self, ago, cell, now, Page};
use crate::core::project::Origin;
use crate::core::table;
use crate::view::{View, MIME};

pub const NAME: &str = "prs";
const URI: &str = "a2ui://prs";
const SURFACE: &str = "prs";

#[derive(Default)]
struct Ui {
    q: String,
    sel: Option<i64>,
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

const BUCKETS: [(&str, &str); 4] = [("needs-me", "Needs me"), ("mine", "Mine"), ("waiting", "Waiting on others"), ("other", "Other")];

fn row(p: &Pr, ui: &Ui, at: i64) -> Value {
    let checks_tone = match p.checks { "passing" => "ok", "failing" => "bad", "pending" => "warn", _ => "muted" };
    let mut cells = vec![
        cell(format!("#{}", p.number), json!({ "key": "number", "mono": true, "tone": "muted" })),
        cell(if p.draft { format!("{} (draft)", p.title) } else { p.title.clone() }, json!({ "key": "title" })),
        cell(&p.author, json!({ "key": "author", "tone": "muted" })),
        cell(if p.checks == "none" { "" } else { p.checks }, json!({ "key": "checks", "chip": p.checks != "none", "tone": checks_tone })),
        cell(p.review, json!({ "key": "review", "tone": "muted" })),
        cell(if p.mergeable == "CONFLICTING" { "conflicts".to_string() } else { p.gaps.clone() }, json!({ "key": "gaps", "mono": true, "tone": if p.mergeable == "CONFLICTING" { "bad" } else { "muted" } })),
        cell(ago(p.updated, at), json!({ "key": "updated", "tone": "muted" })),
    ];
    cells[1]["tone"] = json!(if p.fix { "bad" } else { "fg" });
    json!({ "id": p.number.to_string(), "path": p.number.to_string(), "depth": 0, "kids": 0, "collapsed": false, "dim": p.draft,
        "sel": ui.sel == Some(p.number), "cells": cells, "menu": [] })
}

fn matches(p: &Pr, ui: &Ui) -> bool {
    let hay = format!("{} {} {} #{}", p.title, p.author, p.branch, p.number).to_lowercase();
    ui.q.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

fn components(s: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let at = now();
    let mut page = vec![];
    // No page title or subtitle: the pane's own bar says which page it is.
    let n = |b: &str| s.prs.iter().filter(|p| p.bucket == b).count();
    let cards: Vec<String> = BUCKETS.iter().enumerate().map(|(i, (k, l))| {
        let v = n(k);
        p.stat(&format!("kpi{i}"), &v.to_string(), &l.to_string(), if v == 0 { "muted" } else if *k == "needs-me" { "ok" } else { "fg" })
    }).collect();
    page.push(p.boxed("kpis", "h", cards, json!({ "spacing": 10, "widths": ["flex", "flex", "flex", "flex"] })));
    if let Some(e) = &s.error {
        let l = p.label("problem.t", e, json!({ "tone": "bad" }));
        page.push(p.boxed("problem", "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if s.repo.is_some() {
        p.search("search", "set_query", &ui.q, "Search");
        let shown: Vec<&Pr> = s.prs.iter().filter(|x| matches(x, ui)).collect();
        let count = p.label("count", &format!("{} of {}", shown.len(), s.prs.len()), json!({ "tone": "muted", "size": 11, "tabular": true, "align": "end" }));
        page.push(p.boxed("tools", "h", vec!["search".into(), count], json!({ "spacing": 8, "align": "center", "padding": [12, 0, 0, 0], "widths": ["flex", 96], "trailing": 1 })));
        let sections: Vec<Value> = BUCKETS.iter().filter_map(|(k, title)| {
            let rows: Vec<Value> = shown.iter().filter(|x| x.bucket == *k).map(|x| row(x, ui, at)).collect();
            (!rows.is_empty()).then(|| table::section(title, rows))
        }).collect();
        let columns = vec![table::column("number", "#", json!(60), false, false), table::column("title", "Title", json!("flex"), false, true),
            table::column("author", "Author", json!(110), false, false), table::column("checks", "Checks", json!(84), false, false),
            table::column("review", "Review", json!(130), false, true), table::column("gaps", "Gaps", json!(80), false, false),
            table::column("updated", "Updated", json!(70), false, false)];
        let mut t = table::component(columns, sections, "select_pr", "select_pr", ("updated", false),
            if s.prs.is_empty() { "No open PRs." } else { "Nothing matches the search." });
        t["id"] = json!("table");
        p.put(t);
        page.push(p.boxed("table.b", "v", vec!["table".into()], json!({})));
        if let Some(pr) = s.prs.iter().find(|x| Some(x.number) == ui.sel) {
            let facts = [format!("{} → {}", pr.branch, pr.author), format!("{} commits · {} files · +{} −{}", pr.commits, pr.files, pr.additions, pr.deletions),
                format!("labels: {}", if pr.labels.is_empty() { "none".into() } else { pr.labels.join(", ") }),
                format!("reviewers: {}", if pr.reviewers.is_empty() { "none".into() } else { pr.reviewers.join(", ") }), pr.url.clone()];
            let mut kids = vec![p.label("pr.h", &format!("#{} {}", pr.number, pr.title), json!({ "weight": "bold", "padding": [14, 0, 4, 0] }))];
            for (i, f) in facts.iter().enumerate() {
                kids.push(p.label(&format!("pr.f{i}"), f, json!({ "size": 12, "tone": "muted" })));
            }
            page.push(p.boxed("pr", "v", kids, json!({ "spacing": 3 })));
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
struct Query {
    /// The search text.
    #[serde(alias = "value")]
    q: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Number {
    /// The PR's number, as the row's id.
    path: String,
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
    shared: Arc<Mutex<Shared>>,
}

impl Server {
    fn with(&self, meta: &RequestMetaObject, f: impl FnOnce(&mut Ui)) -> CallToolResult {
        let o = Origin::of(meta);
        let mut s = self.shared.lock().unwrap();
        let snap = s.project(&o.dir).clone();
        let ui = s.panes.entry(o.pane).or_default();
        f(ui);
        answer(&snap, ui)
    }
}

#[tool_router]
impl Server {
    fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self { tool_router: Self::tool_router(), shared }
    }

    #[tool(description = "The PRs page", meta = ui_meta())]
    fn get_prs(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_| {})
    }

    #[tool(description = "Search the PRs", meta = ui_meta())]
    fn set_query(&self, meta: RequestMetaObject, Parameters(Query { q }): Parameters<Query>) -> CallToolResult {
        self.with(&meta, |ui| ui.q = q)
    }

    #[tool(description = "Open or close a PR's details", meta = ui_meta())]
    fn select_pr(&self, meta: RequestMetaObject, Parameters(Number { path }): Parameters<Number>) -> CallToolResult {
        self.with(&meta, |ui| {
            let n = path.parse().ok();
            ui.sel = if ui.sel == n { None } else { n };
        })
    }
}

/// The PRs view.
pub struct PrsView {
    server: Server,
}

impl PrsView {
    pub fn new() -> Self {
        PrsView { server: Server::new(Arc::new(Mutex::new(Shared::default()))) }
    }
}

#[async_trait]
impl View for PrsView {
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
        vec![own.join("logs/HEAD"), common.join("packed-refs"), common.join("refs/remotes/origin")]
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

    fn pr(n: i64, bucket: &'static str) -> Pr {
        Pr { number: n, title: format!("pr {n}"), author: "a".into(), branch: "b".into(), draft: false, mergeable: "MERGEABLE".into(), checks: "passing",
            review: "in-review", bucket, my: None, gaps: String::new(), updated: 0, url: "u".into(), fix: false, additions: 1, deletions: 0,
            files: 1, commits: 1, labels: vec![], reviewers: vec![] }
    }

    #[test]
    fn the_page_groups_prs_by_bucket_and_opens_one() {
        let s = Snapshot { name: "t".into(), repo: Some("o/t".into()), me: Some("me".into()), prs: vec![pr(1, "mine"), pr(2, "needs-me")], error: None };
        let c = components(&s, &Ui { q: String::new(), sel: Some(2) });
        let t = c.iter().find(|v| v["id"] == "table").unwrap();
        assert_eq!(t["sections"][0]["title"], "Needs me");
        assert_eq!(t["sections"][1]["title"], "Mine");
        assert!(c.iter().any(|v| v["id"] == "pr.h" && v["text"] == "#2 pr 2"));
    }
}
