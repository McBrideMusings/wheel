//! wheeld's agent feeds and actions for plugin panes (ADR-0041): an agent's transcript, status and
//! waiting permission requests, read as `wheel.agent.*` feeds with params `{agent}` (an agent
//! pane's id or an agent's name), and `wheel.agent.send` and `wheel.agent.answer`. Each read is
//! kept current by a task the hub's cell holds, so the task ends when the last reader leaves.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use wheel_proto::{AgentEvent, ServerMsg, StampedEvent};

use crate::agent::{fold_event, Agent, AgentAttachment, Fold, TRANSCRIPT_CAP};
use crate::backends::{Publisher, Subscription, Update};
use crate::perms::Perms;
use crate::server::AppState;

/// The agent feeds, named `wheel.<name>`.
pub(crate) const AGENT_FEEDS: [&str; 3] = ["agent.transcript", "agent.status", "agent.requests"];

/// The agent actions, named `wheel.<name>`.
pub(crate) const AGENT_ACTIONS: [&str; 2] = ["agent.send", "agent.answer"];

/// The string field `field` of `args`, or an error naming what `what` takes.
fn field<'a>(args: &'a Value, field: &str, what: &str, takes: &str) -> Result<&'a str, String> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("wheel.{what} takes {takes}; {field} is missing"))
}

/// The transcript feed's value: every event the agent holds now, queued messages included.
fn transcript_value(name: &str, attachment: &AgentAttachment) -> Value {
    json!({"agent": name, "events": attachment.events})
}

/// Fold live event `event` into transcript `value` (`{agent, events}`) the way the agent folds
/// its own: a chunk merges into the reply or thought before it (the last event is replaced), a
/// revision rewrites it, a retraction removes the message's `user` and `turn_start` events, and
/// past [`TRANSCRIPT_CAP`] events the oldest goes. Returns the JSON Patch that makes the same
/// change to a reader's copy, or none when nothing changed.
fn fold_transcript(value: &mut Value, event: &StampedEvent) -> Option<Value> {
    let events = value.get_mut("events")?.as_array_mut()?;
    if let AgentEvent::Retract { turn } = &event.event {
        let gone: Vec<usize> = (0..events.len())
            .filter(|&i| {
                let e = &events[i];
                matches!(e["kind"].as_str(), Some("user" | "turn_start")) && e["turn"].as_u64() == Some(*turn)
            })
            .collect();
        let mut ops = Vec::new();
        for &i in gone.iter().rev() {
            events.remove(i);
            ops.push(json!({"op": "remove", "path": format!("/events/{i}")}));
        }
        return (!ops.is_empty()).then_some(Value::Array(ops));
    }
    // Only an event that can merge reads the last one back.
    let merges = matches!(
        event.event,
        AgentEvent::Text { .. } | AgentEvent::Thought { .. } | AgentEvent::TextRevised { .. } | AgentEvent::ThoughtRevised { .. }
    );
    let mut last: Option<StampedEvent> = merges
        .then(|| events.last().and_then(|e| serde_json::from_value(e.clone()).ok()))
        .flatten();
    match fold_event(last.as_mut().map(|e| &mut e.event), &event.event) {
        Fold::Dropped => None,
        Fold::Merged(_) => {
            let merged = serde_json::to_value(last?).ok()?;
            let i = events.len() - 1;
            events[i] = merged.clone();
            Some(json!([{"op": "replace", "path": format!("/events/{i}"), "value": merged}]))
        }
        Fold::Append => {
            let item = serde_json::to_value(event).ok()?;
            events.push(item.clone());
            let mut ops = vec![json!({"op": "add", "path": "/events/-", "value": item})];
            if events.len() > TRANSCRIPT_CAP {
                events.remove(0);
                ops.push(json!({"op": "remove", "path": "/events/0"}));
            }
            Some(Value::Array(ops))
        }
    }
}

