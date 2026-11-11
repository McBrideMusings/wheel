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

#[derive(Clone, Default)]
struct Ui {
    q: String,
    sel: Option<i64>,
    /// The selected PR in full, as GitHub's GraphQL node; read when it is selected and after each button.
    detail: Option<Value>,
    /// The agents Wheel can run, for the modal's choice.
    agents: Vec<String>,
    ch: detail::Choices,
}

#[derive(Default)]
struct Shared {
    projects: HashMap<String, Snapshot>,
    panes: HashMap<String, Ui>,
    /// Projects whose pane finished loading a PR in the background and so need a redraw.
    done: std::collections::HashSet<String>,
}

impl Shared {
    fn project(&mut self, dir: &str) -> &mut Snapshot {
        self.projects.entry(dir.to_string()).or_insert_with(|| collect(dir))
    }
}

fn ui_meta() -> MetaObject {
    MetaObject(json!({ "ui": { "resourceUri": URI, "mimeType": MIME } }).as_object().unwrap().clone())
}

/// The four counts at the top.
const BUCKETS: [(&str, &str); 4] = [("needs-me", "Needs me"), ("mine", "Mine"), ("waiting", "Waiting on others"), ("other", "Other")];

fn row(p: &Pr, ui: &Ui, at: i64) -> Value {
    let checks_tone = match p.checks { "passing" => "ok", "failing" => "bad", "pending" => "warn", _ => "muted" };
    let mut cells = vec![
        cell(format!("#{}", p.number), json!({ "key": "number", "mono": true, "tone": "muted", "sort": p.number })),
        cell(if p.draft { format!("{} (draft)", p.title) } else { p.title.clone() }, json!({ "key": "title", "sort": p.title.to_lowercase() })),
        cell(&p.branch, json!({ "key": "branch", "mono": true, "size": 12, "tone": "muted", "sort": p.branch.to_lowercase() })),
        cell(&p.summary, json!({ "key": "summary", "tone": "muted", "size": 12, "sort": p.summary.to_lowercase() })),
        cell(&p.next, json!({ "key": "next", "tone": if p.fix { "bad" } else { "fg" }, "sort": p.next })),
        cell(&p.author, json!({ "key": "author", "tone": "muted", "sort": p.author.to_lowercase() })),
        cell(if p.checks == "none" { "" } else { p.checks }, json!({ "key": "checks", "chip": p.checks != "none", "tone": checks_tone, "sort": match p.checks { "failing" => 0, "pending" => 1, "passing" => 2, _ => 3 } })),
        cell(p.review, json!({ "key": "review", "tone": "muted", "sort": p.review })),
        cell(if p.mergeable == "CONFLICTING" { "conflicts".to_string() } else { p.gaps.clone() }, json!({ "key": "gaps", "mono": true, "tone": if p.mergeable == "CONFLICTING" { "bad" } else { "muted" }, "sort": if p.mergeable == "CONFLICTING" { "!".to_string() } else { p.gaps.clone() } })),
        cell(p.issues.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(" "), json!({ "key": "issue", "mono": true, "tone": "muted", "sort": p.issues.first().copied().unwrap_or(0) })),
        cell(ago(p.updated, at), json!({ "key": "updated", "tone": "muted", "sort": p.updated })),
    ];
    cells[1]["tone"] = json!(if p.fix { "bad" } else { "fg" });
    json!({ "id": p.number.to_string(), "path": p.number.to_string(), "depth": 0, "kids": 0, "collapsed": false, "dim": p.draft,
        "sel": ui.sel == Some(p.number), "cells": cells, "menu": [] })
}

