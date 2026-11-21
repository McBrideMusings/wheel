//! An orchestrator's inbox (wheel-eg4e.13): agent events for its scope, queued in wheeld until
//! the orchestrator reads them. One item stands per agent and kind until it is acked; a repeat of
//! the event raises the item's count instead of adding another. wheeld never writes into the
//! orchestrator's chat: the orchestrator reads its inbox with `wheel inbox list` (its brief says
//! when), so an event never starts, interrupts or puts words in one of its turns.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wheel_proto::InboxItem;

use crate::orchestrator_events::Event;

/// The most items an inbox keeps; past it the oldest acked item goes first, then the oldest.
const KEPT: usize = 200;

pub(crate) struct Inbox {
    items: Vec<Kept>,
    next_id: u64,
    /// How long one agent turn runs before it is reported over time.
    pub turn_limit: Duration,
}

impl Default for Inbox {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            turn_limit: Duration::from_secs(1800),
        }
    }
}

struct Kept {
    item: InboxItem,
    /// The permission requests a `permission` item waits on; the item is acked once the last is
    /// answered.
    perms: Vec<(String, String)>,
}

impl Kept {
    /// A permission item's text: one line per request still waiting, joined.
    fn retext(&mut self) {
        if !self.perms.is_empty() {
            self.item.text = self.perms.iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>().join("; ");
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Inbox {
    /// Take in one event; returns the id of the item it made or raised.
    pub(crate) fn apply(&mut self, event: Event) -> Option<u64> {
        match event {
            Event::Add { agent, kind, text, perm } => {
                let at_ms = now_ms();
                if let Some(kept) =
                    self.items.iter_mut().find(|k| !k.item.acked && k.item.agent == agent && k.item.kind == kind)
                {
                    kept.item.count += 1;
                    kept.item.text = text.clone();
                    kept.item.at_ms = at_ms;
                    kept.item.told = false;
                    if let Some(perm) = perm {
                        kept.perms.push((perm, text));
                        kept.retext();
                    }
                    tracing::info!(id = kept.item.id, %agent, ?kind, count = kept.item.count, "inbox repeat");
                    return Some(kept.item.id);
                }
                let id = self.next_id;
                self.next_id += 1;
                tracing::info!(id, %agent, ?kind, "inbox add");
                let perms = perm.into_iter().map(|p| (p, text.clone())).collect();
                self.items.push(Kept {
                    item: InboxItem { id, agent, kind, text, count: 1, at_ms, told: false, acked: false },
                    perms,
                });
                self.trim();
                Some(id)
            }
            Event::Resolved { perm } => {
                let kept = self.items.iter_mut().find(|k| !k.item.acked && k.perms.iter().any(|(p, _)| *p == perm))?;
                kept.perms.retain(|(p, _)| *p != perm);
                kept.retext();
                if kept.perms.is_empty() {
                    kept.item.acked = true;
                    tracing::info!(id = kept.item.id, %perm, "inbox ack: every permission answered");
                }
                Some(kept.item.id)
            }
        }
    }

    fn trim(&mut self) {
        while self.items.len() > KEPT {
            let at = self.items.iter().position(|k| k.item.acked).unwrap_or(0);
            let dropped = self.items.remove(at);
            if !dropped.item.acked {
                tracing::warn!(id = dropped.item.id, agent = %dropped.item.agent, "inbox full: dropped an item not acked");
            }
        }
    }

    /// The items not acked, oldest first, or every item kept when `all`, each as it stood before this listing;
    /// every item listed is then marked told, so the next listing shows which happened since.
    pub(crate) fn list(&mut self, all: bool) -> Vec<InboxItem> {
        let listed: Vec<InboxItem> = self.items.iter().filter(|k| all || !k.item.acked).map(|k| k.item.clone()).collect();
        for kept in self.items.iter_mut().filter(|k| all || !k.item.acked) {
            kept.item.told = true;
        }
        listed
    }

    pub(crate) fn get(&self, id: u64) -> Option<&InboxItem> {
        self.items.iter().map(|k| &k.item).find(|i| i.id == id)
    }

    /// Mark `ids` done. Every id that names an item is acked; fails naming the ones that name
    /// none.
    pub(crate) fn ack(&mut self, ids: &[u64]) -> Result<(), String> {
        let missing: Vec<String> =
            ids.iter().filter(|id| self.get(**id).is_none()).map(|id| id.to_string()).collect();
        for kept in self.items.iter_mut().filter(|k| ids.contains(&k.item.id)) {
            kept.item.acked = true;
        }
        if !missing.is_empty() {
            tracing::info!(?ids, missing = %missing.join(","), "inbox ack: some ids name no item");
            return Err(format!("no inbox item {}", missing.join(", ")));
        }
        tracing::info!(?ids, "inbox ack");
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::InboxKind;

    fn add(agent: &str, kind: InboxKind, text: &str) -> Event {
        Event::Add { agent: agent.into(), kind, text: text.into(), perm: None }
    }

    #[test]
    fn a_repeat_from_one_agent_is_one_item() {
        let mut inbox = Inbox::default();
        let first = inbox.apply(add("w5", InboxKind::Finished, "w5 finished its turn"));
        let again = inbox.apply(add("w5", InboxKind::Finished, "w5 finished its turn"));
        assert_eq!(first, again);
        let items = inbox.list(false);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].count, 2);
        // Another kind, or another agent, is its own item.
        inbox.apply(add("w5", InboxKind::Failed, "w5 failed: exit 1"));
        inbox.apply(add("w7", InboxKind::Finished, "w7 finished its turn"));
        assert_eq!(inbox.list(false).len(), 3);
    }

    #[test]
    fn an_acked_item_is_not_raised_again() {
        let mut inbox = Inbox::default();
        let first = inbox.apply(add("w5", InboxKind::Finished, "a")).unwrap();
        inbox.ack(&[first]).unwrap();
        let next = inbox.apply(add("w5", InboxKind::Finished, "b")).unwrap();
        assert_ne!(first, next);
        assert_eq!(inbox.list(false).len(), 1);
        assert_eq!(inbox.list(true).len(), 2);
    }

    #[test]
    fn an_ack_naming_no_item_still_acks_the_rest() {
        let mut inbox = Inbox::default();
        let id = inbox.apply(add("w5", InboxKind::Finished, "a")).unwrap();
        assert_eq!(inbox.ack(&[id, 99]), Err("no inbox item 99".into()));
        assert!(inbox.list(false).is_empty());
    }

    #[test]
    fn a_listing_marks_its_items_told_and_a_repeat_makes_one_new_again() {
        let mut inbox = Inbox::default();
        inbox.apply(add("w5", InboxKind::Finished, "w5 finished its turn"));
        inbox.apply(add("w7", InboxKind::Finished, "w7 finished its turn"));
        let first = inbox.list(false);
        assert!(first.iter().all(|i| !i.told), "a first listing shows every item new: {first:?}");
        assert!(inbox.list(false).iter().all(|i| i.told), "listed once, they are told");
        inbox.apply(add("w7", InboxKind::Finished, "w7 finished its turn"));
        let again = inbox.list(false);
        assert_eq!(again.iter().map(|i| (i.agent.as_str(), i.told)).collect::<Vec<_>>(), [("w5", true), ("w7", false)], "a repeat is new again");
    }

    #[test]
    fn a_permission_item_is_acked_once_every_request_is_answered() {
        let mut inbox = Inbox::default();
        let ask = |perm: &str| Event::Add {
            agent: "w5".into(),
            kind: InboxKind::Permission,
            text: format!("w5 needs permission: {perm}"),
            perm: Some(perm.into()),
        };
        inbox.apply(ask("q1"));
        inbox.apply(ask("q2"));
        assert_eq!(inbox.list(false).len(), 1);
        assert_eq!(inbox.list(false)[0].text, "w5 needs permission: q1; w5 needs permission: q2", "each waiting request is named");
        assert_eq!(inbox.apply(Event::Resolved { perm: "q9".into() }), None);
        inbox.apply(Event::Resolved { perm: "q1".into() });
        assert_eq!(inbox.list(false).len(), 1, "q2 still waits");
        assert_eq!(inbox.list(false)[0].text, "w5 needs permission: q2");
        inbox.apply(Event::Resolved { perm: "q2".into() });
        assert!(inbox.list(false).is_empty());
    }

    #[test]
    fn the_oldest_acked_item_goes_first_past_the_cap() {
        let mut inbox = Inbox::default();
        for i in 0..KEPT {
            inbox.apply(add(&format!("w{i}"), InboxKind::Finished, "x"));
        }
        inbox.ack(&[5]).unwrap();
        inbox.apply(add("new", InboxKind::Finished, "x"));
        let all = inbox.list(true);
        assert_eq!(all.len(), KEPT);
        assert!(all.iter().all(|i| i.id != 5));
        assert_eq!(all[0].id, 1);
    }
}