/// The requests feed's value: the requests of agent `name` waiting on an answer, oldest first.
fn requests_value(perms: &Perms, name: &str) -> Value {
    let requests: Vec<_> = perms.list().into_iter().filter(|p| p.agent == name).collect();
    json!({"agent": name, "requests": requests})
}

/// Keep the transcript current: each new event is folded in, and the whole value is sent again
/// when the agent moves to another conversation or this task fell behind. When the agent ends
/// the feed goes stale.
fn pump_transcript(agent: Arc<Agent>, out: Publisher) -> (Value, tokio::task::JoinHandle<()>) {
    let name = agent.name.clone();
    let weak = Arc::downgrade(&agent);
    // Subscribed before the transcript is read, as an attach does, so a switch between is seen.
    let mut switched = agent.conversation_changes();
    let attachment = agent.attach();
    drop(agent);
    let value = transcript_value(&name, &attachment);
    let mut rx = attachment.rx;
    let task = tokio::spawn(async move {
        loop {
            loop {
                let event = tokio::select! {
                    biased;
                    event = rx.recv() => event,
                    changed = switched.changed() => {
                        if changed.is_err() {
                            return out.close();
                        }
                        break;
                    }
                };
                match event {
                    Ok(event) => {
                        if !out.apply(|v| fold_transcript(v, &event)) {
                            return;
                        }
                    }
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => return out.close(),
                }
            }
            let Some(agent) = weak.upgrade() else { return out.close() };
            switched = agent.conversation_changes();
            let attachment = agent.attach();
            drop(agent);
            let value = transcript_value(&name, &attachment);
            rx = attachment.rx;
            if !out.set(value) {
                return;
            }
        }
    });
    (value, task)
}

/// Keep the status current from the daemon-wide `agent_state_changed` broadcasts. When the agent
/// ends the feed goes stale.
fn pump_status(
    agent: Arc<Agent>,
    mut rx: broadcast::Receiver<ServerMsg>,
    out: Publisher,
) -> (Value, tokio::task::JoinHandle<()>) {
    let name = agent.name.clone();
    let weak = Arc::downgrade(&agent);
    let me = name.clone();
    let status = move |state: &wheel_proto::AgentState| json!({"agent": me, "state": state});
    let value = status(&agent.state());
    drop(agent);
    let task = tokio::spawn(async move {
        loop {
            let state = match rx.recv().await {
                Ok(ServerMsg::AgentStateChanged { agent, state }) if agent == name => state,
                Ok(_) | Err(RecvError::Lagged(_)) => match weak.upgrade() {
                    // The agent is gone once nothing holds it: checked on every broadcast.
                    None => return out.close(),
                    Some(agent) => agent.state(),
                },
                Err(RecvError::Closed) => return out.close(),
            };
            if !out.set(status(&state)) {
                return;
            }
        }
    });
    (value, task)
}

/// Keep the waiting requests current from the daemon-wide `perm_requested` and `perm_resolved`
/// broadcasts.
fn pump_requests(
    name: String,
    perms: Arc<Perms>,
    mut rx: broadcast::Receiver<ServerMsg>,
    out: Publisher,
) -> (Value, tokio::task::JoinHandle<()>) {
    let value = requests_value(&perms, &name);
    let task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ServerMsg::PermRequested { .. } | ServerMsg::PermResolved { .. }) | Err(RecvError::Lagged(_)) => {}
                Ok(_) => continue,
                Err(RecvError::Closed) => return out.close(),
            }
            if !out.set(requests_value(&perms, &name)) {
                return;
            }
        }
    });
    (value, task)
}

