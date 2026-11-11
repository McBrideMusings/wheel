//! The Issues view: the project's GitHub issues, and opening and closing one from the page.
//!
//! Each pane keeps its search, filter, open issue and new-issue draft; each subscribed project keeps
//! one snapshot, re-read every two minutes and after each write.

use std::collections::{BTreeSet, HashMap};
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

pub const NAME: &str = "issues";
const URI: &str = "a2ui://issues";
const SURFACE: &str = "issues";

struct Ui {
    q: String,
    show: String,
    sel: Option<i64>,
    /// The rows the person selected (a click, a shift-click range, a ⌘-click); the list actions take them
    /// when any of them is listed, else the whole narrowed list.
    selected: BTreeSet<String>,
    title: String,
    notice: Option<(bool, String)>,
    /// The column the table is sorted by, and whether ascending; a header click sets it.
    sort: (String, bool),
}

impl Default for Ui {
    fn default() -> Self {
        Ui { q: String::new(), show: "open".into(), sel: None, selected: BTreeSet::new(), title: String::new(), notice: None, sort: ("updated".into(), false) }
    }
}

const COLS: [&str; 7] = ["number", "title", "state", "labels", "who", "author", "updated"];

fn compare(key: &str, a: &Issue, b: &Issue) -> std::cmp::Ordering {
    match key {
        "number" => a.number.cmp(&b.number),
        "title" => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        "state" => a.state.cmp(&b.state),
        "labels" => a.labels.join(",").to_lowercase().cmp(&b.labels.join(",").to_lowercase()),
        "who" => a.assignees.join(",").to_lowercase().cmp(&b.assignees.join(",").to_lowercase()),
        "author" => a.author.to_lowercase().cmp(&b.author.to_lowercase()),
        _ => a.updated.cmp(&b.updated),
    }
}

/// The issues the page lists now, in the order shown: what the search, the state choice and the sort leave.
fn listed<'a>(s: &'a Snapshot, ui: &Ui) -> Vec<&'a Issue> {
    let mut rows: Vec<&Issue> = s.issues.iter().filter(|i| shown(i, ui)).collect();
    let (key, asc) = (&ui.sort.0, ui.sort.1);
    rows.sort_by(|a, b| {
        let o = compare(key, a, b).then(a.number.cmp(&b.number));
        if asc { o } else { o.reverse() }
    });
    rows
}

