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
use crate::core::list::{self, Filter, Item, List, Spec};
use crate::core::page::{self, ago, cell, now, Page};
use crate::core::project::Origin;
use crate::core::table;
use crate::view::{View, MIME};

pub const NAME: &str = "repo";
const URI: &str = "a2ui://repo";
const SURFACE: &str = "repo";

struct Ui {
    /// The search, filters, sort and selection, as every list page holds them. The History tab takes only the search.
    list: List,
    /// "history" shows stashes and commits; anything else shows the branches.
    tab: String,
    sel: Option<String>,
    notice: Option<(bool, String)>,
}

impl Default for Ui {
    fn default() -> Self {
        Ui { list: List::new("state", true), tab: String::new(), sel: None, notice: None }
    }
}

/// What the branch list narrows by.
const SPEC: Spec = Spec {
    filters: &[
        Filter { key: "state", name: "state", hidden: None },
        Filter { key: "has", name: "has", hidden: None },
        Filter { key: "checks", name: "checks", hidden: None },
    ],
    sorts: &["branch", "worktree", "vs", "pr", "ci", "state", "updated"],
};

fn ci(b: &Branch) -> &'static str {
    match b.pr.as_ref().and_then(|p| p.ci) {
        Some("pass") => "passing",
        Some("fail") => "failing",
        Some(_) => "pending",
        None => "none",
    }
}

fn changed(b: &Branch) -> bool {
    b.worktree.as_ref().is_some_and(|w| w.status.staged + w.status.unstaged + w.status.untracked + w.status.conflicted > 0)
}

impl Item for Branch {
    fn haystack(&self) -> String {
        format!("{} {} {}", self.branch, self.subject, self.pr.as_ref().map(|p| p.title.as_str()).unwrap_or(""))
    }
    fn has(&self, key: &str, value: &str) -> bool {
        match (key, value) {
            ("state", v) => kinds(self) == v,
            ("has", "worktree") => self.worktree.is_some(),
            ("has", "pr") => self.pr.is_some(),
            ("has", "changes") => changed(self),
            ("has", "local-only") => self.local_only > 0,
            ("checks", v) => ci(self) == v,
            _ => false,
        }
    }
}

fn compare(key: &str, a: &Branch, b: &Branch) -> std::cmp::Ordering {
    let wt = |x: &Branch| x.worktree.as_ref().map(|w| w.path.rsplit('/').next().unwrap_or(&w.path).to_lowercase()).unwrap_or_default();
    let rank = |c: &str| match c { "failing" => 0, "pending" => 1, "passing" => 2, _ => 3 };
    match key {
        "branch" => a.branch.to_lowercase().cmp(&b.branch.to_lowercase()),
        "worktree" => wt(a).cmp(&wt(b)),
        "vs" => a.ahead.unwrap_or(-1).cmp(&b.ahead.unwrap_or(-1)),
        "pr" => a.pr.as_ref().map_or(0, |p| p.number).cmp(&b.pr.as_ref().map_or(0, |p| p.number)),
        "ci" => rank(ci(a)).cmp(&rank(ci(b))),
        "state" => group(a).cmp(&group(b)),
        _ => a.ts.cmp(&b.ts),
    }
}

/// Safe-to-delete first, then blocked, then live.
fn group(b: &Branch) -> usize {
    match kinds(b) { "dead" => 0, "blocked" => 1, _ => 2 }
}

/// The branches the page lists now, in the order shown; what the sort leaves equal goes newest first, then by name.
fn listed<'a>(s: &'a Snapshot, ui: &Ui) -> Vec<&'a Branch> {
    ui.list.sorted(&SPEC, &s.branches, compare, |a, b| b.ts.cmp(&a.ts).then(a.branch.cmp(&b.branch)))
}

