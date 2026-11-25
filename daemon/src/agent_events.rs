//! What an event hook is told about (`crate::hooks`): wheeld's global broadcast reduced to agent
//! lifecycle events, each one short line. Raw transcripts never come through here.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use wheel_proto::{AgentState, ServerMsg};

/// What an agent event is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A turn started.
    Started,
    /// A turn ended.
    Finished,
    /// The agent failed, or gave up restarting.
    Failed,
    /// The agent asks for permission.
    Permission,
    /// A permission request was answered.
    Answered,
    /// One turn has run past the turn limit.
    OverTime,
    /// A build or test command the agent ran failed.
    BuildFailed,
}

impl Kind {
    /// The name a hook's `on` gives the events of this kind.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Started => "turn_start",
            Kind::Finished => "turn_end",
            Kind::Failed => "fail",
            Kind::Permission => "permission",
            Kind::Answered => "answered",
            Kind::OverTime => "over_time",
            Kind::BuildFailed => "build_failed",
        }
    }
}

/// One agent event.
#[derive(Debug, PartialEq)]
pub(crate) struct Event {
    /// The agent the event is about.
    pub agent: String,
    pub kind: Kind,
    pub text: String,
    /// The permission request a `permission` or `answered` event is about.
    pub perm: Option<String>,
}

/// Turns the broadcast into events. It remembers which agents are busy and since when, so a
/// turn that starts and one that ends each read as one event, a state that never worked reads as none, and a turn past
/// the limit reads as one `over_time` event.
#[derive(Default)]
pub(crate) struct Filter {
    busy: HashMap<String, Instant>,
    /// Busy agents already reported over the limit this turn.
    overdue: HashSet<String>,
    /// The agent each waiting permission request is from, so its answer names the agent.
    perms: HashMap<String, String>,
}

