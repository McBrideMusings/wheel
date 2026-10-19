//! The lead's inbox (ADR-0008): what waits on an agent pane, kept in `inbox.json` in the instance
//! directory and rewritten on every change. An agent pane's turn ending, permission request or
//! failure adds an item; the item closes when its pane gets a new message, or when its pane or
//! space closes. Nothing here starts a lead turn: the daemon adds the count of new items to each
//! message a person sends the lead.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use wheel_proto::{
    unix_ms_now, InboxItem, InboxKind, InboxSource, InboxState, PaneKind, ServerMsg, SessionEvent,
    SESSION_ENDED,
};

use crate::server::AppState;
use crate::spaces::{agent_panes, panes, Spaces};

/// The most closed items the file keeps; older ones are dropped when the file is written.
const KEEP_CLOSED: usize = 200;

/// The file's content.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    next_id: u64,
    items: Vec<InboxItem>,
}

pub(crate) struct Inbox {
    path: PathBuf,
    inner: Mutex<Saved>,
    /// The `(new, open)` counts last announced, so a change that moves neither says nothing.
    announced: Mutex<(usize, usize)>,
    events: broadcast::Sender<ServerMsg>,
}

impl Inbox {
    /// The inbox kept in `dir`, loaded from `inbox.json` when it is there. A file that cannot be
    /// read or parsed starts an empty inbox and is logged; it is overwritten by the first change.
    pub(crate) fn load(dir: &Path, events: broadcast::Sender<ServerMsg>) -> Inbox {
        let path = dir.join("inbox.json");
        let saved = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Saved>(&text).unwrap_or_else(|e| {
                tracing::error!(path = %path.display(), error = %e, "inbox file unreadable; starting empty");
                Saved::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved::default(),
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "inbox file unreadable; starting empty");
                Saved::default()
            }
        };
        let mut saved = saved;
        // A permission request lives in memory, so none outlives the daemon.
        for item in &mut saved.items {
            if item.kind == InboxKind::Permission && item.state != InboxState::Closed {
                item.state = InboxState::Closed;
            }
        }
        let counts = counts(&saved.items);
        tracing::info!(path = %path.display(), items = saved.items.len(), new = counts.0, open = counts.1, "inbox loaded");
        Inbox {
            path,
            inner: Mutex::new(saved),
            announced: Mutex::new(counts),
            events,
        }
    }

    /// Write the file and announce the counts if they moved. Called with `inner` locked, so the
    /// file and the announcements follow the order of the changes.
    fn changed(&self, saved: &mut Saved) {
        let closed = saved
            .items
            .iter()
            .filter(|i| i.state == InboxState::Closed)
            .count();
        if closed > KEEP_CLOSED {
            let mut drop = closed - KEEP_CLOSED;
            saved.items.retain(|i| {
                let old = i.state == InboxState::Closed && drop > 0;
                if old {
                    drop -= 1;
                }
                !old
            });
        }
        match write(&self.path, saved) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!(path = %self.path.display(), error = %e, "inbox save failed")
            }
        }
        let now = counts(&saved.items);
        let mut announced = self.announced.lock().unwrap();
        if *announced != now {
            *announced = now;
            tracing::info!(new = now.0, open = now.1, "inbox count");
            let _ = self.events.send(ServerMsg::InboxCount {
                new: now.0,
                open: now.1,
            });
        }
    }

    fn add(
        &self,
        source: InboxSource,
        kind: InboxKind,
        detail: Option<String>,
        perm: Option<String>,
        supervisor: Option<String>,
        to_person: bool,
    ) -> InboxItem {
        let mut saved = self.inner.lock().unwrap();
        saved.next_id += 1;
        let item = InboxItem {
            id: format!("i{}", saved.next_id),
            source,
            kind,
            time: unix_ms_now(),
            state: InboxState::New,
            detail,
            perm,
            supervisor,
            to_person,
        };
        tracing::info!(item = %item.id, ?kind, to_person, space = %item.source.space, pane = %item.source.pane, session = %item.source.session, detail = item.detail.as_deref().unwrap_or("-"), supervisor = item.supervisor.as_deref().unwrap_or("-"), "inbox item added");
        saved.items.push(item.clone());
        self.changed(&mut saved);
        item
    }

    /// Close every open item `matches` picks; returns how many closed.
    fn close_where(&self, why: &str, matches: impl Fn(&InboxItem) -> bool) -> usize {
        let mut saved = self.inner.lock().unwrap();
        let mut closed = 0;
        for item in &mut saved.items {
            if item.state != InboxState::Closed && matches(item) {
                item.state = InboxState::Closed;
                closed += 1;
                tracing::info!(item = %item.id, pane = %item.source.pane, why, "inbox item closed");
            }
        }
        if closed > 0 {
            self.changed(&mut saved);
        }
        closed
    }

    /// Move item `id` to `state`. An item only moves forward.
    pub(crate) fn set_state(&self, id: &str, state: InboxState) -> Result<InboxItem, String> {
        let mut saved = self.inner.lock().unwrap();
        let item = saved
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| format!("no inbox item {id}"))?;
        if state < item.state {
            return Err(format!(
                "inbox item {id} is {} and only moves forward; it cannot go back to {}",
                item.state.name(),
                state.name()
            ));
        }
        if state == item.state {
            return Ok(item.clone());
        }
        tracing::info!(item = %id, from = ?item.state, to = ?state, "inbox item moved");
        item.state = state;
        let item = item.clone();
        self.changed(&mut saved);
        Ok(item)
    }

    /// Every item addressed to the person (`to_person`) or to the lead, the open ones oldest first,
    /// then the closed ones.
    pub(crate) fn list(&self, to_person: bool) -> Vec<InboxItem> {
        let saved = self.inner.lock().unwrap();
        let (closed, open): (Vec<_>, Vec<_>) = saved
            .items
            .iter()
            .filter(|i| i.to_person == to_person)
            .cloned()
            .partition(|i| i.state == InboxState::Closed);
        open.into_iter().chain(closed).collect()
    }

    /// The lead's `new` items, oldest first.
    fn new_items(&self) -> Vec<InboxItem> {
        let saved = self.inner.lock().unwrap();
        saved
            .items
            .iter()
            .filter(|i| !i.to_person && i.state == InboxState::New)
            .cloned()
            .collect()
    }

    fn mark_seen(&self, ids: &[String]) {
        let mut saved = self.inner.lock().unwrap();
        let mut moved = false;
        for item in &mut saved.items {
            if item.state == InboxState::New && ids.contains(&item.id) {
                item.state = InboxState::Seen;
                moved = true;
            }
        }
        if moved {
            tracing::info!(items = ids.len(), "inbox items seen");
            self.changed(&mut saved);
        }
    }

    fn has_perm(&self, perm: &str) -> bool {
        let saved = self.inner.lock().unwrap();
        saved.items.iter().any(|i| i.perm.as_deref() == Some(perm))
    }

    pub(crate) fn new_count(&self) -> usize {
        counts(&self.inner.lock().unwrap().items).0
    }
}

