//! ACP agent sessions. The lead is the session named `lead`, started with the orchestrator
//! context and the daemon's MCP tools (ADR-0002); a worker is a session an agent pane owns, named
//! `w<n>`, running the same harness in its space's directory. Every session works the same way:
//! messages queue and run one turn at a time; the conversation is kept so a client that attaches
//! later sees it; when the agent process stops, the daemon starts a new one and the queue carries
//! on. A worker ends when its pane closes or changes kind.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, McpServer, NewSessionRequest,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, TextContent, ToolCallContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use wheel_proto::{PermOption, PlanItem, ServerMsg, SessionEvent, SessionState, ToolContent};

use crate::perms::Perms;

/// Appended to the agent's own system prompt for the lead session.
pub const ORCHESTRATOR_CONTEXT: &str = "You are the lead in Wheel, an environment for running coding agents across many projects. \
You are the person's single point of contact: keep a running list of what is in flight, which project it belongs to, and what is waiting on them. \
When they come back after a break, tell them what carried over and what is stuck. Never approve a permission request yourself.";

/// Events kept for clients that attach later; the oldest go first.
const TRANSCRIPT_CAP: usize = 2000;

/// The lead's session name.
pub const LEAD: &str = "lead";

/// What a client sees on attach, taken at one instant.
pub struct SessionAttachment {
    pub state: SessionState,
    pub events: Vec<SessionEvent>,
    pub rx: broadcast::Receiver<SessionEvent>,
}

enum Cmd {
    Prompt { turn: u64, text: String },
    Cancel,
}

struct Inner {
    /// The conversation so far, capped at `TRANSCRIPT_CAP`.
    events: VecDeque<SessionEvent>,
    /// The `user` events of messages whose turn has not started, in turn order. Kept apart from
    /// `events` so the cap never drops a message that is still waiting to run.
    queued: VecDeque<SessionEvent>,
    state: SessionState,
}

pub struct Session {
    pub name: String,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<SessionEvent>,
    cmds: mpsc::UnboundedSender<Cmd>,
    next_turn: AtomicU64,
    /// Daemon-wide events, for state changes every client should see.
    global: broadcast::Sender<ServerMsg>,
    /// Runs the agent process; aborting it kills the process.
    runner: Mutex<Option<JoinHandle<()>>>,
    /// Set by `stop`, before it withdraws the session's permission requests.
    stopped: AtomicBool,
}

/// How a session's agent is started.
#[derive(Clone)]
pub struct SessionConfig {
    /// The agent command line, e.g. `claude-agent-acp`.
    pub command: String,
    pub cwd: PathBuf,
    /// Appended to the agent's system prompt; empty for none.
    pub context: String,
    /// The daemon's MCP tools; only the lead gets them.
    pub mcp: Option<McpServer>,
}

/// Messages waiting for a turn. Lives outside any one agent process, so a restart keeps them.
type Queue = VecDeque<(u64, String)>;

