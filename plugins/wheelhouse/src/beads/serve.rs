//! The Beads view: the Beads page's tools over MCP, as a view `wheelhouse serve` registers.
//!
//! Each pane keeps its own view, search and filters; each subscribed project keeps one snapshot
//! of its beads, re-read when `.beads/last-touched` moves or a minute passes.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, MetaObject, RequestMetaObject, ResourceContents, Tool};
use rmcp::{schemars, service::RequestContext, tool, tool_router, ErrorData, RoleServer};

use super::buttons::State;
use super::*;
use crate::core::project::Origin;
use crate::view::{View, MIME};

/// The artifact name this view answers to, and so the host of its page's URI.
pub const NAME: &str = "beads";
const URI: &str = "a2ui://beads";

struct Project {
    snap: Snapshot,
}

#[derive(Default)]
struct Shared {
    projects: HashMap<String, Project>,
    panes: HashMap<String, Ui>,
}

impl Shared {
    fn project(&mut self, dir: &str) -> &mut Project {
        self.projects.entry(dir.to_string()).or_insert_with(|| Project { snap: collect(dir) })
    }
}

fn ui_meta() -> MetaObject {
    let value = json!({ "ui": { "resourceUri": URI, "mimeType": MIME } });
    MetaObject(value.as_object().unwrap().clone())
}

/// The project and pane a request is for.
fn origin(meta: &RequestMetaObject) -> (String, String) {
    let o = Origin::of(meta);
    (o.dir, o.pane)
}

