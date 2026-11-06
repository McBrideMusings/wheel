//! The Implement and Swarm views' data: one row per item an agent pane says it works on, read
//! from the agents the core follows (`wheel agent watch`). A pane that declared nothing is one
//! row with no item.

use serde::Serialize;

use crate::core::agents::Agent;

pub mod serve;

/// The steps of a pass, in order.
pub const STAGES: [&str; 6] = ["plan", "edit", "build", "verify", "review", "gate"];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    pub pane: String,
    pub space: String,
    pub agent: String,
    pub model: Option<String>,
    /// `idle`, `working`, `needs_you` or `stopped`; `unknown` before the daemon says.
    pub status: String,
    /// `bead wheel-cmbu.6`, `pr 31`; none when the pane declared no item.
    pub item: Option<String>,
    pub stage: Option<String>,
    pub note: String,
}

pub fn rows(agents: &[Agent]) -> Vec<Row> {
    let mut out = vec![];
    for a in agents {
        let base = |item: Option<String>| Row {
            pane: a.pane.clone(),
            space: a.space.clone(),
            agent: a.agent.clone(),
            model: a.model.clone(),
            status: a.status.clone().unwrap_or_else(|| "unknown".into()),
            item,
            stage: a.work.as_ref().and_then(|w| w.stage.clone()),
            note: a.work.as_ref().and_then(|w| w.note.clone()).unwrap_or_default(),
        };
        match a.work.as_ref().map(|w| &w.items).filter(|i| !i.is_empty()) {
            Some(items) => out.extend(items.iter().map(|i| base(Some(format!("{} {}", i.kind, i.id))))),
            None => out.push(base(None)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::agents::{Work, WorkItem};

    fn agent(pane: &str, work: Option<Work>) -> Agent {
        Agent { space: "s".into(), cwd: "/p".into(), pane: pane.into(), agent: "w1".into(), harness: "claude".into(), model: None,
            status: Some("working".into()), work }
    }

    #[test]
    fn a_pane_is_a_row_per_item_or_one_row_without() {
        let work = Work {
            items: vec![WorkItem { kind: "bead".into(), id: "a".into() }, WorkItem { kind: "pr".into(), id: "3".into() }],
            stage: Some("edit".into()),
            note: Some("n".into()),
        };
        let rows = rows(&[agent("p1", Some(work)), agent("p2", None)]);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[1].item.as_deref(), rows[1].stage.as_deref()), (Some("pr 3"), Some("edit")));
        assert_eq!((rows[2].item.clone(), rows[2].stage.clone()), (None, None));
    }
}