impl Session {
    /// Start the session's agent, restarting it whenever it stops, until `stop`. `pane` names
    /// the agent pane that owns a worker, for the log.
    pub fn start(
        name: String,
        pane: Option<String>,
        config: SessionConfig,
        perms: Arc<Perms>,
        global: broadcast::Sender<ServerMsg>,
    ) -> Arc<Session> {
        let (cmds, mut cmd_rx) = mpsc::unbounded_channel();
        let (tx, _) = broadcast::channel(4096);
        let session = Arc::new(Session {
            name,
            inner: Mutex::new(Inner {
                events: VecDeque::new(),
                queued: VecDeque::new(),
                state: SessionState::Starting,
            }),
            tx,
            cmds,
            next_turn: AtomicU64::new(1),
            global,
            runner: Mutex::new(None),
            stopped: AtomicBool::new(false),
        });
        let run = session.clone();
        let pane = pane.unwrap_or_else(|| "-".into());
        let runner = tokio::spawn(async move {
            let mut queue = Queue::new();
            let mut backoff = Duration::from_secs(1);
            let name = run.name.clone();
            loop {
                run.set_state(SessionState::Starting);
                let started = tokio::time::Instant::now();
                tracing::info!(session = %name, %pane, command = %config.command, cwd = %config.cwd.display(), mcp = %mcp_url(&config.mcp), "session spawned");
                let result = run
                    .clone()
                    .run(&config, perms.clone(), &mut cmd_rx, &mut queue)
                    .await;
                perms.drop_session(&name);
                let message = match result {
                    Ok(()) => {
                        tracing::info!(session = %name, "session agent exited");
                        "the agent exited".to_string()
                    }
                    Err(e) => {
                        tracing::error!(session = %name, error = %e, "session failed");
                        e
                    }
                };
                run.push(SessionEvent::Error {
                    turn: None,
                    message: format!("{message}; restarting it"),
                });
                run.set_state(SessionState::Failed { message });
                if started.elapsed() > Duration::from_secs(60) {
                    backoff = Duration::from_secs(1);
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        });
        *session.runner.lock().unwrap() = Some(runner);
        session
    }

    /// End the session for good: kill its agent and withdraw its permission requests. Clients
    /// attached to it see an `error` with no turn, then their stream ends.
    pub fn stop(&self, perms: &Perms) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(runner) = self.runner.lock().unwrap().take() {
            runner.abort();
        }
        perms.drop_session(&self.name);
        self.push(SessionEvent::Error {
            turn: None,
            message: wheel_proto::SESSION_ENDED.into(),
        });
        tracing::info!(session = %self.name, "session ended");
    }

    /// Queue a message and show it to every client at once; returns the turn id its events will
    /// carry. The turn id, the queue and the `user` event are ordered under one lock, so `user`
    /// events appear in the order their turns will run and always before their `turn_start`.
    pub fn send(&self, text: String) -> Result<u64, String> {
        let mut inner = self.inner.lock().unwrap();
        let turn = self.next_turn.fetch_add(1, Ordering::SeqCst);
        self.cmds
            .send(Cmd::Prompt {
                turn,
                text: text.clone(),
            })
            .map_err(|_| format!("session {} is not running", self.name))?;
        tracing::info!(session = %self.name, turn, chars = text.len(), "prompt queued");
        let event = SessionEvent::User { turn, text };
        inner.queued.push_back(event.clone());
        let _ = self.tx.send(event);
        Ok(turn)
    }

    /// The runner took up `turn`: its `user` event joins the transcript, then `turn_start`.
    fn start_turn(&self, turn: u64) {
        let mut inner = self.inner.lock().unwrap();
        let position = inner
            .queued
            .iter()
            .position(|e| matches!(e, SessionEvent::User { turn: t, .. } if *t == turn));
        if let Some(user) = position.and_then(|i| inner.queued.remove(i)) {
            Self::record(&mut inner, user);
        }
        self.push_locked(&mut inner, SessionEvent::TurnStart { turn });
    }

    /// Stop the turn in progress. Queued messages still run after it.
    pub fn cancel(&self) {
        let _ = self.cmds.send(Cmd::Cancel);
    }

    pub fn state(&self) -> SessionState {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn attach(&self) -> SessionAttachment {
        let inner = self.inner.lock().unwrap();
        SessionAttachment {
            state: inner.state.clone(),
            events: inner.events.iter().chain(&inner.queued).cloned().collect(),
            rx: self.tx.subscribe(),
        }
    }

    /// Record an event and broadcast it, under the lock attach() reads, so no attach misses or
    /// repeats one. Consecutive text chunks are merged in the stored transcript.
    fn push(&self, event: SessionEvent) {
        self.push_locked(&mut self.inner.lock().unwrap(), event);
    }

    fn push_locked(&self, inner: &mut Inner, event: SessionEvent) {
        Self::record(inner, event.clone());
        let _ = self.tx.send(event);
    }

    /// Add an event to the stored transcript only.
    fn record(inner: &mut Inner, event: SessionEvent) {
        match (inner.events.back_mut(), &event) {
            (Some(SessionEvent::Text { text: last }), SessionEvent::Text { text }) => {
                last.push_str(text)
            }
            _ => inner.events.push_back(event),
        }
        while inner.events.len() > TRANSCRIPT_CAP {
            inner.events.pop_front();
        }
    }

    fn set_state(&self, state: SessionState) {
        self.inner.lock().unwrap().state = state.clone();
        let _ = self.global.send(ServerMsg::SessionStateChanged {
            session: self.name.clone(),
            state,
        });
    }

    /// One agent process, from spawn until it stops.
    async fn run(
        self: Arc<Self>,
        config: &SessionConfig,
        perms: Arc<Perms>,
        cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>,
        queue: &mut Queue,
    ) -> Result<(), String> {
        let agent = AcpAgent::from_str(&config.command)
            .map_err(|e| format!("bad agent command {:?}: {e}", config.command))?;
        let on_update = self.clone();
        let on_perm = perms.clone();
        let alive = self.clone();
        let main = self.clone();
        let name = self.name.clone();
        let asker = self.name.clone();
        let cwd = config.cwd.clone();
        let context = config.context.clone();
        let mcp: Vec<McpServer> = config.mcp.iter().cloned().collect();
        Client
            .builder()
            .on_receive_notification(
                async move |n: SessionNotification, _cx| {
                    if let Some(event) = map_update(&on_update.name, n.update) {
                        on_update.push(event);
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |req: RequestPermissionRequest, responder, cx| {
                    let title = req.tool_call.fields.title.clone().unwrap_or_else(|| req.tool_call.tool_call_id.to_string());
                    let options: Vec<PermOption> =
                        req.options.iter().map(|o| PermOption { id: o.option_id.to_string(), name: o.name.clone(), kind: wire(&o.kind) }).collect();
                    let tool_call = Some(req.tool_call.tool_call_id.to_string());
                    let (_, answer) = on_perm.open(&asker, title, options, tool_call);
                    // A runner still finishing a poll after `stop` must not leave a request nobody can
                    // answer: `stop` sets the flag before it withdraws, so one of the two withdraws it.
                    if alive.stopped.load(Ordering::SeqCst) {
                        on_perm.drop_session(&asker);
                    }
                    // Answering waits on a person; never block the dispatch loop for it. A request
                    // withdrawn at the end of its turn resolves as Cancelled.
                    cx.spawn(async move {
                        let outcome = match answer.await {
                            Ok(option) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option)),
                            Err(_) => RequestPermissionOutcome::Cancelled,
                        };
                        responder.respond(RequestPermissionResponse::new(outcome))
                    })
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, async move |conn: ConnectionTo<Agent>| {
                conn.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
                let mut request = NewSessionRequest::new(cwd).mcp_servers(mcp);
                if !context.is_empty() {
                    let mut meta = serde_json::Map::new();
                    meta.insert("systemPrompt".into(), serde_json::json!({ "append": context }));
                    request = request.meta(meta);
                }
                let session = conn.send_request(request).block_task().await?.session_id;
                tracing::info!(session = %name, acp_session = %session, "session ready");
                main.set_state(if queue.is_empty() { SessionState::Ready } else { SessionState::Busy { queued: queue.len() } });

                loop {
                    let (turn, text) = match queue.pop_front() {
                        Some(next) => next,
                        None => match cmd_rx.recv().await {
                            Some(Cmd::Prompt { turn, text }) => (turn, text),
                            Some(Cmd::Cancel) => continue,
                            None => return Ok(()),
                        },
                    };
                    main.start_turn(turn);
                    main.set_state(SessionState::Busy { queued: queue.len() });
                    tracing::info!(session = %name, turn, chars = text.len(), "prompt start");
                    let request = conn.send_request(PromptRequest::new(session.clone(), vec![ContentBlock::Text(TextContent::new(text))])).block_task();
                    tokio::pin!(request);
                    let result = loop {
                        tokio::select! {
                            result = &mut request => break result,
                            Some(cmd) = cmd_rx.recv() => match cmd {
                                Cmd::Prompt { turn, text } => {
                                    queue.push_back((turn, text));
                                    main.set_state(SessionState::Busy { queued: queue.len() });
                                }
                                Cmd::Cancel => {
                                    tracing::info!(session = %name, turn, "prompt cancel");
                                    // ACP: a client that cancels answers its open permission requests as cancelled.
                                    perms.drop_session(&name);
                                    conn.send_notification(CancelNotification::new(session.clone()))?;
                                }
                            },
                        }
                    };
                    // Nothing this turn asked for can still be answered once it is over.
                    perms.drop_session(&name);
                    match result {
                        Ok(response) => {
                            let stop_reason = wire(&response.stop_reason);
                            tracing::info!(session = %name, turn, %stop_reason, "prompt end");
                            main.push(SessionEvent::TurnEnd { turn, stop_reason });
                        }
                        Err(e) => {
                            tracing::warn!(session = %name, turn, error = %e, "prompt failed");
                            main.push(SessionEvent::Error { turn: Some(turn), message: e.to_string() });
                        }
                    }
                    main.set_state(if queue.is_empty() { SessionState::Ready } else { SessionState::Busy { queued: queue.len() } });
                }
            })
            .await
            .map_err(|e| e.to_string())
    }
}

/// The MCP url a session reaches the daemon at, for the log; `-` for none.
fn mcp_url(mcp: &Option<McpServer>) -> String {
    match mcp {
        Some(McpServer::Http(http)) => http.url.clone(),
        Some(_) => "other".into(),
        None => "-".into(),
    }
}

/// An ACP enum as the schema itself names it on the wire (`end_turn`, `in_progress`, `allow_once`).
fn wire<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

/// The parts of an ACP session update a client renders; everything else is dropped.
fn map_update(session: &str, update: SessionUpdate) -> Option<SessionEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(t) => Some(SessionEvent::Text { text: t.text }),
            _ => None,
        },
        SessionUpdate::ToolCall(call) => {
            let (content, dropped) = tool_content(call.content);
            let id = call.tool_call_id.to_string();
            let status = wire(&call.status);
            tracing::info!(
                session,
                id,
                %status,
                items = content.len(),
                dropped,
                bytes = content_bytes(&content),
                "tool call"
            );
            Some(SessionEvent::ToolCall {
                id,
                tool_kind: wire(&call.kind),
                status,
                title: call.title,
                content,
            })
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let (content, dropped) = match update.fields.content.map(tool_content) {
                Some((content, dropped)) => (Some(content), dropped),
                None => (None, 0),
            };
            let id = update.tool_call_id.to_string();
            let status = update.fields.status.as_ref().map(wire);
            tracing::info!(
                session,
                id,
                status = %status.as_deref().unwrap_or("-"),
                items = content.as_ref().map(Vec::len),
                dropped,
                bytes = content.as_deref().map(content_bytes),
                "tool update"
            );
            Some(SessionEvent::ToolUpdate {
                id,
                title: update.fields.title,
                status,
                content,
            })
        }
        SessionUpdate::Plan(plan) => Some(SessionEvent::Plan {
            entries: plan
                .entries
                .into_iter()
                .map(|e| PlanItem {
                    status: wire(&e.status),
                    content: e.content,
                })
                .collect(),
        }),
        _ => None,
    }
}