fn answer(snap: &Snapshot, ui: &Ui) -> CallToolResult {
    let messages = crate::core::page::update(SURFACE, page_components(snap, ui));
    CallToolResult::success(vec![ContentBlock::resource(ResourceContents::TextResourceContents {
        uri: URI.into(),
        mime_type: Some(MIME.into()),
        text: messages.to_string(),
        meta: None,
    })])
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Value1 {
    /// The new value.
    value: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Filter {
    /// status, type, label or pri.
    key: String,
    /// The value; empty for any.
    value: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Query {
    /// The search text.
    #[serde(alias = "value")]
    q: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Key {
    /// A column key.
    key: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TreePath {
    /// A row's tree path.
    path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct BeadId {
    /// The bead's id.
    id: String,
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
    /// The button's name in `[beads.list.<name>]`.
    script: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Press {
    /// The bead's id.
    bead: String,
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
    shared: Arc<Mutex<Shared>>,
}

impl Server {
    /// Change the asking pane's state and answer with its page.
    fn with(&self, meta: &RequestMetaObject, f: impl FnOnce(&Snapshot, &mut Ui)) -> CallToolResult {
        let (dir, pane) = origin(meta);
        let mut snap = self.shared.lock().unwrap().project(&dir).snap.clone();
        super::fill_acts(&mut snap);
        let mut s = self.shared.lock().unwrap();
        let ui = s.panes.entry(pane).or_default();
        ui.centre = false;
        ui.said = None;
        graph::Model::new(&snap, ui).trim(ui);
        f(&snap, ui);
        answer(&snap, ui)
    }
}

#[tool_router]
impl Server {
    fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self { tool_router: Self::tool_router(), shared }
    }

    #[tool(description = "The beads page", meta = ui_meta())]
    fn get_beads(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, _| {})
    }

    #[tool(description = "Show a view: list, epics, deps, unlocks, focus or graph", meta = ui_meta())]
    fn set_view(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if ["list", "epics", "deps", "unlocks", "focus", "graph"].contains(&value.as_str()) {
                ui.view = value;
                ui.collapsed.clear();
            }
        })
    }

    #[tool(description = "Search the beads", meta = ui_meta())]
    fn set_query(&self, meta: RequestMetaObject, Parameters(Query { q }): Parameters<Query>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.q = q)
    }

    #[tool(description = "Set one filter", meta = ui_meta())]
    fn set_filter(&self, meta: RequestMetaObject, Parameters(Filter { key, value }): Parameters<Filter>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if ["status", "type", "label", "pri"].contains(&key.as_str()) {
                ui.f.insert(key, value);
            }
        })
    }

    #[tool(description = "Tick or untick one value of a filter; the key is `<filter>:<value>`", meta = ui_meta())]
    fn toggle_filter(&self, meta: RequestMetaObject, Parameters(Filter { key, value }): Parameters<Filter>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            let Some((field, v)) = key.split_once(':') else { return };
            if !["status", "type", "label", "pri"].contains(&field) {
                return;
            }
            let mut vals: Vec<String> = ui.values(field).iter().map(|x| x.to_string()).collect();
            vals.retain(|x| x != v);
            if value == "true" {
                vals.push(v.to_string());
            }
            ui.f.insert(field.to_string(), vals.join(","));
        })
    }

    #[tool(description = "Make a filter exclude its values instead of matching them", meta = ui_meta())]
    fn set_not(&self, meta: RequestMetaObject, Parameters(Filter { key, value }): Parameters<Filter>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if value == "true" { ui.not.insert(key); } else { ui.not.remove(&key); }
        })
    }

    #[tool(description = "Sort the list by a column, ascending or not", meta = ui_meta())]
    fn set_sort(&self, meta: RequestMetaObject, Parameters(Sort { key, asc }): Parameters<Sort>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if COLS.iter().any(|c| c.key == key) {
                ui.sort = (key, asc);
            }
        })
    }

    #[tool(description = "Run one of the project's list buttons over the beads the page lists now", meta = ui_meta())]
    fn run_list_script(&self, meta: RequestMetaObject, Parameters(ListScript { script }): Parameters<ListScript>) -> CallToolResult {
        let (dir, pane) = origin(&meta);
        let (snap, listed) = {
            let mut s = self.shared.lock().unwrap();
            let snap = s.project(&dir).snap.clone();
            let ui = s.panes.entry(pane.clone()).or_default();
            let listed = ids(&Ix::new(&snap), ui);
            (snap, listed)
        };
        let mut env = vec![("BEAD_REPO", snap.name.as_str()), ("BEAD_PATH", snap.path.as_str())];
        let data = Origin::of(&meta).data;
        if let Some(d) = &data {
            env.push(("WHEEL_DATA_DIR", d.as_str()));
        }
        match crate::core::buttons::run_list(&snap.scripts, &script, &listed, "BEAD_IDS", &env, &snap.path, &pane, "beads") {
            Ok(said) => self.with(&meta, |_, ui| ui.said = Some(said)),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
    }

    #[tool(description = "Clear the search and the filters", meta = ui_meta())]
    fn clear(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| {
            ui.q.clear();
            ui.f.clear();
            ui.not.clear();
        })
    }

    #[tool(description = "Fold or unfold a tree row", meta = ui_meta())]
    fn toggle_path(&self, meta: RequestMetaObject, Parameters(TreePath { path }): Parameters<TreePath>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if !ui.collapsed.remove(&path) {
                ui.collapsed.insert(path);
            }
        })
    }

    #[tool(description = "Open a bead from the list", meta = ui_meta())]
    fn select_bead(&self, meta: RequestMetaObject, Parameters(BeadId { id }): Parameters<BeadId>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            ui.hist.clear();
            ui.ran = None;
            ui.sel = Some(id.clone());
            ui.detail = Some(id);
        })
    }

    #[tool(description = "Open a bead from the open bead's links", meta = ui_meta())]
    fn open_bead(&self, meta: RequestMetaObject, Parameters(BeadId { id }): Parameters<BeadId>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if let Some(cur) = ui.detail.take() {
                ui.hist.push(cur);
            }
            ui.sel = Some(id.clone());
            ui.detail = Some(id);
            ui.ran = None;
        })
    }

    #[tool(description = "Go back to the bead opened before", meta = ui_meta())]
    fn back(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if let Some(prev) = ui.hist.pop() {
                ui.detail = Some(prev);
            }
        })
    }

    #[tool(description = "Close the open bead", meta = ui_meta())]
    fn close_detail(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| {
            ui.detail = None;
            ui.hist.clear();
            ui.ran = None;
        })
    }

    #[tool(description = "Focus: how many hops to show on each side, 1 to 3", meta = ui_meta())]
    fn set_depth(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if let Ok(d) = value.parse::<usize>() {
                ui.depth = d.clamp(1, 3);
                ui.more = None;
            }
        })
    }

    #[tool(description = "Focus: centre on a bead, keeping the one before in the history", meta = ui_meta())]
    fn focus_on(&self, meta: RequestMetaObject, Parameters(BeadId { id }): Parameters<BeadId>) -> CallToolResult {
        self.with(&meta, |snap, ui| {
            let m = graph::Model::new(snap, ui);
            if let (true, Some(cur)) = (m.has(&id), m.focus_idx(ui)) {
                let cur = m.id_of(cur).to_string();
                if cur != id {
                    ui.fhist.push(cur);
                }
                ui.sel = Some(id);
                ui.more = None;
            }
        })
    }

    #[tool(description = "Focus: back to the bead centred before", meta = ui_meta())]
    fn focus_back(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if let Some(id) = ui.fhist.pop() {
                ui.sel = Some(id);
                ui.more = None;
            }
        })
    }

    #[tool(description = "Graph: select a bead (the same one again, or an empty id, deselects)", meta = ui_meta())]
    fn graph_select(&self, meta: RequestMetaObject, Parameters(BeadId { id }): Parameters<BeadId>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            ui.sel = if id.is_empty() || ui.sel.as_deref() == Some(&id) { None } else { Some(id) };
        })
    }

    #[tool(description = "Focus: open or close the list behind a + N more node", meta = ui_meta())]
    fn graph_more(&self, meta: RequestMetaObject, Parameters(Key { key }): Parameters<Key>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            ui.more = if ui.more.as_deref() == Some(&key) { None } else { Some(key) };
        })
    }

    #[tool(description = "Focus: the width of the canvas, in points", meta = ui_meta())]
    fn set_width(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if let Ok(w) = value.parse::<f64>() {
                ui.cw = w.clamp(300.0, 4000.0);
            }
        })
    }

    #[tool(description = "Escape in Focus or Graph: close the open list, go back, or deselect", meta = ui_meta())]
    fn escape(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if ui.more.take().is_some() {
            } else if ui.view == "focus" && !ui.fhist.is_empty() {
                ui.sel = ui.fhist.pop();
            } else if ui.view == "graph" {
                ui.sel = None;
            }
        })
    }

    #[tool(description = "Press a bead's button: focus its pane, resume it or start it, as its state says", meta = ui_meta())]
    fn press(&self, meta: RequestMetaObject, Parameters(Press { bead }): Parameters<Press>) -> CallToolResult {
        let (dir, pane) = origin(&meta);
        let mut snap = self.shared.lock().unwrap().project(&dir).snap.clone();
        super::fill_acts(&mut snap);
        let Some(b) = snap.beads.iter().find(|b| b.id == bead) else {
            return CallToolResult::error(vec![ContentBlock::text(format!("no bead {bead}"))]);
        };
        let Some(act) = snap.acts.get(&bead).cloned().filter(|a| !a.disabled) else {
            return CallToolResult::error(vec![ContentBlock::text(format!("{bead} has no button to press"))]);
        };
        if let State::Live(p) = &act.state {
            return match crate::core::run::text("wheel", &snap.path, &["pane", "focus", p]) {
                Ok(_) => self.with(&meta, |_, ui| ui.ran = Some(format!("Focused {p} for {bead}."))),
                Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
            };
        }
        let epic = super::groups(b);
        let Some(run) = buttons::command(&snap.buttons, epic, &act.state) else {
            return CallToolResult::error(vec![ContentBlock::text(format!("{bead} has no command for {}", act.label))]);
        };
        let log = std::env::temp_dir().join(format!("wheel-beads-{}.log", sane(&b.id)));
        let conversation = buttons::recorded(&buttons::records_file(), &snap.path, &bead).map(|r| r.conversation).unwrap_or_default();
        let mut env = vec![("BEAD_ID", b.id.as_str()), ("BEAD_TITLE", &b.title), ("BEAD_TYPE", &b.kind), ("BEAD_REPO", &snap.name), ("BEAD_PATH", &snap.path)];
        if act.state == State::Saved {
            env.push(("CONVERSATION", conversation.as_str()));
        }
        let data = Origin::of(&meta).data;
        if let Some(d) = &data {
            env.push(("WHEEL_DATA_DIR", d.as_str()));
        }
        if !buttons::claim(&snap.path, &bead) {
            return CallToolResult::error(vec![ContentBlock::text(format!("{bead} is already starting"))]);
        }
        let (path, id) = (snap.path.clone(), bead.clone());
        let remember = move |started: Option<String>| {
            if let Some(pane) = started.filter(|p| !p.is_empty()) {
                let conversation = buttons::conversation_of(&path, &pane).unwrap_or_else(|e| {
                    eprintln!("beads: {id}: {e}");
                    String::new()
                });
                if let Err(e) = buttons::record(&buttons::records_file(), &path, &id, buttons::Record { pane, conversation }) {
                    eprintln!("beads: could not record {id}: {e}");
                }
            }
            buttons::release(&path, &id);
        };
        match crate::core::run::button_then(&act.label, run, &snap.path, &pane, &env, &log, remember) {
            Ok(()) => self.with(&meta, |_, ui| ui.ran = Some(format!("{} started for {bead}. See the new pane.", act.label))),
            Err(e) => {
                buttons::release(&snap.path, &bead);
                CallToolResult::error(vec![ContentBlock::text(e)])
            }
        }
    }
}

/// The Beads view.
pub struct BeadsView {
    server: Server,
}

impl BeadsView {
    pub fn new() -> Self {
        BeadsView { server: Server::new(Arc::new(Mutex::new(Shared::default()))) }
    }
}

#[async_trait]
impl View for BeadsView {
    fn name(&self) -> &str {
        NAME
    }

    fn tools(&self) -> Vec<Tool> {
        self.server.tool_router.list_all()
    }

    async fn call(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.server.tool_router.call(ToolCallContext::new(&self.server, params, context)).await
    }

    fn skeleton(&self) -> Value {
        skeleton()
    }

    fn watches(&self, dir: &str) -> Vec<PathBuf> {
        vec![PathBuf::from(dir).join(".beads/last-touched")]
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
            Some(p) if p.snap != fresh => {
                p.snap = fresh;
                true
            }
            _ => false,
        }
    }
}