impl Filter {
    /// The event `msg` makes, if it makes one; `now` stamps a turn that starts.
    pub(crate) fn event(&mut self, msg: &ServerMsg, now: Instant) -> Option<Event> {
        let add = |agent: &str, kind, text: String| {
            Some(Event { agent: agent.to_string(), kind, text, perm: None })
        };
        match msg {
            ServerMsg::PermRequested { perm } => {
                self.perms.insert(perm.id.clone(), perm.agent.clone());
                Some(Event {
                    agent: perm.agent.clone(),
                    kind: Kind::Permission,
                    text: format!("{} needs permission: {}", perm.agent, perm.title),
                    perm: Some(perm.id.clone()),
                })
            }
            ServerMsg::PermResolved { id, choice } => {
                let agent = self.perms.remove(id)?;
                Some(Event {
                    text: format!("{agent}'s permission request was answered: {choice}"),
                    agent,
                    kind: Kind::Answered,
                    perm: Some(id.clone()),
                })
            }
            ServerMsg::AgentStateChanged { agent, state } => {
                let was_busy = !matches!(state, AgentState::Busy { .. }) && self.end_turn(agent);
                match state {
                    AgentState::Busy { .. } => match self.busy.entry(agent.clone()) {
                        std::collections::hash_map::Entry::Occupied(_) => None,
                        std::collections::hash_map::Entry::Vacant(turn) => {
                            turn.insert(now);
                            add(agent, Kind::Started, format!("{agent} started a turn"))
                        }
                    },
                    AgentState::Ready if was_busy => add(agent, Kind::Finished, format!("{agent} finished its turn")),
                    AgentState::Failed { message } => add(agent, Kind::Failed, format!("{agent} failed: {message}")),
                    AgentState::GaveUp { message, .. } => {
                        add(agent, Kind::Failed, format!("{agent} gave up and will not restart: {message}"))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// One `over_time` event for each agent whose turn has run `limit` or longer by `now`, once
    /// per turn.
    pub(crate) fn overdue(&mut self, now: Instant, limit: Duration) -> Vec<Event> {
        let mut events = Vec::new();
        for (agent, since) in &self.busy {
            let ran = now.saturating_duration_since(*since);
            if ran >= limit && self.overdue.insert(agent.clone()) {
                events.push(Event {
                    agent: agent.clone(),
                    kind: Kind::OverTime,
                    text: format!("{agent} has worked on one turn for {}", minutes(ran)),
                    perm: None,
                });
            }
        }
        events
    }

    /// When the next busy agent not yet reported reaches `limit`.
    pub(crate) fn next_overdue(&self, limit: Duration) -> Option<Instant> {
        self.busy
            .iter()
            .filter(|(agent, _)| !self.overdue.contains(*agent))
            .map(|(_, since)| *since + limit)
            .min()
    }

    /// Drop the busy agents `gone` names, which ended mid-turn and will report no state again.
    pub(crate) fn forget(&mut self, gone: impl Fn(&str) -> bool) {
        let ended: Vec<String> = self.busy.keys().filter(|a| gone(a)).cloned().collect();
        for agent in ended {
            self.end_turn(&agent);
        }
    }

    /// `agent` is no longer busy; whether it was.
    fn end_turn(&mut self, agent: &str) -> bool {
        self.overdue.remove(agent);
        self.busy.remove(agent).is_some()
    }
}

/// The event a failed build or test command makes (`crate::agent::build_command`).
pub(crate) fn build_failed(agent: &str, command: &str) -> Event {
    Event {
        agent: agent.to_string(),
        kind: Kind::BuildFailed,
        text: format!("{agent} ran a build or test that failed: {command}"),
        perm: None,
    }
}

/// `d` in whole minutes, as speech: "1 minute", "31 minutes", or seconds under a minute.
fn minutes(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{s} seconds"),
        s if s < 120 => "1 minute".into(),
        s => format!("{} minutes", s / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::{PermInfo, PermWhy};

    fn state(agent: &str, state: AgentState) -> ServerMsg {
        ServerMsg::AgentStateChanged { agent: agent.into(), state }
    }

    fn perm(id: &str, title: &str) -> ServerMsg {
        ServerMsg::PermRequested {
            perm: PermInfo {
                id: id.into(),
                agent: "w5".into(),
                title: title.into(),
                options: Vec::new(),
                tool_call: None,
                why: PermWhy::default(),
            },
        }
    }

    fn text(event: Option<Event>) -> (Kind, String) {
        match event {
            Some(Event { kind, text, .. }) => (kind, text),
            other => panic!("not an added event: {other:?}"),
        }
    }

    #[test]
    fn a_permission_request_names_its_request() {
        let event = Filter::default().event(&perm("q1", "Bash: cargo test"), Instant::now());
        assert_eq!(
            event,
            Some(Event {
                agent: "w5".into(),
                kind: Kind::Permission,
                text: "w5 needs permission: Bash: cargo test".into(),
                perm: Some("q1".into()),
            })
        );
    }

    #[test]
    fn an_answered_request_names_its_agent() {
        let mut filter = Filter::default();
        let msg = ServerMsg::PermResolved { id: "q1".into(), choice: "allow".into() };
        assert_eq!(filter.event(&msg, Instant::now()), None, "a request it never saw names no agent");
        filter.event(&perm("q1", "Bash: cargo test"), Instant::now());
        assert_eq!(
            filter.event(&msg, Instant::now()),
            Some(Event {
                agent: "w5".into(),
                kind: Kind::Answered,
                text: "w5's permission request was answered: allow".into(),
                perm: Some("q1".into()),
            })
        );
    }

    #[test]
    fn a_turn_starts_and_ends_as_one_event_each_and_a_start_up_is_none() {
        let mut filter = Filter::default();
        let now = Instant::now();
        assert_eq!(filter.event(&state("w5", AgentState::Starting), now), None);
        assert_eq!(filter.event(&state("w5", AgentState::Ready), now), None);
        assert_eq!(
            text(filter.event(&state("w5", AgentState::Busy { queued: 0 }), now)),
            (Kind::Started, "w5 started a turn".into())
        );
        assert_eq!(filter.event(&state("w5", AgentState::Busy { queued: 1 }), now), None, "a queued message is the same turn");
        assert_eq!(
            text(filter.event(&state("w5", AgentState::Ready), now)),
            (Kind::Finished, "w5 finished its turn".into())
        );
        assert_eq!(filter.event(&state("w5", AgentState::Ready), now), None);
    }

    #[test]
    fn a_failure_and_a_give_up_are_failures() {
        let mut filter = Filter::default();
        let now = Instant::now();
        assert_eq!(
            text(filter.event(&state("w5", AgentState::Failed { message: "exit 1".into() }), now)),
            (Kind::Failed, "w5 failed: exit 1".into())
        );
        assert_eq!(
            text(filter.event(&state("w5", AgentState::GaveUp { tries: 3, message: "no login".into() }), now)),
            (Kind::Failed, "w5 gave up and will not restart: no login".into())
        );
    }

    #[test]
    fn a_turn_past_the_limit_is_over_time_once() {
        let mut filter = Filter::default();
        let start = Instant::now();
        let limit = Duration::from_secs(1800);
        filter.event(&state("w5", AgentState::Busy { queued: 0 }), start);
        // A second busy state (a queued message) keeps the turn's start.
        filter.event(&state("w5", AgentState::Busy { queued: 1 }), start + Duration::from_secs(60));
        assert_eq!(filter.next_overdue(limit), Some(start + limit));
        assert!(filter.overdue(start + Duration::from_secs(1799), limit).is_empty());
        let events = filter.overdue(start + Duration::from_secs(1860), limit);
        assert_eq!(
            events,
            vec![Event {
                agent: "w5".into(),
                kind: Kind::OverTime,
                text: "w5 has worked on one turn for 31 minutes".into(),
                perm: None,
            }]
        );
        assert!(filter.overdue(start + Duration::from_secs(3600), limit).is_empty(), "once a turn");
        assert_eq!(filter.next_overdue(limit), None, "nothing left to report this turn");
        // The next turn can run over again.
        filter.event(&state("w5", AgentState::Ready), start + Duration::from_secs(3600));
        filter.event(&state("w5", AgentState::Busy { queued: 0 }), start + Duration::from_secs(3600));
        assert_eq!(filter.overdue(start + Duration::from_secs(5400), limit).len(), 1);
        // An agent that ended mid-turn is forgotten, and never reported over time.
        filter.event(&state("w6", AgentState::Busy { queued: 0 }), start);
        filter.forget(|agent| agent == "w6");
        assert!(filter.overdue(start + Duration::from_secs(9000), limit).is_empty());
    }

    #[test]
    fn transcripts_and_other_events_say_nothing() {
        assert_eq!(Filter::default().event(&ServerMsg::Ok { req: 1 }, Instant::now()), None);
    }
}