impl AppState {
    /// Read agent feed `name` (`agent.transcript`, `agent.status` or `agent.requests`) with
    /// `params` `{agent}`. The grant is checked by the caller.
    pub(crate) fn agent_feed_subscribe(
        &self,
        name: &str,
        params: &Value,
    ) -> Result<(Subscription, Value, bool, broadcast::Receiver<Update>), String> {
        let target = field(params, "agent", name, "params {agent: <agent pane id or agent name>}")?;
        let agent = self.resolve_agent(target)?.agent;
        let hub = &self.backends.hub;
        // Each pump subscribes to its broadcast before it reads the value, so no change between
        // is missed.
        Ok(match name {
            "agent.transcript" => hub.subscribe_pumped(name, params, |out| pump_transcript(agent, out)),
            "agent.status" => hub.subscribe_pumped(name, params, |out| pump_status(agent, self.global.subscribe(), out)),
            _ => hub.subscribe_pumped(name, params, |out| {
                pump_requests(agent.name.clone(), self.perms.clone(), self.perms.subscribe(), out)
            }),
        })
    }

    /// Run agent action `name` (`agent.send` or `agent.answer`) for plugin pane `pane`. The grant
    /// is checked by the caller.
    pub(crate) fn agent_action(&self, pane: &str, name: &str, args: &Value) -> Result<Value, String> {
        if name == "agent.send" {
            let takes = "args {agent, text}";
            let target = field(args, "agent", name, takes)?;
            let text = field(args, "text", name, takes)?;
            let found = self.resolve_agent(target)?;
            // Sized, never the text: a prompt can quote anything.
            tracing::info!(pane, agent = %found.agent.name, chars = text.len(), "plugin agent send");
            let turn = found.agent.send(text.to_string())?;
            return Ok(json!({"agent": found.agent.name, "turn": turn}));
        }
        let takes = "args {request, choice}";
        let request = field(args, "request", name, takes)?;
        let choice = field(args, "choice", name, takes)?;
        let chosen = self.perms.answer(request, choice, &format!("plugin pane {pane}"))?;
        Ok(json!({"request": request, "choice": chosen}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: AgentEvent) -> StampedEvent {
        StampedEvent { at: Some(1), event }
    }

    /// Fold `events` into an empty transcript, applying every patch to a reader's copy too.
    fn fold_all(events: Vec<AgentEvent>) -> (Value, Value) {
        let mut value = json!({"agent": "a", "events": []});
        let mut reader = value.clone();
        for event in events {
            if let Some(patch) = fold_transcript(&mut value, &ev(event)) {
                let patch: json_patch::Patch = serde_json::from_value(patch).unwrap();
                json_patch::patch(&mut reader, &patch).unwrap();
            }
        }
        (value, reader)
    }

    #[test]
    fn streamed_chunks_merge_into_one_reply_as_the_agent_stores_it() {
        let (value, reader) = fold_all(vec![
            AgentEvent::User { turn: 1, text: "hi".into() },
            AgentEvent::TurnStart { turn: 1 },
            AgentEvent::Text { text: "he".into() },
            AgentEvent::Text { text: "llo".into() },
            AgentEvent::TextRevised { streamed: "llo".into(), text: "LLO".into() },
            AgentEvent::TurnEnd { turn: 1, stop_reason: "end_turn".into() },
        ]);
        assert_eq!(value, reader, "the patches keep a reader's copy equal to the feed's value");
        let kinds: Vec<_> = value["events"].as_array().unwrap().iter().map(|e| e["kind"].clone()).collect();
        assert_eq!(kinds, [json!("user"), json!("turn_start"), json!("text"), json!("turn_end")]);
        assert_eq!(value["events"][2]["text"], "heLLO");
    }

    #[test]
    fn a_retracted_message_leaves_the_transcript() {
        let (value, reader) = fold_all(vec![
            AgentEvent::User { turn: 1, text: "a".into() },
            AgentEvent::User { turn: 2, text: "b".into() },
            AgentEvent::Retract { turn: 1 },
            AgentEvent::Retract { turn: 9 },
        ]);
        assert_eq!(value, reader);
        assert_eq!(value["events"], json!([{"at": 1, "kind": "user", "turn": 2, "text": "b"}]));
    }
}
