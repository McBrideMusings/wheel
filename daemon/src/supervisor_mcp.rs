//! A supervisor's tools: wheeld serves MCP at `/mcp-supervisor/<id>` for the supervisor with that
//! id, and its agent is started with it (ADR-0007). The agent settles the items in its inbox —
//! answer the worker, close the item, or forward it to the lead — and can touch nothing else.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::Deserialize;

use crate::mcp::{authorize, json, reply, SUPERVISOR_ROUTE};
use crate::server::AppState;

#[derive(Clone)]
pub struct SupervisorTools {
    state: Arc<AppState>,
    /// The supervisor these tools act for.
    id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MessageWorker {
    /// The inbox item's id, e.g. `s1`.
    item: String,
    /// What the worker needs to go on. It starts again in its worktree with this text; a bead it
    /// had set blocked is open again.
    text: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CloseItem {
    /// The inbox item's id, e.g. `s1`.
    item: String,
    /// Why nothing needs doing.
    note: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ForwardToLead {
    /// The inbox item's id, e.g. `s1`.
    item: String,
    /// One line for the lead: what is stuck and what only it or the person can decide.
    summary: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct FileBead {
    /// The inbox item's id, e.g. `s1`.
    item: String,
    /// The new bead's title.
    title: String,
    /// The new bead's description: what to do, where, and why.
    body: String,
    /// Labels from the repo's `.beads/labels.toml`.
    labels: Vec<String>,
}

#[tool_router]
impl SupervisorTools {
    #[tool(
        description = "The items waiting in your inbox, oldest first. Each has an `id`, the `worker` and `bead` it is about, a `kind` (`blocked`, `escalated` or `follow_up`), a `detail`, and the tail of the worker's `last_turn`."
    )]
    fn list_inbox(&self) -> CallToolResult {
        reply(
            "list_inbox",
            self.state.supervisor_inbox(&self.id).map(|i| json(&i)),
        )
    }

    #[tool(
        description = "Answer the worker behind an inbox item. The worker starts again in its worktree with your text, its bead open, and the item leaves your inbox."
    )]
    async fn message_worker(
        &self,
        Parameters(MessageWorker { item, text }): Parameters<MessageWorker>,
    ) -> CallToolResult {
        reply(
            "message_worker",
            self.state.supervisor_answer(&self.id, &item, text).await,
        )
    }

    #[tool(
        description = "File a `follow_up` inbox item (any other kind is refused) as a new bead in the project's tracker. It records that the bead was discovered while working the item's bead, returns the new id, and drops the item. A label that is not in the repo's .beads/labels.toml is refused."
    )]
    async fn file_bead(
        &self,
        Parameters(FileBead {
            item,
            title,
            body,
            labels,
        }): Parameters<FileBead>,
    ) -> CallToolResult {
        reply(
            "file_bead",
            self.state
                .supervisor_file_bead(&self.id, &item, title, body, labels)
                .await,
        )
    }

    #[tool(description = "Drop an inbox item when nothing needs doing about it.")]
    fn close_item(
        &self,
        Parameters(CloseItem { item, note }): Parameters<CloseItem>,
    ) -> CallToolResult {
        reply(
            "close_item",
            self.state.supervisor_close_item(&self.id, &item, note),
        )
    }

    #[tool(
        description = "Forward an inbox item to the lead's inbox as an escalation with a one-line summary, and drop it from yours. Use it when only the lead or the person can settle the item."
    )]
    fn forward_to_lead(
        &self,
        Parameters(ForwardToLead { item, summary }): Parameters<ForwardToLead>,
    ) -> CallToolResult {
        reply(
            "forward_to_lead",
            self.state.supervisor_forward(&self.id, &item, summary),
        )
    }
}

#[tool_handler(
    instructions = "You supervise workers on beads. list_inbox shows what waits on you. message_worker answers a worker, \
which starts again with your answer; close_item drops an item that needs nothing; forward_to_lead hands an item to the \
lead with a one-line summary when only the lead or the person can settle it; file_bead files a follow_up item as a new bead."
)]
impl ServerHandler for SupervisorTools {}

#[derive(Clone)]
struct Served {
    state: Arc<AppState>,
    listen: SocketAddr,
}

/// The `/mcp-supervisor/<id>` route, behind the daemon token like `/mcp`.
pub fn router(state: Arc<AppState>, listen: SocketAddr) -> Router {
    let served = Served {
        state: state.clone(),
        listen,
    };
    Router::new()
        .route(&format!("{SUPERVISOR_ROUTE}/{{id}}"), any(handle))
        .with_state(served)
        .route_layer(middleware::from_fn_with_state(state, authorize))
}

async fn handle(
    State(served): State<Served>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    if served.state.supervisor_show(&id).is_err() {
        return (StatusCode::NOT_FOUND, format!("no supervisor {id}")).into_response();
    }
    let tools = SupervisorTools {
        state: served.state.clone(),
        id,
    };
    let service = StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(
                ["localhost", "127.0.0.1", "::1"]
                    .map(String::from)
                    .into_iter()
                    .chain([served.listen.ip().to_string()]),
            )
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None),
    );
    service.handle(request).await.map(axum::body::Body::new)
}
