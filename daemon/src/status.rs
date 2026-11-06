//! What each pane and space asks of the person: `needs_you`, `stopped`, `working` or `idle`. The
//! daemon derives it from each agent's state and the permission requests waiting, and
//! tells every client when a value changes.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;
use wheel_proto::{Node, PaneKind, PaneStatus, ReportedStatus, ServerMsg, AgentState};

use crate::server::AppState;
use crate::spaces::{space_panes, Spaces};

/// The statuses last sent, so a recompute that gives the same values sends nothing.
#[derive(Default)]
pub(crate) struct StatusBook {
    panes: HashMap<String, PaneStatus>,
    spaces: HashMap<String, PaneStatus>,
    /// Counts every finish and every show, so two moments compare without a clock.
    tick: u64,
    /// The agent panes in a turn as of the last recompute: an agent busy or waiting on a
    /// permission, a terminal agent working or waiting. Starting up is not a turn.
    turning: std::collections::HashSet<String>,
    /// The tick at which each agent pane's turn last ended.
    finished: HashMap<String, u64>,
    /// The tick at which any client last showed each pane, or stopped showing it.
    shown: HashMap<String, u64>,
}

impl StatusBook {
    /// A turn ended after the pane was last shown. Held back while a client shows it now.
    fn is_done(&self, pane: &str, showing: bool) -> bool {
        !showing && self.finished.get(pane).is_some_and(|f| *f > self.shown.get(pane).copied().unwrap_or(0))
    }
}

/// One space's panes and its own status, as of one recompute.
struct SpaceStatus {
    id: String,
    panes: Vec<String>,
    status: Option<PaneStatus>,
}

/// A pane's status from its agent's state and the permission requests waiting on it.
pub(crate) fn pane_status(state: Option<&AgentState>, waiting: u32) -> Option<PaneStatus> {
    if waiting > 0 {
        return Some(PaneStatus::NeedsYou {
            waiting,
            reason: None,
        });
    }
    Some(match state? {
        AgentState::Failed { .. } | AgentState::GaveUp { .. } => PaneStatus::Stopped,
        AgentState::Busy { .. } | AgentState::Starting | AgentState::Switching => {
            PaneStatus::Working
        }
        AgentState::Ready => PaneStatus::Idle,
    })
}

impl AppState {
    /// Every pane's status and every space's, as of now.
    fn statuses(
        &self,
        spaces: &Spaces,
        done: &dyn Fn(&str) -> bool,
        turning: &mut std::collections::HashSet<String>,
    ) -> (HashMap<String, PaneStatus>, Vec<SpaceStatus>) {
        let perms = self.perms.list();
        let reports = self.reports.lock().unwrap().clone();
        let waiting = |who: &str| perms.iter().filter(|p| p.agent == who).count() as u32;
        let mut by_pane = HashMap::new();
        let mut by_space = Vec::new();
        for space in &spaces.spaces {
            let mut ids = Vec::new();
            let mut found = Vec::new();
            for (pane, kind) in space_panes(space) {
                ids.push(pane.to_string());
                let status = match kind {
                    PaneKind::Agent { agent, .. } => {
                        let state = self.agent(agent).ok().map(|s| s.state());
                        // An agent that is not running has no state, and no status until it is.
                        pane_status(state.as_ref(), waiting(agent))
                    }
                    // A refresh command's request names the web pane, not an agent.
                    PaneKind::Web { .. } => Some(waiting(pane))
                        .filter(|n| *n > 0)
                        .map(|waiting| PaneStatus::NeedsYou {
                            waiting,
                            reason: None,
                        }),
                    // A terminal's agent says for itself, through the hooks its wrapper added.
                    PaneKind::Terminal {
                        term,
                        agent: Some(_),
                        ..
                    } => reports.get(term).map(|reported| match reported.status {
                        ReportedStatus::NeedsYou => PaneStatus::NeedsYou {
                            waiting: 1,
                            reason: reported.reason.clone(),
                        },
                        ReportedStatus::Working => PaneStatus::Working,
                        ReportedStatus::Idle => PaneStatus::Idle,
                    }),
                    _ => None,
                };
                let in_turn = match kind {
                    PaneKind::Agent { agent, .. } => {
                        waiting(agent) > 0
                            || self.agent(agent).is_ok_and(|a| matches!(a.state(), AgentState::Busy { .. }))
                    }
                    PaneKind::Terminal { .. } => matches!(
                        status,
                        Some(PaneStatus::Working | PaneStatus::NeedsYou { .. })
                    ),
                    _ => false,
                };
                if in_turn {
                    turning.insert(pane.to_string());
                }
                let status = match status {
                    Some(PaneStatus::Idle) if done(pane) => Some(PaneStatus::Done),
                    other => other,
                };
                if let Some(status) = status {
                    by_pane.insert(pane.to_string(), status.clone());
                    found.push(status);
                }
            }
            by_space.push(SpaceStatus {
                id: space.id.clone(),
                panes: ids,
                status: PaneStatus::of_space(found),
            });
        }
        (by_pane, by_space)
    }