/// The `(new, open)` counts of the lead's items; the person's list is not the lead's.
fn counts(items: &[InboxItem]) -> (usize, usize) {
    let lead = || items.iter().filter(|i| !i.to_person);
    (
        lead().filter(|i| i.state == InboxState::New).count(),
        lead().filter(|i| i.state != InboxState::Closed).count(),
    )
}

/// Write `saved` to `path` through a temporary file, so a crash leaves the old file whole.
fn write(path: &Path, saved: &Saved) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(saved).map_err(std::io::Error::other)?;
    let mut file = std::fs::File::create(&tmp)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

impl AppState {
    /// The lead's inbox.
    pub fn inbox_list(&self) -> Vec<InboxItem> {
        self.inbox.list(false)
    }

    /// The person's For you list: what a pane the person unlocked would have sent the lead.
    pub fn for_you_list(&self) -> Vec<InboxItem> {
        self.inbox.list(true)
    }

    /// Open escalation `item`, in the lead's inbox or the person's list.
    pub(crate) fn open_escalation(&self, item: &str) -> Option<InboxItem> {
        self.inbox
            .list(false)
            .into_iter()
            .chain(self.inbox.list(true))
            .find(|i| i.id == item && i.state != InboxState::Closed)
    }

    /// Close item `id` once nothing waits on it.
    pub(crate) fn inbox_close(&self, id: &str) -> Result<(), String> {
        self.inbox.set_state(id, InboxState::Closed).map(|_| ())
    }

    pub fn inbox_set(&self, id: &str, state: InboxState) -> Result<InboxItem, String> {
        self.inbox.set_state(id, state)
    }

