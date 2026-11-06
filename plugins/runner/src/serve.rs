//! `wheel-runner serve`: the runner over MCP on standard input and output, for a host. One
//! process serves every project the host has open; the host names the project in `_meta`
//! `wheel/dir` on each call. A run belongs to this process, not to a page: it keeps going and its
//! status and log stay readable after the page that started it closes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult, MetaObject, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, RequestMetaObject, ResourceContents, ServerCapabilities,
    ServerConfig,
};
use rmcp::{schemars, service::RequestContext, tool, tool_router, ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{json, Value};

use crate::config;
use crate::page::{self, Ui};
use crate::runs::{Runs, StartError};
use crate::status;
use crate::store::State;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Named {
    /// The `[runner]` entry's name.
    name: String,
    /// The entry's command text the person answered the `confirm` question with yes for; an
    /// entry whose command differs asks again.
    confirmed: Option<String>,
    /// Run in this git worktree of the project; the entry must set `worktree = true`.
    worktree: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct CheckArgs {
    /// The `[runner]` entry's name.
    name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RunId {
    /// The run's id, from `list_runs`.
    id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LogArgs {
    /// The run's id, from `list_runs`.
    id: String,
    /// Read from this byte; the answer's `next` is where to continue. Default 0.
    offset: Option<u64>,
    /// Only the last this many lines.
    tail: Option<usize>,
}

/// The project a request is for: `_meta` `wheel/dir`, else the process's own directory.
fn project(meta: &RequestMetaObject) -> String {
    meta.0 .0.get("wheel/dir").and_then(|v| v.as_str()).map(String::from)
        .or_else(|| std::env::var("WHEEL_SPACE_DIR").ok())
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

fn reply(v: Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(v.to_string())])
}

fn refuse(e: impl ToString) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(e.to_string())])
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Press {
    /// The `[runner]` entry's name.
    name: String,
    /// The command text of the Run button the person pressed to answer the entry's confirm question.
    confirmed: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Row {
    /// The run's id, as the row's path.
    path: String,
}

fn ui_meta() -> MetaObject {
    MetaObject(json!({ "ui": { "resourceUri": page::URI, "mimeType": page::MIME } }).as_object().unwrap().clone())
}

fn mtime(p: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Whether the page for a project is stale, read from file metadata alone: the runs folder moves
/// whenever a run starts or ends (a record is renamed into it), the `wheel.toml` files move when
/// a command changes, and a running run's log grows. No record is read to tell.
#[derive(Clone, Debug, PartialEq)]
struct Stamp {
    folder: Option<std::time::SystemTime>,
    configs: [Option<std::time::SystemTime>; 2],
    /// The byte length of each log that was running when the folder last moved.
    growing: Vec<(String, u64)>,
}

fn stamp(dir: &str, growing: Option<&[(String, u64)]>) -> Stamp {
    let folder = crate::store::folder(dir);
    let configs = [mtime(&config::machine_file()), mtime(&std::path::Path::new(dir).join("wheel.toml"))];
    let own = mtime(&folder);
    let growing = match growing {
        Some(g) => g.iter().map(|(id, _)| (id.clone(), std::fs::metadata(crate::store::log_path(&folder, id)).map(|m| m.len()).unwrap_or(0))).collect(),
        None => Runs::new(dir).list().iter().filter(|r| r.state == State::Running)
            .map(|r| (r.id.clone(), std::fs::metadata(&r.log).map(|m| m.len()).unwrap_or(0))).collect(),
    };
    Stamp { folder: own, configs, growing }
}

#[derive(Default)]
struct Shared {
    panes: HashMap<String, Ui>,
    /// The projects a host subscribed to: how many subscriptions, and the stamp last announced.
    subscribed: HashMap<String, (usize, Stamp)>,
}

fn pane_of(meta: &RequestMetaObject) -> String {
    meta.0 .0.get("wheel/pane").and_then(|v| v.as_str()).unwrap_or("default").to_string()
}

/// The page for `dir` as pane `pane` has it open.
fn answer(dir: &str, ui: &Ui) -> CallToolResult {
    let runs = Runs::new(dir);
    let list = runs.list();
    let runners = config::list(&config::machine_file(), dir);
    let log = ui.open.as_deref().filter(|id| list.iter().any(|r| r.id == *id))
        .and_then(|id| runs.log(id, 0, Some(page::LOG_LINES)).ok().map(|(t, _)| t));
    let messages = page::update(page::components(&runners, &list, ui, log.as_deref(), crate::now()));
    CallToolResult::success(vec![ContentBlock::resource(ResourceContents::TextResourceContents {
        uri: page::URI.into(), mime_type: Some(page::MIME.into()), text: messages.to_string(), meta: None })])
}

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
    shared: Arc<Mutex<Shared>>,
}

#[tool_router]
impl Server {
    fn build() -> Self {
        Self { tool_router: Self::tool_router(), shared: Arc::default() }
    }

    /// Change what the asking pane has open, then draw its page. The lock is held only to take
    /// and put back the pane's state, never across file work.
    fn with(&self, meta: &RequestMetaObject, f: impl FnOnce(&str, &mut Ui)) -> CallToolResult {
        let (dir, key) = (project(meta), format!("{}\u{0}{}", project(meta), pane_of(meta)));
        let mut ui = self.shared.lock().unwrap().panes.get(&key).cloned().unwrap_or_default();
        f(&dir, &mut ui);
        let drawn = answer(&dir, &ui);
        // A pane with nothing open keeps nothing, so a closed pane leaves no entry behind.
        let mut shared = self.shared.lock().unwrap();
        if ui == Ui::default() {
            shared.panes.remove(&key);
        } else {
            shared.panes.insert(key, ui);
        }
        drawn
    }

    #[tool(description = "The Runner page", meta = ui_meta())]
    fn get_runner(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, _| {})
    }

    #[tool(description = "The Run button of a command: starts it and opens its log, or shows its confirm question first", meta = ui_meta())]
    fn press_run(&self, meta: RequestMetaObject, Parameters(a): Parameters<Press>) -> CallToolResult {
        self.with(&meta, |dir, ui| {
            ui.problem = None;
            let Some(runner) = config::list(&config::machine_file(), dir).into_iter().find(|r| r.name == a.name) else {
                ui.problem = Some(format!("no runner named {}", a.name));
                return;
            };
            match Runs::new(dir).start(&runner, a.confirmed.as_deref(), None) {
                Ok(s) => {
                    ui.asking = None;
                    ui.open = Some(s.record.id);
                }
                Err(StartError::Confirm(_)) => {
                    if a.confirmed.is_some() {
                        ui.problem = Some(format!("{} changed since it was asked about; answer again", a.name));
                    }
                    ui.asking = Some(a.name);
                }
                Err(StartError::Guard(m) | StartError::Failed(m)) => ui.problem = Some(format!("{} did not start: {m}", a.name)),
            }
        })
    }

    #[tool(description = "Cancel the confirm question", meta = ui_meta())]
    fn cancel_ask(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| ui.asking = None)
    }

    #[tool(description = "Open a run's log, or close it when it is the open one", meta = ui_meta())]
    fn open_log(&self, meta: RequestMetaObject, Parameters(Row { path }): Parameters<Row>) -> CallToolResult {
        self.with(&meta, |_, ui| ui.open = if ui.open.as_deref() == Some(path.as_str()) { None } else { Some(path) })
    }

    #[tool(description = "Close the open log", meta = ui_meta())]
    fn close_log(&self, meta: RequestMetaObject) -> CallToolResult {
        self.with(&meta, |_, ui| ui.open = None)
    }

    #[tool(description = "Stop the run whose log is open", meta = ui_meta())]
    async fn stop_open(&self, meta: RequestMetaObject) -> CallToolResult {
        let dir = project(&meta);
        let open = self.shared.lock().unwrap().panes.get(&format!("{dir}\u{0}{}", pane_of(&meta))).and_then(|u| u.open.clone());
        let problem = match open {
            // Ending a run waits for its processes to go; that must not hold up other calls.
            Some(id) => {
                let d = dir.clone();
                tokio::task::spawn_blocking(move || Runs::new(&d).stop(&id)).await.unwrap_or_else(|e| Err(e.to_string())).err()
            }
            None => Some("no log is open".into()),
        };
        self.with(&meta, |_, ui| ui.problem = problem)
    }

    #[tool(description = "The commands the project's [runner] table names, and this machine's")]
    fn list_runners(&self, meta: RequestMetaObject) -> CallToolResult {
        reply(json!(config::list(&config::machine_file(), &project(&meta))))
    }

    #[tool(description = "Start a [runner] command. An entry with a confirm question answers {started:false, confirm} with its command text, until it is started again with confirmed set to that command text (an entry edited since asks again); a failing guard answers {started:false, guard}; worktree runs an entry that sets worktree = true in another git worktree of the project")]
    async fn run_runner(&self, meta: RequestMetaObject, Parameters(a): Parameters<Named>) -> CallToolResult {
        let dir = project(&meta);
        let Some(runner) = config::list(&config::machine_file(), &dir).into_iter().find(|r| r.name == a.name) else {
            return refuse(format!("no runner named {}", a.name));
        };
        let runs = match &a.worktree {
            Some(w) => Runs::new(&dir).in_worktree(w),
            None => Runs::new(&dir),
        };
        // Guards run commands and may fetch; that must not hold up other calls.
        let (confirmed, runner_command) = (a.confirmed, runner.command.clone());
        let started = tokio::task::spawn_blocking(move || runs.start(&runner, confirmed.as_deref(), None)).await;
        let Ok(started) = started else { return refuse("the start panicked") };
        match started {
            Ok(s) => reply(json!({ "started": true, "id": s.record.id, "log": s.record.log })),
            Err(StartError::Confirm(q)) => reply(json!({ "started": false, "confirm": q, "command": runner_command })),
            Err(StartError::Guard(m)) => reply(json!({ "started": false, "guard": m })),
            Err(StartError::Failed(e)) => refuse(e),
        }
    }

    #[tool(description = "A command's status check (http, json or command): {up, text}")]
    async fn check_runner(&self, meta: RequestMetaObject, Parameters(a): Parameters<CheckArgs>) -> CallToolResult {
        let dir = project(&meta);
        let Some(runner) = config::list(&config::machine_file(), &dir).into_iter().find(|r| r.name == a.name) else {
            return refuse(format!("no runner named {}", a.name));
        };
        let Some(check) = runner.status else { return refuse(format!("{} has no status check", a.name)) };
        // A check waits on the network or a script; it must not hold up other calls.
        match tokio::task::spawn_blocking(move || crate::check::run(&check, std::path::Path::new(&dir))).await {
            Ok(reading) => reply(json!(reading)),
            Err(e) => refuse(e),
        }
    }

    #[tool(description = "The project's runs, newest first: state, exit code, times, log path")]
    fn list_runs(&self, meta: RequestMetaObject) -> CallToolResult {
        reply(json!(Runs::new(&project(&meta)).list()))
    }

    #[tool(description = "A run's log from a byte offset, or its last lines; the answer's next is the offset to continue from")]
    fn get_log(&self, meta: RequestMetaObject, Parameters(a): Parameters<LogArgs>) -> CallToolResult {
        match Runs::new(&project(&meta)).log(&a.id, a.offset.unwrap_or(0), a.tail) {
            Ok((text, next)) => reply(json!({ "text": text, "next": next })),
            Err(e) => refuse(e),
        }
    }

    #[tool(description = "End a running run and everything it started")]
    async fn stop_run(&self, meta: RequestMetaObject, Parameters(a): Parameters<RunId>) -> CallToolResult {
        // Ending a run waits for its processes to go; that must not hold up other calls.
        let dir = project(&meta);
        let ended = tokio::task::spawn_blocking(move || Runs::new(&dir).stop(&a.id)).await;
        match ended.unwrap_or_else(|e| Err(e.to_string())) {
            Ok(r) => reply(json!({ "stopping": r.id })),
            Err(e) => refuse(e),
        }
    }

    #[tool(description = "The status line: the latest run, its state, how long it ran and its log")]
    fn get_status(&self, meta: RequestMetaObject) -> CallToolResult {
        reply(json!({ "line": status::line(Runs::new(&project(&meta)).latest().as_ref(), crate::now()) }))
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().enable_resources_subscribe().build())
    }

    async fn subscribe(
        &self,
        request: rmcp::model::SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let dir = page_dir(&request.uri)?;
        let first = stamp(&dir, None);
        let mut shared = self.shared.lock().unwrap();
        shared.subscribed.entry(dir).and_modify(|e| e.0 += 1).or_insert((1, first));
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: rmcp::model::UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        if let Ok(dir) = page_dir(&request.uri) {
            let mut shared = self.shared.lock().unwrap();
            if let Some(e) = shared.subscribed.get_mut(&dir) {
                e.0 -= 1;
                if e.0 == 0 {
                    shared.subscribed.remove(&dir);
                    // No host shows this project any more: its panes' state goes with them.
                    let prefix = format!("{dir}\u{0}");
                    shared.panes.retain(|k, _| !k.starts_with(&prefix));
                }
            }
        }
        Ok(())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        page_dir(&request.uri)?;
        Ok(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
            uri: request.uri,
            mime_type: Some(page::MIME.into()),
            text: page::skeleton().to_string(),
            meta: None,
        }])
        .into())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult { tools: self.tool_router.list_all(), ..Default::default() })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.tool_router.call(ToolCallContext::new(self, request, context)).await
    }
}

