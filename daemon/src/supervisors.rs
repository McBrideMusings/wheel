//! Supervisors (ADR-0007): a pane that pursues one goal epic or backlog focus. This module owns
//! the procedures on one — start, pause, resume, stop, set workers, set cap, list, show — which the
//! protocol, the lead's MCP tools and `wheel supervisor` all reach, so they return the same
//! `SupervisorInfo`. A supervisor lives in its pane's kind and is saved with the layout; its agent
//! is a session started from Wheel's own configuration (`crate::lead::AgentHome`).
//!
//! On start the agent is woken once with the scope's ready beads and answers with an ordered queue,
//! a model per bead, which is stored in the supervisor. The loop that starts workers from that
//! queue is a later slice.

use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;
use wheel_proto::{
    CreatedBy, Dir, Extra, Node, PaneKind, QueueEntry, Supervisor, SupervisorInfo, SupervisorState,
};

use crate::agents::{quiet_turn, Wait};
use crate::server::{AgentRole, AppState};
use crate::spaces::panes;

/// Workers a supervisor runs at once when its start names no number.
const DEFAULT_WORKERS: u32 = 2;

/// How long the agent has to answer the planning turn.
const PLAN_WAIT: Duration = Duration::from_secs(15 * 60);

/// One bead of `bd ready --json`, as far as the planning prompt shows it.
#[derive(Deserialize)]
struct ReadyBead {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    issue_type: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
}

fn check_workers(workers: u32) -> Result<u32, String> {
    if workers == 0 {
        return Err("workers is 0; a supervisor runs 1 or more".into());
    }
    Ok(workers)
}

fn check_cap(cap: Option<u32>) -> Result<Option<u32>, String> {
    match cap {
        Some(cap) if !(1..=100).contains(&cap) => Err(format!(
            "cap is {cap}; use 1 to 100, the share of the usage limit"
        )),
        cap => Ok(cap),
    }
}

/// A scope field as `bd` takes it as a flag value: trimmed, never empty, never a flag.
fn scope_word(what: &str, value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value.map(|v| v.trim().to_string()) else {
        return Ok(None);
    };
    if value.is_empty() || value.starts_with('-') || value.contains(char::is_whitespace) {
        return Err(format!("{value:?} is not a {what}"));
    }
    Ok(Some(value))
}

/// The goal or the focus as an id prefix: letters, digits, `.` and `_` kept, runs of anything
/// else one `-`.
fn id_prefix(scope: &str) -> String {
    let mut out = String::new();
    for c in scope.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Every supervisor in the tree with its pane.
fn supervisors_in(node: &Node) -> Vec<(&str, &Supervisor)> {
    panes(node)
        .into_iter()
        .filter_map(|(pane, kind)| match kind {
            PaneKind::Supervisor { supervisor, .. } => Some((pane, &**supervisor)),
            _ => None,
        })
        .collect()
}

/// The supervisors in the tree, with their panes, to start again after a restart.
pub(crate) fn supervisor_panes(node: &Node) -> Vec<(String, Supervisor)> {
    supervisors_in(node)
        .into_iter()
        .map(|(pane, supervisor)| (pane.to_string(), supervisor.clone()))
        .collect()
}

fn find_mut<'a>(node: &'a mut Node, id: &str) -> Option<(&'a str, &'a mut Supervisor)> {
    match node {
        Node::Pane {
            id: pane,
            kind: PaneKind::Supervisor { supervisor, .. },
            ..
        } if supervisor.id == id => Some((pane.as_str(), &mut **supervisor)),
        Node::Pane { .. } => None,
        Node::Split { a, b, .. } => find_mut(a, id).or_else(|| find_mut(b, id)),
    }
}

