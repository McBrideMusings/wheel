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
use wheel_proto::{
    ArtifactInfo, ArtifactScope, CreatedBy, Dir, InboxState, NewPaneKind, PaneKind, PermOption,
    StartKind,
};

use crate::agents::ReadSpan;
use crate::beads::{bead_id, push_flags};
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
    /// Leave the person's focus and the space being shown as they are. Defaults to true; pass
    /// false to focus the new pane and show its space.
    no_focus: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SetPaneKind {
    pane: String,
    /// What the pane shows from now on; whatever it showed before is closed.
    kind: NewPaneKind,
    /// For `web` only, and required there: the name of the artifact to show. One plain name, no
    /// slash.
    artifact: Option<String>,
    /// For `web` only: which root the artifact is under. With none, `project` if it has a folder by
    /// that name, else `machine`.
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
pub struct AgentTarget {
    /// An agent pane's id (e.g. `p2`) or its session name (e.g. `w5`).
    agent: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ReadAgent {
    /// An agent pane's id or its session name.
    agent: String,
    /// Return the last this many turns (1 or more). Without this, `grep` or `all`, the last turn.
    last_turns: Option<u64>,
    /// Return every message and tool line containing this text (not a pattern; case ignored),
    /// across all turns, each tagged `[turn N]`. It matches the line as printed, so the `> `,
    /// `[tool]` and `(queued)` markers count.
    grep: Option<String>,
    /// Return the whole stored transcript.
    all: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MessageAgent {
    /// An agent pane's id or its session name.
    agent: String,
    /// The message to queue.
    text: String,
    /// Wait up to this many seconds (0 to 3600) for the reply to finish and return it. Without
    /// it, return as soon as the message is queued.
    wait_secs: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct WaitForAgent {
    /// An agent pane's id or its session name.
    agent: String,
    /// How long to wait, in seconds (0 to 3600). Defaults to 60.
    timeout_secs: Option<f64>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListInbox {
    /// Include closed items too. Defaults to false.
    include_closed: Option<bool>,
}

/// Who gives an `answer_forward` answer.
#[derive(Deserialize, schemars::JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ForwardAnswerer {
    Lead,
    Person,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct AnswerForward {
    /// The lead inbox item's id, e.g. `i3`: an open `escalation` that names a supervisor.
    item: String,
    /// What the blocked worker needs to go on.
    answer: String,
    /// Required when `from` is `lead`: the decision bead, ADR, or earlier words of the person the
    /// answer rests on.
    source: Option<String>,
    /// `lead` when you decide it; `person` when the person told you the answer.
    from: ForwardAnswerer,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SetInboxItem {
    /// The item's id, e.g. `i3`.
    id: String,
    /// Where it stands now: `seen`, `surfaced` or `closed`. An item only moves forward.
    state: InboxState,
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
    /// When a pane is split for it: leave the person's focus and the space being shown as they
    /// are. Defaults to true; pass false to focus the new pane and show its space.
    no_focus: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct StartSupervisor {
    /// The space whose project it supervises. With none, the space being shown.
    space: Option<String>,
    /// The epic whose descendants it works, e.g. `wheel-lr2`.
    goal: Option<String>,
    /// A bead label; only beads carrying it are in scope. Give a goal, a focus, or both.
    focus: Option<String>,
    /// How many workers it runs at once. Defaults to the `supervisor.workers` setting (3).
    workers: Option<u32>,
    /// The share of the usage limit, 1 to 100, it may spend before it holds. Defaults to the `supervisor.cap` setting (90); 100 is effectively uncapped.
    cap: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SupervisorRef {
    /// A supervisor's id, e.g. `wheel-lr2-u7`.
    id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SetSupervisorWorkers {
    id: String,
    /// How many workers it runs at once, 1 or more.
    workers: u32,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SetSupervisorCap {
    id: String,
    /// The share of the usage limit, 1 to 100, it may spend before it holds; omit to remove the
    /// cap.
    cap: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ArtifactSpace {
    /// The space whose `project` root is looked in, alongside the `machine` one. With none, the space being shown.
    space: Option<String>,
    /// Only the artifacts whose entry says `pane = true`: the ones asking to be offered in a
    /// pane picker, rather than the one-offs somebody opened once by name.
    #[serde(default)]
    pane: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NewArtifact {
    /// The artifact's name: one plain name, no slash, no dot.
    name: String,
    /// Write `pane = true` into the entry: offer this artifact in a pane picker. Leave it off
    /// for a one-off nobody needs to find in a menu.
    #[serde(default)]
    pane: bool,
    /// Which root to make it under: `project` (in the project's directory) or `machine` (every
    /// project on this machine). Defaults to `project`.
    scope: Option<ArtifactScope>,
    /// The space whose directory the `project` root sits under. With none, the space
    /// being shown.
    space: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct AddArtifact {
    /// The page to register, as an absolute path: a folder holding index.html, or one HTML file.
    /// It can be anywhere on this machine; nothing in its folder is changed.
    path: String,
    /// One plain name. With none, the folder's name, or the file's without its extension.
    name: Option<String>,
    /// Whose wheel.toml holds the entry: `project` (the space's, the default) or `machine` (every
    /// project on this machine).
    scope: Option<ArtifactScope>,
    /// The space whose wheel.toml holds a `project` entry. With none, the space being shown.
    space: Option<String>,
    /// Shown wherever the artifact is listed; the name with none.
    title: Option<String>,
    /// A refresh command, run in the space's directory once the person approves it.
    command: Option<String>,
    /// Seconds between timed refreshes; needs `command`.
    every: Option<u64>,
    /// Offer this artifact in a pane picker.
    #[serde(default)]
    pane: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct RemoveArtifact {
    /// The artifact whose entry to delete.
    name: String,
    /// Whose wheel.toml to delete it from. With none, the scope the bare name resolves to.
    scope: Option<ArtifactScope>,
    /// The space whose wheel.toml holds a `project` entry. With none, the space being shown.
    space: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OpenArtifact {
    /// The artifact to show: one plain name, no slash.
    name: String,
    /// Which root it is under. With none, `project` if its wheel.toml has an entry by that name, else
    /// `machine`.
    scope: Option<ArtifactScope>,
    /// The space to open it in. With none, the space being shown.
    space: Option<String>,
    /// The pane to show it in; whatever it showed is closed. Without one, the space's focused
    /// pane is split and the artifact opens in the new pane on its right.
    pane: Option<String>,
    /// When a pane is split for it: leave the person's focus and the space being shown as they
    /// are. Defaults to true; pass false to focus the new pane and show its space.
    no_focus: Option<bool>,
}

/// What `open_artifact` returns.
#[derive(Serialize)]
struct Opened {
    pane: String,
    shows: PaneKind,
}

/// What `list_artifacts` returns: every artifact in both scopes, and what a bare name means.
#[derive(Serialize)]
struct Artifacts {
    artifacts: Vec<ArtifactInfo>,
    resolves: BTreeMap<String, ArtifactScope>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListBeads {
    /// The space whose directory holds the beads.
    space: String,
    /// Only open beads with no active blocker, as `bd ready` lists them. Cannot be combined with
    /// `status` or `all`.
    #[serde(default)]
    ready: bool,
    /// Only beads in this status: `open`, `in_progress`, `blocked`, `deferred` or `closed`;
    /// several comma-separated. Without it, closed beads are left out.
    status: Option<String>,
    /// Include closed beads too.
    #[serde(default)]
    all: bool,
    /// Only beads with all of these labels.
    labels: Option<Vec<String>>,
    /// Only beads assigned to this person or agent.
    assignee: Option<String>,
    /// Only the children of this bead.
    parent: Option<String>,
    /// Only this type: `bug`, `feature`, `task`, `epic`, `chore` or `decision`.
    kind: Option<String>,
    /// Only this priority, 0 (highest) to 4.
    priority: Option<u8>,
    /// The most beads to return. Defaults to 50; 0 for all.
    limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct BeadRef {
    /// The space whose directory holds the beads.
    space: String,
    /// The bead's id, e.g. `wheel-2n5.5`.
    id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateBead {
    /// The space whose directory holds the beads.
    space: String,
    title: String,
    description: Option<String>,
    /// `bug`, `feature`, `task`, `epic`, `chore` or `decision`. Defaults to `task`.
    kind: Option<String>,
    /// 0 (highest) to 4. Defaults to 2.
    priority: Option<u8>,
    labels: Option<Vec<String>>,
    /// Make this a child of that bead.
    parent: Option<String>,
    assignee: Option<String>,
    acceptance: Option<String>,
    notes: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct UpdateBead {
    /// The space whose directory holds the beads.
    space: String,
    id: String,
    title: Option<String>,
    description: Option<String>,
    /// `open`, `in_progress`, `blocked`, `deferred` or `closed`.
    status: Option<String>,
    kind: Option<String>,
    priority: Option<u8>,
    assignee: Option<String>,
    acceptance: Option<String>,
    /// Replace the notes.
    notes: Option<String>,
    /// Add to the notes, after what is there.
    append_notes: Option<String>,
    add_labels: Option<Vec<String>>,
    remove_labels: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct Focus {
    /// The space to show. Optional when `pane` is given.
    space: Option<String>,
    /// The pane to focus; its space is shown too.
    pane: Option<String>,
}

/// A tool's answer: the text on success, an error result the model reads on failure.
pub(crate) fn reply(tool: &str, result: Result<String, String>) -> CallToolResult {
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

/// Whether a pane-making verb moves the person's view: only when the lead passes `no_focus: false`.
fn focuses(no_focus: Option<bool>) -> bool {
    !no_focus.unwrap_or(true)
}

pub(crate) fn json<T: serde::Serialize>(value: &T) -> String {
    // Layout types are strings, numbers and enums with string tags: serializing cannot fail.
    serde_json::to_string_pretty(value).expect("layout serializes")
}

#[tool_router]
impl Tools {
    /// Read `pane`'s terminal for up to `timeout_secs`, stopping at `until`.
    async fn read_terminal_text(
        &self,
        cx: &RequestContext<RoleServer>,
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
        self.state.pane_term(pane)?;
        self.ask_if_persons(cx, tool, &[pane.to_string()], None)
            .await?;
        // The pane may have closed or changed kind while the request waited.
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
        description = "Open a space for a project directory and show it. The person approves each call first, and it names the directory; the call returns once they answer. Returns the new space, including its one empty pane."
    )]
    async fn create_space(
        &self,
        Parameters(CreateSpace { cwd, name }): Parameters<CreateSpace>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        reply(
            "create_space",
            self.create_space_asking(&context, cwd, name).await,
        )
    }

    #[tool(
        description = "Close a space and every pane in it, ending their terminals. When the person made any of its panes, they approve the call first."
    )]
    async fn close_space(
        &self,
        Parameters(SpaceRef { space }): Parameters<SpaceRef>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            let panes: Vec<String> = self
                .state
                .space_creators(&space)?
                .into_iter()
                .map(|(pane, _)| pane)
                .collect();
            let approved = self
                .ask_if_persons(&context, &format!("close_space {space}"), &panes, None)
                .await?;
            // Checked again under the layout's lock: a pane the person made while the request
            // waited was not part of what they answered.
            self.state
                .space_close(&space, Some(&approved))
                .map(|()| format!("closed {space}"))
        };
        reply("close_space", work.await)
    }

    #[tool(
        description = "Split a pane in two, putting a new empty pane on the given side. The person's focus stays where it is unless `no_focus` is false. Returns the new pane's id."
    )]
    fn split_pane(
        &self,
        Parameters(SplitPane {
            pane,
            dir,
            no_focus,
        }): Parameters<SplitPane>,
    ) -> CallToolResult {
        reply(
            "split_pane",
            self.state
                .pane_split(&pane, dir, CreatedBy::Lead, focuses(no_focus)),
        )
    }

    #[tool(
        description = "Close a pane, ending its terminal. Closing a space's last pane closes the space. On a pane the person made, they approve the call first."
    )]
    async fn close_pane(
        &self,
        Parameters(PaneRef { pane }): Parameters<PaneRef>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            self.ask_if_persons(&context, "close_pane", std::slice::from_ref(&pane), None)
                .await?;
            self.state
                .pane_close(&pane)
                .map(|()| format!("closed {pane}"))
        };
        reply("close_pane", work.await)
    }

    #[tool(
        description = "Change what a pane shows: `terminal` starts a shell in the space's directory, `empty` clears it, `web` shows the artifact named by `artifact` (its index.html) and reloads whenever a file in its folder changes. An artifact is named, never pathed: give one plain name, and `scope` (`project` or `machine`) to pin which root it is under. On a pane the person made, they approve the call first."
    )]
    async fn set_pane_kind(
        &self,
        Parameters(SetPaneKind {
            pane,
            kind,
            artifact,
            scope,
        }): Parameters<SetPaneKind>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            self.ask_if_persons(&context, "set_pane_kind", std::slice::from_ref(&pane), None)
                .await?;
            self.state
                .pane_set_kind(&pane, kind, artifact, scope, None)
                .await
                .map(|_| format!("{pane} is now {}", json(&kind).trim_matches('"')))
        };
        reply("set_pane_kind", work.await)
    }

    #[tool(
        description = "Type text into a terminal pane's shell, optionally pressing Return. On a pane the person made, they approve each call first, seeing the text; the call returns once they answer."
    )]
    async fn type_into_pane(
        &self,
        Parameters(TypeIntoPane { pane, text, enter }): Parameters<TypeIntoPane>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        reply(
            "type_into_pane",
            self.type_into(&context, &pane, text, enter).await,
        )
    }

    #[tool(
        description = "Read a terminal pane: the screen as the daemon holds it (with its escape sequences), then live output until `until` appears or `timeout_secs` passes. Returns everything read. With `until`, an error when the time runs out first. On a pane the person made, they approve the call first."
    )]
    async fn read_terminal(
        &self,
        Parameters(ReadTerminal {
            pane,
            until,
            timeout_secs,
        }): Parameters<ReadTerminal>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = self.read_terminal_text(
            &context,
            "read_terminal",
            &pane,
            until,
            timeout_secs.unwrap_or(1.0),
        );
        reply("read_terminal", work.await)
    }

    #[tool(
        description = "Wait until literal `text` appears in a terminal pane. Output already on the screen counts, so a command typed earlier is found. Returns everything read up to the match; an error naming the pane when `timeout_secs` passes first. On a pane the person made, they approve the call first."
    )]
    async fn wait_for_text(
        &self,
        Parameters(WaitForText {
            pane,
            text,
            timeout_secs,
        }): Parameters<WaitForText>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            if text.is_empty() {
                return Err("text is empty".to_string());
            }
            self.read_terminal_text(
                &context,
                "wait_for_text",
                &pane,
                Some(text),
                timeout_secs.unwrap_or(30.0),
            )
            .await
        };
        reply("wait_for_text", work.await)
    }

    #[tool(
        description = "Start a shell (`terminal`) or a coding agent (`agent`) in a pane of a space, running in the space's directory. An agent pane is for work that is not a bead (exploring code, a spike, a question about a codebase); implementation goes as beads and start_supervisor. Given `pane`, it replaces what that pane shows, and on a pane the person made they approve the call first; without one, it gets a new pane and the person's focus stays where it is unless `no_focus` is false. An agent takes `message`, its first message, the task in full. For an agent it waits up to a minute for the agent to be ready, then returns `{pane, session, state}`: the agent pane's session name and its state (`ready`, `busy`, or, if the minute ran out first, `starting` or `failed` while it is still being retried). An error when the agent could not start. The person sees the agent's conversation in the pane and talks to it there."
    )]
    async fn start_pane(
        &self,
        Parameters(StartPane {
            space,
            pane,
            kind,
            message,
            no_focus,
        }): Parameters<StartPane>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            if let Some(pane) = &pane {
                self.ask_about_pane_in(&context, "start_pane", pane, Some(&space))
                    .await?;
            }
            self.state
                .pane_start(
                    &space,
                    pane,
                    kind,
                    message,
                    CreatedBy::Lead,
                    focuses(no_focus),
                )
                .await
                .map(|s| json(&s))
        };
        reply("start_pane", work.await)
    }

    #[tool(
        description = "Read an agent pane's conversation: each message sent (`> `), the agent's reply and one line per tool call, with the session's state. `agent` is the pane id or the session name; the result names both. By default only the last turn; `last_turns: N` the last N turns; `grep: text` every message and tool line containing the text across all turns, each tagged `[turn N]`; `all: true` the whole transcript (give at most one of the three). The result's `turns` is how many turns the conversation has and `shown` which of them the transcript holds. A result over 64 KiB keeps its newest messages. On a pane the person made, they approve the call first."
    )]
    async fn read_agent(
        &self,
        Parameters(ReadAgent {
            agent,
            last_turns,
            grep,
            all,
        }): Parameters<ReadAgent>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            let span = ReadSpan::from_args(last_turns, grep, all)?;
            self.ask_about_agent(&context, "read_agent", &agent).await?;
            self.state.agent_read(&agent, span).map(|r| json(&r))
        };
        reply("read_agent", work.await)
    }

    #[tool(
        description = "Queue a message for an agent pane's agent; it runs after any turn in progress. With `wait_secs`, wait for the reply to finish and return it (`finished: false` with the reply so far if the time ran out; the message still runs). An error naming the state when the session has `gave_up` or `failed`; nothing is queued then. Returns the pane, the session and the turn. On a pane the person made, they approve the call first, seeing the message."
    )]
    async fn message_agent(
        &self,
        Parameters(MessageAgent {
            agent,
            text,
            wait_secs,
        }): Parameters<MessageAgent>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            let wait = match wait_secs {
                None => None,
                Some(s) if (0.0..=3600.0).contains(&s) => Some(Duration::from_secs_f64(s)),
                Some(s) => return Err(format!("wait_secs is {s}; use 0 to 3600")),
            };
            let (pane, _) = self.state.resolve_agent_pane(&agent)?;
            self.ask_if_persons(
                &context,
                "message_agent",
                &[pane],
                Some(format!("{text:?}")),
            )
            .await?;
            self.state
                .agent_message(&agent, text, wait)
                .await
                .map(|m| json(&m))
        };
        reply("message_agent", work.await)
    }

    #[tool(
        description = "Wait until an agent pane's session is `ready` (no turn running or queued). Returns the pane, the session and its state. An error when `timeout_secs` passes first, or at once when the session has `gave_up`. On a pane the person made, they approve the call first."
    )]
    async fn wait_for_agent(
        &self,
        Parameters(WaitForAgent {
            agent,
            timeout_secs,
        }): Parameters<WaitForAgent>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            let secs = timeout_secs.unwrap_or(60.0);
            if !(0.0..=3600.0).contains(&secs) {
                return Err(format!("timeout_secs is {secs}; use 0 to 3600"));
            }
            self.ask_about_agent(&context, "wait_for_agent", &agent)
                .await?;
            let w = self
                .state
                .agent_wait(&agent, Duration::from_secs_f64(secs))
                .await?;
            if w.ready {
                Ok(json(&w))
            } else {
                Err(format!(
                    "timed out after {secs}s: {} in {} is not ready: {}",
                    w.session,
                    w.pane,
                    serde_json::to_string(&w.state).unwrap_or_default()
                ))
            }
        };
        reply("wait_for_agent", work.await)
    }

    #[tool(
        description = "Stop an agent pane's current turn (or, with none running, the next queued message). Messages behind it still run. An error when there is no turn to cancel. Returns the pane, the session and the turn stopped. On a pane the person made, they approve the call first."
    )]
    async fn cancel_agent(
        &self,
        Parameters(AgentTarget { agent }): Parameters<AgentTarget>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            self.ask_about_agent(&context, "cancel_agent", &agent)
                .await?;
            self.state.agent_cancel(&agent).map(|c| json(&c))
        };
        reply("cancel_agent", work.await)
    }

    #[tool(
        description = "The inbox: what waits on agent panes and supervisors. Each item has an `id`, its `source` (space, pane and session), a `kind` (`turn_ended`, `permission`, `failed` or `escalation`, which a supervisor forwarded and names in `supervisor`), a `time` in milliseconds, a `state` (`new`, `seen`, `surfaced` or `closed`) and a `detail`. Open items first, oldest first; closed ones only with `include_closed`. A person's message to you starts with `[inbox: N new — space k, …]` when N items are new."
    )]
    fn list_inbox(
        &self,
        Parameters(ListInbox { include_closed }): Parameters<ListInbox>,
    ) -> CallToolResult {
        let include = include_closed.unwrap_or(false);
        let items: Vec<_> = self
            .state
            .inbox_list()
            .into_iter()
            .filter(|i| include || i.state != InboxState::Closed)
            .collect();
        reply("list_inbox", Ok(json(&items)))
    }

    #[tool(
        description = "Move an inbox item to `seen`, `surfaced` (you told the person) or `closed` (nothing waits on it any more). An item only moves forward. Returns the item."
    )]
    fn set_inbox_item(
        &self,
        Parameters(SetInboxItem { id, state }): Parameters<SetInboxItem>,
    ) -> CallToolResult {
        reply(
            "set_inbox_item",
            self.state.inbox_set(&id, state).map(|i| json(&i)),
        )
    }

    #[tool(
        description = "Answer the blocked worker behind an `escalation` inbox item a supervisor forwarded. The worker starts again in its worktree with your answer, its bead open and an `answered:` note on it; the answer is recorded with the supervisor as given by `from` (`lead` or `person`), and the item closes in your inbox and the supervisor's. `source` is required when `from` is `lead`: the decision bead, ADR or earlier words of the person it rests on. An item that is not an open escalation naming a supervisor, or whose worker is gone, is an error."
    )]
    async fn answer_forward(
        &self,
        Parameters(AnswerForward {
            item,
            answer,
            source,
            from,
        }): Parameters<AnswerForward>,
    ) -> CallToolResult {
        let from = match from {
            ForwardAnswerer::Lead => wheel_proto::AnswerFrom::Lead,
            ForwardAnswerer::Person => wheel_proto::AnswerFrom::Person,
        };
        reply(
            "answer_forward",
            self.state.answer_forward(&item, answer, source, from).await,
        )
    }

    #[tool(
        description = "Start a supervisor in a new pane beside the focused one of a space; this is how implementation starts, on beads written with create_bead, even for one bead: a pane that pursues one goal (an epic, whose descendants it works), one focus (a bead label), or both, with its own agent in the space's directory. `workers` and `cap` default to the `supervisor.workers` and `supervisor.cap` settings (3 and 90), not to an uncapped supervisor. Returns the supervisor at once: its `id`, scope, `workers`, `cap`, `state` (`running`, `paused`, `held` or `stopped`), `pane` and `agent` state. Its agent is woken with the scope's ready beads and answers with an ordered `queue` (a bead and a model each), which shows in the supervisor a moment later; `plan_error` says why when the answer was unusable. The new pane leaves the person's focus alone."
    )]
    async fn start_supervisor(
        &self,
        Parameters(StartSupervisor {
            space,
            goal,
            focus,
            workers,
            cap,
        }): Parameters<StartSupervisor>,
    ) -> CallToolResult {
        reply(
            "start_supervisor",
            self.state
                .supervisor_start(space, goal, focus, workers, cap, CreatedBy::Lead)
                .await
                .map(|s| json(&s)),
        )
    }

    #[tool(
        description = "Pause a running or held supervisor. Returns the supervisor. On a supervisor the person started, they approve the call first."
    )]
    async fn pause_supervisor(
        &self,
        Parameters(SupervisorRef { id }): Parameters<SupervisorRef>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            self.ask_about_supervisor(&context, "pause_supervisor", &id)
                .await?;
            self.state.supervisor_pause(&id).map(|s| json(&s))
        };
        reply("pause_supervisor", work.await)
    }

    #[tool(
        description = "Resume a paused or held supervisor. Returns the supervisor. On a supervisor the person started, they approve the call first."
    )]
    async fn resume_supervisor(
        &self,
        Parameters(SupervisorRef { id }): Parameters<SupervisorRef>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            self.ask_about_supervisor(&context, "resume_supervisor", &id)
                .await?;
            self.state.supervisor_resume(&id).map(|s| json(&s))
        };
        reply("resume_supervisor", work.await)
    }

    #[tool(
        description = "Stop a supervisor for good: its agent ends and its pane keeps showing it as `stopped`; start a new one to supervise again. Returns the supervisor. On a supervisor the person started, they approve the call first."
    )]
    async fn stop_supervisor(
        &self,
        Parameters(SupervisorRef { id }): Parameters<SupervisorRef>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            self.ask_about_supervisor(&context, "stop_supervisor", &id)
                .await?;
            self.state.supervisor_stop(&id).map(|s| json(&s))
        };
        reply("stop_supervisor", work.await)
    }

    #[tool(
        description = "Set how many workers a supervisor runs at once (1 or more). Returns the supervisor. On a supervisor the person started, they approve the call first."
    )]
    async fn set_supervisor_workers(
        &self,
        Parameters(SetSupervisorWorkers { id, workers }): Parameters<SetSupervisorWorkers>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            self.ask_about_supervisor(&context, "set_supervisor_workers", &id)
                .await?;
            self.state
                .supervisor_set_workers(&id, workers)
                .map(|s| json(&s))
        };
        reply("set_supervisor_workers", work.await)
    }

    #[tool(
        description = "Set a supervisor's usage cap, the share of the usage limit (1 to 100) it may spend before it holds, or remove the cap by omitting `cap`. Returns the supervisor. On a supervisor the person started, they approve the call first."
    )]
    async fn set_supervisor_cap(
        &self,
        Parameters(SetSupervisorCap { id, cap }): Parameters<SetSupervisorCap>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            self.ask_about_supervisor(&context, "set_supervisor_cap", &id)
                .await?;
            self.state.supervisor_set_cap(&id, cap).map(|s| json(&s))
        };
        reply("set_supervisor_cap", work.await)
    }

    #[tool(
        description = "Every supervisor in every space, each as start_supervisor returns it: scope, workers, cap, state, queue and its agent's state."
    )]
    fn list_supervisors(&self) -> CallToolResult {
        reply("list_supervisors", Ok(json(&self.state.supervisor_list())))
    }

    #[tool(
        description = "One supervisor by id, as start_supervisor returns it, plus `running`: each running worker's bead and `session`, the name `wheel supervisor push` takes."
    )]
    fn show_supervisor(
        &self,
        Parameters(SupervisorRef { id }): Parameters<SupervisorRef>,
    ) -> CallToolResult {
        reply(
            "show_supervisor",
            self.state.supervisor_show(&id).map(|s| json(&s)),
        )
    }

    #[tool(
        description = "List the beads (`bd`) in a space's project, as `bd list --json` does, or with `ready: true` as `bd ready --json` does: open beads with no active blocker. Filter by status, labels, assignee, parent, kind and priority. An error carrying `bd`'s own message when it fails; never an empty list standing in for a failure."
    )]
    async fn list_beads(
        &self,
        Parameters(ListBeads {
            space,
            ready,
            status,
            all,
            labels,
            assignee,
            parent,
            kind,
            priority,
            limit,
        }): Parameters<ListBeads>,
    ) -> CallToolResult {
        let work = async {
            if ready && (status.is_some() || all) {
                return Err("ready cannot be combined with status or all".to_string());
            }
            let mut args: Vec<String> = vec![if ready { "ready" } else { "list" }.into()];
            push_flags(
                &mut args,
                [
                    ("status", status),
                    ("assignee", assignee),
                    ("parent", parent),
                    ("type", kind),
                    ("priority", priority.map(|p| p.to_string())),
                    ("limit", limit.map(|l| l.to_string())),
                ],
            );
            for label in labels.into_iter().flatten() {
                args.push(format!("--label={label}"));
            }
            if all {
                args.push("--all".into());
            }
            self.state.bd(&space, args).await
        };
        reply("list_beads", work.await)
    }

    #[tool(
        description = "Show one bead in a space's project: its description, acceptance criteria, notes, labels, parent, children and dependencies (`bd show --json`). An error carrying `bd`'s message when there is no such bead."
    )]
    async fn show_bead(
        &self,
        Parameters(BeadRef { space, id }): Parameters<BeadRef>,
    ) -> CallToolResult {
        let work = async {
            let id = bead_id(&id)?;
            self.state.bd(&space, vec!["show".into(), id]).await
        };
        reply("show_bead", work.await)
    }

    #[tool(
        description = "Create a bead in a space's project (`bd create --json`) and return it, id included. An error carrying `bd`'s message when it refuses."
    )]
    async fn create_bead(
        &self,
        Parameters(CreateBead {
            space,
            title,
            description,
            kind,
            priority,
            labels,
            parent,
            assignee,
            acceptance,
            notes,
        }): Parameters<CreateBead>,
    ) -> CallToolResult {
        let work = async {
            if title.trim().is_empty() {
                return Err("title is empty".to_string());
            }
            let mut args: Vec<String> = vec!["create".into()];
            push_flags(
                &mut args,
                [
                    ("title", Some(title)),
                    ("description", description),
                    ("type", kind),
                    ("priority", priority.map(|p| p.to_string())),
                    ("labels", labels.map(|l| l.join(","))),
                    ("parent", parent),
                    ("assignee", assignee),
                    ("acceptance", acceptance),
                    ("notes", notes),
                ],
            );
            self.state.bd(&space, args).await
        };
        reply("create_bead", work.await)
    }

    #[tool(
        description = "Change a bead in a space's project (`bd update --json`): its title, description, status, kind, priority, assignee, acceptance criteria, notes (replace, or `append_notes` to add) and labels. Returns the updated bead. An error carrying `bd`'s message when it refuses, and when no field is given."
    )]
    async fn update_bead(
        &self,
        Parameters(UpdateBead {
            space,
            id,
            title,
            description,
            status,
            kind,
            priority,
            assignee,
            acceptance,
            notes,
            append_notes,
            add_labels,
            remove_labels,
        }): Parameters<UpdateBead>,
    ) -> CallToolResult {
        let work = async {
            let mut args: Vec<String> = vec!["update".into(), bead_id(&id)?];
            let before = args.len();
            push_flags(
                &mut args,
                [
                    ("title", title),
                    ("description", description),
                    ("status", status),
                    ("type", kind),
                    ("priority", priority.map(|p| p.to_string())),
                    ("assignee", assignee),
                    ("acceptance", acceptance),
                    ("notes", notes),
                    ("append-notes", append_notes),
                ],
            );
            for label in add_labels.into_iter().flatten() {
                args.push(format!("--add-label={label}"));
            }
            for label in remove_labels.into_iter().flatten() {
                args.push(format!("--remove-label={label}"));
            }
            if args.len() == before {
                return Err("nothing to change; give at least one field".to_string());
            }
            self.state.bd(&space, args).await
        };
        reply("update_bead", work.await)
    }

    #[tool(
        description = "Every artifact this project and this machine have, in both scopes, each with its `scope`, `name`, the `entry` (the wheel.toml holding it), the page's real folder `dir` and `page`, `title`, the refresh `command` and `every` its entry declares, whether it asks to be offered in a pane picker (`pane`), and any `problems` with it — plus `resolves`, naming the scope each bare name lands in. Pass `pane: true` for only the artifacts asking to be offered in a pane picker, which is what a picker lists; without it you get those and the one-offs. An artifact is an `[artifacts.<name>]` entry in a wheel.toml — the project's or this machine's — naming a page anywhere (a folder whose page is index.html, or one HTML file), shown by a web pane."
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
        description = "Make a new artifact: a folder holding a placeholder index.html, under the project's .wheel/artifacts or this machine's artifacts folder as the scope says, and the entry naming it in that scope's wheel.toml. Refused when an artifact of that name is already there, so nothing one holds is overwritten. Pass `pane: true` to offer it in a pane picker, which is what you want for anything the person will open again; leave it off for a one-off. The page it leaves is a placeholder: writing the real page is an edit to that file, and declaring a refresh command an edit to the entry (`command`, `every`). For a `project` artifact, whose files are in the project, they are a bead worked by a supervisor (create_bead, start_supervisor); open the artifact once that bead closes. For a `machine` one, an agent pane in the space that owns the project makes them (start_pane) — it can run the command and see the result; open the artifact once that agent is done."
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
            .artifact_new(&name, scope, space.as_deref(), pane, None)
            .map(|a| json(&a));
        reply("new_artifact", result)
    }

    #[tool(
        description = "Register a page that already exists — a folder holding index.html, or one HTML file, anywhere on this machine — as an artifact: writes its entry into the scope's wheel.toml and changes nothing in its folder, so a course, a report or a site kept in its own repo shows in a pane without being moved or copied. Returns the record `list_artifacts` gives for it. Refused when there is no page at `path`, when the scope already has an artifact of that name, or when `every` comes without `command`. A `command` is never run by registering it: it runs when a pane shows the artifact and the person approves it."
    )]
    fn add_artifact(
        &self,
        Parameters(AddArtifact {
            path,
            name,
            scope,
            space,
            title,
            command,
            every,
            pane,
        }): Parameters<AddArtifact>,
    ) -> CallToolResult {
        let declared = crate::artifacts::Declared {
            title,
            command,
            every,
            pane,
        };
        let result = self
            .state
            .artifact_add(&path, name.as_deref(), scope, space.as_deref(), declared)
            .map(|a| json(&a));
        reply("add_artifact", result)
    }

    #[tool(
        description = "Delete an artifact's entry from its wheel.toml, leaving every file it names where it is, and return the entry as it was. A pane still showing it serves nothing until it is registered again."
    )]
    fn remove_artifact(
        &self,
        Parameters(RemoveArtifact { name, scope, space }): Parameters<RemoveArtifact>,
    ) -> CallToolResult {
        let result = self
            .state
            .artifact_remove(&name, scope, space.as_deref())
            .map(|a| json(&a));
        reply("remove_artifact", result)
    }

    #[tool(
        description = "Show an artifact in a pane: in `pane` when one is given (on a pane the person made, they approve the call first), otherwise in a new pane split right of the space's focused pane, leaving the person's focus where it is unless `no_focus` is false. Returns the pane and what it now shows. The page is served read-only and reloads whenever a file in its folder changes. An artifact whose entry declares a refresh command raises a permission request on open, which the person answers — opening it is not approving it."
    )]
    async fn open_artifact(
        &self,
        Parameters(OpenArtifact {
            name,
            scope,
            space,
            pane,
            no_focus,
        }): Parameters<OpenArtifact>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let work = async {
            if let Some(pane) = &pane {
                self.ask_about_pane_in(&context, "open_artifact", pane, space.as_deref())
                    .await?;
            }
            self.state
                .open_artifact(
                    &name,
                    scope,
                    space.as_deref(),
                    pane,
                    CreatedBy::Lead,
                    focuses(no_focus),
                )
                .await
                .map(|(pane, shows)| json(&Opened { pane, shows }))
        };
        reply("open_artifact", work.await)
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
    /// Raise a permission request with allow and deny, and wait for the answer; `Err` with
    /// `denied()` when the person does not allow it. Only the wait is cancellable: when the HTTP
    /// caller hangs up (rmcp cancels the request's token), the request is withdrawn, so a later
    /// answer does nothing, and the tool returns before acting. Whatever the tool does after an
    /// answer runs to its end, clean-up included.
    async fn ask_person(
        &self,
        cx: &RequestContext<RoleServer>,
        verb: &str,
        title: String,
        denied: impl FnOnce() -> String,
    ) -> Result<(), String> {
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
        let choice = tokio::select! {
            choice = self.state.perms.ask(LEAD, title, options) => choice?,
            () = cx.ct.cancelled() => {
                tracing::info!(verb, "caller gone");
                return Err("the caller hung up".into());
            }
        };
        if choice == "allow" {
            Ok(())
        } else {
            Err(denied())
        }
    }

    /// The one gate on a lead verb that acts on panes that already exist: when the person made
    /// any of `panes`, ask them first, naming the verb and those panes (plus `detail`, what the
    /// verb would send), and `Err` when they do not allow it. The lead's own panes pass without a
    /// request. A pane that does not exist is an error before anyone is asked. Returns the
    /// person's panes the answer covers.
    async fn ask_if_persons(
        &self,
        cx: &RequestContext<RoleServer>,
        verb: &str,
        panes: &[String],
        detail: Option<String>,
    ) -> Result<Vec<String>, String> {
        let mut persons = Vec::new();
        for pane in panes {
            if self.state.pane_created_by(pane)? == CreatedBy::Person {
                persons.push(pane.clone());
            }
        }
        tracing::info!(verb, ?panes, persons = ?persons, asked = !persons.is_empty(), "pane gate");
        if persons.is_empty() {
            return Ok(persons);
        }
        let list = persons.join(", ");
        let noun = if persons.len() == 1 { "pane" } else { "panes" };
        let detail = detail.map(|d| format!(": {d}")).unwrap_or_default();
        let title = format!("{verb} on your {noun} {list}{detail}");
        self.ask_person(cx, verb, title, || {
            format!("the person denied {verb} on {list}")
        })
        .await?;
        Ok(persons)
    }

    /// The gate for an agent verb, which names its pane by pane id or session name.
    async fn ask_about_agent(
        &self,
        cx: &RequestContext<RoleServer>,
        verb: &str,
        agent: &str,
    ) -> Result<(), String> {
        let (pane, _) = self.state.resolve_agent_pane(agent)?;
        self.ask_if_persons(cx, verb, &[pane], None).await?;
        Ok(())
    }

    /// The gate for a verb on a supervisor, which names its pane by its own id.
    async fn ask_about_supervisor(
        &self,
        cx: &RequestContext<RoleServer>,
        verb: &str,
        id: &str,
    ) -> Result<(), String> {
        let pane = self.state.supervisor_pane(id)?;
        self.ask_if_persons(cx, verb, &[pane], None).await?;
        Ok(())
    }

    /// The gate for a verb that replaces what `pane` shows; with `space`, a pane outside it is
    /// refused before anyone is asked.
    async fn ask_about_pane_in(
        &self,
        cx: &RequestContext<RoleServer>,
        verb: &str,
        pane: &str,
        space: Option<&str>,
    ) -> Result<(), String> {
        let holding = self.state.space_holding(pane)?;
        if let Some(space) = space.filter(|s| *s != holding) {
            return Err(format!("pane {pane} is not in space {space}"));
        }
        self.ask_if_persons(cx, verb, &[pane.to_string()], None)
            .await?;
        Ok(())
    }

    async fn create_space_asking(
        &self,
        cx: &RequestContext<RoleServer>,
        cwd: String,
        name: Option<String>,
    ) -> Result<String, String> {
        let title = format!("Open a space for {cwd:?}");
        self.ask_person(cx, "create_space", title, || {
            format!("the person denied opening a space for {cwd}")
        })
        .await?;
        self.state
            .space_create(cwd, name, CreatedBy::Lead)
            .map(|s| json(&s))
    }

    /// Send `text` to the pane's terminal: at once on the lead's own pane, after the person allows
    /// it on theirs.
    async fn type_into(
        &self,
        cx: &RequestContext<RoleServer>,
        pane: &str,
        mut text: String,
        enter: bool,
    ) -> Result<String, String> {
        // Refuse what cannot be typed before asking anyone.
        if text.len() > MAX_TYPED {
            return Err(format!(
                "text is {} bytes; type at most {MAX_TYPED} per call",
                text.len()
            ));
        }
        self.state.pane_term(pane)?;
        // Quoted and escaped, so the person sees every newline and control byte that will be sent.
        let detail = format!("{text:?}{}", if enter { " ⏎" } else { "" });
        self.ask_if_persons(cx, "type_into_pane", &[pane.to_string()], Some(detail))
            .await?;
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
type_into_pane runs commands in a terminal pane; \
wait_for_text and read_terminal then show what it printed, so a command is never typed blind. \
start_pane starts a shell or a coding agent in a space for work that is not a bead (exploring code, a spike, a question); the person follows and answers an agent in its pane. \
read_agent, message_agent, wait_for_agent and cancel_agent then work on that agent pane, by its pane id or its session name; \
read_agent returns the last turn unless you ask for more, so ask for what you need (last_turns, grep, all). \
list_beads, show_bead, create_bead and update_bead work the beads of a space's project. \
Every pane records who made it (created_by in list_spaces). Your own panes are yours to use; \
reading, messaging, typing into, cancelling, closing or replacing a pane the person made asks them first, so prefer panes you made. \
New panes leave the person's focus alone unless you pass no_focus: false; focus is the one tool that moves their view. \
list_inbox shows what waits on agent panes and set_inbox_item moves an item to seen, surfaced or closed. \
implementation goes as beads (create_bead) worked by a supervisor, even for one bead: start_supervisor starts one on a goal epic or a bead label; pause_supervisor, resume_supervisor, stop_supervisor, \
set_supervisor_workers and set_supervisor_cap steer it, and list_supervisors and show_supervisor read it back."
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

/// The server entry a supervisor's agent is started with: its own tools, at `/mcp-supervisor/<id>`
/// beside the lead's `/mcp`.
pub fn for_supervisor(url: &str, id: &str, token: &str) -> McpServer {
    let base = url.strip_suffix("/mcp").unwrap_or(url);
    for_lead(&format!("{base}{SUPERVISOR_ROUTE}/{id}"), token)
}

/// Where the supervisors' tools answer, followed by the supervisor's id.
pub(crate) const SUPERVISOR_ROUTE: &str = "/mcp-supervisor";

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
pub(crate) async fn authorize(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
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
