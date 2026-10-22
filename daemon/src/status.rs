//! What each pane and space asks of the person: `needs_you`, `stopped`, `working` or `idle`. The
//! daemon derives it from each agent session's state and the permission requests waiting, and
//! tells every client when a value changes.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;
use wheel_proto::{Node, PaneKind, PaneStatus, ServerMsg, SessionState};

use crate::server::AppState;
use crate::spaces::{panes, Spaces};

/// The statuses last sent, so a recompute that gives the same values sends nothing.
#[derive(Default)]
pub(crate) struct StatusBook {
    panes: HashMap<String, PaneStatus>,
    spaces: HashMap<String, PaneStatus>,
}

/// One space's panes and its own status, as of one recompute.
struct SpaceStatus {
    id: String,
    panes: Vec<String>,
    status: Option<PaneStatus>,
}

/// A pane's status from its session's state and the permission requests waiting on it.
pub(crate) fn pane_status(state: Option<&SessionState>, waiting: u32) -> Option<PaneStatus> {
    if waiting > 0 {
        return Some(PaneStatus::NeedsYou { waiting });
    }
    Some(match state? {
        SessionState::Failed { .. } | SessionState::GaveUp { .. } => PaneStatus::Stopped,
        SessionState::Busy { .. } | SessionState::Starting | SessionState::Switching => {
            PaneStatus::Working
        }
        SessionState::Ready => PaneStatus::Idle,
    })
}

impl AppState {
    /// Every pane's status and every space's, as of now.
    fn statuses(&self, spaces: &Spaces) -> (HashMap<String, PaneStatus>, Vec<SpaceStatus>) {
        let perms = self.perms.list();
        let waiting = |who: &str| perms.iter().filter(|p| p.session == who).count() as u32;
        let mut by_pane = HashMap::new();
        let mut by_space = Vec::new();
        for space in &spaces.spaces {
            let mut ids = Vec::new();
            let mut found = Vec::new();
            for (pane, kind) in panes(&space.root) {
                ids.push(pane.to_string());
                let status = match kind {
                    PaneKind::Agent { session, .. } => {
                        let state = self.session(session).ok().map(|s| s.state());
                        // A session that is not running has no state, and no status until it is.
                        pane_status(state.as_ref(), waiting(session))
                    }
                    // A refresh command's request names the web pane, not a session.
                    PaneKind::Web { .. } => Some(waiting(pane))
                        .filter(|n| *n > 0)
                        .map(|waiting| PaneStatus::NeedsYou { waiting }),
                    _ => None,
                };
                if let Some(status) = status {
                    by_pane.insert(pane.to_string(), status);
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
        let (by_pane, by_space) = self.statuses(&spaces);
        for (space, computed) in spaces.spaces.iter_mut().zip(by_space) {
            space.status = computed.status;
            set_pane_statuses(&mut space.root, &by_pane);
        }
        spaces
    }

    /// Recompute every status and send `status_changed` for each one that differs from the last
    /// sent.
    pub(crate) fn refresh_statuses(&self) {
        let (by_pane, by_space) = self.statuses(&self.spaces());
        let mut book = self.statuses.lock().unwrap();
        let mut sent = Vec::new();
        for SpaceStatus {
            id: space,
            panes: pane_ids,
            status: space_status,
        } in &by_space
        {
            let old_space = book.spaces.get(space).copied();
            let mut any = false;
            for pane in pane_ids {
                let (old, new) = (book.panes.get(pane).copied(), by_pane.get(pane).copied());
                if old == new {
                    continue;
                }
                any = true;
                tracing::info!(%pane, %space, old = ?old, new = ?new, space_old = ?old_space, space_new = ?space_status, "status changed");
                sent.push(ServerMsg::StatusChanged {
                    space: space.clone(),
                    pane: Some(pane.clone()),
                    status: new,
                    space_status: *space_status,
                });
            }
            if !any && old_space != *space_status {
                // A pane closed or changed kind elsewhere than in this space's own list.
                tracing::info!(%space, old = ?old_space, new = ?space_status, "space status changed");
                sent.push(ServerMsg::StatusChanged {
                    space: space.clone(),
                    pane: None,
                    status: None,
                    space_status: *space_status,
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

fn set_pane_statuses(node: &mut Node, by_pane: &HashMap<String, PaneStatus>) {
    match node {
        Node::Pane { id, status, .. } => *status = by_pane.get(id).copied(),
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
                    ServerMsg::SessionStateChanged { .. }
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