/// The tool call content a client renders, and how many items were dropped: content blocks other
/// than text (images, resources) and kinds this build does not know.
fn tool_content(content: Vec<ToolCallContent>) -> (Vec<ToolContent>, usize) {
    let total = content.len();
    let kept: Vec<_> = content
        .into_iter()
        .filter_map(|c| match c {
            ToolCallContent::Content(c) => match c.content {
                ContentBlock::Text(t) => Some(ToolContent::Text { text: t.text }),
                _ => None,
            },
            ToolCallContent::Diff(d) => Some(ToolContent::Diff {
                path: d.path.display().to_string(),
                old_text: d.old_text,
                new_text: d.new_text,
            }),
            ToolCallContent::Terminal(t) => Some(ToolContent::Terminal {
                id: t.terminal_id.to_string(),
            }),
            _ => None,
        })
        .collect();
    let dropped = total - kept.len();
    (kept, dropped)
}

/// Bytes of text and diff text, for the log, which never carries the text itself: it can hold
/// file contents.
fn content_bytes(content: &[ToolContent]) -> usize {
    content
        .iter()
        .map(|c| match c {
            ToolContent::Text { text } => text.len(),
            ToolContent::Diff {
                old_text, new_text, ..
            } => old_text.as_ref().map_or(0, String::len) + new_text.len(),
            ToolContent::Terminal { .. } | ToolContent::Unknown => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session with no runner: `send` queues into a channel nobody reads.
    fn idle_session() -> (Session, mpsc::UnboundedReceiver<Cmd>) {
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let session = Session {
            name: LEAD.into(),
            inner: Mutex::new(Inner {
                events: VecDeque::new(),
                queued: VecDeque::new(),
                state: SessionState::Ready,
            }),
            tx: broadcast::channel(16).0,
            cmds,
            next_turn: AtomicU64::new(1),
            global: broadcast::channel(16).0,
            runner: Mutex::new(None),
            stopped: AtomicBool::new(false),
        };
        (session, cmd_rx)
    }

    fn tool(n: usize) -> SessionEvent {
        SessionEvent::ToolCall {
            id: n.to_string(),
            title: "ls".into(),
            tool_kind: "execute".into(),
            status: "pending".into(),
            content: vec![],
        }
    }

    #[test]
    fn a_long_turn_never_drops_a_queued_message() {
        let (session, _rx) = idle_session();
        session.send("first".into()).unwrap();
        session.start_turn(1);
        session.send("second".into()).unwrap();
        for n in 0..TRANSCRIPT_CAP + 10 {
            session.push(tool(n));
        }
        let events = session.attach().events;
        assert_eq!(events.len(), TRANSCRIPT_CAP + 1);
        assert_eq!(
            events.last(),
            Some(&SessionEvent::User {
                turn: 2,
                text: "second".into()
            })
        );
        session.start_turn(2);
        let events = session.attach().events;
        assert_eq!(
            events[events.len() - 2..],
            [
                SessionEvent::User {
                    turn: 2,
                    text: "second".into()
                },
                SessionEvent::TurnStart { turn: 2 }
            ]
        );
    }
}
