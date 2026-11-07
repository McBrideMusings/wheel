//! The Beads view: the Beads page's tools over MCP, as a view `wheelhouse serve` registers.
//!
//! Each pane keeps its own view, search and filters; each subscribed project keeps one snapshot
//! of its beads, re-read when `.beads/last-touched` moves or a minute passes.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, MetaObject, RequestMetaObject, ResourceContents, Tool};
use rmcp::{schemars, service::RequestContext, tool, tool_router, ErrorData, RoleServer};

use super::*;
use crate::core::project::Origin;
use crate::view::{View, MIME};

/// The artifact name this view answers to, and so the host of its page's URI.
pub const NAME: &str = "beads-native";
const URI: &str = "a2ui://beads-native";

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
struct RunScript {
    /// The bead's id.
    bead: String,
    /// The button's name in `[beads.buttons.<name>]`.
    script: String,
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
        let mut s = self.shared.lock().unwrap();
        let snap = s.project(&dir).snap.clone();
        let ui = s.panes.entry(pane).or_default();
        ui.centre = false;
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

    #[tool(description = "Hide or show done beads: value true or false", meta = ui_meta())]
    fn set_hide(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.hide = value == "true")
    }

    #[tool(description = "Sort by a column from the Sort menu", meta = ui_meta())]
    fn set_sort(&self, meta: RequestMetaObject, Parameters(Value1 { value }): Parameters<Value1>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if let Some(c) = COLS.iter().find(|c| c.key == value) {
                ui.sort = (c.key.into(), !c.desc_first);
            }
        })
    }

    #[tool(description = "Sort by a column header: the same column again reverses it", meta = ui_meta())]
    fn sort_by(&self, meta: RequestMetaObject, Parameters(Key { key }): Parameters<Key>) -> CallToolResult {
        self.with(&meta, |_, ui| {
            if let Some(c) = COLS.iter().find(|c| c.key == key) {
                ui.sort = if ui.sort.0 == key { (key, !ui.sort.1) } else { (key, !c.desc_first) };
            }
        })
    }

    #[tool(description = "Clear the search and the filters", meta = ui_meta())]
    fn clear(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| {
            ui.q.clear();
            ui.f.clear();
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
            ui.detail = Some(id);
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

    #[tool(description = "Show in graph from Focus, or Focus on selected from Graph", meta = ui_meta())]
    fn graph_cross(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |snap, ui| {
            if ui.view == "focus" {
                let m = graph::Model::new(snap, ui);
                if let Some(c) = m.focus_idx(ui) {
                    ui.sel = Some(m.id_of(c).to_string());
                }
                ui.centre = true;
                ui.view = "graph".into();
            } else if ui.sel.is_some() {
                ui.view = "focus".into();
            }
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

    #[tool(description = "Run a bead's button", meta = ui_meta())]
    fn run_script(&self, meta: RequestMetaObject, Parameters(RunScript { bead, script }): Parameters<RunScript>) -> CallToolResult {
        let (dir, _) = origin(&meta);
        let snap = self.shared.lock().unwrap().project(&dir).snap.clone();
        let Some(b) = snap.beads.iter().find(|b| b.id == bead) else {
            return CallToolResult::error(vec![ContentBlock::text(format!("no bead {bead}"))]);
        };
        let Some(sc) = snap.scripts.iter().find(|x| x.name == script) else {
            return CallToolResult::error(vec![ContentBlock::text(format!("no button {script}"))]);
        };
        let log = std::env::temp_dir().join(format!("wheel-beads-{}-{}.log", sane(&b.id), sane(&sc.name)));
        let file = match std::fs::File::create(&log) {
            Ok(f) => f,
            Err(e) => return CallToolResult::error(vec![ContentBlock::text(format!("{}: {e}", log.display()))]),
        };
        let spawned = Command::new("/bin/sh")
            .arg("-c")
            .arg(&sc.run)
            .current_dir(&snap.path)
            .env("BEAD_ID", &b.id)
            .env("BEAD_TITLE", &b.title)
            .env("BEAD_TYPE", &b.kind)
            .env("BEAD_REPO", &snap.name)
            .env("BEAD_PATH", &snap.path)
            .stdin(std::process::Stdio::null())
            .stdout(file.try_clone().unwrap())
            .stderr(file)
            .spawn();
        match spawned {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                self.with(&meta, |_, _| {})
            }
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{script}: {e}"))]),
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
