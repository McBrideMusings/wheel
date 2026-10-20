//! Verbs on an agent pane's session, addressed by pane id or session name. Every request that
//! names a session reaches it through `resolve_agent`, so one resolver and one set of rules (what
//! a session in `gave_up` accepts, what a cancel with no turn does) serve them all.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use wheel_proto::{PaneKind, SessionState};

use crate::server::AppState;
use crate::session::Session;
use crate::spaces::{agent_panes, find_pane};

/// An agent session and the pane that owns it; `pane` is unset once the pane has gone.
pub(crate) struct AgentRef {
    pub pane: Option<String>,
    pub session: Arc<Session>,
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
    /// The session `target` names: an agent pane's id, or a session name.
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
                .find(|(_, name, _, _)| name == target)
                .map(|(pane, _, _, _)| pane)
        };
        Ok(AgentRef { pane, session })
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
}
