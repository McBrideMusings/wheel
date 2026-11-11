//! An agent pane's conversation as the daemon streams it: `agent_attached` carries the
//! transcript so far, then `agent` events follow, text arriving in chunks.
use wheel_proto::{AgentEvent, AgentState, ServerMsg};

#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    User(String),
    Reply(String),
    Thought(String),
    Tool { id: String, title: String, status: String },
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    pub lines: Vec<Line>,
    /// The agent's state as its wire phase (`starting`, `ready`, `busy`, `failed`, `switching`,
    /// `gave_up`).
    pub state: String,
}

impl Transcript {
    /// Take one server message. Returns the agent it was about, when it was about one.
    pub fn on_message(&mut self, message: &ServerMsg, agent: &str) -> bool {
        match message {
            ServerMsg::AgentAttached { agent: a, state, events, .. } if a == agent => {
                self.lines.clear();
                self.state = state_name(state).to_owned();
                for event in events {
                    self.event(&event.event);
                }
                true
            }
            ServerMsg::Agent { agent: a, event } if a == agent => {
                self.event(&event.event);
                true
            }
            ServerMsg::AgentStateChanged { agent: a, state } if a == agent => {
                self.state = state_name(state).to_owned();
                true
            }
            _ => false,
        }
    }
    fn event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::User { text, .. } => self.lines.push(Line::User(text.clone())),
            AgentEvent::Text { text } => match self.lines.last_mut() {
                Some(Line::Reply(t)) => t.push_str(text),
                _ => self.lines.push(Line::Reply(text.clone())),
            },
            AgentEvent::Thought { text } => match self.lines.last_mut() {
                Some(Line::Thought(t)) => t.push_str(text),
                _ => self.lines.push(Line::Thought(text.clone())),
            },
            AgentEvent::TextRevised { streamed, text } => {
                if let Some(Line::Reply(t)) = self.lines.iter_mut().rev().find(|l| matches!(l, Line::Reply(_)))
                    && let Some(at) = t.rfind(streamed.as_str())
                {
                    t.truncate(at);
                    t.push_str(text);
                }
            }
            AgentEvent::ToolCall { id, title, status, .. } => self.lines.push(Line::Tool { id: id.clone(), title: title.clone(), status: status.clone() }),
            AgentEvent::ToolUpdate { id, title: new_title, status: new_status, .. } => {
                for line in &mut self.lines {
                    if let Line::Tool { id: i, title, status } = line
                        && i == id
                    {
                        if let Some(t) = new_title {
                            *title = t.clone();
                        }
                        if let Some(s) = new_status {
                            *status = s.clone();
                        }
                    }
                }
            }
            AgentEvent::Error { message, .. } => self.lines.push(Line::Error(message.clone())),
            _ => {}
        }
    }
}

/// The state's wire phase name (the `phase` tag of `AgentState`).
fn state_name(state: &AgentState) -> &'static str {
    match state {
        AgentState::Starting => "starting",
        AgentState::Ready => "ready",
        AgentState::Busy { .. } => "busy",
        AgentState::Failed { .. } => "failed",
        AgentState::Switching => "switching",
        AgentState::GaveUp { .. } => "gave_up",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(wire: serde_json::Value) -> ServerMsg {
        serde_json::from_value(wire).unwrap()
    }

    #[test]
    fn attach_replaces_and_chunks_merge() {
        let mut t = Transcript::default();
        t.on_message(&msg(json!({"ev":"agent","agent":"a1","event":{"kind":"text","text":"stale"}})), "a1");
        assert!(t.on_message(&msg(json!({"ev":"agent_attached","agent":"a1","state":{"phase":"ready"},"now":0,"events":[
            {"kind":"user","turn":1,"text":"hi"},{"kind":"text","text":"hel"},{"kind":"text","text":"lo"},
            {"kind":"tool_call","id":"c1","title":"ls","tool_kind":"execute","status":"running"}]})), "a1"));
        t.on_message(&msg(json!({"ev":"agent","agent":"a1","event":{"kind":"tool_update","id":"c1","status":"completed"}})), "a1");
        assert_eq!(t.state, "ready");
        assert_eq!(t.lines, vec![
            Line::User("hi".into()), Line::Reply("hello".into()),
            Line::Tool { id: "c1".into(), title: "ls".into(), status: "completed".into() }]);
    }

    #[test]
    fn state_names_follow_the_wire_phase() {
        let mut t = Transcript::default();
        t.on_message(&msg(json!({"ev":"agent_state_changed","agent":"a1","state":{"phase":"gave_up","tries":3,"message":"x"}})), "a1");
        assert_eq!(t.state, "gave_up");
        assert_eq!(serde_json::to_value(AgentState::Busy { queued: 1 }).unwrap()["phase"], "busy");
    }

    #[test]
    fn another_agents_events_are_ignored() {
        let mut t = Transcript::default();
        assert!(!t.on_message(&msg(json!({"ev":"agent","agent":"a2","event":{"kind":"text","text":"x"}})), "a1"));
        assert!(t.lines.is_empty());
    }

    #[test]
    fn a_revised_reply_replaces_its_streamed_chunks() {
        let mut t = Transcript::default();
        t.on_message(&msg(json!({"ev":"agent","agent":"a1","event":{"kind":"text","text":"draft"}})), "a1");
        t.on_message(&msg(json!({"ev":"agent","agent":"a1","event":{"kind":"text_revised","streamed":"draft","text":"final"}})), "a1");
        assert_eq!(t.lines, vec![Line::Reply("final".into())]);
    }
}
