//! What the voice orchestrator is told about: wheeld's global broadcast reduced to short spoken
//! lines. A permission request or a failure is urgent and is sent to the orchestrator at once,
//! which says it unprompted; a finished turn or an answered request waits in a buffer until the
//! orchestrator asks (`voice_events`). Raw transcripts never come through here.

use std::collections::HashSet;

use wheel_proto::{AgentState, ServerMsg};

/// One event as a line of speech.
#[derive(Debug, PartialEq)]
pub(crate) struct Line {
    /// The agent the event is about.
    pub agent: String,
    pub text: String,
    /// Said unprompted: a permission request or a failure.
    pub urgent: bool,
}

/// Turns the broadcast into lines. It remembers which agents are busy, so a turn that ends reads
/// as one line and a state that never worked reads as none.
#[derive(Default)]
pub(crate) struct Filter {
    busy: HashSet<String>,
}

impl Filter {
    /// The line `msg` makes, if it makes one.
    pub(crate) fn line(&mut self, msg: &ServerMsg) -> Option<Line> {
        match msg {
            ServerMsg::PermRequested { perm } => Some(Line {
                agent: perm.agent.clone(),
                text: if wheel_proto::is_irreversible(&perm.title) {
                    format!("{} needs permission: {} (this cannot be undone: ask the person out loud, then answer with --confirmed)", perm.agent, perm.title)
                } else {
                    format!("{} needs permission: {}", perm.agent, perm.title)
                },
                urgent: true,
            }),
            ServerMsg::PermResolved { id, choice } => Some(Line {
                agent: String::new(),
                text: format!("Permission request {id} was answered: {choice}"),
                urgent: false,
            }),
            ServerMsg::AgentStateChanged { agent, state } => match state {
                AgentState::Busy { .. } => {
                    self.busy.insert(agent.clone());
                    None
                }
                AgentState::Ready => self.busy.remove(agent).then(|| Line {
                    agent: agent.clone(),
                    text: format!("{agent} finished its turn"),
                    urgent: false,
                }),
                AgentState::Failed { message } => {
                    self.busy.remove(agent);
                    Some(Line {
                        agent: agent.clone(),
                        text: format!("{agent} failed: {message}"),
                        urgent: true,
                    })
                }
                AgentState::GaveUp { message, .. } => {
                    self.busy.remove(agent);
                    Some(Line {
                        agent: agent.clone(),
                        text: format!("{agent} gave up and will not restart: {message}"),
                        urgent: true,
                    })
                }
                AgentState::Starting | AgentState::Switching => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::{PermInfo, PermWhy};

    fn state(agent: &str, state: AgentState) -> ServerMsg {
        ServerMsg::AgentStateChanged { agent: agent.into(), state }
    }

    #[test]
    fn a_permission_request_is_urgent() {
        let perm = PermInfo {
            id: "q1".into(),
            agent: "w5".into(),
            title: "Bash: cargo test".into(),
            options: Vec::new(),
            tool_call: None,
            why: PermWhy::default(),
        };
        let line = Filter::default().line(&ServerMsg::PermRequested { perm }).unwrap();
        assert_eq!(line.text, "w5 needs permission: Bash: cargo test");
        assert!(line.urgent);
        assert_eq!(line.agent, "w5");
    }

    #[test]
    fn an_irreversible_request_says_to_confirm_out_loud() {
        let perm = PermInfo {
            id: "q2".into(),
            agent: "w5".into(),
            title: "Bash: git push origin main".into(),
            options: Vec::new(),
            tool_call: None,
            why: PermWhy::default(),
        };
        let line = Filter::default().line(&ServerMsg::PermRequested { perm }).unwrap();
        assert!(line.text.contains("cannot be undone: ask the person out loud"), "{}", line.text);
        assert!(line.urgent);
    }

    #[test]
    fn an_answered_request_waits_to_be_asked() {
        let line = Filter::default()
            .line(&ServerMsg::PermResolved { id: "q1".into(), choice: "allow".into() })
            .unwrap();
        assert_eq!(line.text, "Permission request q1 was answered: allow");
        assert!(!line.urgent);
    }

    #[test]
    fn a_turn_that_ends_is_one_line_and_a_start_up_is_none() {
        let mut filter = Filter::default();
        assert_eq!(filter.line(&state("w5", AgentState::Starting)), None);
        assert_eq!(filter.line(&state("w5", AgentState::Ready)), None);
        assert_eq!(filter.line(&state("w5", AgentState::Busy { queued: 0 })), None);
        let line = filter.line(&state("w5", AgentState::Ready)).unwrap();
        assert_eq!(line.text, "w5 finished its turn");
        assert!(!line.urgent);
        assert_eq!(filter.line(&state("w5", AgentState::Ready)), None);
    }

    #[test]
    fn a_failure_and_a_give_up_are_urgent() {
        let mut filter = Filter::default();
        let failed = filter.line(&state("w5", AgentState::Failed { message: "exit 1".into() })).unwrap();
        assert_eq!(failed.text, "w5 failed: exit 1");
        assert!(failed.urgent);
        let gave_up = filter.line(&state("w5", AgentState::GaveUp { tries: 3, message: "no login".into() })).unwrap();
        assert_eq!(gave_up.text, "w5 gave up and will not restart: no login");
        assert!(gave_up.urgent);
    }

    #[test]
    fn transcripts_and_other_events_say_nothing() {
        let toast = ServerMsg::Ok { req: 1 };
        assert_eq!(Filter::default().line(&toast), None);
    }
}