/// The planning message: the scope, the ready beads and the shape of the reply.
fn plan_prompt(supervisor: &Supervisor, ready: &[ReadyBead]) -> String {
    let scope = match (&supervisor.goal, &supervisor.focus) {
        (Some(goal), Some(focus)) => format!("the goal {goal}, among beads labelled {focus}"),
        (Some(goal), None) => format!("the goal {goal}"),
        (None, Some(focus)) => format!("the beads labelled {focus}"),
        (None, None) => String::new(),
    };
    let mut prompt = format!(
        "You supervise {scope}, with {} workers at once. Order these ready beads for workers to take, first first, \
and name the model each worker runs on (opus, sonnet or haiku). Leave out any bead not worth working now.\n\
Reply with only a JSON array: [{{\"bead\": \"<id>\", \"model\": \"<model>\"}}, ...]\n\nReady beads:\n",
        supervisor.workers
    );
    for bead in ready {
        let priority = bead.priority.map(|p| format!("P{p}")).unwrap_or_default();
        let kind = bead.issue_type.clone().unwrap_or_default();
        let labels = if bead.labels.is_empty() {
            String::new()
        } else {
            format!(" labels: {}", bead.labels.join(","))
        };
        prompt.push_str(&format!(
            "- {} ({priority} {kind}{labels}) {}\n",
            bead.id, bead.title
        ));
    }
    prompt
}

/// The JSON array in a reply: the whole reply, or what lies between its first `[` and last `]`
/// when the agent wrapped it in prose or a code fence.
fn json_array(reply: &str) -> Option<&str> {
    let (start, end) = (reply.find('[')?, reply.rfind(']')?);
    (start < end).then(|| &reply[start..=end])
}

/// The queue the agent's reply holds. Every bead must be one of the ready beads it was shown,
/// named once, with a model.
fn parse_queue(reply: &str, ready: &HashSet<&str>) -> Result<Vec<QueueEntry>, String> {
    let json = json_array(reply).ok_or("the agent's reply holds no JSON array")?;
    let queue: Vec<QueueEntry> = serde_json::from_str(json)
        .map_err(|e| format!("the agent's queue is not a list of {{bead, model}}: {e}"))?;
    let mut seen = HashSet::new();
    for entry in &queue {
        if !ready.contains(entry.bead.as_str()) {
            return Err(format!(
                "the agent queued {}, which is not ready",
                entry.bead
            ));
        }
        if entry.model.trim().is_empty() {
            return Err(format!("the agent gave {} no model", entry.bead));
        }
        if !seen.insert(entry.bead.as_str()) {
            return Err(format!("the agent queued {} twice", entry.bead));
        }
    }
    Ok(queue)
}

impl AppState {
    /// Start a supervisor in a new pane beside the focused one of `space` (the active space when
    /// None), and wake its agent with the scope's ready beads in the background. Returns at once;
    /// the queue shows in the supervisor when the agent has answered.
    pub async fn supervisor_start(
        &self,
        space: Option<String>,
        goal: Option<String>,
        focus: Option<String>,
        workers: Option<u32>,
        cap: Option<u32>,
        by: CreatedBy,
    ) -> Result<SupervisorInfo, String> {
        let goal = scope_word("bead id", goal)?;
        let focus = scope_word("bead label", focus)?;
        let Some(scope) = goal.as_deref().or(focus.as_deref()) else {
            return Err(
                "a supervisor needs a goal (an epic), a focus (a bead label), or both".into(),
            );
        };
        let base = id_prefix(scope);
        if base.is_empty() {
            return Err(format!(
                "{scope:?} has no letter or digit to name a supervisor after"
            ));
        }
        let workers = check_workers(workers.unwrap_or(DEFAULT_WORKERS))?;
        let cap = check_cap(cap)?;
        let space = match space {
            Some(space) => space,
            None => self
                .spaces()
                .active
                .ok_or("no space is open; give the space to supervise in")?,
        };
        let cwd = self.space_repo(&space)?;
        let focused = {
            let spaces = self.spaces.lock().unwrap();
            spaces.spaces[spaces.index(&space)?].focus.clone()
        };
        let pane = self.pane_split(&focused, Dir::Right, by, false)?;
        let (id, session) = (format!("{base}-{}", self.new_id('u')), self.new_id('w'));
        let started = self
            .spawn_agent(
                &session,
                &pane,
                &cwd,
                false,
                None,
                AgentRole::Supervisor(id.clone()),
            )
            .and_then(|()| {
                let supervisor = Supervisor {
                    id: id.clone(),
                    goal,
                    focus,
                    workers,
                    cap,
                    state: SupervisorState::Running,
                    session: session.clone(),
                    acp_session: None,
                    queue: None,
                    plan_error: None,
                    working: Vec::new(),
                };
                self.install(
                    &pane,
                    PaneKind::Supervisor {
                        supervisor: Box::new(supervisor),
                        extra: Extra::default(),
                    },
                    None,
                )
            });
        if let Err(e) = started {
            // A pane made only for this supervisor goes with it, and so does its agent.
            self.end_session(&session);
            let _ = self.pane_close(&pane);
            return Err(e);
        }
        tracing::info!(%space, %pane, supervisor = %id, workers, ?cap, "supervisor started");
        self.start_loop(&id);
        self.plan_in_background(&id);
        self.supervisor_show(&id)
    }

