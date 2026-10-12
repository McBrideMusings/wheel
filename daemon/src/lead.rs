//! The lead: one ACP agent session the daemon keeps running, started with the orchestrator
//! context (ADR-0002). Messages queue and run one turn at a time; the conversation is kept so a
//! client that attaches later sees it. When the agent process stops, the daemon starts a new one
//! and the queue carries on.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, McpServer, NewSessionRequest,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};
use wheel_proto::{LeadEvent, LeadState, PermOption, PlanItem, ServerMsg};

use crate::perms::Perms;

/// Appended to the agent's own system prompt for the lead session.
pub const ORCHESTRATOR_CONTEXT: &str = "You are the lead in Wheel, an environment for running coding agents across many projects. \
You are the person's single point of contact: keep a running list of what is in flight, which project it belongs to, and what is waiting on them. \
When they come back after a break, tell them what carried over and what is stuck. Never approve a permission request yourself.";

/// Events kept for clients that attach later; the oldest go first.
const TRANSCRIPT_CAP: usize = 2000;

/// The permission session name for the lead.
pub(crate) const SESSION: &str = "lead";

/// What a client sees on attach, taken at one instant.
pub struct LeadAttachment {
    pub state: LeadState,
    pub events: Vec<LeadEvent>,
    pub rx: broadcast::Receiver<LeadEvent>,
}

enum Cmd {
    Prompt { turn: u64, text: String },
    Cancel,
}

struct Inner {
    /// The conversation so far, capped at `TRANSCRIPT_CAP`.
    events: VecDeque<LeadEvent>,
    /// The `user` events of messages whose turn has not started, in turn order. Kept apart from
    /// `events` so the cap never drops a message that is still waiting to run.
    queued: VecDeque<LeadEvent>,
    state: LeadState,
}

pub struct Lead {
    inner: Mutex<Inner>,
    tx: broadcast::Sender<LeadEvent>,
    cmds: mpsc::UnboundedSender<Cmd>,
    next_turn: AtomicU64,
    /// Daemon-wide events, for state changes every client should see.
    global: broadcast::Sender<ServerMsg>,
}

/// How the lead is started.
pub struct LeadConfig {
    /// The agent command line, e.g. `claude-agent-acp`.
    pub command: String,
    pub cwd: PathBuf,
    pub context: String,
    /// Where the session reaches the daemon's MCP tools (`mcp::url`).
    pub mcp_url: String,
}

/// Messages waiting for a turn. Lives outside any one agent process, so a restart keeps them.
type Queue = VecDeque<(u64, String)>;

