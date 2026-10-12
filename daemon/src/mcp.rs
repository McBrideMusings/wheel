//! The lead's tools: wheeld serves MCP over streamable HTTP at `/mcp`, and the lead session is
//! started with it (ADR-0002). Every tool changes the layout through the same `AppState` methods
//! as a client op, so the lead's changes save and broadcast like anyone else's.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use agent_client_protocol::schema::v1::{HttpHeader, McpServer, McpServerHttp};
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::Deserialize;
use wheel_proto::{Dir, NewPaneKind};

use crate::server::AppState;

/// The name the lead sees the tools under.
pub const SERVER_NAME: &str = "wheel";

#[derive(Clone)]
pub struct Tools {
    state: Arc<AppState>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateSpace {
    /// Absolute path to the project's directory.
    cwd: String,
    /// Tab name; defaults to the directory's name.
    name: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SpaceRef {
    /// Space id, e.g. `s1`.
    space: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct PaneRef {
    /// Pane id, e.g. `p2`.
    pane: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SplitPane {
    /// The pane to split.
    pane: String,
    /// Which side of it the new pane goes on.
    dir: Dir,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SetPaneKind {
    pane: String,
    /// What the pane shows from now on; whatever it showed before is closed.
    kind: NewPaneKind,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct Focus {
    /// The space to show. Optional when `pane` is given.
    space: Option<String>,
    /// The pane to focus; its space is shown too.
    pane: Option<String>,
}

/// A tool's answer: the text on success, an error result the model reads on failure.
fn reply(tool: &str, result: Result<String, String>) -> CallToolResult {
    match result {
        Ok(text) => {
            tracing::info!(tool, "mcp tool ok");
            CallToolResult::success(vec![ContentBlock::text(text)])
        }
        Err(message) => {
            tracing::warn!(tool, error = %message, "mcp tool failed");
            CallToolResult::error(vec![ContentBlock::text(message)])
        }
    }
}

fn json<T: serde::Serialize>(value: &T) -> String {
    // Layout types are strings, numbers and enums with string tags: serializing cannot fail.
    serde_json::to_string_pretty(value).expect("layout serializes")
}

#[tool_router]
impl Tools {
    #[tool(
        description = "Every space in tab order, which one is showing (`active`), and each space's name, directory, pane tree and focused pane."
    )]
    fn list_spaces(&self) -> CallToolResult {
        reply("list_spaces", Ok(json(&self.state.spaces())))
    }

    #[tool(
        description = "Open a space for a project directory and show it. Returns the new space, including its one empty pane."
    )]
    fn create_space(
        &self,
        Parameters(CreateSpace { cwd, name }): Parameters<CreateSpace>,
    ) -> CallToolResult {
        let result = self.state.space_create(cwd, name).map(|s| json(&s));
        reply("create_space", result)
    }

    #[tool(description = "Close a space and every pane in it, ending their terminals.")]
    fn close_space(&self, Parameters(SpaceRef { space }): Parameters<SpaceRef>) -> CallToolResult {
        let result = self
            .state
            .space_close(&space)
            .map(|()| format!("closed {space}"));
        reply("close_space", result)
    }

    #[tool(
        description = "Split a pane in two, putting a new empty pane on the given side and focusing it. Returns the new pane's id."
    )]
    fn split_pane(
        &self,
        Parameters(SplitPane { pane, dir }): Parameters<SplitPane>,
    ) -> CallToolResult {
        reply("split_pane", self.state.pane_split(&pane, dir))
    }

    #[tool(
        description = "Close a pane, ending its terminal. Closing a space's last pane closes the space."
    )]
    fn close_pane(&self, Parameters(PaneRef { pane }): Parameters<PaneRef>) -> CallToolResult {
        let result = self
            .state
            .pane_close(&pane)
            .map(|()| format!("closed {pane}"));
        reply("close_pane", result)
    }

    #[tool(
        description = "Change what a pane shows: `terminal` starts a shell in the space's directory, `empty` clears it."
    )]
    async fn set_pane_kind(
        &self,
        Parameters(SetPaneKind { pane, kind }): Parameters<SetPaneKind>,
    ) -> CallToolResult {
        let result = self
            .state
            .pane_set_kind(&pane, kind)
            .await
            .map(|()| format!("{pane} is now {}", json(&kind).trim_matches('"')));
        reply("set_pane_kind", result)
    }

    #[tool(description = "Show a space, or focus a pane and show its space.")]
    fn focus(&self, Parameters(Focus { space, pane }): Parameters<Focus>) -> CallToolResult {
        let result = self
            .state
            .focus(space.as_deref(), pane.as_deref())
            .map(|()| "focused".to_string());
        reply("focus", result)
    }
}

#[tool_handler(
    instructions = "Spaces are the person's projects: each has a name, a directory and a split tree of panes. \
The person's messages do not say which space they are about; call list_spaces and decide from the names, directories and panes. \
A new space starts with one empty pane; set_pane_kind turns a pane into a terminal in the space's directory."
)]
impl ServerHandler for Tools {}

/// Where a process on this machine reaches `/mcp` on a daemon listening at `addr`.
pub fn url(addr: SocketAddr) -> String {
    let ip = match addr.ip() {
        ip if ip.is_unspecified() && ip.is_ipv4() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        ip if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}/mcp", SocketAddr::new(ip, addr.port()))
}

/// The server entry the lead session is started with.
pub fn for_lead(url: &str, token: &str) -> McpServer {
    McpServer::Http(
        McpServerHttp::new(SERVER_NAME, url).headers(vec![HttpHeader::new(
            "Authorization",
            format!("Bearer {token}"),
        )]),
    )
}

/// The `/mcp` route: stateless, one JSON response per request, behind the daemon token. Hosts
/// other than loopback and the address the daemon listens on are refused, against DNS rebinding.
pub fn router(state: Arc<AppState>, listen: SocketAddr) -> Router {
    let tools = Tools {
        state: state.clone(),
    };
    let service = StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(
                ["localhost", "127.0.0.1", "::1"]
                    .map(String::from)
                    .into_iter()
                    .chain([listen.ip().to_string()]),
            )
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None),
    );
    Router::new()
        .nest_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(state, authorize))
}

/// Native clients only, carrying the daemon token as a bearer header.
async fn authorize(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let via = format!("mcp {} {}", request.method(), request.uri().path());
    match state.admit(&via, request.headers(), bearer) {
        Ok(()) => next.run(request).await,
        Err(status) => status.into_response(),
    }
}