/// The project a page URI names (`a2ui://runner-native?dir=<project>`), or an error when the URI
/// is for no page this process serves.
fn page_dir(uri: &str) -> Result<String, ErrorData> {
    let u = url::Url::parse(uri).ok().filter(|u| u.scheme() == "a2ui" && u.host_str() == page::URI.strip_prefix("a2ui://"))
        .ok_or_else(|| ErrorData::resource_not_found(format!("no resource {uri}"), None))?;
    u.query_pairs().find(|(k, _)| k == "dir").map(|(_, v)| v.into_owned())
        .ok_or_else(|| ErrorData::invalid_params(format!("{uri} names no project: add ?dir=<project>"), None))
}

impl Server {
    /// One second passed: the URIs of the subscribed projects whose page now differs.
    fn changed(&self) -> Vec<String> {
        let subs: Vec<(String, Stamp)> = self.shared.lock().unwrap().subscribed.iter().map(|(d, e)| (d.clone(), e.1.clone())).collect();
        let mut uris = vec![];
        for (dir, last) in subs {
            let mut now = stamp(&dir, Some(&last.growing));
            if now.folder != last.folder || now.configs != last.configs {
                // A run started or ended: which logs are running is read once, here.
                now = stamp(&dir, None);
            }
            if now != last {
                let mut shared = self.shared.lock().unwrap();
                if let Some(e) = shared.subscribed.get_mut(&dir) {
                    e.1 = now;
                    let mut u = url::Url::parse(page::URI).unwrap();
                    u.query_pairs_mut().append_pair("dir", &dir);
                    uris.push(u.to_string());
                }
            }
        }
        uris
    }
}

pub async fn run() -> anyhow::Result<()> {
    let server = Server::build();
    let running = server.clone().serve(rmcp::transport::stdio()).await?;
    let peer = running.peer().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let s = server.clone();
            let changed = tokio::task::spawn_blocking(move || s.changed()).await.unwrap_or_default();
            for uri in changed {
                if peer.notify_resource_updated(rmcp::model::ResourceUpdatedNotificationParam::new(uri)).await.is_err() {
                    return;
                }
            }
        }
    });
    running.waiting().await?;
    Ok(())
}
