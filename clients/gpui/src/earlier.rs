//! The conversations an agent pane showed before the one it shows now (WheelUI's
//! `EarlierConversations.swift`): wheeld's `visited` less its last id, oldest first. Each is read
//! once with `agent_conversation_history` the first time its divider is drawn, and drawn dimmed
//! above the transcript under a divider titled with its first message, with a Resume button.
use std::collections::HashSet;
use wheel_proto::StampedEvent;
use crate::transcript::{Kind, Transcript};

/// One earlier conversation: its events once wheeld has read them, or `missing` when the agent's
/// store no longer holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Past {
    pub id: String,
    pub chat: Option<Transcript>,
    pub missing: bool,
}

impl Past {
    /// What its divider says: the first message the person sent in it, on one line, once read.
    pub fn title(&self) -> String {
        if self.missing {
            return "Conversation no longer in the agent's store".into();
        }
        let first = self.chat.as_ref().and_then(|t| t.rows.iter().find_map(|r| match &r.kind {
            Kind::User { text, .. } => Some(text.split_whitespace().collect::<Vec<_>>().join(" ")),
            _ => None,
        }));
        first.filter(|l| !l.is_empty()).unwrap_or_else(|| "Earlier conversation".into())
    }
}

/// One agent's earlier conversations, and which of them wheeld has been asked to read.
#[derive(Clone, Debug, Default)]
pub struct Earlier {
    pub list: Vec<Past>,
    asked: HashSet<String>,
    /// Bumped whenever what the list draws changes, so the column lays its items out again.
    pub generation: u64,
}

impl Earlier {
    /// Show these, oldest first; a conversation already read keeps its events. Returns whether
    /// the list changed.
    pub fn show(&mut self, ids: &[String]) -> bool {
        if self.list.iter().map(|p| p.id.as_str()).eq(ids.iter().map(String::as_str)) {
            return false;
        }
        let mut held = std::mem::take(&mut self.list);
        self.list = ids.iter().map(|id| match held.iter().position(|p| &p.id == id) {
            Some(i) => held.swap_remove(i),
            None => Past { id: id.clone(), ..Past::default() },
        }).collect();
        self.generation += 1;
        true
    }
    /// Whether to ask wheeld for conversation `id` now: once, while it is listed and unread.
    pub fn ask(&mut self, id: &str) -> bool {
        self.list.iter().any(|p| p.id == id && p.chat.is_none() && !p.missing) && self.asked.insert(id.to_owned())
    }
    /// Conversation `id`'s events, read.
    pub fn loaded(&mut self, id: &str, events: &[StampedEvent]) {
        let Some(past) = self.list.iter_mut().find(|p| p.id == id) else { return };
        let mut chat = Transcript::default();
        for e in events {
            chat.event(&e.event);
        }
        past.chat = Some(chat);
        self.generation += 1;
    }
    /// The agent's store no longer holds conversation `id`.
    pub fn gone(&mut self, id: &str) {
        let Some(past) = self.list.iter_mut().find(|p| p.id == id) else { return };
        past.missing = true;
        self.generation += 1;
    }
}

/// The earlier conversations a pane whose `visited` is this draws: every one but the last, which
/// is the one it shows now; none from a wheeld that cannot read them.
pub fn earlier_ids(visited: &[String], reads: bool) -> &[String] {
    if reads { &visited[..visited.len().saturating_sub(1)] } else { &[] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::AgentEvent;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }
    fn stamped(event: AgentEvent) -> StampedEvent {
        StampedEvent { at: None, event }
    }

    #[test]
    fn earlier_is_every_visited_but_the_current_one() {
        let visited = ids(&["a", "b", "c"]);
        assert_eq!(earlier_ids(&visited, true), &visited[..2]);
        assert!(earlier_ids(&visited, false).is_empty());
        assert!(earlier_ids(&[], true).is_empty());
    }

    #[test]
    fn reads_once_and_keeps_what_it_read_across_a_reorder() {
        let mut e = Earlier::default();
        assert!(e.show(&ids(&["a", "b"])));
        assert!(!e.show(&ids(&["a", "b"])), "the same list is no change");
        assert!(e.ask("a"));
        assert!(!e.ask("a"), "asked once");
        assert!(!e.ask("z"), "an id not listed is never asked");
        e.loaded("a", &[
            stamped(AgentEvent::User { turn: 1, text: "fix  the\nflaky test".into() }),
            stamped(AgentEvent::TurnStart { turn: 1 }),
        ]);
        assert_eq!(e.list[0].title(), "fix the flaky test");
        assert_eq!(e.list[1].title(), "Earlier conversation");
        e.gone("b");
        assert_eq!(e.list[1].title(), "Conversation no longer in the agent's store");
        assert!(!e.ask("b"), "a missing one is never asked");
        let before = e.generation;
        assert!(e.show(&ids(&["b", "a", "c"])));
        assert!(e.generation > before);
        assert_eq!(e.list[1].title(), "fix the flaky test", "a read conversation keeps its events");
        assert!(e.list[0].missing);
        assert!(e.ask("c"));
    }
}
