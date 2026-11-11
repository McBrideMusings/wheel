//! An agent pane's conversation as the daemon streams it: `agent_attached` carries the
//! transcript so far, then `agent` events follow, text arriving in chunks.
use serde_json::Value;

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
    pub state: String,
}

impl Transcript {
    /// Take one server message. Returns the agent it was about, when it was about one.
    pub fn on_message(&mut self, message: &Value, agent: &str) -> bool {
        if message["agent"].as_str() != Some(agent) {
            return false;
        }
        match message["ev"].as_str() {
            Some("agent_attached") => {
                self.lines.clear();
                self.state = state_name(&message["state"]);
                for event in message["events"].as_array().into_iter().flatten() {
                    self.event(event);
                }
                true
            }
            Some("agent") => {
                self.event(&message["event"]);
                true
            }
            Some("agent_state_changed") => {
                self.state = state_name(&message["state"]);
                true
            }
            _ => false,
        }
    }
    fn event(&mut self, event: &Value) {
        let text = || event["text"].as_str().unwrap_or_default().to_owned();
        match event["kind"].as_str() {
            Some("user") => self.lines.push(Line::User(text())),
            Some("text") => match self.lines.last_mut() {
                Some(Line::Reply(t)) => t.push_str(&text()),
                _ => self.lines.push(Line::Reply(text())),
            },
            Some("thought") => match self.lines.last_mut() {
                Some(Line::Thought(t)) => t.push_str(&text()),
                _ => self.lines.push(Line::Thought(text())),
            },
            Some("text_revised") => {
                if let Some(Line::Reply(t)) = self.lines.iter_mut().rev().find(|l| matches!(l, Line::Reply(_))) {
                    let streamed = event["streamed"].as_str().unwrap_or_default();
                    if let Some(at) = t.rfind(streamed) {
                        t.truncate(at);
                        t.push_str(&text());
                    }
                }
            }
            Some("tool_call") => self.lines.push(Line::Tool {
                id: event["id"].as_str().unwrap_or_default().to_owned(),
                title: event["title"].as_str().unwrap_or_default().to_owned(),
                status: event["status"].as_str().unwrap_or_default().to_owned(),
            }),
            Some("tool_update") => {
                let id = event["id"].as_str().unwrap_or_default();
                for line in &mut self.lines {
                    if let Line::Tool { id: i, title, status } = line {
                        if i == id {
                            if let Some(t) = event["title"].as_str() {
                                *title = t.to_owned();
                            }
                            if let Some(s) = event["status"].as_str() {
                                *status = s.to_owned();
                            }
                        }
                    }
                }
            }
            Some("error") => self.lines.push(Line::Error(event["message"].as_str().map(str::to_owned).unwrap_or_else(text))),
            _ => {}
        }
    }
}

fn state_name(state: &Value) -> String {
    state["phase"].as_str().unwrap_or("unknown").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn attach_replaces_and_chunks_merge() {
        let mut t = Transcript::default();
        t.on_message(&json!({"ev":"agent","agent":"a1","event":{"kind":"text","text":"stale"}}), "a1");
        assert!(t.on_message(&json!({"ev":"agent_attached","agent":"a1","state":{"phase":"ready"},"events":[
            {"kind":"user","turn":1,"text":"hi"},{"kind":"text","text":"hel"},{"kind":"text","text":"lo"},
            {"kind":"tool_call","id":"c1","title":"ls","tool_kind":"execute","status":"running"}]}), "a1"));
        t.on_message(&json!({"ev":"agent","agent":"a1","event":{"kind":"tool_update","id":"c1","status":"completed"}}), "a1");
        assert_eq!(t.state, "ready");
        assert_eq!(t.lines, vec![
            Line::User("hi".into()), Line::Reply("hello".into()),
            Line::Tool { id: "c1".into(), title: "ls".into(), status: "completed".into() }]);
    }

    #[test]
    fn another_agents_events_are_ignored() {
        let mut t = Transcript::default();
        assert!(!t.on_message(&json!({"ev":"agent","agent":"a2","event":{"kind":"text","text":"x"}}), "a1"));
        assert!(t.lines.is_empty());
    }

    #[test]
    fn a_revised_reply_replaces_its_streamed_chunks() {
        let mut t = Transcript::default();
        t.on_message(&json!({"ev":"agent","agent":"a1","event":{"kind":"text","text":"draft"}}), "a1");
        t.on_message(&json!({"ev":"agent","agent":"a1","event":{"kind":"text_revised","streamed":"draft","text":"final"}}), "a1");
        assert_eq!(t.lines, vec![Line::Reply("final".into())]);
    }
}