    /// The layout for a reader that asks: `listing`'s, with every pane's and space's status.
    pub(crate) fn with_statuses(&self, mut spaces: Spaces) -> Spaces {
        let done = self.done_panes();
        let (by_pane, by_space) = self.statuses(&spaces, &|p| done.contains(p), &mut Default::default());
        for (space, computed) in spaces.spaces.iter_mut().zip(by_space) {
            space.status = computed.status;
            for tree in space.trees_mut() {
                set_pane_statuses(tree, &by_pane);
            }
        }
        spaces
    }

    /// Recompute every status and send `status_changed` for each one that differs from the last
    /// sent.
    pub(crate) fn refresh_statuses(&self) {
        let spaces = self.spaces();
        // A turn ended when a pane that was in a turn is now idle; this pass reads statuses
        // without the done mark, which the second pass adds.
        let mut turning = std::collections::HashSet::new();
        let raw = self.statuses(&spaces, &|_| false, &mut turning).0;
        let showing = |pane: &str| self.views.shows(pane);
        let mut book = self.statuses.lock().unwrap();
        for (pane, status) in &raw {
            if book.turning.contains(pane) && *status == PaneStatus::Idle {
                book.tick += 1;
                let tick = book.tick;
                book.finished.insert(pane.clone(), tick);
            }
        }
        book.turning = turning;
        book.finished.retain(|pane, _| raw.contains_key(pane));
        book.shown.retain(|pane, _| raw.contains_key(pane));
        let done: std::collections::HashSet<String> = raw
            .keys()
            .filter(|pane| book.is_done(pane, showing(pane)))
            .cloned()
            .collect();
        let (by_pane, by_space) = self.statuses(&spaces, &|p| done.contains(p), &mut Default::default());
        let mut sent = Vec::new();
        for SpaceStatus {
            id: space,
            panes: pane_ids,
            status: space_status,
        } in &by_space
        {
            let old_space = book.spaces.get(space).cloned();
            let mut any = false;
            for pane in pane_ids {
                let (old, new) = (book.panes.get(pane).cloned(), by_pane.get(pane).cloned());
                if old == new {
                    continue;
                }
                any = true;
                tracing::info!(%pane, %space, old = ?old, new = ?new, space_old = ?old_space, space_new = ?space_status, "status changed");
                sent.push(ServerMsg::StatusChanged {
                    space: space.clone(),
                    pane: Some(pane.clone()),
                    status: new,
                    space_status: space_status.clone(),
                });
            }
            if !any && old_space != *space_status {
                // A pane closed or changed kind elsewhere than in this space's own list.
                tracing::info!(%space, old = ?old_space, new = ?space_status, "space status changed");
                sent.push(ServerMsg::StatusChanged {
                    space: space.clone(),
                    pane: None,
                    status: None,
                    space_status: space_status.clone(),
                });
            }
        }
        book.panes = by_pane;
        book.spaces = by_space
            .into_iter()
            .filter_map(|s| s.status.map(|status| (s.id, status)))
            .collect();
        drop(book);
        for msg in sent {
            let _ = self.global.send(msg);
        }
    }
}

impl AppState {
    /// The panes a client showed or stopped showing just now: each counts as shown at this
    /// moment, so a turn that ended before it reads as seen and one that ends after it as done.
    pub(crate) fn panes_shown(&self, panes: &[String]) {
        if panes.is_empty() {
            return;
        }
        {
            let mut book = self.statuses.lock().unwrap();
            for pane in panes {
                book.tick += 1;
                let tick = book.tick;
                book.shown.insert(pane.clone(), tick);
            }
        }
        self.refresh_statuses();
    }

    /// The panes whose last turn ended after they were last shown, and that no client shows now.
    fn done_panes(&self) -> std::collections::HashSet<String> {
        let book = self.statuses.lock().unwrap();
        book.finished
            .keys()
            .filter(|pane| book.is_done(pane, self.views.shows(pane)))
            .cloned()
            .collect()
    }
}

fn set_pane_statuses(node: &mut Node, by_pane: &HashMap<String, PaneStatus>) {
    match node {
        Node::Pane { id, status, .. } => *status = by_pane.get(id).cloned(),
        Node::Split { a, b, .. } => {
            set_pane_statuses(a, by_pane);
            set_pane_statuses(b, by_pane);
        }
    }
}

/// Follow the events that change a status and recompute on each.
pub(crate) fn follow(state: Arc<AppState>) {
    let mut events = state.global.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(
                    ServerMsg::AgentStateChanged { .. }
                    | ServerMsg::PermRequested { .. }
                    | ServerMsg::PermResolved { .. }
                    | ServerMsg::Spaces { .. },
                )
                | Err(RecvError::Lagged(_)) => state.refresh_statuses(),
                Ok(_) => {}
                Err(RecvError::Closed) => return,
            }
        }
    });
}