impl Lead {
    pub fn start(
        config: LeadConfig,
        mcp: McpServer,
        perms: Arc<Perms>,
        global: broadcast::Sender<ServerMsg>,
    ) -> Arc<Lead> {
        let (cmds, mut cmd_rx) = mpsc::unbounded_channel();
        let (tx, _) = broadcast::channel(4096);
        let lead = Arc::new(Lead {
            inner: Mutex::new(Inner {
                events: VecDeque::new(),
                queued: VecDeque::new(),
                state: LeadState::Starting,
            }),
            tx,
            cmds,
            next_turn: AtomicU64::new(1),
            global,
        });
        let run = lead.clone();
        tokio::spawn(async move {
            let mut queue = Queue::new();
            let mut backoff = Duration::from_secs(1);
            loop {
                run.set_state(LeadState::Starting);
                let started = tokio::time::Instant::now();
                let result = run
                    .clone()
                    .run(&config, mcp.clone(), perms.clone(), &mut cmd_rx, &mut queue)
                    .await;
                perms.drop_session(SESSION);
                let message = match result {
                    Ok(()) => {
                        tracing::info!("lead agent exited");
                        "the lead agent exited".to_string()
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "lead failed");
                        e
                    }
                };
                run.push(LeadEvent::Error {
                    turn: None,
                    message: format!("{message}; restarting it"),
                });
                run.set_state(LeadState::Failed { message });
                if started.elapsed() > Duration::from_secs(60) {
                    backoff = Duration::from_secs(1);
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        });
        lead
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
            .map_err(|_| "the lead is not running".to_string())?;
        tracing::info!(turn, chars = text.len(), "prompt queued");
        let event = LeadEvent::User { turn, text };
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
            .position(|e| matches!(e, LeadEvent::User { turn: t, .. } if *t == turn));
        if let Some(user) = position.and_then(|i| inner.queued.remove(i)) {
            Self::record(&mut inner, user);
        }
        self.push_locked(&mut inner, LeadEvent::TurnStart { turn });
    }

    /// Stop the turn in progress. Queued messages still run after it.
    pub fn cancel(&self) {
        let _ = self.cmds.send(Cmd::Cancel);
    }

    pub fn state(&self) -> LeadState {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn attach(&self) -> LeadAttachment {
        let inner = self.inner.lock().unwrap();
        LeadAttachment {
            state: inner.state.clone(),
            events: inner.events.iter().chain(&inner.queued).cloned().collect(),
            rx: self.tx.subscribe(),
        }
    }

    /// Record an event and broadcast it, under the lock attach() reads, so no attach misses or
    /// repeats one. Consecutive text chunks are merged in the stored transcript.
    fn push(&self, event: LeadEvent) {
        self.push_locked(&mut self.inner.lock().unwrap(), event);
    }

    fn push_locked(&self, inner: &mut Inner, event: LeadEvent) {
        Self::record(inner, event.clone());
        let _ = self.tx.send(event);
    }

    /// Add an event to the stored transcript only.
    fn record(inner: &mut Inner, event: LeadEvent) {
        match (inner.events.back_mut(), &event) {
            (Some(LeadEvent::Text { text: last }), LeadEvent::Text { text }) => last.push_str(text),
            _ => inner.events.push_back(event),
        }
        while inner.events.len() > TRANSCRIPT_CAP {
            inner.events.pop_front();
        }
    }

    fn set_state(&self, state: LeadState) {
        self.inner.lock().unwrap().state = state.clone();
        let _ = self.global.send(ServerMsg::LeadStateChanged { state });
    }

    /// One agent process, from spawn until it stops.
    async fn run(
        self: Arc<Self>,
        config: &LeadConfig,
        mcp: McpServer,
        perms: Arc<Perms>,
        cmd_rx: &mut mpsc::UnboundedReceiver<Cmd>,
        queue: &mut Queue,
    ) -> Result<(), String> {
        let agent = AcpAgent::from_str(&config.command)
            .map_err(|e| format!("bad lead command {:?}: {e}", config.command))?;
        tracing::info!(command = %config.command, cwd = %config.cwd.display(), mcp = %config.mcp_url, "lead spawned");
        let on_update = self.clone();
        let on_perm = perms.clone();
        let main = self.clone();
        let cwd = config.cwd.clone();
        let context = config.context.clone();
        Client
            .builder()
            .on_receive_notification(
                async move |n: SessionNotification, _cx| {
                    if let Some(event) = map_update(n.update) {
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
                    let (_, answer) = on_perm.open(SESSION, title, options);
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
                let mut meta = serde_json::Map::new();
                meta.insert("systemPrompt".into(), serde_json::json!({ "append": context }));
                let session = conn.send_request(NewSessionRequest::new(cwd).meta(meta).mcp_servers(vec![mcp])).block_task().await?.session_id;
                tracing::info!(session = %session, "lead ready");
                main.set_state(if queue.is_empty() { LeadState::Ready } else { LeadState::Busy { queued: queue.len() } });

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
                    main.set_state(LeadState::Busy { queued: queue.len() });
                    tracing::info!(turn, chars = text.len(), "prompt start");
                    let request = conn.send_request(PromptRequest::new(session.clone(), vec![ContentBlock::Text(TextContent::new(text))])).block_task();
                    tokio::pin!(request);
                    let result = loop {
                        tokio::select! {
                            result = &mut request => break result,
                            Some(cmd) = cmd_rx.recv() => match cmd {
                                Cmd::Prompt { turn, text } => {
                                    queue.push_back((turn, text));
                                    main.set_state(LeadState::Busy { queued: queue.len() });
                                }
                                Cmd::Cancel => {
                                    tracing::info!(turn, "prompt cancel");
                                    // ACP: a client that cancels answers its open permission requests as cancelled.
                                    perms.drop_session(SESSION);
                                    conn.send_notification(CancelNotification::new(session.clone()))?;
                                }
                            },
                        }
                    };
                    // Nothing this turn asked for can still be answered once it is over.
                    perms.drop_session(SESSION);
                    match result {
                        Ok(response) => {
                            let stop_reason = wire(&response.stop_reason);
                            tracing::info!(turn, %stop_reason, "prompt end");
                            main.push(LeadEvent::TurnEnd { turn, stop_reason });
                        }
                        Err(e) => {
                            tracing::warn!(turn, error = %e, "prompt failed");
                            main.push(LeadEvent::Error { turn: Some(turn), message: e.to_string() });
                        }
                    }
                    main.set_state(if queue.is_empty() { LeadState::Ready } else { LeadState::Busy { queued: queue.len() } });
                }
            })
            .await
            .map_err(|e| e.to_string())
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
fn map_update(update: SessionUpdate) -> Option<LeadEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(t) => Some(LeadEvent::Text { text: t.text }),
            _ => None,
        },
        SessionUpdate::ToolCall(call) => Some(LeadEvent::ToolCall {
            id: call.tool_call_id.to_string(),
            tool_kind: wire(&call.kind),
            status: wire(&call.status),
            title: call.title,
        }),
        SessionUpdate::ToolCallUpdate(update) => Some(LeadEvent::ToolUpdate {
            id: update.tool_call_id.to_string(),
            title: update.fields.title,
            status: update.fields.status.as_ref().map(wire),
        }),
        SessionUpdate::Plan(plan) => Some(LeadEvent::Plan {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A lead with no runner: `send` queues into a channel nobody reads.
    fn idle_lead() -> (Lead, mpsc::UnboundedReceiver<Cmd>) {
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let lead = Lead {
            inner: Mutex::new(Inner {
                events: VecDeque::new(),
                queued: VecDeque::new(),
                state: LeadState::Ready,
            }),
            tx: broadcast::channel(16).0,
            cmds,
            next_turn: AtomicU64::new(1),
            global: broadcast::channel(16).0,
        };
        (lead, cmd_rx)
    }

    fn tool(n: usize) -> LeadEvent {
        LeadEvent::ToolCall {
            id: n.to_string(),
            title: "ls".into(),
            tool_kind: "execute".into(),
            status: "pending".into(),
        }
    }

    #[test]
    fn a_long_turn_never_drops_a_queued_message() {
        let (lead, _rx) = idle_lead();
        lead.send("first".into()).unwrap();
        lead.start_turn(1);
        lead.send("second".into()).unwrap();
        for n in 0..TRANSCRIPT_CAP + 10 {
            lead.push(tool(n));
        }
        let events = lead.attach().events;
        assert_eq!(events.len(), TRANSCRIPT_CAP + 1);
        assert_eq!(
            events.last(),
            Some(&LeadEvent::User {
                turn: 2,
                text: "second".into()
            })
        );
        lead.start_turn(2);
        let events = lead.attach().events;
        assert_eq!(
            events[events.len() - 2..],
            [
                LeadEvent::User {
                    turn: 2,
                    text: "second".into()
                },
                LeadEvent::TurnStart { turn: 2 }
            ]
        );
    }
}
