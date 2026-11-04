//! `wheelhouse serve`: the Beads page over MCP, one process for every project the host has open.
//!
//! The host names the asking pane and project in `_meta` (`wheel/pane`, `wheel/dir`) on each call
//! and read; a process run with neither serves the project in its own directory. Each pane keeps
//! its own view, search and filters. The host subscribes to `a2ui://beads?dir=<project>` while a
//! pane shows the project, and a change to its beads is announced with
//! `notifications/resources/updated` for that URI; a project nobody subscribes to is not watched.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, MetaObject, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    RequestMetaObject, ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::{
    schemars, service::RequestContext, tool, tool_handler, tool_router, ErrorData,
    RoleServer, ServerHandler, ServiceExt,
};

use super::*;

const MIME: &str = "application/a2ui+json";
const URI: &str = "a2ui://beads";

struct Project {
    snap: Snapshot,
    seen: Option<SystemTime>,
    since: u64,
}

#[derive(Default)]
struct Shared {
    projects: HashMap<String, Project>,
    panes: HashMap<String, Ui>,
    /// The subscribed resource URIs, by project directory.
    watched: HashMap<String, String>,
}

impl Shared {
    fn project(&mut self, dir: &str) -> &mut Project {
        self.projects.entry(dir.to_string()).or_insert_with(|| Project {
            snap: collect(dir),
            seen: touched(dir),
            since: 0,
        })
    }
}

fn touched(dir: &str) -> Option<SystemTime> {
    std::fs::metadata(PathBuf::from(dir).join(".beads/last-touched")).and_then(|m| m.modified()).ok()
}

fn ui_meta() -> MetaObject {
    let value = json!({ "ui": { "resourceUri": URI, "mimeType": MIME } });
    MetaObject(value.as_object().unwrap().clone())
}

/// The project and pane a request is for.
fn origin(meta: &RequestMetaObject) -> (String, String) {
    let get = |k: &str| meta.0.get(k).and_then(|v| v.as_str()).map(String::from);
    let dir = get("wheel/dir")
        .or_else(|| std::env::var("WHEEL_SPACE_DIR").ok())
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_default();
    (dir, get("wheel/pane").unwrap_or_else(|| "default".into()))
}

fn answer(snap: &Snapshot, ui: &Ui) -> CallToolResult {
    let messages = json!([{ "version": "v0.9", "updateComponents": {
        "surfaceId": SURFACE, "components": page_components(snap, ui) } }]);
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

/// The project a resource URI names: its `dir` query.
fn dir_of(uri: &str) -> Option<String> {
    url::Url::parse(uri).ok()?.query_pairs().find(|(k, _)| k == "dir").map(|(_, v)| v.into_owned())
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder().enable_tools().enable_resources().enable_resources_subscribe().build(),
        )
    }

    async fn subscribe(
        &self,
        request: rmcp::model::SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        if let Some(dir) = dir_of(&request.uri) {
            let mut s = self.shared.lock().unwrap();
            s.project(&dir);
            s.watched.insert(dir, request.uri);
        }
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: rmcp::model::UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        if let Some(dir) = dir_of(&request.uri) {
            let mut s = self.shared.lock().unwrap();
            s.watched.remove(&dir);
            s.projects.remove(&dir);
        }
        Ok(())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if !request.uri.starts_with(URI) {
            return Err(ErrorData::resource_not_found(format!("no resource {}", request.uri), None));
        }
        Ok(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
            uri: request.uri,
            mime_type: Some(MIME.into()),
            text: skeleton().to_string(),
            meta: None,
        }])
        .into())
    }
}

pub async fn run() -> anyhow::Result<()> {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let running = Server::new(shared.clone()).serve(rmcp::transport::stdio()).await?;
    let peer = running.peer().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            // Which projects are due: their `.beads/last-touched` moved, or a minute passed.
            let due: Vec<String> = {
                let mut s = shared.lock().unwrap();
                let watched: Vec<String> = s.watched.keys().cloned().collect();
                s.projects
                    .iter_mut()
                    .filter(|(dir, _)| watched.contains(dir))
                    .filter_map(|(dir, p)| {
                        p.since += 1;
                        let now = touched(dir);
                        (now != p.seen || p.since >= 60).then(|| {
                            p.seen = now;
                            p.since = 0;
                            dir.clone()
                        })
                    })
                    .collect()
            };
            for dir in due {
                let d = dir.clone();
                let fresh = tokio::task::spawn_blocking(move || collect(&d)).await.unwrap_or_default();
                let changed = {
                    let mut s = shared.lock().unwrap();
                    match s.projects.get_mut(&dir) {
                        Some(p) if p.snap != fresh => {
                            p.snap = fresh;
                            true
                        }
                        _ => false,
                    }
                };
                let uri = shared.lock().unwrap().watched.get(&dir).cloned();
                if let (true, Some(uri)) = (changed, uri) {
                    if peer.notify_resource_updated(rmcp::model::ResourceUpdatedNotificationParam::new(uri)).await.is_err() {
                        return;
                    }
                }
            }
        }
    });
    running.waiting().await?;
    Ok(())
}