/// The numbers the list actions (Copy IDs, the project's list buttons) take, as bare numbers: the
/// selected rows that are listed now, in the order shown, or every listed row when none of the
/// selection is listed.
fn ids(s: &Snapshot, ui: &Ui) -> Vec<String> {
    table::taken(listed(s, ui).iter().map(|i| i.number.to_string()).collect(), &ui.selected)
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

fn shown(i: &Issue, ui: &Ui) -> bool {
    let hay = format!("#{} {} {} {} {}", i.number, i.title, i.labels.join(" "), i.assignees.join(" "), i.author).to_lowercase();
    ui.q.to_lowercase().split_whitespace().all(|w| hay.contains(w))
        && match ui.show.as_str() {
            "closed" => i.state == "closed",
            "all" => true,
            _ => i.state == "open",
        }
}

fn row(i: &Issue, ui: &Ui, at: i64) -> Value {
    let cells = vec![
        cell(format!("#{}", i.number), json!({ "key": "number", "mono": true, "tone": "muted", "sort": i.number })),
        cell(&i.title, json!({ "key": "title", "sort": i.title.to_lowercase() })),
        cell(&i.state, json!({ "key": "state", "chip": true, "tone": if i.state == "open" { "ok" } else { "muted" } })),
        cell(i.labels.join(", "), json!({ "key": "labels", "tone": "muted", "size": 12, "sort": i.labels.join(",").to_lowercase() })),
        cell(i.assignees.join(", "), json!({ "key": "who", "tone": "muted", "sort": i.assignees.join(",").to_lowercase() })),
        cell(&i.author, json!({ "key": "author", "tone": "muted", "sort": i.author.to_lowercase() })),
        cell(ago(i.updated, at), json!({ "key": "updated", "tone": "muted", "sort": i.updated })),
    ];
    json!({ "id": i.number.to_string(), "path": i.number.to_string(), "depth": 0, "kids": 0, "collapsed": false, "dim": i.state == "closed",
        "sel": ui.sel == Some(i.number), "cells": cells, "menu": [] })
}

fn detail(p: &mut Page, i: &Issue) -> String {
    let mut c = vec![p.label("d.title", &i.title, json!({ "size": 17, "weight": "semibold" }))];
    let fact = |p: &mut Page, n: &str, name: &str, value: String| -> String {
        let l = p.label(&format!("d.f.{n}.k"), name, json!({ "tone": "muted", "size": 12 }));
        p.boxed(&format!("d.f.{n}"), "h", vec![l, value], json!({ "spacing": 12, "widths": [84, "flex"], "align": "firstTextBaseline" }))
    };
    let v = p.label("d.f.id.v", &format!("#{}", i.number), json!({ "mono": true, "size": 12 }));
    c.push(fact(p, "id", "Number", v));
    let v = p.label("d.f.st.v", &i.state, json!({ "chip": true, "tone": if i.state == "open" { "ok" } else { "muted" } }));
    c.push(fact(p, "st", "State", v));
    let v = p.label("d.f.au.v", &i.author, json!({}));
    c.push(fact(p, "au", "Author", v));
    if !i.labels.is_empty() {
        let v = p.label("d.f.lb.v", &i.labels.join("  ·  "), json!({}));
        c.push(fact(p, "lb", "Labels", v));
    }
    if !i.assignees.is_empty() {
        let v = p.label("d.f.as.v", &i.assignees.join(", "), json!({}));
        c.push(fact(p, "as", "Assignees", v));
    }
    let v = p.label("d.f.url.v", &i.url, json!({ "mono": true, "size": 12 }));
    c.push(fact(p, "url", "URL", v));
    if !i.body.is_empty() {
        let l = p.label("d.desc", &i.body.chars().take(3000).collect::<String>(), json!({ "size": 13 }));
        c.push(p.boxed("d.desc.b", "v", vec![l], json!({ "padding": [10, 12], "fill": "soft", "radius": 8 })));
    }
    if i.state == "open" {
        c.push(p.button("d.close", "Close issue", "close_issue", json!({}), json!({})));
    }
    let body = p.boxed("d.body", "v", c, json!({ "spacing": 12, "align": "start" }));
    p.put(json!({ "id": "detail", "component": "Modal", "presented": true, "dismissAction": "close_detail", "children": [body] }));
    "detail".into()
}

fn components(s: &Snapshot, ui: &Ui) -> Vec<Value> {
    let mut p = Page { out: vec![] };
    let at = now();
    let mut page = vec![];
    let t = p.runs("title", json!([{ "text": format!("Issues · {}", s.name), "size": 20, "weight": "bold" }]), json!({}));
    page.push(p.boxed("head", "h", vec![t], json!({ "padding": [0, 0, 12, 0] })));
    for (n, e) in s.error.iter().chain(s.problems.iter()).enumerate() {
        let l = p.label(&format!("problem{n}.t"), e, json!({ "tone": "bad" }));
        page.push(p.boxed(&format!("problem{n}"), "v", vec![l], json!({ "border": "bad", "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if let Some((ok, text)) = &ui.notice {
        let l = p.label("notice.t", text, json!({ "tone": if *ok { "ok" } else { "bad" } }));
        page.push(p.boxed("notice", "v", vec![l], json!({ "border": if *ok { "ok" } else { "bad" }, "radius": 8, "padding": [8, 12], "margin": [10, 0, 0, 0] })));
    }
    if s.error.is_none() {
        p.search("new.title", "set_draft", &ui.title, "New issue title");
        let create = p.button("new.create", "Create", "create_issue", json!({}), json!({ "disabled": ui.title.trim().is_empty() }));
        page.push(p.boxed("new", "h", vec!["new.title".into(), create],
            json!({ "spacing": 8, "widths": ["flex", null], "padding": [0, 0, 12, 0] })));
        p.search("search", "set_query", &ui.q, "Search");
        p.choose("views", "/set_show", &ui.show, vec![
            json!({ "value": "open", "text": "Open" }), json!({ "value": "closed", "text": "Closed" }),
            json!({ "value": "all", "text": "All" })], true, false);
        let sorted = listed(s, ui);
        // Copy IDs gives bare numbers, the form the selector table and `iterate` take; the buttons are the project's
        // `[issues.list.<name>]` ones.
        let actions = crate::core::buttons::actions(&mut p, &s.scripts, &ids(s, ui), sorted.len(), "issue numbers", "issues");
        page.push(p.boxed("tools", "h", vec!["search".into(), "views".into(), actions],
            json!({ "spacing": 8, "align": "center", "widths": ["flex", null, null] })));
        let columns: Vec<Value> = vec![
            table::column("number", "#", json!(60), false, false), table::column("title", "Title", json!("flex"), false, true),
            table::column("state", "State", json!(80), false, false), table::column("labels", "Labels", json!(160), false, true),
            table::column("who", "Assignee", json!(110), false, true), table::column("author", "Author", json!(110), false, false),
            table::column("updated", "Updated", json!(70), false, false),
        ].into_iter().map(table::sortable).collect();
        let sections = vec![table::section("", sorted.iter().map(|i| row(i, ui, at)).collect())];
        let t = table::component(columns, sections, "select_issue", "select_issue", "set_sort", (&ui.sort.0, ui.sort.1),
            if s.issues.is_empty() { "No issues on GitHub." } else { "Nothing matches the search." });
        let mut t = table::selectable(t, "select_rows", &ui.selected.iter().cloned().collect::<Vec<_>>());
        t["id"] = json!("table");
        p.put(t);
        page.push(p.boxed("table.b", "v", vec!["table".into()], json!({})));
        match ui.sel.and_then(|n| s.issues.iter().find(|i| i.number == n)) {
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
struct Sort {
    /// A column key.
    key: String,
    /// Ascending when true.
    #[serde(deserialize_with = "crate::core::table::asc")]
    asc: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListScript {
    /// The button's name in `[issues.list.<name>]`.
    script: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct IssueId {
    /// The issue's number, as the row's id.
    id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Rows {
    /// The numbers of the rows selected now, every one of them; empty clears the selection.
    #[serde(deserialize_with = "crate::core::table::ids")]
    ids: Vec<String>,
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

    /// Run a write against the project, re-read the issues, and answer with what it said.
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
        self.with(&meta, |_, ui| { ui.q = value; ui.selected.clear(); })
    }

    #[tool(description = "Show open, closed or all issues", meta = ui_meta())]
    fn set_show(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| { ui.show = value; ui.selected.clear(); })
    }

    #[tool(description = "Sort the table by a column, ascending or not", meta = ui_meta())]
    fn set_sort(&self, meta: RequestMetaObject, Parameters(Sort { key, asc }): Parameters<Sort>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if COLS.contains(&key.as_str()) {
                ui.sort = (key, asc);
            }
        })
    }

    #[tool(description = "Run one of the project's list buttons over the issues the page lists now", meta = ui_meta())]
    fn run_list_script(&self, meta: RequestMetaObject, Parameters(ListScript { script }): Parameters<ListScript>) -> CallToolResult {
        let o = Origin::of(&meta);
        let (snap, numbers) = {
            let mut s = self.shared.lock().unwrap();
            let snap = s.project(&o.dir).clone();
            let ui = s.panes.entry(o.pane.clone()).or_default();
            let numbers = ids(&snap, ui);
            (snap, numbers)
        };
        let mut env = vec![("ISSUE_REPO", snap.name.as_str()), ("ISSUE_PATH", o.dir.as_str())];
        if let Some(d) = &o.data {
            env.push(("WHEEL_DATA_DIR", d.as_str()));
        }
        let result = crate::core::buttons::run_list(&snap.scripts, &script, &numbers, "ISSUE_NUMBERS", &env, &o.dir, &o.pane, "issues");
        match result {
            Ok(said) => self.with(&meta, |_, ui| ui.notice = Some((true, said))),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
    }

    #[tool(description = "Select rows of the list: the list actions take them instead of the whole list", meta = ui_meta())]
    fn select_rows(&self, meta: RequestMetaObject, Parameters(Rows { ids }): Parameters<Rows>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.selected = ids.into_iter().collect())
    }

    #[tool(description = "Open an issue's panel (a double-click or Return on its row)", meta = ui_meta())]
    fn select_issue(&self, meta: RequestMetaObject, Parameters(IssueId { id }): Parameters<IssueId>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.sel = id.parse().ok())
    }

    #[tool(description = "Close the open issue's panel", meta = ui_meta())]
    fn close_detail(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| ui.sel = None)
    }

    #[tool(description = "Type the new issue's title", meta = ui_meta())]
    fn set_draft(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.title = value)
    }

    #[tool(description = "Open a GitHub issue with the title typed on the page", meta = ui_meta())]
    fn create_issue(&self, meta: RequestMetaObject) -> CallToolResult {
        self.write(&meta, |dir, ui| {
            let n = super::create(dir, ui.title.trim())?;
            ui.title.clear();
            Ok(format!("Opened #{n}"))
        })
    }

    #[tool(description = "Close the open issue on GitHub", meta = ui_meta())]
    fn close_issue(&self, meta: RequestMetaObject) -> CallToolResult {
        self.write(&meta, |dir, ui| {
            let n = ui.sel.take().ok_or("no issue is open")?;
            super::close(dir, n)?;
            Ok(format!("Closed #{n}"))
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

    /// GitHub has no file to watch: the two-minute refresh picks up a change made elsewhere.
    fn watches(&self, _dir: &str) -> Vec<PathBuf> {
        vec![]
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

    fn issue(number: i64, state: &str) -> Issue {
        Issue { number, title: format!("title {number}"), state: state.into(), author: "a".into(), updated: 100 + number, ..Default::default() }
    }

    #[test]
    fn the_filter_hides_closed_issues_until_asked_for() {
        let mut ui = Ui::default();
        assert!(shown(&issue(1, "open"), &ui));
        assert!(!shown(&issue(1, "closed"), &ui));
        ui.show = "closed".into();
        assert!(shown(&issue(1, "closed"), &ui));
        assert!(!shown(&issue(1, "open"), &ui));
    }

    #[test]
    fn the_page_lists_issues_and_an_open_issue_gets_a_panel_with_close() {
        let s = Snapshot { name: "t".into(), issues: vec![issue(7, "open")], ..Default::default() };
        let mut ui = Ui::default();
        let c = components(&s, &ui);
        let table = c.iter().find(|v| v["id"] == "table").unwrap();
        assert_eq!(table["sections"][0]["rows"][0]["cells"][0]["text"], "#7");
        assert!(c.iter().any(|v| v["id"] == "detail" && v["presented"] == false), "a closed panel is sent as a Modal that is not presented");
        ui.sel = Some(7);
        let c = components(&s, &ui);
        assert!(c.iter().any(|v| v["id"] == "d.close" && v["action"]["event"]["name"] == "close_issue"));
    }

    #[test]
    fn copy_ids_holds_the_listed_numbers_in_the_sorted_order_and_a_list_button_follows_them() {
        let mut a = issue(7, "open");
        a.updated = 10;
        let mut b = issue(12, "open");
        b.updated = 20;
        let mut s = Snapshot { name: "t".into(), issues: vec![a, b, issue(3, "closed")], ..Default::default() };
        s.scripts = vec![crate::core::buttons::Script { name: "iterate".into(), label: Some("Iterate".into()), run: "true".into() }];
        let copied = |c: &[Value]| c.iter().find(|x| x["id"] == "list.copy").unwrap()["action"]["copy"]["text"].as_str().unwrap().to_string();
        let disabled = |c: &[Value], id: &str| c.iter().find(|x| x["id"] == id).unwrap()["modifiers"].as_array().is_some_and(|m| m.iter().any(|x| x["disabled"] == true));
        let mut ui = Ui::default();
        let c = components(&s, &ui);
        assert_eq!(copied(&c), "12 7", "open issues, newest update first, as bare numbers");
        let tools = c.iter().find(|x| x["id"] == "tools").unwrap();
        assert_eq!(tools["children"], json!(["search", "views", "list.actions"]), "search, the views and the Actions chip");
        let title = |c: &[Value]| c.iter().find(|x| x["id"] == "list.actions.title").unwrap()["text"].as_str().unwrap().to_string();
        assert_eq!(title(&c), "2 listed · Actions ⌄");
        assert_eq!(c.iter().find(|x| x["id"] == "list.actions.menu").unwrap()["children"], json!(["list.copy", "list.iterate"]), "Copy IDs, then the list button");
        assert_eq!(c.iter().find(|x| x["id"] == "list.iterate.t").unwrap()["text"], "Iterate  2");
        let table = c.iter().find(|x| x["id"] == "table").unwrap();
        assert_eq!((table["sortAction"].as_str(), &table["sort"]), (Some("set_sort"), &json!({ "key": "updated", "asc": false })));
        assert_eq!((table["selectAction"].as_str(), &table["selected"]), (Some("select_rows"), &json!([])));
        ui.selected = ["7".to_string(), "3".to_string()].into_iter().collect();
        let c = components(&s, &ui);
        assert_eq!(copied(&c), "7", "the selected rows that are listed; the closed one is not shown");
        assert_eq!(title(&c), "1 selected of 2 · Actions ⌄");
        ui.selected = ["3".to_string()].into_iter().collect();
        let c = components(&s, &ui);
        assert_eq!(copied(&c), "12 7", "a selection with no listed row means the whole list");
        ui.selected.clear();
        ui.sort = ("number".into(), true);
        let c = components(&s, &ui);
        assert_eq!(copied(&c), "7 12", "a header click's sort orders the numbers");
        ui.show = "all".into();
        let c = components(&s, &ui);
        assert_eq!(copied(&c), "3 7 12");
        ui.q = "nothing-here".into();
        let c = components(&s, &ui);
        assert!(disabled(&c, "list.copy") && disabled(&c, "list.iterate"), "an empty list keeps the buttons in place, disabled");
    }

    #[test]
    fn gh_json_becomes_an_issue() {
        let v = json!({ "number": 3, "title": "T", "state": "OPEN", "labels": [{ "name": "bug" }], "assignees": [{ "login": "me" }],
            "author": { "login": "you" }, "body": "b", "url": "https://github.com/o/r/issues/3", "updatedAt": "2026-10-30T01:07:41Z" });
        let i = super::issue(&v);
        assert_eq!((i.number, i.state.as_str(), i.labels, i.assignees, i.author.as_str()), (3, "open", vec!["bug".to_string()], vec!["me".to_string()], "you"));
        assert!(i.updated > 0);
    }
}