    /// Close the items whose pane is no longer an agent or supervisor pane running their session: the pane
    /// closed, its space closed, or its kind changed. Called with the spaces lock held, from the
    /// place every layout change passes through.
    pub(crate) fn inbox_prune(&self, spaces: &Spaces) {
        let live: HashMap<String, String> = spaces
            .spaces
            .iter()
            .flat_map(|s| {
                let agents = agent_panes(&s.root)
                    .into_iter()
                    .map(|(pane, session, _, _)| (pane, session));
                let supervisors = crate::supervisors::supervisor_panes(&s.root)
                    .into_iter()
                    .map(|(pane, supervisor)| (pane, supervisor.session));
                agents.chain(supervisors).collect::<Vec<_>>()
            })
            .collect();
        // A worker shows in its supervisor's pane, which is no agent pane: its items stay while
        // that pane does, and close with the request they wait on.
        let supervising: HashSet<String> = spaces
            .spaces
            .iter()
            .flat_map(|s| panes(&s.root))
            .filter(|(_, kind)| matches!(kind, PaneKind::Supervisor { .. }))
            .map(|(pane, _)| pane.to_string())
            .collect();
        self.inbox.close_where("pane gone", |item| {
            live.get(&item.source.pane) != Some(&item.source.session)
                && !supervising.contains(&item.source.pane)
        });
    }

    /// A permission request of a supervisor's worker waits for a person: add an item for it to
    /// the lead's inbox, or to the person's list when the worker's pane is unlocked
    /// (`to_person`), sourced from the supervisor's pane. The item closes when the request is
    /// answered or withdrawn.
    pub(crate) fn inbox_worker_permission(
        &self,
        space: &str,
        pane: &str,
        session: &str,
        title: String,
        perm: String,
        to_person: bool,
    ) {
        // Subscribe before looking at the request, so an answer from here on is not missed.
        let mut global = self.global.subscribe();
        let source = InboxSource {
            space: space.to_string(),
            pane: pane.to_string(),
            session: session.to_string(),
        };
        self.inbox.add(
            source,
            InboxKind::Permission,
            Some(title),
            Some(perm.clone()),
            None,
            to_person,
        );
        let me = self.me.clone();
        tokio::spawn(async move {
            loop {
                let Some(state) = me.upgrade() else { return };
                if !state.perms.list().iter().any(|p| p.id == perm) {
                    state
                        .inbox
                        .close_where("permission answered", |i| i.perm.as_deref() == Some(&perm));
                    return;
                }
                drop(state);
                match global.recv().await {
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                }
            }
        });
    }

    /// The prefix for a message a person sends the lead — `[inbox: 3 new — web 2, api 1]` — and
    /// the ids of the items it counts; None when nothing is new.
    pub(crate) fn inbox_prefix(&self) -> Option<(String, Vec<String>)> {
        let items = self.inbox.new_items();
        if items.is_empty() {
            return None;
        }
        let spaces = self.spaces.lock().unwrap();
        let mut by_space: Vec<(String, String, usize)> = Vec::new();
        for item in &items {
            let name = spaces
                .spaces
                .iter()
                .find(|s| s.id == item.source.space)
                .map_or_else(
                    || item.source.space.clone(),
                    |s| {
                        // A name cannot end the line or the bracket early.
                        s.name
                            .chars()
                            .map(|c| if c.is_control() || c == ']' { ' ' } else { c })
                            .collect()
                    },
                );
            match by_space
                .iter_mut()
                .find(|(id, _, _)| *id == item.source.space)
            {
                Some((_, _, k)) => *k += 1,
                None => by_space.push((item.source.space.clone(), name, 1)),
            }
        }
        let list = by_space
            .iter()
            .map(|(_, name, k)| format!("{name} {k}"))
            .collect::<Vec<_>>()
            .join(", ");
        Some((
            format!("[inbox: {} new — {list}]", items.len()),
            items.into_iter().map(|i| i.id).collect(),
        ))
    }

    /// The lead has been told of these items, in a message's prefix.
    pub(crate) fn inbox_seen(&self, ids: &[String]) {
        self.inbox.mark_seen(ids);
    }

