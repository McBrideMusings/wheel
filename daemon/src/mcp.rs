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
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{schemars, tool, tool_handler, tool_router, RoleServer, ServerHandler};
use serde::Deserialize;
use wheel_proto::{Dir, NewPaneKind, PermOption};

use crate::server::AppState;
use crate::session::LEAD;

/// The name the lead sees the tools under.
pub const SERVER_NAME: &str = "wheel";

/// The most `type_into_pane` sends in one call; the whole text goes into the permission title.
const MAX_TYPED: usize = 4096;

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
    /// For `web` only, and required there: absolute path to the folder of HTML to show.
    dir: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct TypeIntoPane {
    /// A terminal pane.
    pane: String,
    /// What to type, as is.
    text: String,
    /// Press Return after the text.
    #[serde(default)]
    enter: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct StartWorker {
    /// The space whose directory the worker runs in.
    space: String,
    /// The pane to run it in; whatever it showed is closed. Without one, the space's focused
    /// pane is split and the worker runs in the new pane on its right.
    pane: Option<String>,
    /// The worker's first message: the task, in full.
    prompt: String,
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
        description = "Change what a pane shows: `terminal` starts a shell in the space's directory, `agent` starts a worker there with no task yet (start_worker gives it one), `empty` clears it, `web` shows the folder `dir` (its index.html) and reloads whenever a file in it changes."
    )]
    async fn set_pane_kind(
        &self,
        Parameters(SetPaneKind { pane, kind, dir }): Parameters<SetPaneKind>,
    ) -> CallToolResult {
        let result = self
            .state
            .pane_set_kind(&pane, kind, dir)
            .await
            .map(|_| format!("{pane} is now {}", json(&kind).trim_matches('"')));
        reply("set_pane_kind", result)
    }

    #[tool(
        description = "Type text into a terminal pane's shell, optionally pressing Return. The person approves each call first; the call returns once they answer."
    )]
    async fn type_into_pane(
        &self,
        Parameters(TypeIntoPane { pane, text, enter }): Parameters<TypeIntoPane>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        // rmcp cancels the request's token when its HTTP caller hangs up. Dropping the call then
        // withdraws its permission request, so a later answer types nothing.
        tokio::select! {
            result = self.type_into(&pane, text, enter) => reply("type_into_pane", result),
            () = context.ct.cancelled() => {
                tracing::info!(%pane, "type_into_pane caller gone");
                CallToolResult::error(vec![ContentBlock::text("the caller hung up")])
            }
        }
    }

    #[tool(
        description = "Start a worker: a coding agent session in an agent pane of a space, running in the space's directory, and give it its first message. Returns the worker's session name. The person sees the worker's conversation in the pane and talks to it there."
    )]
    async fn start_worker(
        &self,
        Parameters(StartWorker {
            space,
            pane,
            prompt,
        }): Parameters<StartWorker>,
    ) -> CallToolResult {
        let result = self.state.start_worker(&space, pane, prompt).await;
        reply("start_worker", result)
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

impl Tools {
    /// Ask the person, then send `text` to the pane's terminal if they allow it.
    async fn type_into(&self, pane: &str, mut text: String, enter: bool) -> Result<String, String> {
        // Refuse what cannot be typed before asking anyone.
        if text.len() > MAX_TYPED {
            return Err(format!(
                "text is {} bytes; type at most {MAX_TYPED} per call",
                text.len()
            ));
        }
        self.state.pane_term(pane)?;
        // Quoted and escaped, so the person sees every newline and control byte that will be sent.
        let title = format!(
            "Type into {pane}: {text:?}{}",
            if enter { " ⏎" } else { "" }
        );
        let options = [
            ("allow", "Allow", "allow_once"),
            ("deny", "Deny", "reject_once"),
        ]
        .map(|(id, name, kind)| PermOption {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
        })
        .to_vec();
        let choice = self.state.perms.ask(LEAD, title, options).await?;
        if choice != "allow" {
            return Err(format!("the person denied typing into {pane}"));
        }
        // The pane may have closed or changed kind while the request waited.
        let term = self.state.pane_term(pane)?;
        if enter {
            text.push('\r');
        }
        let bytes = text.len();
        term.write(text.into_bytes()).map_err(|e| e.to_string())?;
        // Logged by size only, like client input.
        tracing::info!(%pane, term = %term.id, bytes, "typed into pane");
        Ok(format!("typed {bytes} bytes into {pane}"))
    }
}

#[tool_handler(
    instructions = "Spaces are the person's projects: each has a name, a directory and a split tree of panes. \
The person's messages do not say which space they are about; call list_spaces and decide from the names, directories and panes. \
A new space starts with one empty pane; set_pane_kind turns a pane into a terminal in the space's directory, \
or into a web view of a folder: write HTML there (index.html first) and the pane reloads on every change. \
type_into_pane runs commands in a terminal pane once the person approves. \
start_worker hands a task to a new coding agent in a space; the person follows and answers it in its pane."
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
