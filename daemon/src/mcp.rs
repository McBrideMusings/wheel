//! The lead's tools: wheeld serves MCP over streamable HTTP at `/mcp`, and the lead session is
//! started with it (ADR-0002). Every tool changes the layout through the same `AppState` methods
//! as a client op, so the lead's changes save and broadcast like anyone else's.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

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
use serde::{Deserialize, Serialize};
use wheel_proto::{ArtifactInfo, ArtifactScope, Dir, NewPaneKind, PaneKind, PermOption, StartKind};

use crate::pty::ReadEnd;
use crate::server::AppState;
use crate::session::LEAD;

/// The name the lead sees the tools under.
pub const SERVER_NAME: &str = "wheel";

/// The most `type_into_pane` sends in one call; the whole text goes into the permission title.
const MAX_TYPED: usize = 4096;

/// The most output one terminal read returns; a longer read keeps its end.
const MAX_READ_BYTES: usize = 64 * 1024;

/// The longest a terminal read or wait lasts, in seconds.
const MAX_READ_SECS: f64 = 600.0;

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
    /// For `web` only, and required there: the name of the artifact to show. One plain name, no
    /// slash.
    artifact: Option<String>,
    /// For `web` only: which root the artifact is under. With none, the first of `local`, `repo`
    /// and `shared` that has a folder by that name.
    scope: Option<ArtifactScope>,
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
pub struct ReadTerminal {
    /// A terminal pane.
    pane: String,
    /// Stop as soon as this literal text (not a pattern) appears. Without it, the read lasts the
    /// whole `timeout_secs`.
    until: Option<String>,
    /// How long to keep reading live output, in seconds (0 to 600). Defaults to 1.
    timeout_secs: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct WaitForText {
    /// A terminal pane.
    pane: String,
    /// The literal text (not a pattern) to wait for.
    text: String,
    /// How long to wait, in seconds (0 to 600). Defaults to 30.
    timeout_secs: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct StartPane {
    /// The space whose directory it runs in.
    space: String,
    /// The pane to start it in; whatever it showed is closed. Without one, the space's focused
    /// pane is split and it starts in the new pane on its right.
    pane: Option<String>,
    /// What to start: a shell, or a coding agent.
    kind: StartKind,
    /// For `agent` only: its first message, the task in full.
    message: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ArtifactSpace {
    /// The space whose `repo` and `local` roots are looked in, alongside the machine-wide
    /// `shared` one. With none, the space being shown.
    space: Option<String>,
    /// Only the artifacts whose manifest says `pane = true`: the ones asking to be offered in a
    /// pane picker, rather than the one-offs somebody opened once by name.
    #[serde(default)]
    pane: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NewArtifact {
    /// The artifact's name: one plain name, no slash, no dot.
    name: String,
    /// Write `pane = true` into the manifest: offer this artifact in a pane picker. Leave it off
    /// for a one-off nobody needs to find in a menu.
    #[serde(default)]
    pane: bool,
    /// Which root to make it under: `repo` (committed with the project), `local` (this checkout
    /// only, gitignored) or `shared` (every project on this machine). Defaults to `repo`.
    scope: Option<ArtifactScope>,
    /// The space whose directory the `repo` and `local` roots sit under. With none, the space
    /// being shown.
    space: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OpenArtifact {
    /// The artifact to show: one plain name, no slash.
    name: String,
    /// Which root it is under. With none, the first of `local`, `repo` and `shared` that has a
    /// folder by that name.
    scope: Option<ArtifactScope>,
    /// The space to open it in. With none, the space being shown.
    space: Option<String>,
    /// The pane to show it in; whatever it showed is closed. Without one, the space's focused
    /// pane is split and the artifact opens in the new pane on its right.
    pane: Option<String>,
}

/// What `open_artifact` returns.
#[derive(Serialize)]
struct Opened {
    pane: String,
    shows: PaneKind,
}

/// What `list_artifacts` returns: every artifact in all three scopes, and what a bare name means.
#[derive(Serialize)]
struct Artifacts {
    artifacts: Vec<ArtifactInfo>,
    resolves: BTreeMap<String, ArtifactScope>,
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
    /// Read `pane`'s terminal for up to `timeout_secs`, stopping at `until`.
    async fn read_terminal_text(
        &self,
        tool: &str,
        pane: &str,
        until: Option<String>,
        timeout_secs: f64,
    ) -> Result<String, String> {
        if !(0.0..=MAX_READ_SECS).contains(&timeout_secs) {
            return Err(format!(
                "timeout_secs is {timeout_secs}; use 0 to {MAX_READ_SECS}"
            ));
        }
        let term = self.state.pane_term(pane)?;
        let started = std::time::Instant::now();
        let (bytes, end) = term
            .read_text(until.as_deref(), Duration::from_secs_f64(timeout_secs))
            .await;
        tracing::info!(tool, %pane, term = %term.id, bytes = bytes.len(), ?end,
            ms = started.elapsed().as_millis() as u64, "read terminal");
        // The end of a long read is the part the lead is waiting for.
        let kept = &bytes[bytes.len().saturating_sub(MAX_READ_BYTES)..];
        let mut text = String::from_utf8_lossy(kept).into_owned();
        if kept.len() < bytes.len() {
            text = format!(
                "[first {} bytes left out]\n{text}",
                bytes.len() - kept.len()
            );
        }
        match (end, until) {
            (ReadEnd::Matched, _) | (ReadEnd::TimedOut, None) => Ok(text),
            (ReadEnd::TimedOut, Some(until)) => Err(format!(
                "timed out after {timeout_secs}s: {until:?} did not appear in {pane}"
            )),
            (ReadEnd::Exited(code), None) => {
                Ok(format!("{text}\n[{pane}'s shell exited: {code:?}]"))
            }
            (ReadEnd::Exited(code), Some(until)) => Err(format!(
                "{pane}'s shell exited ({code:?}) before {until:?} appeared"
            )),
        }
    }

    #[tool(
        description = "Every space in tab order, which one is showing (`active`), each space's name, directory, pane tree and focused pane. An agent pane carries its `session` name and its `state`: `starting`, `ready`, `busy`, `failed` (its agent stopped and is being started again) or `gave_up` with `tries` (its agent never started; the pane can be closed)."
    )]
    fn list_spaces(&self) -> CallToolResult {
        reply("list_spaces", Ok(json(&self.state.listing())))
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
        description = "Change what a pane shows: `terminal` starts a shell in the space's directory, `empty` clears it, `web` shows the artifact named by `artifact` (its index.html) and reloads whenever a file in its folder changes. An artifact is named, never pathed: give one plain name, and `scope` (`repo`, `local` or `shared`) to pin which root it is under."
    )]
    async fn set_pane_kind(
        &self,
        Parameters(SetPaneKind {
            pane,
            kind,
            artifact,
            scope,
        }): Parameters<SetPaneKind>,
    ) -> CallToolResult {
        let result = self
            .state
            .pane_set_kind(&pane, kind, artifact, scope, None)
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
        description = "Read a terminal pane: the screen as the daemon holds it (with its escape sequences), then live output until `until` appears or `timeout_secs` passes. Returns everything read. Needs no approval. With `until`, an error when the time runs out first."
    )]
    async fn read_terminal(
        &self,
        Parameters(ReadTerminal {
            pane,
            until,
            timeout_secs,
        }): Parameters<ReadTerminal>,
    ) -> CallToolResult {
        let result = self
            .read_terminal_text("read_terminal", &pane, until, timeout_secs.unwrap_or(1.0))
            .await;
        reply("read_terminal", result)
    }

    #[tool(
        description = "Wait until literal `text` appears in a terminal pane. Output already on the screen counts, so a command typed earlier is found. Returns everything read up to the match; an error naming the pane when `timeout_secs` passes first. Needs no approval."
    )]
    async fn wait_for_text(
        &self,
        Parameters(WaitForText {
            pane,
            text,
            timeout_secs,
        }): Parameters<WaitForText>,
    ) -> CallToolResult {
        let result = if text.is_empty() {
            Err("text is empty".to_string())
        } else {
            self.read_terminal_text(
                "wait_for_text",
                &pane,
                Some(text),
                timeout_secs.unwrap_or(30.0),
            )
            .await
        };
        reply("wait_for_text", result)
    }

    #[tool(
        description = "Start a shell (`terminal`) or a coding agent (`agent`) in a pane of a space, running in the space's directory. An agent takes `message`, its first message, the task in full. For an agent it waits up to a minute for the agent to be ready, then returns `{pane, session, state}`: the agent pane's session name and its state (`ready`, `busy`, or, if the minute ran out first, `starting` or `failed` while it is still being retried). An error when the agent could not start. The person sees the agent's conversation in the pane and talks to it there."
    )]
    async fn start_pane(
        &self,
        Parameters(StartPane {
            space,
            pane,
            kind,
            message,
        }): Parameters<StartPane>,
    ) -> CallToolResult {
        let result = self.state.pane_start(&space, pane, kind, message).await;
        reply("start_pane", result.map(|s| json(&s)))
    }

    #[tool(
        description = "Every artifact this project and this machine have, in all three scopes, each with its `scope`, `name`, resolved `dir`, `title`, the refresh `command` and `every` its artifact.toml declares, whether it asks to be offered in a pane picker (`pane`), and any `problems` with it — plus `resolves`, naming the scope each bare name lands in. Pass `pane: true` for only the artifacts asking to be offered in a pane picker, which is what a picker lists; without it you get those and the one-offs. An artifact is a folder holding index.html and an artifact.toml manifest, shown by a web pane."
    )]
    fn list_artifacts(
        &self,
        Parameters(ArtifactSpace { space, pane }): Parameters<ArtifactSpace>,
    ) -> CallToolResult {
        let result = self
            .state
            .artifact_ls(space.as_deref(), pane)
            .map(|(artifacts, resolves)| {
                json(&Artifacts {
                    artifacts,
                    resolves,
                })
            });
        reply("list_artifacts", result)
    }

    #[tool(
        description = "Make a new artifact: its folder, an artifact.toml naming it and a placeholder index.html, under the root the scope names. Refused when an artifact of that name is already there, so nothing one holds is overwritten. Pass `pane: true` to offer it in a pane picker, which is what you want for anything the person will open again; leave it off for a one-off. The page it leaves is a placeholder: writing the real page and declaring a refresh command are edits to those two files, which an agent pane in the space that owns the project should make (start_pane) — it can run the command and see the result. Open the artifact once that agent is done."
    )]
    fn new_artifact(
        &self,
        Parameters(NewArtifact {
            name,
            scope,
            space,
            pane,
        }): Parameters<NewArtifact>,
    ) -> CallToolResult {
        let result = self
            .state
            .artifact_new(&name, scope, space.as_deref(), pane)
            .map(|a| json(&a));
        reply("new_artifact", result)
    }

    #[tool(
        description = "Show an artifact in a pane: in `pane` when one is given, otherwise in a new pane split right of the space's focused pane. Returns the pane and what it now shows. The page is served read-only and reloads whenever a file in its folder changes. An artifact whose manifest declares a refresh command raises a permission request on open, which the person answers — opening it is not approving it."
    )]
    async fn open_artifact(
        &self,
        Parameters(OpenArtifact {
            name,
            scope,
            space,
            pane,
        }): Parameters<OpenArtifact>,
    ) -> CallToolResult {
        let result = self
            .state
            .open_artifact(&name, scope, space.as_deref(), pane)
            .await
            .map(|(pane, shows)| json(&Opened { pane, shows }));
        reply("open_artifact", result)
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
type_into_pane runs commands in a terminal pane once the person approves; \
wait_for_text and read_terminal then show what it printed, so a command is never typed blind. \
start_pane starts a shell or hands a task to a new coding agent in a space; the person follows and answers an agent in its pane."
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
