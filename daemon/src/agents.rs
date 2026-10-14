//! Verbs on an agent pane's session, addressed by pane id or session name. The lead's tools and
//! the `wheel agent` commands both reach a session through `resolve_agent`, so one resolver and
//! one set of rules (what a session in `gave_up` accepts, what a cancel with no turn does) serve
//! both.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::broadcast;
use wheel_proto::{PaneKind, SessionEvent, SessionState, SESSION_ENDED};

use crate::server::AppState;
use crate::session::Session;
use crate::spaces::{agent_panes, find_pane};

/// The most transcript `read` returns; a longer conversation keeps its end.
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

/// An agent session and the pane that owns it; `pane` is unset for the lead, which has none.
pub(crate) struct AgentRef {
    pub pane: Option<String>,
    pub session: Arc<Session>,
}

/// What `read` returns.
#[derive(Debug, Serialize)]
pub struct AgentRead {
    pub pane: String,
    pub session: String,
    pub state: SessionState,
    pub transcript: String,
}

/// What `message` returns: the turn the message got, and when it waited, the reply.
#[derive(Debug, Serialize)]
pub struct AgentMessaged {
    pub pane: String,
    pub session: String,
    pub turn: u64,
    /// Present only when the call waited. False when the timeout passed first; the message still
    /// runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<bool>,
    /// The agent's reply text so far, when the call waited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
}

/// What `wait` returns.
#[derive(Debug, Serialize)]
pub struct AgentWaited {
    pub pane: String,
    pub session: String,
    pub state: SessionState,
    /// False when the timeout passed with the session not ready.
    pub ready: bool,
}

/// What `cancel` returns.
#[derive(Debug, Serialize)]
pub struct AgentCancelled {
    pub pane: String,
    pub session: String,
    pub turn: u64,
}

/// The phase name a state is reported under.
fn phase(state: &SessionState) -> &'static str {
    match state {
        SessionState::Starting => "starting",
        SessionState::Ready => "ready",
        SessionState::Busy { .. } => "busy",
        SessionState::Failed { .. } => "failed",
        SessionState::GaveUp { .. } => "gave_up",
    }
}