/// The branch names the list actions take: the selected listed branches, else every listed one.
fn ids(s: &Snapshot, ui: &Ui) -> Vec<String> {
    list::taken(listed(s, ui).iter().map(|b| b.branch.clone()).collect(), &ui.list)
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
    let ci = match ci(b) {
        "passing" => cell("passing", json!({ "chip": true, "tone": "ok", "sort": 2 })),
        "failing" => cell("failing", json!({ "chip": true, "tone": "bad", "sort": 0 })),
        "pending" => cell("pending", json!({ "chip": true, "tone": "warn", "sort": 1 })),
        _ => cell("", json!({ "sort": 3 })),
    };
    let state = match (b.dead, &b.blocked) {
        (Some(why), None) => cell(why, json!({ "chip": true, "tone": "ok" })),
        (Some(_), Some(why)) => cell(why, json!({ "tone": "warn" })),
        _ => cell(if b.local_only > 0 { format!("{} local-only", b.local_only) } else { String::new() }, json!({ "tone": "muted" })),
    };
    let wt_sort = b.worktree.as_ref().map(|w| w.path.rsplit('/').next().unwrap_or(&w.path).to_lowercase()).unwrap_or_default();
    let cells = vec![
        cell(&b.branch, json!({ "key": "branch", "mono": true, "sort": b.branch.to_lowercase() })),
        cell(wt.unwrap_or_default(), json!({ "key": "worktree", "tone": "muted", "sort": wt_sort })),
        cell(vs, json!({ "key": "vs", "mono": true, "tone": "muted", "sort": b.ahead.unwrap_or(-1) })),
        cell(pr, json!({ "key": "pr", "mono": true, "sort": b.pr.as_ref().map(|p| p.number).unwrap_or(0) })),
        { let mut c = ci; c["key"] = json!("ci"); c },
        { let mut c = state; c["key"] = json!("state"); c },
        cell(ago(b.ts, at), json!({ "key": "updated", "tone": "muted", "sort": b.ts })),
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
    // The boxes set one filter each: a tap picks it, a tap on the chosen one clears it.
    let n = |key: &str, v: &str| s.branches.iter().filter(|b| b.has(key, v)).count();
    page.push(list::boxes(&mut p, &ui.list, &[("Safe to delete", "state", "dead", n("state", "dead"), true), ("Blocked", "state", "blocked", n("state", "blocked"), false),
        ("Worktrees", "has", "worktree", n("has", "worktree"), false), ("With a PR", "has", "pr", n("has", "pr"), false)]));
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
        page.push(list::find(&mut p, &ui.list, vec!["tab".into()]));
        let q = ui.list.q.to_lowercase();
        let words = |hay: String| { let hay = hay.to_lowercase(); q.split_whitespace().all(|w| hay.contains(w)) };
        let stashes: Vec<&(String, String, String)> = s.stashes.iter().filter(|(r, subj, _)| words(format!("{r} {subj}"))).collect();
        let commits: Vec<&Commit> = s.commits.iter().filter(|c| words(format!("{} {} {}", c.sha, c.subject, c.author))).collect();
        let shown = listed(s, ui);
        let options = vec![
            list::fixed("state", &s.branches, &[("dead", "safe to delete"), ("blocked", "blocked"), ("live", "live")]),
            list::fixed("has", &s.branches, &[("worktree", "worktree"), ("pr", "PR"), ("changes", "uncommitted changes"), ("local-only", "local-only commits")]),
            list::fixed("checks", &s.branches, &[("failing", "failing"), ("pending", "pending"), ("passing", "passing"), ("none", "none")]),
        ];
        let actions = crate::core::buttons::actions(&mut p, &s.scripts, &ids(s, ui), shown.len(), "branch names", "branches");
        page.push(list::bar(&mut p, &SPEC, &ui.list, options, actions));
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
            let columns: Vec<Value> = vec![
                table::column("ref", "Commit", json!(90), false, false), table::column("subject", "Subject", json!("flex"), false, true),
                table::column("author", "Author", json!(140), false, true), table::column("when", "When", json!(70), false, false),
            ].into_iter()
                // A stash's age is only text ("3 days ago"), so the When column keeps the page's order.
                .map(|c| if c["key"] == "when" { c } else { table::sortable(c) }).collect();
            table::component(columns, sections, "select_branch", "select_branch", "", ("when", false),
                if s.stashes.is_empty() && s.commits.is_empty() { "No stashes or commits." } else { "Nothing matches the search." })
        } else {
            let sections = vec![table::section("", shown.iter().map(|b| row(b, ui, at)).collect())];
            let columns: Vec<Value> = vec![
                table::column("branch", "Branch", json!("flex"), false, true), table::column("worktree", "Worktree", json!(180), false, true),
                table::column("vs", format!("vs {}", s.default.as_deref().unwrap_or("default")).as_str(), json!(90), false, false),
                table::column("pr", "PR", json!(60), false, false), table::column("ci", "Checks", json!(84), false, false),
                table::column("state", "State", json!(150), false, true), table::column("updated", "Updated", json!(70), false, false),
            ].into_iter().map(table::sortable).collect();
            let t = table::component(columns, sections, "select_branch", "select_branch", "set_sort", (&ui.list.sort.0, ui.list.sort.1),
                if s.branches.is_empty() { "No branches besides the default." } else { "Nothing matches the search and filters." });
            table::selectable(t, "select_rows", &ui.list.selected.iter().cloned().collect::<Vec<_>>())
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
struct ListScript {
    /// The button's name in `[repo.list.<name>]`.
    script: String,
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

    #[tool(description = "The repo page", meta = ui_meta())]
    fn get_repo(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, _| {})
    }

    #[tool(description = "Show the branches or the history", meta = ui_meta())]
    fn set_tab(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.tab = value)
    }

    #[tool(description = "Run one of the project's list buttons over the branches the page lists now", meta = ui_meta())]
    fn run_list_script(&self, meta: RequestMetaObject, Parameters(ListScript { script }): Parameters<ListScript>) -> CallToolResult {
        let o = Origin::of(&meta);
        let (snap, names) = {
            let mut s = self.shared.lock().unwrap();
            let snap = s.project(&o.dir).clone();
            let ui = s.panes.entry(o.pane.clone()).or_default();
            let names = ids(&snap, ui);
            (snap, names)
        };
        let mut env = vec![("REPO_PATH", snap.top.as_str())];
        if let Some(d) = &o.data {
            env.push(("WHEEL_DATA_DIR", d.as_str()));
        }
        match crate::core::buttons::run_list(&snap.scripts, &script, &names, "BRANCHES", &env, &o.dir, &o.pane, "branches") {
            Ok(said) => self.with(&meta, |_, ui| ui.notice = Some((true, said))),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
    }

    #[tool(description = "Open a branch's row (a double-click or Return)", meta = ui_meta())]
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
        let mut t = self.server.tool_router.list_all();
        t.extend(list::tools(&ui_meta()));
        t
    }

    async fn call(&self, params: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        if let Some(op) = list::op(&params) {
            return Ok(self.server.with(&context.meta, |_, ui| ui.list.apply(&SPEC, op)).into());
        }
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
        let page = c.iter().find(|v| v["id"] == "page").unwrap()["children"].clone();
        assert_eq!(page, json!(["kpis", "find", "bar", "table.b"]), "the History tab keeps every control where the Branches tab has it");
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
