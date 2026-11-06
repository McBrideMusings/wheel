//! `wheel-runner serve`: the runner over MCP on standard input and output, for a host. One
//! process serves every project the host has open; the host names the project in `_meta`
//! `wheel/dir` on each call. A run belongs to this process, not to a page: it keeps going and its
//! status and log stay readable after the page that started it closes.

use rmcp::handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult, PaginatedRequestParams,
    RequestMetaObject, ServerCapabilities, ServerConfig,
};
use rmcp::{schemars, service::RequestContext, tool, tool_router, ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{json, Value};

use crate::config;
use crate::runs::{Runs, StartError};
use crate::status;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Named {
    /// The `[runner]` entry's name.
    name: String,
    /// True once the person has answered the entry's `confirm` question with yes.
    confirmed: Option<bool>,
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

#[derive(Clone)]
struct Server {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    fn build() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    #[tool(description = "The commands the project's [runner] table names, and this machine's")]
    fn list_runners(&self, meta: RequestMetaObject) -> CallToolResult {
        reply(json!(config::list(&config::machine_file(), &project(&meta))))
    }

    #[tool(description = "Start a [runner] command. An entry with a confirm question answers {started:false, confirm} until it is started again with confirmed true")]
    fn run_runner(&self, meta: RequestMetaObject, Parameters(a): Parameters<Named>) -> CallToolResult {
        let dir = project(&meta);
        let Some(runner) = config::list(&config::machine_file(), &dir).into_iter().find(|r| r.name == a.name) else {
            return refuse(format!("no runner named {}", a.name));
        };
        match Runs::new(&dir).start(&runner, a.confirmed == Some(true), None) {
            Ok(s) => reply(json!({ "started": true, "id": s.record.id, "log": s.record.log })),
            Err(StartError::Confirm(q)) => reply(json!({ "started": false, "confirm": q })),
            Err(StartError::Failed(e)) => refuse(e),
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
        reply(json!({ "line": status::line(&Runs::new(&project(&meta)).list(), crate::now()) }))
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
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

pub async fn run() -> anyhow::Result<()> {
    Server::build().serve(rmcp::transport::stdio()).await?.waiting().await?;
    Ok(())
}