impl AppState {
    /// The session `target` names: an agent pane's id, or a session name (`lead` included).
    pub(crate) fn resolve_agent(&self, target: &str) -> Result<AgentRef, String> {
        let pane_session = {
            let spaces = self.spaces.lock().unwrap();
            match spaces
                .spaces
                .iter()
                .find_map(|s| find_pane(&s.root, target))
            {
                Some(PaneKind::Agent { session, .. }) => Some(Ok(session.clone())),
                Some(other) => {
                    let kind = serde_json::to_value(other)
                        .ok()
                        .and_then(|v| v["kind"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    Some(Err(format!(
                        "{target} is not an agent pane; its kind is {kind}"
                    )))
                }
                None => None,
            }
        };
        if let Some(session) = pane_session {
            let session = session?;
            return Ok(AgentRef {
                pane: Some(target.to_string()),
                session: self.session(&session)?,
            });
        }
        let session = self.session(target)?;
        let pane = {
            let spaces = self.spaces.lock().unwrap();
            spaces
                .spaces
                .iter()
                .flat_map(|s| agent_panes(&s.root))
                .find(|(_, name, _)| name == target)
                .map(|(pane, _, _)| pane)
        };
        Ok(AgentRef { pane, session })
    }

    /// Like `resolve_agent`, for a verb that needs the pane too.
    pub(crate) fn resolve_agent_pane(
        &self,
        target: &str,
    ) -> Result<(String, Arc<Session>), String> {
        let found = self.resolve_agent(target)?;
        match found.pane {
            Some(pane) => Ok((pane, found.session)),
            None => Err(format!(
                "session {target} has no pane; give the id of an agent pane or its session name"
            )),
        }
    }

    /// The conversation so far, as text.
    pub fn agent_read(&self, target: &str) -> Result<AgentRead, String> {
        let (pane, session) = self.resolve_agent_pane(target)?;
        let attached = session.attach();
        let mut transcript = render(&attached.events);
        if transcript.len() > MAX_TRANSCRIPT_BYTES {
            let mut cut = transcript.len() - MAX_TRANSCRIPT_BYTES;
            while !transcript.is_char_boundary(cut) {
                cut += 1;
            }
            transcript = format!("[first {cut} bytes left out]\n{}", &transcript[cut..]);
        }
        tracing::info!(%pane, session = %session.name, events = attached.events.len(), "agent read");
        Ok(AgentRead {
            pane,
            session: session.name.clone(),
            state: attached.state,
            transcript,
        })
    }

    /// Queue `text` for the agent. A session that gave up or failed takes nothing, and the error
    /// names its state. With `wait`, the call returns when the message's turn ends, or after that
    /// many seconds.
    pub async fn agent_message(
        &self,
        target: &str,
        text: String,
        wait: Option<Duration>,
    ) -> Result<AgentMessaged, String> {
        let (pane, session) = self.resolve_agent_pane(target)?;
        // Subscribe before sending, so no event of the new turn is missed.
        let mut rx = session.attach().rx;
        let turn = session.send(text)?;
        let name = session.name.clone();
        let Some(wait) = wait else {
            return Ok(AgentMessaged {
                pane,
                session: name,
                turn,
                finished: None,
                reply: None,
            });
        };
        let mut reply = String::new();
        let mut started = false;
        let finished = tokio::time::timeout(wait, async {
            loop {
                match rx.recv().await {
                    Ok(SessionEvent::TurnStart { turn: t }) if t == turn => started = true,
                    Ok(SessionEvent::Text { text }) if started => reply.push_str(&text),
                    Ok(SessionEvent::TurnEnd { turn: t, .. }) if t == turn => return Ok(()),
                    Ok(SessionEvent::Error {
                        turn: Some(t),
                        message,
                    }) if t == turn => return Err(message),
                    Ok(SessionEvent::Error {
                        turn: None,
                        message,
                    }) if started || message == SESSION_ENDED => return Err(message),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Err(SESSION_ENDED.into()),
                }
            }
        })
        .await;
        let finished = match finished {
            Ok(Ok(())) => true,
            Ok(Err(message)) => {
                return Err(format!("{name} failed the turn: {message}"));
            }
            Err(_) => false,
        };
        tracing::info!(%pane, session = %name, turn, finished, "agent message waited");
        Ok(AgentMessaged {
            pane,
            session: name,
            turn,
            finished: Some(finished),
            reply: Some(reply),
        })
    }

    /// Wait until the session is `ready`, up to `timeout`. A session that gave up never will be,
    /// so that is an error at once.
    pub async fn agent_wait(&self, target: &str, timeout: Duration) -> Result<AgentWaited, String> {
        let found = self.resolve_agent(target)?;
        let session = found.session;
        let pane = found.pane.unwrap_or_default();
        // Subscribed before the first look at the state, so a change in between is not missed.
        let mut global = self.global.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        let state = loop {
            let state = session.state();
            match &state {
                SessionState::Ready => break (state, true),
                SessionState::GaveUp { tries, message } => {
                    return Err(format!(
                        "session {} gave up after {tries} tries and will not be ready: {message}",
                        session.name
                    ))
                }
                _ => {}
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            // Any message, or a lag that dropped some, is a reason to look at the state again.
            if tokio::time::timeout(left, global.recv()).await.is_err() {
                break (session.state(), false);
            }
        };
        tracing::info!(%pane, session = %session.name, state = phase(&state.0), ready = state.1, "agent wait");
        Ok(AgentWaited {
            pane,
            session: session.name.clone(),
            state: state.0,
            ready: state.1,
        })
    }

    /// Stop the session's current turn; an error when none is running.
    pub fn agent_cancel(&self, target: &str) -> Result<AgentCancelled, String> {
        let (pane, session) = self.resolve_agent_pane(target)?;
        let turn = session.cancel()?;
        Ok(AgentCancelled {
            pane,
            session: session.name.clone(),
            turn,
        })
    }
}

/// The messages and replies of a conversation as plain text: `> ` before each message, the reply
/// chunks joined, one line per tool call.
fn render(events: &[SessionEvent]) -> String {
    let mut out = String::new();
    let mut in_text = false;
    for event in events {
        if !matches!(event, SessionEvent::Text { .. }) && in_text {
            out.push('\n');
            in_text = false;
        }
        match event {
            SessionEvent::User { text, .. } => out.push_str(&format!("> {text}\n")),
            SessionEvent::Text { text } => {
                out.push_str(text);
                in_text = true;
            }
            SessionEvent::Thought { text } => out.push_str(&format!("[thought] {text}\n")),
            SessionEvent::Block { block_kind, name } => out.push_str(&format!(
                "[{block_kind}] {}\n",
                name.as_deref().unwrap_or("-")
            )),
            SessionEvent::ToolCall { title, status, .. } => {
                out.push_str(&format!("[tool] {title} ({status})\n"))
            }
            SessionEvent::Error { message, .. } => out.push_str(&format!("[error] {message}\n")),
            _ => {}
        }
    }
    if in_text {
        out.push('\n');
    }
    out
}