/// A Unix time as the local `HH:MM`.
fn clock(ts: i64) -> String {
    crate::core::run::text("/bin/date", "/", &["-r", &ts.to_string(), "+%H:%M"]).map(|t| t.trim().to_string()).unwrap_or_default()
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
        // When GitHub was last asked and the latest the page checks again, as clock times: they stay true until the next draw.
        let status = if s.fetched > 0 { format!("Updated {} · next by {}", clock(s.fetched), clock(s.fetched + 120)) } else { String::new() };
        let status = p.label("refresh.s", &status, json!({ "tone": "muted", "size": 11, "tabular": true, "align": "end", "lineLimit": 1 }));
        let refresh = p.button("refresh", "Refresh", "refresh_prs", json!({}), json!({ "icon": "arrow.clockwise", "style": "plain", "help": "Ask GitHub again now" }));
        page.push(p.boxed("tools", "h", vec!["search".into(), count, status, refresh], json!({ "spacing": 8, "align": "center", "padding": [12, 0, 0, 0], "widths": ["flex", 96, 190, 28], "trailing": 1 })));
        // One list, no group headers; the newest first by default.
        let rows: Vec<Value> = shown.iter().map(|x| row(x, ui, at)).collect();
        let sections = vec![table::section("", rows)];
        let columns: Vec<Value> = vec![table::column("number", "#", json!(60), false, false), table::column("title", "Title", json!("flex"), false, true),
            table::column("summary", "Description", json!("flex"), false, true), table::column("next", "Next", json!(170), false, false),
            table::column("branch", "Branch", json!(200), false, false), table::column("author", "Author", json!(110), false, false),
            table::column("checks", "Checks", json!(84), false, false), table::column("review", "Review", json!(130), false, true),
            table::column("issue", "Issue", json!(70), false, false), table::column("gaps", "Gaps", json!(80), false, false),
            table::column("updated", "Updated", json!(70), false, false)].into_iter().map(table::sortable).collect();
        let mut t = table::component(columns, sections, "select_pr", "select_pr", "", ("updated", false),
            if s.prs.is_empty() { "No open PRs." } else { "Nothing matches the search." });
        t["id"] = json!("table");
        p.put(t);
        page.push(p.boxed("table.b", "v", vec!["table".into()], json!({})));
        // A closed panel is sent as a Modal that is not presented: the client keeps every component it was sent.
        match s.prs.iter().find(|x| Some(x.number) == ui.sel) {
            Some(pr) => {
                let node = ui.detail.clone().unwrap_or(Value::Null);
                detail::modal(&mut p, pr, &node, &ui.agents, &s.buttons, &ui.ch);
            }
            None => detail::closed(&mut p),
        }
        page.push("detail".into());
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
struct Choice {
    /// The chosen value.
    value: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ButtonName {
    /// The button's name, as under `[prs.buttons]` in wheel.toml.
    name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Number {
    /// The PR's number, as the row's id.
    id: String,
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

impl Server {
    /// Run a modal button for the selected PR. `f` does the work outside the lock and answers what to say; the
    /// snapshot is re-read after it and the PR read again, so the modal shows what the button changed.
    fn act(&self, meta: &RequestMetaObject, f: impl FnOnce(&Origin, &Snapshot, &Pr, &Ui) -> Result<String, String>) -> CallToolResult {
        let o = Origin::of(meta);
        let (snap, mut ui) = {
            let mut s = self.shared.lock().unwrap();
            (s.project(&o.dir).clone(), s.panes.entry(o.pane.clone()).or_default().clone())
        };
        let Some(pr) = snap.prs.iter().find(|x| Some(x.number) == ui.sel).cloned() else {
            return CallToolResult::error(vec![ContentBlock::text("no PR is open")]);
        };
        ui.ch.confirm = None;
        let result = f(&o, &snap, &pr, &ui);
        let fresh = collect_with(&o.dir, true);
        if let Some(repo) = &fresh.repo {
            ui.detail = shared_detail(&o.dir, repo, pr.number, true).ok();
        }
        ui.ch.ran = result.as_ref().ok().cloned();
        let mut s = self.shared.lock().unwrap();
        s.projects.insert(o.dir.clone(), fresh.clone());
        s.panes.insert(o.pane.clone(), ui.clone());
        match result {
            Ok(_) => answer(&fresh, &ui),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
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
    fn select_pr(&self, meta: RequestMetaObject, Parameters(Number { id }): Parameters<Number>) -> CallToolResult {
        let o = Origin::of(&meta);
        let n: Option<i64> = id.parse().ok();
        // Answer at once with what the list holds. A PR read in the last 30 seconds opens complete; otherwise the modal
        // opens with a spinner and a thread reads the rest, then asks for a redraw.
        let (repo, load) = {
            let mut s = self.shared.lock().unwrap();
            let repo = s.project(&o.dir).repo.clone();
            let cached = repo.as_deref().zip(n).and_then(|(r, n)| cached_detail(r, n));
            let ui = s.panes.entry(o.pane.clone()).or_default();
            let open = ui.sel != n;
            ui.sel = if open { n } else { None };
            ui.detail = if open { cached.clone() } else { None };
            ui.ch = detail::Choices { method: ui.ch.method.clone(), agent: ui.ch.agent.clone(), open: ui.ch.open.clone(), loading: open && cached.is_none(), ..Default::default() };
            (repo, open && (cached.is_none() || ui.agents.is_empty()))
        };
        if let (true, Some(repo), Some(n)) = (load, repo, n) {
            let (shared, dir, pane) = (self.shared.clone(), o.dir.clone(), o.pane.clone());
            std::thread::spawn(move || {
                let node = shared_detail(&dir, &repo, n, false).ok();
                if let Some(d) = &node {
                    detail::prefetch_images(&dir, d);
                }
                let agents = detail::agent_names(&dir);
                let mut s = shared.lock().unwrap();
                if let Some(ui) = s.panes.get_mut(&pane).filter(|u| u.sel == Some(n)) {
                    if node.is_some() {
                        ui.detail = node;
                    }
                    ui.agents = agents;
                    ui.ch.loading = false;
                }
                s.done.insert(dir);
            });
        }
        self.with(&meta, |_| {})
    }

    #[tool(description = "Ask GitHub again now, for every pane showing this repo", meta = ui_meta())]
    fn refresh_prs(&self, meta: RequestMetaObject) -> CallToolResult {
        let o = Origin::of(&meta);
        let fresh = collect_with(&o.dir, true);
        let sel = self.shared.lock().unwrap().panes.get(&o.pane).and_then(|u| u.sel);
        let node = match (&fresh.repo, sel) {
            (Some(repo), Some(n)) => shared_detail(&o.dir, repo, n, true).ok(),
            _ => None,
        };
        let mut s = self.shared.lock().unwrap();
        s.projects.insert(o.dir.clone(), fresh.clone());
        let ui = s.panes.entry(o.pane).or_default();
        if node.is_some() {
            ui.detail = node;
        }
        answer(&fresh, ui)
    }

    #[tool(description = "Close the open PR's details", meta = ui_meta())]
    fn close_pr(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |ui| {
            ui.sel = None;
            ui.detail = None;
            ui.ch.confirm = None;
        })
    }

    #[tool(description = "Choose how Merge merges", meta = ui_meta())]
    fn set_method(&self, meta: RequestMetaObject, Parameters(Choice { value }): Parameters<Choice>) -> CallToolResult {
        self.with(&meta, |ui| ui.ch.method = value)
    }

    #[tool(description = "Choose where Agent in worktree opens: this space or a new one", meta = ui_meta())]
    fn set_open(&self, meta: RequestMetaObject, Parameters(Choice { value }): Parameters<Choice>) -> CallToolResult {
        self.with(&meta, |ui| ui.ch.open = value)
    }

    #[tool(description = "Choose which agent Worktree + agent opens", meta = ui_meta())]
    fn set_agent(&self, meta: RequestMetaObject, Parameters(Choice { value }): Parameters<Choice>) -> CallToolResult {
        self.with(&meta, |ui| ui.ch.agent = value)
    }

    #[tool(description = "Merge: the first click, which asks to confirm", meta = ui_meta())]
    fn ask_merge(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |ui| {
            ui.ch.confirm = Some("merge".into());
            ui.ch.ran = None;
        })
    }

    #[tool(description = "Close PR: the first click, which asks to confirm", meta = ui_meta())]
    fn ask_close(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |ui| {
            ui.ch.confirm = Some("close".into());
            ui.ch.ran = None;
        })
    }

    #[tool(description = "Merge the open PR with the chosen method", meta = ui_meta())]
    fn merge_pr(&self, meta: RequestMetaObject) -> CallToolResult {
        self.act(&meta, |o, _, pr, ui| {
            detail::merge(&o.dir, pr.number, &ui.ch.method)?;
            Ok(format!("Merged #{} ({}).", pr.number, ui.ch.method))
        })
    }

    #[tool(description = "Close the open PR without merging", meta = ui_meta())]
    fn close_pr_now(&self, meta: RequestMetaObject) -> CallToolResult {
        self.act(&meta, |o, _, pr, _| {
            detail::close(&o.dir, pr.number)?;
            Ok(format!("Closed #{}.", pr.number))
        })
    }

    #[tool(description = "Put the open PR into draft, or take it out of draft", meta = ui_meta())]
    fn toggle_draft(&self, meta: RequestMetaObject) -> CallToolResult {
        self.act(&meta, |o, _, pr, ui| {
            let was = ui.detail.as_ref().map_or(pr.draft, |d| d["isDraft"] == true);
            detail::set_draft(&o.dir, pr.number, !was)?;
            Ok(if was { format!("#{} is ready for review.", pr.number) } else { format!("#{} is a draft.", pr.number) })
        })
    }

    #[tool(description = "Switch the project's checkout to the open PR's branch and open a terminal pane in it", meta = ui_meta())]
    fn checkout_pr(&self, meta: RequestMetaObject) -> CallToolResult {
        self.act(&meta, |o, _, pr, _| {
            detail::checkout(&o.dir, pr.number)?;
            let pane = detail::terminal(&o.dir, &o.pane)?;
            Ok(format!("Checked out {} here; terminal {pane} opened.", pr.branch))
        })
    }

    #[tool(description = "Open the open PR's branch in a worktree with an agent in a new pane", meta = ui_meta())]
    fn worktree_pr(&self, meta: RequestMetaObject) -> CallToolResult {
        self.act(&meta, |o, _, pr, ui| {
            let cross = ui.detail.as_ref().map_or(false, |d| d["isCrossRepository"] == true);
            let wt = detail::worktree(&o.dir, pr.number, &pr.branch, cross)?;
            let pane = detail::place_agent(o, pr, &wt, &ui.ch.agent, &ui.ch.open)?;
            Ok(format!("{} is in {wt}; agent pane {pane} started.", pr.branch))
        })
    }

    #[tool(description = "Run one of the project's PR buttons", meta = ui_meta())]
    fn run_button(&self, meta: RequestMetaObject, Parameters(ButtonName { name }): Parameters<ButtonName>) -> CallToolResult {
        self.act(&meta, |o, snap, pr, ui| {
            let b = snap.buttons.iter().find(|b| b.name == name).ok_or_else(|| format!("no button {name}"))?;
            let cross = ui.detail.as_ref().map_or(false, |d| d["isCrossRepository"] == true);
            detail::run_button(o, snap.repo.as_deref().unwrap_or(""), pr, cross, b)?;
            Ok(format!("{} started for #{}. See the new pane.", detail::label(b), pr.number))
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

    fn finished(&self, dir: &str) -> bool {
        self.server.shared.lock().unwrap().done.remove(dir)
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
            files: 1, commits: 1, labels: vec![], reviewers: vec![], next: String::new(), summary: String::new(), issues: vec![] }
    }

    #[test]
    fn the_page_groups_prs_by_bucket_and_opens_one() {
        let s = Snapshot { name: "t".into(), repo: Some("o/t".into()), me: Some("me".into()), prs: vec![pr(1, "mine"), pr(2, "needs-me")], error: None, fetched: 0, buttons: vec![] };
        let c = components(&s, &Ui { sel: Some(2), ..Default::default() });
        let t = c.iter().find(|v| v["id"] == "table").unwrap();
        assert_eq!(t["sections"].as_array().unwrap().len(), 1, "one list, no group headers");
        assert_eq!(t["sections"][0]["count"], 2);
        assert!(c.iter().any(|v| v["id"] == "pr.title" && v["text"] == "#2 pr 2"));
        let modal = c.iter().find(|v| v["id"] == "detail").unwrap();
        assert_eq!(modal["presented"], true);
        let acts: Vec<&str> = modal["actions"].as_array().unwrap().iter().filter_map(|a| a.as_str()).collect();
        assert!(acts.contains(&"pr.checkout") && acts.contains(&"pr.worktree") && acts.contains(&"pr.merge"));
        let c = components(&s, &Ui::default());
        assert_eq!(c.iter().find(|v| v["id"] == "detail").unwrap()["presented"], false);
    }
}
