//! The Repo view: branches and worktrees of the opened space's project, and deleting a dead one.
//!
//! Each subscribed project keeps one snapshot, re-read when its git state moves (HEAD's log, the
//! refs, the worktree list) or two minutes pass (GitHub is asked then too).

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

pub const NAME: &str = "repo";
const URI: &str = "a2ui://repo";
const SURFACE: &str = "repo";

#[derive(Default)]
struct Ui {
    q: String,
    /// "history" shows stashes and commits; anything else shows the branches.
    tab: String,
    /// The column the branches are ordered by; empty is `state`, which groups safe-to-delete, blocked and live.
    sort: String,
    show: String,
    sel: Option<String>,
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

fn kinds(b: &Branch) -> &'static str {
    match (&b.dead, &b.blocked) {
        (Some(_), None) => "dead",
        (Some(_), Some(_)) => "blocked",
        _ => "live",
    }
}

fn matches(b: &Branch, ui: &Ui) -> bool {
    let q = ui.q.to_lowercase();
    let hay = format!("{} {} {}", b.branch, b.subject, b.pr.as_ref().map(|p| p.title.as_str()).unwrap_or("")).to_lowercase();
    q.split_whitespace().all(|w| hay.contains(w))
        && match ui.show.as_str() {
            "dead" => b.dead.is_some(),
            "worktrees" => b.worktree.is_some(),
            "prs" => b.pr.is_some(),
            _ => true,
        }
}

/// The order of two branches under `sort`; ties fall back to the newest first, then the name.
fn order(a: &Branch, b: &Branch, sort: &str) -> std::cmp::Ordering {
    let group = |x: &Branch| match kinds(x) { "dead" => 0, "blocked" => 1, _ => 2 };
    let ci = |x: &Branch| match x.pr.as_ref().and_then(|p| p.ci) { Some("fail") => 0, Some("pass") => 2, Some(_) => 1, None => 3 };
    let wt = |x: &Branch| x.worktree.as_ref().map(|w| w.path.rsplit('/').next().unwrap_or(&w.path).to_string());
    let first = match sort {
        "branch" => a.branch.cmp(&b.branch),
        "worktree" => match (wt(a), wt(b)) { (Some(x), Some(y)) => x.cmp(&y), (Some(_), None) => std::cmp::Ordering::Less, (None, Some(_)) => std::cmp::Ordering::Greater, _ => std::cmp::Ordering::Equal },
        "pr" => match (a.pr.as_ref().map(|p| p.number), b.pr.as_ref().map(|p| p.number)) { (Some(x), Some(y)) => y.cmp(&x), (Some(_), None) => std::cmp::Ordering::Less, (None, Some(_)) => std::cmp::Ordering::Greater, _ => std::cmp::Ordering::Equal },
        "checks" => ci(a).cmp(&ci(b)),
        "updated" => std::cmp::Ordering::Equal,
        _ => group(a).cmp(&group(b)),
    };
    first.then(b.ts.cmp(&a.ts)).then(a.branch.cmp(&b.branch))
}

fn row(b: &Branch, ui: &Ui, at: i64) -> Value {
    let wt = b.worktree.as_ref().map(|w| {
        let name = w.path.rsplit('/').next().unwrap_or(&w.path).to_string();
        if w.status.staged + w.status.unstaged + w.status.untracked + w.status.conflicted > 0 { format!("{name} (changes)") } else { name }
    });
    let vs = match (b.ahead, b.behind) {
        (Some(a), Some(c)) => format!("+{a} -{c}"),
        _ => String::new(),
    };
    let pr = b.pr.as_ref().map(|p| format!("#{}", p.number)).unwrap_or_default();
    let ci = match b.pr.as_ref().and_then(|p| p.ci) {
        Some("pass") => cell("passing", json!({ "chip": true, "tone": "ok" })),
        Some("fail") => cell("failing", json!({ "chip": true, "tone": "bad" })),
        Some(_) => cell("pending", json!({ "chip": true, "tone": "warn" })),
        None => cell("", json!({})),
    };
    let state = match (b.dead, &b.blocked) {
        (Some(why), None) => cell(why, json!({ "chip": true, "tone": "ok" })),
        (Some(_), Some(why)) => cell(why, json!({ "tone": "warn" })),
        _ => cell(if b.local_only > 0 { format!("{} local-only", b.local_only) } else { String::new() }, json!({ "tone": "muted" })),
    };
    let cells = vec![
        cell(&b.branch, json!({ "key": "branch", "mono": true })),
        cell(wt.unwrap_or_default(), json!({ "key": "worktree", "tone": "muted" })),
        cell(vs, json!({ "key": "vs", "mono": true, "tone": "muted" })),
        cell(pr, json!({ "key": "pr", "mono": true })),
        { let mut c = ci; c["key"] = json!("ci"); c },
        { let mut c = state; c["key"] = json!("state"); c },
        cell(ago(b.ts, at), json!({ "key": "updated", "tone": "muted" })),
    ];
    let menu = if kinds(b) == "dead" {
        vec![json!({ "label": "Delete branch", "action": "delete_branch", "context": { "branch": b.branch } })]
    } else {
        vec![]
    };
    json!({ "id": b.branch, "path": b.branch, "depth": 0, "kids": 0, "collapsed": false, "dim": false,
        "sel": ui.sel.as_deref() == Some(b.branch.as_str()), "cells": cells, "menu": menu })
}