    /// Pause a running or held supervisor.
    pub fn supervisor_pause(&self, id: &str) -> Result<SupervisorInfo, String> {
        self.supervisor_change(id, "pause", |s| {
            ensure_live(s)?;
            match s.state {
                SupervisorState::Running | SupervisorState::Held => {
                    s.state = SupervisorState::Paused;
                    Ok(())
                }
                other => Err(format!(
                    "supervisor {} is {}; only a running or held supervisor pauses",
                    s.id,
                    state_name(other)
                )),
            }
        })
    }

    /// Resume a paused or held supervisor.
    pub fn supervisor_resume(&self, id: &str) -> Result<SupervisorInfo, String> {
        self.supervisor_change(id, "resume", |s| {
            ensure_live(s)?;
            match s.state {
                SupervisorState::Paused | SupervisorState::Held => {
                    s.state = SupervisorState::Running;
                    Ok(())
                }
                other => Err(format!(
                    "supervisor {} is {}; only a paused or held supervisor resumes",
                    s.id,
                    state_name(other)
                )),
            }
        })
    }

    /// Stop a supervisor for good: its agent ends and the pane keeps showing it as `stopped`.
    pub fn supervisor_stop(&self, id: &str) -> Result<SupervisorInfo, String> {
        let info = self.supervisor_change(id, "stop", |s| match s.state {
            SupervisorState::Stopped => Err(format!("supervisor {} is stopped already", s.id)),
            _ => {
                s.state = SupervisorState::Stopped;
                Ok(())
            }
        })?;
        self.end_session(&info.supervisor.session);
        self.end_workers(id);
        // Report the agent as gone, as a show from now on will.
        Ok(SupervisorInfo {
            agent: None,
            ..info
        })
    }

    /// Set how many workers the supervisor runs at once.
    pub fn supervisor_set_workers(&self, id: &str, workers: u32) -> Result<SupervisorInfo, String> {
        let workers = check_workers(workers)?;
        self.supervisor_change(id, "set workers", |s| {
            ensure_live(s)?;
            s.workers = workers;
            Ok(())
        })
    }

    /// Set the supervisor's usage cap, or remove it with None.
    pub fn supervisor_set_cap(&self, id: &str, cap: Option<u32>) -> Result<SupervisorInfo, String> {
        let cap = check_cap(cap)?;
        self.supervisor_change(id, "set cap", |s| {
            ensure_live(s)?;
            s.cap = cap;
            Ok(())
        })
    }

    /// Every supervisor in every space, in layout order.
    pub fn supervisor_list(&self) -> Vec<SupervisorInfo> {
        let spaces = self.spaces();
        spaces
            .spaces
            .iter()
            .flat_map(|space| {
                supervisors_in(&space.root)
                    .into_iter()
                    .map(|(pane, s)| self.info_of(&space.id, pane, s))
            })
            .collect()
    }

    /// One supervisor.
    pub fn supervisor_show(&self, id: &str) -> Result<SupervisorInfo, String> {
        self.supervisor_list()
            .into_iter()
            .find(|s| s.supervisor.id == id)
            .ok_or_else(|| format!("no supervisor {id}"))
    }

    /// The pane showing supervisor `id`.
    pub fn supervisor_pane(&self, id: &str) -> Result<String, String> {
        self.supervisor_show(id).map(|s| s.pane)
    }