    /// Feed the lead's inbox from agent pane `pane`'s session `name`, whose events are `events`.
    /// Runs until the session ends. Holds neither the session nor the state alive.
    pub(crate) fn watch_for_inbox(
        &self,
        name: &str,
        pane: &str,
        mut events: broadcast::Receiver<wheel_proto::StampedEvent>,
    ) {
        let (me, name, pane) = (self.me.clone(), name.to_string(), pane.to_string());
        let mut global = self.global.subscribe();
        tokio::spawn(async move {
            // Messages shown (`user`) whose turn has not started: while any waits behind the turn
            // that just ended, the pane is not waiting on anyone. Counted from the stream, which is
            // in order, never read off the session, which may have moved on by now.
            let mut queued = 0usize;
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        Ok(event) => {
                            match &event.event {
                                SessionEvent::User { .. } => queued += 1,
                                SessionEvent::TurnStart { .. } => queued = queued.saturating_sub(1),
                                _ => {}
                            }
                            let Some(state) = me.upgrade() else { return };
                            if state.inbox_session_event(&name, &pane, event.event, queued) {
                                return;
                            }
                        }
                        Err(RecvError::Lagged(missed)) => {
                            tracing::warn!(session = %name, %pane, missed, "inbox watcher fell behind the session's events; some may be missing");
                        }
                        Err(RecvError::Closed) => return,
                    },
                    message = global.recv() => match message {
                        Ok(ServerMsg::PermRequested { perm }) if perm.session == name => {
                            let Some(state) = me.upgrade() else { return };
                            state.inbox_add(&name, &pane, InboxKind::Permission, Some(perm.title), Some(perm.id));
                        }
                        Ok(ServerMsg::PermResolved { id, .. }) => {
                            let Some(state) = me.upgrade() else { return };
                            state.inbox.close_where("permission answered", |i| i.perm.as_deref() == Some(id.as_str()));
                        }
                        Ok(_) => {}
                        Err(RecvError::Lagged(missed)) => {
                            // A lost permission request would leave the pane blocked with nothing
                            // in the inbox, so look at what is waiting now.
                            tracing::warn!(session = %name, %pane, missed, "inbox watcher fell behind the daemon's events; looking at the waiting permissions");
                            let Some(state) = me.upgrade() else { return };
                            for perm in state.perms.list().into_iter().filter(|p| p.session == name) {
                                if !state.inbox.has_perm(&perm.id) {
                                    state.inbox_add(&name, &pane, InboxKind::Permission, Some(perm.title), Some(perm.id));
                                }
                            }
                        }
                        Err(RecvError::Closed) => return,
                    },
                }
            }
        });
    }

    /// One event of agent pane `pane`'s session; true when the session has ended.
    fn inbox_session_event(
        &self,
        session: &str,
        pane: &str,
        event: SessionEvent,
        queued: usize,
    ) -> bool {
        match event {
            SessionEvent::User { .. } => {
                self.inbox.close_where("pane got a message", |i| {
                    i.source.session == session && i.source.pane == pane
                });
            }
            SessionEvent::TurnEnd { stop_reason, .. } => {
                // A cancelled turn was asked for, and a message queued behind the turn means the
                // pane is about to work again: neither leaves it waiting on anyone.
                if queued == 0 && stop_reason != "cancelled" {
                    self.inbox_add(session, pane, InboxKind::TurnEnded, Some(stop_reason), None);
                }
            }
            SessionEvent::Error { message, .. } => {
                if message == SESSION_ENDED {
                    return true;
                }
                self.inbox_add(session, pane, InboxKind::Failed, Some(message), None);
            }
            _ => {}
        }
        false
    }

    /// A supervisor's agent forwards something it cannot settle: an `escalation` item from the
    /// supervisor's pane, with its one-line `summary`, for the lead or for the person (`to_person`).
    pub(crate) fn inbox_escalate(
        &self,
        supervisor: &wheel_proto::SupervisorInfo,
        summary: String,
        to_person: bool,
    ) -> InboxItem {
        let source = InboxSource {
            space: supervisor.space.clone(),
            pane: supervisor.pane.clone(),
            session: supervisor.supervisor.session.clone(),
        };
        self.inbox.add(
            source,
            InboxKind::Escalation,
            Some(summary),
            None,
            Some(supervisor.supervisor.id.clone()),
            to_person,
        )
    }

    fn inbox_add(
        &self,
        session: &str,
        pane: &str,
        kind: InboxKind,
        detail: Option<String>,
        perm: Option<String>,
    ) {
        // The space holding the pane now; a pane that is already gone adds nothing.
        let space = {
            let spaces = self.spaces.lock().unwrap();
            spaces.holding(pane).ok().and_then(|i| {
                let space = &spaces.spaces[i];
                agent_panes(&space.root)
                    .into_iter()
                    .any(|(p, s, _, _)| p == pane && s == session)
                    .then(|| space.id.clone())
            })
        };
        let Some(space) = space else {
            tracing::debug!(%session, %pane, ?kind, "inbox event for a pane that is gone; dropped");
            return;
        };
        let source = InboxSource {
            space,
            pane: pane.to_string(),
            session: session.to_string(),
        };
        self.inbox.add(source, kind, detail, perm, None, false);
    }
}