fn components(s: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let at = now();
    let mut page = vec![];
    // No page title or subtitle: the pane's own bar says which page it is.
    let n = |f: &dyn Fn(&Branch) -> bool| s.branches.iter().filter(|b| f(b)).count();
    let kpis = [("Branches", s.branches.len(), "fg"), ("Safe to delete", n(&|b| kinds(b) == "dead"), "ok"),
        ("Worktrees", n(&|b| b.worktree.is_some()), "fg"), ("Open PRs", n(&|b| b.pr.is_some()), "fg")];
    let cards: Vec<String> = kpis.iter().enumerate().map(|(i, (l, v, tone))| {
        p.stat(&format!("kpi{i}"), &v.to_string(), &l.to_string(), if *v == 0 { "muted" } else { tone })
    }).collect();
    page.push(p.boxed("kpis", "h", cards, json!({ "spacing": 10, "widths": ["flex", "flex", "flex", "flex"] })));
    for (i, e) in s.error.iter().chain(s.pr_error.iter()).enumerate() {
        let l = p.label(&format!("problem{i}.t"), e, json!({ "tone": "bad" }));
        page.push(p.boxed(&format!("problem{i}"), "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if let Some((ok, text)) = &ui.notice {
        let l = p.label("notice.t", text, json!({ "tone": if *ok { "ok" } else { "bad" } }));
        page.push(p.boxed("notice", "v", vec![l], json!({ "border": if *ok { "ok" } else { "bad" }, "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if s.error.is_none() {
        let history = ui.tab == "history";
        p.choose("tab", "/set_tab", if history { "history" } else { "branches" },
            vec![json!({ "value": "branches", "text": "Branches" }), json!({ "value": "history", "text": "History" })], true, false);
        p.search("search", "set_query", &ui.q, "Search");
        // The views choice keeps its place on the History tab, disabled, so no control moves between tabs.
        p.choose("views", "/set_show", if ui.show.is_empty() { "all" } else { &ui.show }, vec![
            json!({ "value": "all", "text": "All" }), json!({ "value": "dead", "text": "Safe to delete" }),
            json!({ "value": "worktrees", "text": "Worktrees" }), json!({ "value": "prs", "text": "With a PR" })], true, history);
        let sort = if ui.sort.is_empty() { "state" } else { ui.sort.as_str() };
        p.choose("sort", "/set_sort", sort, vec![
            json!({ "value": "state", "text": "Sort: State" }), json!({ "value": "updated", "text": "Sort: Updated" }),
            json!({ "value": "branch", "text": "Sort: Branch" }), json!({ "value": "worktree", "text": "Sort: Worktree" }),
            json!({ "value": "pr", "text": "Sort: PR" }), json!({ "value": "checks", "text": "Sort: Checks" })], false, history);
        let q = ui.q.to_lowercase();
        let stashes: Vec<&(String, String, String)> = s.stashes.iter().filter(|(r, subj, _)| q.is_empty() || format!("{r} {subj}").to_lowercase().contains(&q)).collect();
        let commits: Vec<&Commit> = s.commits.iter().filter(|c| q.is_empty() || format!("{} {} {}", c.sha, c.subject, c.author).to_lowercase().contains(&q)).collect();
        let mut shown: Vec<&Branch> = s.branches.iter().filter(|b| matches(b, ui)).collect();
        shown.sort_by(|a, b| order(a, b, sort));
        let count_text = if history { format!("{} of {}", stashes.len() + commits.len(), s.stashes.len() + s.commits.len()) } else { format!("{} of {}", shown.len(), s.branches.len()) };
        let count = p.label("count", &count_text, json!({ "tone": "muted", "size": 11, "tabular": true, "align": "end" }));
        page.push(p.boxed("tools", "h", vec!["tab".into(), "search".into(), "views".into(), "sort".into(), count],
            json!({ "spacing": 8, "align": "center", "padding": [12, 0, 0, 0], "widths": [null, "flex", null, null, 96], "trailing": 1 })));
        let mut t = if history {
            let default = s.default.as_deref().unwrap_or("the default branch");
            let stash_rows: Vec<Value> = stashes.iter().map(|(r, subj, when)| json!({ "id": r, "path": r, "depth": 0, "kids": 0, "collapsed": false, "dim": false,
                "sel": ui.sel.as_deref() == Some(r.as_str()), "menu": [], "cells": [
                    cell(r.as_str(), json!({ "key": "ref", "mono": true })), cell(subj.as_str(), json!({ "key": "subject" })),
                    cell("", json!({ "key": "author" })), cell(when.as_str(), json!({ "key": "when", "tone": "muted" }))] })).collect();
            let commit_rows: Vec<Value> = commits.iter().map(|c| json!({ "id": c.sha, "path": c.sha, "depth": 0, "kids": 0, "collapsed": false, "dim": false,
                "sel": ui.sel.as_deref() == Some(c.sha.as_str()), "menu": [], "cells": [
                    cell(c.sha.as_str(), json!({ "key": "ref", "mono": true })), cell(c.subject.as_str(), json!({ "key": "subject" })),
                    cell(c.author.as_str(), json!({ "key": "author", "tone": "muted" })), cell(ago(c.ts, at), json!({ "key": "when", "tone": "muted" }))] })).collect();
            let sections: Vec<Value> = [
                (!stash_rows.is_empty()).then(|| table::section("Stashes", stash_rows)),
                (!commit_rows.is_empty()).then(|| table::section(&format!("Commits on {default}"), commit_rows)),
            ].into_iter().flatten().collect();
            let columns = vec![
                table::column("ref", "Commit", json!(90), false, false), table::column("subject", "Subject", json!("flex"), false, true),
                table::column("author", "Author", json!(140), false, true), table::column("when", "When", json!(70), false, false),
            ];
            table::component(columns, sections, "select_branch", "select_branch", ("when", false),
                if s.stashes.is_empty() && s.commits.is_empty() { "No stashes or commits." } else { "Nothing matches the search." })
        } else {
            let sections = vec![table::section("", shown.iter().map(|b| row(b, ui, at)).collect())];
            let columns = vec![
                table::column("branch", "Branch", json!("flex"), false, true), table::column("worktree", "Worktree", json!(180), false, true),
                table::column("vs", format!("vs {}", s.default.as_deref().unwrap_or("default")).as_str(), json!(90), false, false),
                table::column("pr", "PR", json!(60), false, false), table::column("ci", "Checks", json!(84), false, false),
                table::column("state", "State", json!(150), false, true), table::column("updated", "Updated", json!(70), false, false),
            ];
            table::component(columns, sections, "select_branch", "select_branch", ("updated", false),
                if s.branches.is_empty() { "No branches besides the default." } else { "Nothing matches the search." })
        };
        t["id"] = json!("table");
        p.put(t);
        page.push(p.boxed("table.b", "v", vec!["table".into()], json!({})));
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
struct Query {
    /// The search text.
    #[serde(alias = "value")]
    q: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct BranchName {
    /// The local branch.
    branch: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RowId {
    /// The picked row's id, which is its branch.
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

    #[tool(description = "The repo page", meta = ui_meta())]
    fn get_repo(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, _| {})
    }

    #[tool(description = "Search the branches", meta = ui_meta())]
    fn set_query(&self, meta: RequestMetaObject, Parameters(Query { q }): Parameters<Query>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.q = q)
    }

    #[tool(description = "Show the branches or the history", meta = ui_meta())]
    fn set_tab(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.tab = value)
    }

    #[tool(description = "Order the branches by a column", meta = ui_meta())]
    fn set_sort(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.sort = value)
    }

    #[tool(description = "Show all, dead, worktrees or prs", meta = ui_meta())]
    fn set_show(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.show = if value == "all" { String::new() } else { value })
    }

    #[tool(description = "Select a branch's row", meta = ui_meta())]
    fn select_branch(&self, meta: RequestMetaObject, Parameters(RowId { id: branch }): Parameters<RowId>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.sel = if ui.sel.as_deref() == Some(&branch) { None } else { Some(branch) })
    }

    #[tool(description = "Delete a dead branch, and its worktree first when it has one")]
    fn delete_branch(&self, meta: RequestMetaObject, Parameters(BranchName { branch }): Parameters<BranchName>) -> CallToolResult {
        let o = Origin::of(&meta);
        let result = delete(&o.dir, &branch);
        let mut s = self.shared.lock().unwrap();
        s.projects.insert(o.dir.clone(), collect(&o.dir));
        let snap = s.project(&o.dir).clone();
        let ui = s.panes.entry(o.pane).or_default();
        ui.notice = Some(match result {
            Ok(did) => (true, did.join("; ")),
            Err(e) => (false, format!("{branch}: {e}")),
        });
        answer(&snap, ui)
    }
}

/// The Repo view.
pub struct RepoView {
    server: Server,
}

impl RepoView {
    pub fn new() -> Self {
        RepoView { server: Server::new(Arc::new(Mutex::new(Shared::default()))) }
    }
}

#[async_trait]
impl View for RepoView {
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
        vec![own.join("logs/HEAD"), own.join("HEAD"), common.join("packed-refs"), common.join("refs/heads"),
            common.join("refs/remotes/origin"), common.join("worktrees"), common.join("refs/stash")]
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
    fn a_dead_branch_offers_delete_and_a_live_one_does_not() {
        let b = |dead| Branch { branch: "x".into(), ts: 0, subject: String::new(), author: String::new(), ahead: Some(0), behind: Some(0),
            upstream: None, upstream_gone: false, remote: None, unpushed: None, worktree: None, pr: None, pr_merged: false,
            local_only: 0, dead, blocked: None };
        let ui = Ui::default();
        assert_eq!(row(&b(Some("merged")), &ui, 0)["menu"][0]["action"], "delete_branch");
        assert_eq!(row(&b(None), &ui, 0)["menu"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn the_history_tab_lists_stashes_and_commits_in_a_table() {
        let mut s = Snapshot { name: "t".into(), default: Some("origin/main".into()), ..Default::default() };
        s.stashes.push(("stash@{0}".into(), "On main: wip".into(), "2h ago".into()));
        let ui = Ui { tab: "history".into(), ..Default::default() };
        let c = components(&s, &ui);
        let table = c.iter().find(|v| v["id"] == "table").unwrap();
        assert_eq!(table["sections"][0]["title"], "Stashes");
        assert_eq!(table["sections"][0]["rows"][0]["cells"][1]["text"], "On main: wip");
        assert!(c.iter().any(|v| v["id"] == "views.seg" && v["modifiers"][0]["disabled"] == true));
    }

    #[test]
    fn the_page_lists_its_branches_in_sections() {
        let mut s = Snapshot { name: "t".into(), default: Some("origin/main".into()), ..Default::default() };
        s.branches.push(Branch { branch: "feat".into(), ts: 0, subject: "s".into(), author: "a".into(), ahead: Some(2), behind: Some(0),
            upstream: None, upstream_gone: false, remote: None, unpushed: None, worktree: None, pr: None, pr_merged: false,
            local_only: 2, dead: None, blocked: None });
        let c = components(&s, &Ui::default());
        let table = c.iter().find(|v| v["id"] == "table").unwrap();
        assert_eq!(table["sections"][0]["title"], "");
        assert_eq!(table["sections"][0]["rows"][0]["cells"][0]["text"], "feat");
    }
}