    /// Show supervisor `id`'s running worker `session` in a pane of its own, split beside the
    /// supervisor's. Returns the new pane's id.
    pub fn worker_push(&self, id: &str, session: &str, by: CreatedBy) -> Result<String, String> {
        let info = self.supervisor_show(id)?;
        let Some(worker) = info.running.iter().find(|w| w.session == session) else {
            return Err(format!(
                "{session} is not a running worker of supervisor {id}"
            ));
        };
        if let Some(pane) = &worker.pane {
            return Err(format!("worker {session} is already in pane {pane}"));
        }
        let pane = self.pane_split(&info.pane, Dir::Right, by, false)?;
        let shown = self.install(
            &pane,
            PaneKind::Worker {
                session: session.to_string(),
                extra: Extra::default(),
            },
            None,
        );
        if let Err(e) = shown {
            let _ = self.pane_close(&pane);
            return Err(e);
        }
        // The worker may have ended, or a second push got in, between the check above and the
        // install: the pane would show a session nothing runs, or a worker twice.
        let ended = !self.loop_summary(id).0.iter().any(|w| w.session == session);
        let twice = self
            .spaces()
            .spaces
            .iter()
            .flat_map(|space| panes(&space.root))
            .filter(|(_, kind)| matches!(kind, PaneKind::Worker { session: s, .. } if s == session))
            .count()
            > 1;
        if ended || twice {
            let _ = self.pane_close(&pane);
            return Err(if ended {
                format!("{session} ended before it could be shown")
            } else {
                format!("worker {session} is already in a pane")
            });
        }
        tracing::info!(supervisor = %id, %session, %pane, "worker pushed to a pane");
        Ok(pane)
    }

    /// Each worker session a pane shows, with that pane.
    fn worker_panes(&self) -> std::collections::HashMap<String, String> {
        let spaces = self.spaces();
        spaces
            .spaces
            .iter()
            .flat_map(|space| panes(&space.root))
            .filter_map(|(pane, kind)| match kind {
                PaneKind::Worker { session, .. } => Some((session.clone(), pane.to_string())),
                _ => None,
            })
            .collect()
    }

    fn info_of(&self, space: &str, pane: &str, supervisor: &Supervisor) -> SupervisorInfo {
        let (mut running, inbox) = self.loop_summary(&supervisor.id);
        let shown = self.worker_panes();
        for worker in &mut running {
            worker.pane = shown.get(&worker.session).cloned();
        }
        SupervisorInfo {
            space: space.to_string(),
            pane: pane.to_string(),
            supervisor: supervisor.clone(),
            agent: self.session(&supervisor.session).ok().map(|s| s.state()),
            running,
            inbox,
        }
    }

    /// Apply `change` to supervisor `id`, then save and broadcast the layout, so every client
    /// sees the new state. A refused change leaves the supervisor as it was.
    pub(crate) fn supervisor_change(
        &self,
        id: &str,
        verb: &str,
        change: impl FnOnce(&mut Supervisor) -> Result<(), String>,
    ) -> Result<SupervisorInfo, String> {
        let (space, pane, supervisor) = {
            let mut spaces = self.spaces.lock().unwrap();
            let mut found = None;
            for space in &mut spaces.spaces {
                if let Some((pane, supervisor)) = find_mut(&mut space.root, id) {
                    change(supervisor)?;
                    found = Some((space.id.clone(), pane.to_string(), supervisor.clone()));
                    break;
                }
            }
            let found = found.ok_or_else(|| format!("no supervisor {id}"))?;
            self.changed(&spaces);
            found
        };
        tracing::info!(supervisor = %id, %space, %pane, verb, state = state_name(supervisor.state),
            workers = supervisor.workers, cap = ?supervisor.cap, "supervisor changed");
        Ok(self.info_of(&space, &pane, &supervisor))
    }

