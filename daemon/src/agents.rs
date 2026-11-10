//! Verbs on an agent pane's agent, addressed by pane id or agent name. Every request that
//! names an agent reaches it through `resolve_agent`, so one resolver and one set of rules (what
//! an agent in `gave_up` accepts, what a cancel with no turn does) serve them all.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use wheel_proto::{PaneKind, AgentState};

use crate::server::AppState;
use crate::agent::Agent;
use crate::spaces::{agent_panes, space_find};

/// An agent and the pane that owns it; `pane` is unset once the pane has gone.
pub(crate) struct AgentRef {
    pub pane: Option<String>,
    pub agent: Arc<Agent>,
}

/// What `wait` returns.
#[derive(Debug, Serialize)]
pub struct AgentWaited {
    pub pane: String,
    pub agent: String,
    pub state: AgentState,
    /// False when the timeout passed with the agent not ready.
    pub ready: bool,
}

/// The phase name a state is reported under.
fn phase(state: &AgentState) -> &'static str {
    match state {
        AgentState::Starting => "starting",
        AgentState::Ready => "ready",
        AgentState::Busy { .. } => "busy",
        AgentState::Failed { .. } => "failed",
        AgentState::GaveUp { .. } => "gave_up",
        AgentState::Switching => "switching",
    }
}

impl AppState {
    /// Reserve conversation `id` for `asking`: Err with the other agent's name when another agent is
    /// in it or is being moved into it. The caller removes the reservation from `switching_into`
    /// once its switch has ended; by then a successful switch has set the agent's own id.
    pub(crate) fn claim_conversation(&self, id: &str, asking: &Arc<Agent>) -> Result<(), String> {
        let mut claims = self.switching_into.lock().unwrap();
        if let Some(other) = claims.get(id).filter(|name| **name != asking.name) {
            return Err(other.clone());
        }
        let held = self
            .agents
            .lock()
            .unwrap()
            .values()
            .find(|a| !Arc::ptr_eq(a, asking) && a.conversation_id().as_deref() == Some(id))
            .map(|a| a.name.clone());
        if let Some(name) = held {
            return Err(name);
        }
        claims.insert(id.to_string(), asking.name.clone());
        Ok(())
    }

    /// The agent `target` names: an agent pane's id, or an agent name.
    pub(crate) fn resolve_agent(&self, target: &str) -> Result<AgentRef, String> {
        let pane_agent = {
            let spaces = self.spaces.lock().unwrap();
            match spaces
                .spaces
                .iter()
                .find_map(|s| space_find(s, target))
            {
                Some(PaneKind::Agent { agent, .. }) => Some(Ok(agent.clone())),
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
        if let Some(agent) = pane_agent {
            let agent = agent?;
            return Ok(AgentRef {
                pane: Some(target.to_string()),
                agent: self.agent(&agent)?,
            });
        }
        let agent = self.agent(target)?;
        let pane = {
            let spaces = self.spaces.lock().unwrap();
            spaces
                .spaces
                .iter()
                .flat_map(agent_panes)
                .find(|p| p.agent == target)
                .map(|p| p.pane)
        };
        Ok(AgentRef { pane, agent })
    }

    /// Wait until the agent is `ready`, up to `timeout`. An agent that gave up never will be,
    /// so that is an error at once.
    pub async fn agent_wait(&self, target: &str, timeout: Duration) -> Result<AgentWaited, String> {
        let found = self.resolve_agent(target)?;
        let agent = found.agent;
        let pane = found.pane.unwrap_or_default();
        // Subscribed before the first look at the state, so a change in between is not missed.
        let mut global = self.global.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        let state = loop {
            let state = agent.state();
            match &state {
                AgentState::Ready => break (state, true),
                AgentState::GaveUp { tries, message } => {
                    return Err(format!(
                        "agent {} gave up after {tries} tries and will not be ready: {message}",
                        agent.name
                    ))
                }
                _ => {}
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            // Any message, or a lag that dropped some, is a reason to look at the state again.
            if tokio::time::timeout(left, global.recv()).await.is_err() {
                break (agent.state(), false);
            }
        };
        tracing::info!(%pane, agent = %agent.name, state = phase(&state.0), ready = state.1, "agent wait");
        Ok(AgentWaited {
            pane,
            agent: agent.name.clone(),
            state: state.0,
            ready: state.1,
        })
    }
}