    /// Wake supervisor `id`'s agent with the scope's ready beads, off the caller's task.
    pub(crate) fn plan_in_background(&self, id: &str) {
        let (me, id) = (self.me.clone(), id.to_string());
        tokio::spawn(async move {
            if let Some(state) = me.upgrade() {
                let planned = state.plan_queue(&id).await;
                state.store_plan(&id, planned);
            }
        });
    }

    /// Plan again for every supervisor that has no queue and no reason it has none: its daemon
    /// went down between waking the agent and the agent answering.
    pub(crate) fn resume_planning(&self) {
        let waiting: Vec<String> = self
            .supervisor_list()
            .into_iter()
            // A supervisor whose agent could not start (a daemon with no lead) waits for a start
            // that has one, rather than recording an error that would keep it from planning then.
            .filter(|s| s.agent.is_some())
            .map(|s| s.supervisor)
            .filter(|s| {
                s.state != SupervisorState::Stopped && s.queue.is_none() && s.plan_error.is_none()
            })
            .map(|s| s.id)
            .collect();
        for id in waiting {
            tracing::info!(supervisor = %id, "planning again after a restart");
            self.plan_in_background(&id);
        }
    }

    /// Ask the agent for the queue: the scope's ready beads in, an ordered list with a model per
    /// bead out.
    pub(crate) async fn plan_queue(&self, id: &str) -> Result<Vec<QueueEntry>, String> {
        let info = self.supervisor_show(id)?;
        let supervisor = &info.supervisor;
        let mut args = vec!["ready".to_string(), "--limit=0".to_string()];
        if let Some(goal) = &supervisor.goal {
            args.push(format!("--parent={goal}"));
        }
        if let Some(focus) = &supervisor.focus {
            args.push(format!("--label={focus}"));
        }
        let ready: Vec<ReadyBead> = serde_json::from_str(&self.bd(&info.space, args).await?)
            .map_err(|e| format!("bd ready printed something that is not a list of beads: {e}"))?;
        tracing::info!(supervisor = %id, ready = ready.len(), "supervisor scope read");
        if ready.is_empty() {
            return Ok(Vec::new());
        }
        let session = self.session(&supervisor.session)?;
        let turn = quiet_turn(
            &session,
            plan_prompt(supervisor, &ready),
            Wait::Total(PLAN_WAIT),
        )
        .await?;
        if !turn.finished {
            return Err(format!(
                "the agent did not answer within {} seconds",
                PLAN_WAIT.as_secs()
            ));
        }
        tracing::info!(supervisor = %id, turn = turn.turn, reply_chars = turn.reply.len(), "supervisor agent answered");
        parse_queue(&turn.reply, &ready.iter().map(|b| b.id.as_str()).collect())
    }

    /// Keep what planning came to. A supervisor stopped meanwhile keeps nothing: its agent was
    /// ended, so an error here is not news.
    pub(crate) fn store_plan(&self, id: &str, planned: Result<Vec<QueueEntry>, String>) {
        let result = self.supervisor_change(id, "plan", |s| {
            if s.state == SupervisorState::Stopped {
                return Err("stopped".into());
            }
            match &planned {
                Ok(queue) => {
                    s.queue = Some(queue.clone());
                    s.plan_error = None;
                }
                Err(e) => s.plan_error = Some(e.clone()),
            }
            Ok(())
        });
        match (&planned, result) {
            (Ok(queue), Ok(_)) => {
                tracing::info!(supervisor = %id, queued = queue.len(), "supervisor queue stored")
            }
            (Err(e), Ok(_)) => {
                tracing::warn!(supervisor = %id, error = %e, "supervisor could not plan")
            }
            (_, Err(e)) => tracing::info!(supervisor = %id, reason = %e, "supervisor plan dropped"),
        }
    }
}

fn ensure_live(supervisor: &Supervisor) -> Result<(), String> {
    if supervisor.state == SupervisorState::Stopped {
        return Err(format!(
            "supervisor {} is stopped; start a new one",
            supervisor.id
        ));
    }
    Ok(())
}

fn state_name(state: SupervisorState) -> &'static str {
    match state {
        SupervisorState::Running => "running",
        SupervisorState::Paused => "paused",
        SupervisorState::Held => "held",
        SupervisorState::Stopped => "stopped",
    }
}
