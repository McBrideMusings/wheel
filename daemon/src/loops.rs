//! The supervisor loop (ADR-0007). One task per running supervisor fills its free worker slots
//! from the stored queue: it claims the bead, makes the bead's worktree and branch, and starts a
//! worker as a background agent session. Workers belong to the supervisor, not to a pane. When a
//! worker's turn ends the loop reads the bead and decides from its status alone: closed is landed,
//! blocked goes to the supervisor's inbox, anything else gets a continue message, three times, then
//! an inbox item. A worker silent past the stall timeout is killed and started again, three times,
//! then an inbox item. Every decision is a [`LoopEvent`], logged and kept per supervisor.
//!
//! A closed bead's branch lands before it counts as landed: the loop merges it onto `main` in a
//! worktree of its own, runs the project's `project.check` there, and only on a pass moves `main`
//! in the space's repo. A conflict or a red check sends the branch back to the same worker once,
//! with the reason; a second failure goes to the supervisor's inbox. Landings in one repo run one
//! at a time.
//!
//! A worker's permission requests are answered by its supervisor's policy (`crate::policy`). The
//! usage cap (wheel-lr2.9) is a later slice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;
use wheel_proto::{
    unix_ms_now, LoopEvent, LoopEventKind, QueueEntry, Supervisor, SupervisorInboxItem,
    SupervisorInfo, SupervisorState, WorkerInfo,
};

use crate::agents::{session_turn, Wait};
use crate::policy::{Policy, WorkerGate};
use crate::server::AppState;
use crate::session::{Session, SessionConfig};
use crate::settings::CHECK;

/// Continue messages a worker gets for one bead before its supervisor hears of it.
const MAX_CONTINUES: u32 = 3;

/// Times a stalled worker is started again before its supervisor hears of it.
const MAX_RESTARTS: u32 = 3;

/// The most events kept per supervisor; the oldest go first.
const KEEP_EVENTS: usize = 1000;

/// How long one `git` call may run.
const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the project's check may run on one landing.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The branch workers' branches land on.
const MAIN: &str = "main";

/// How much of a failing check's output a sent-back message and an inbox item keep.
const KEEP_OUTPUT: usize = 4000;

/// The loop's waits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopTiming {
    /// How often a supervisor's loop looks for a free slot.
    pub tick: Duration,
    /// How long a worker's turn may go without the session saying anything before the worker is
    /// killed and started again.
    pub stall: Duration,
}

impl Default for LoopTiming {
    fn default() -> Self {
        LoopTiming {
            tick: Duration::from_secs(2),
            stall: Duration::from_secs(10 * 60),
        }
    }
}

/// What the loop keeps for one supervisor.
#[derive(Default)]
struct Kept {
    events: Vec<LoopEvent>,
    next_event: u64,
    inbox: Vec<SupervisorInboxItem>,
    next_item: u64,
    running: Vec<WorkerInfo>,
    /// The space the running workers' beads live in, for releasing them when their supervisor's
    /// pane no longer says.
    space: String,
    /// Set from the first failed `bd` call until a call succeeds, so a tracker that stays down
    /// raises one event, not one per tick.
    bd_failing: bool,
}

/// The loops' state, by supervisor id. A supervisor with an entry has a loop task.
#[derive(Default)]
pub(crate) struct Loops {
    kept: Mutex<HashMap<String, Kept>>,
    /// One lock per repo directory, held for the length of a landing.
    landing: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Everything a worker's task needs to know about its bead.
struct Worker {
    supervisor: String,
    space: String,
    bead: String,
    title: String,
    model: String,
    session: String,
    worktree: PathBuf,
}

/// Why a landing did not happen.
enum LandError {
    /// The branch conflicts with `main` or the check is red: the worker can fix it.
    Rejected(String),
    /// Wheel could not try: no worktree, no `main`, a failed fast-forward. The worker cannot help.
    Failed(String),
}

/// A landing that went through.
struct Landed {
    /// `main` after the landing.
    commit: String,
    /// What was checked, for the event.
    note: String,
}

/// How the loop reads a bead after a worker's turn.
enum Verdict {
    Closed,
    Blocked,
    Open(String),
}

impl AppState {
    /// Read the supervisors' loops from `timing` instead of the defaults. Tests shorten these.
    pub fn set_loop_timing(&self, timing: LoopTiming) {
        *self.loop_timing.lock().unwrap() = timing;
    }

    /// Read the workers' brief and skills from `dir` instead of the repo's `worker/` folder this
    /// daemon was built from.
    pub fn set_worker_dir(&self, dir: PathBuf) {
        *self.worker_dir.lock().unwrap() = dir;
    }

    /// Start supervisor `id`'s loop, unless it has one.
    pub(crate) fn start_loop(&self, id: &str) {
        {
            let mut kept = self.loops.kept.lock().unwrap();
            if kept.contains_key(id) {
                return;
            }
            kept.insert(id.to_string(), Kept::default());
        }
        let (me, id) = (self.me.clone(), id.to_string());
        tracing::info!(supervisor = %id, "supervisor loop started");
        tokio::spawn(async move {
            loop {
                let Some(state) = me.upgrade() else { return };
                let tick = state.loop_timing.lock().unwrap().tick;
                if !state.loop_tick(&id).await {
                    return;
                }
                drop(state);
                tokio::time::sleep(tick).await;
            }
        });
    }

    /// Start the loop of every supervisor that is not stopped, after a restart.
    pub(crate) fn resume_loops(&self) {
        for info in self.supervisor_list() {
            let id = info.supervisor.id.clone();
            // Workers die with the daemon: a bead the saved supervisor still lists as given to
            // one has none now.
            if !info.supervisor.working.is_empty() {
                let (me, space, beads) = (
                    self.me.clone(),
                    info.space.clone(),
                    info.supervisor.working.clone(),
                );
                let supervisor = id.clone();
                tokio::spawn(async move {
                    for bead in beads {
                        let Some(state) = me.upgrade() else { return };
                        state
                            .release(
                                &supervisor,
                                &space,
                                None,
                                &bead,
                                "wheeld restarted, so the worker that had this bead is gone",
                            )
                            .await;
                    }
                });
            }
            if info.supervisor.state != SupervisorState::Stopped {
                self.start_loop(&id);
            }
        }
    }

    /// The decisions supervisor `id`'s loop has made, oldest first; the last `limit` of them when
    /// given.
    pub fn supervisor_events(
        &self,
        id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<LoopEvent>, String> {
        self.supervisor_show(id)?;
        let kept = self.loops.kept.lock().unwrap();
        let events = kept.get(id).map(|k| k.events.clone()).unwrap_or_default();
        let from = limit.map_or(0, |n| events.len().saturating_sub(n as usize));
        Ok(events[from..].to_vec())
    }

    /// What waits on supervisor `id`'s agent, oldest first.
    pub fn supervisor_inbox(&self, id: &str) -> Result<Vec<SupervisorInboxItem>, String> {
        self.supervisor_show(id)?;
        let kept = self.loops.kept.lock().unwrap();
        Ok(kept.get(id).map(|k| k.inbox.clone()).unwrap_or_default())
    }

    /// The workers supervisor `id` runs now, and how many items wait in its inbox.
    pub(crate) fn loop_summary(&self, id: &str) -> (Vec<WorkerInfo>, usize) {
        let kept = self.loops.kept.lock().unwrap();
        kept.get(id)
            .map(|k| (k.running.clone(), k.inbox.len()))
            .unwrap_or_default()
    }

    /// End every worker supervisor `id` runs and release their beads, each with an event; their
    /// tasks see the sessions end and finish.
    pub(crate) fn end_workers(&self, id: &str) {
        let (space, workers): (String, Vec<WorkerInfo>) = {
            let kept = self.loops.kept.lock().unwrap();
            kept.get(id)
                .map(|k| (k.space.clone(), k.running.clone()))
                .unwrap_or_default()
        };
        for worker in &workers {
            self.end_session(&worker.session);
        }
        if workers.is_empty() {
            return;
        }
        let (me, id) = (self.me.clone(), id.to_string());
        tokio::spawn(async move {
            for worker in workers {
                let Some(state) = me.upgrade() else { return };
                state
                    .release(
                        &id,
                        &space,
                        Some(&worker.session),
                        &worker.bead,
                        "its supervisor was stopped",
                    )
                    .await;
            }
        });
    }

    /// Record that a worker holds `bead` (or no longer does), in the supervisor's saved state.
    fn track(&self, id: &str, bead: &str, held: bool) {
        let _ = self.supervisor_change(id, "track worker bead", |s: &mut Supervisor| {
            s.working.retain(|b| b != bead);
            if held {
                s.working.push(bead.to_string());
            }
            Ok(())
        });
    }

    /// Give `bead` back to the tracker when it is still claimed: open, no assignee, and an event
    /// saying why. A bead in any other state is left as it is. Returns whether it was released.
    /// A failure is an event and leaves the bead listed as held, so a restart tries again.
    pub(crate) async fn release(
        &self,
        supervisor: &str,
        space: &str,
        worker: Option<&str>,
        bead: &str,
        why: &str,
    ) -> bool {
        let released = match self.read_bead(space, bead).await {
            Ok((status, _)) if status != "in_progress" => {
                tracing::info!(%supervisor, %bead, %status, "bead is not in_progress; nothing to release");
                self.track(supervisor, bead, false);
                return false;
            }
            Ok(_) => {
                self.bd(
                    space,
                    vec![
                        "update".into(),
                        bead.into(),
                        "--status".into(),
                        "open".into(),
                        "--assignee".into(),
                        String::new(),
                    ],
                )
                .await
            }
            Err(e) => Err(e),
        };
        match released {
            Ok(_) => {
                self.emit(
                    supervisor,
                    worker,
                    Some(bead),
                    LoopEventKind::Released,
                    format!("released: {why}"),
                );
                self.track(supervisor, bead, false);
                true
            }
            Err(e) => {
                self.emit(
                    supervisor,
                    worker,
                    Some(bead),
                    LoopEventKind::BdFailed,
                    format!("cannot release the bead ({why}): {e}"),
                );
                false
            }
        }
    }

    /// Record one decision: logged with its raw values and kept with the supervisor.
    pub(crate) fn emit(
        &self,
        supervisor: &str,
        worker: Option<&str>,
        bead: Option<&str>,
        kind: LoopEventKind,
        detail: impl Into<String>,
    ) {
        let detail = detail.into();
        tracing::info!(%supervisor, worker = worker.unwrap_or("-"), bead = bead.unwrap_or("-"), ?kind, %detail, "loop event");
        let mut kept = self.loops.kept.lock().unwrap();
        // A supervisor whose pane has closed keeps nothing.
        let Some(kept) = kept.get_mut(supervisor) else {
            return;
        };
        kept.next_event += 1;
        kept.events.push(LoopEvent {
            id: kept.next_event,
            at: unix_ms_now(),
            supervisor: supervisor.to_string(),
            worker: worker.map(String::from),
            bead: bead.map(String::from),
            kind,
            detail,
        });
        if kept.events.len() > KEEP_EVENTS {
            kept.events.remove(0);
        }
    }

    /// A decision the loop cannot settle: the event, and an item in the supervisor's inbox.
    fn raise(&self, w: &Worker, kind: LoopEventKind, detail: String) {
        self.emit(
            &w.supervisor,
            Some(&w.session),
            Some(&w.bead),
            kind,
            detail.clone(),
        );
        let mut kept = self.loops.kept.lock().unwrap();
        let Some(kept) = kept.get_mut(&w.supervisor) else {
            return;
        };
        kept.next_item += 1;
        kept.inbox.push(SupervisorInboxItem {
            id: format!("s{}", kept.next_item),
            at: unix_ms_now(),
            supervisor: w.supervisor.clone(),
            worker: w.session.clone(),
            bead: w.bead.clone(),
            kind,
            detail,
        });
    }

    /// A `bd` call failed: one event for the run of failures it starts.
    fn bd_failed(&self, supervisor: &str, bead: Option<&str>, error: &str) {
        let first = {
            let mut kept = self.loops.kept.lock().unwrap();
            let Some(kept) = kept.get_mut(supervisor) else {
                return;
            };
            !std::mem::replace(&mut kept.bd_failing, true)
        };
        if first {
            self.emit(supervisor, None, bead, LoopEventKind::BdFailed, error);
        }
    }

    fn bd_worked(&self, supervisor: &str) {
        if let Some(kept) = self.loops.kept.lock().unwrap().get_mut(supervisor) {
            kept.bd_failing = false;
        }
    }

    /// One look at supervisor `id`: fill its free slots when it is running. False when the loop
    /// is done, because the supervisor is stopped or its pane is gone.
    async fn loop_tick(&self, id: &str) -> bool {
        let Ok(info) = self.supervisor_show(id) else {
            self.loops.kept.lock().unwrap().remove(id);
            tracing::info!(supervisor = %id, "supervisor loop ended: its pane is gone");
            return false;
        };
        if info.supervisor.state == SupervisorState::Stopped {
            tracing::info!(supervisor = %id, "supervisor loop ended: stopped");
            return false;
        }
        if info.supervisor.state != SupervisorState::Running {
            return true;
        }
        let running = self.loop_summary(id).0.len();
        let free = (info.supervisor.workers as usize).saturating_sub(running);
        for _ in 0..free {
            let Ok(current) = self.supervisor_show(id) else {
                return true;
            };
            let Some(entry) = current
                .supervisor
                .queue
                .as_ref()
                .and_then(|q| q.first().cloned())
            else {
                break;
            };
            if !self.start_worker(&current, entry).await {
                break;
            }
        }
        true
    }

    /// Take `entry`, the head of the queue: claim the bead, make its worktree and start its
    /// worker. False when this tick should fill no more slots.
    async fn start_worker(&self, info: &SupervisorInfo, entry: QueueEntry) -> bool {
        let id = &info.supervisor.id;
        let bead = &entry.bead;
        let (status, title) = match self.read_bead(&info.space, bead).await {
            Ok(read) => {
                self.bd_worked(id);
                read
            }
            Err(e) => {
                self.bd_failed(id, Some(bead), &e);
                return false;
            }
        };
        if status != "open" {
            // Someone else took it, or it closed: not this supervisor's to start.
            tracing::info!(supervisor = %id, %bead, %status, "queued bead is not open; dropped from the queue");
            self.drop_from_queue(id, bead);
            return true;
        }
        // Listed before the claim, so a daemon that dies between the two finds the bead.
        self.track(id, bead, true);
        if let Err(e) = self
            .bd(
                &info.space,
                vec!["update".into(), bead.clone(), "--claim".into()],
            )
            .await
        {
            // The bead stays queued: the next look reads it again, and drops it if another
            // supervisor took it meanwhile.
            self.track(id, bead, false);
            self.bd_failed(id, Some(bead), &e);
            return false;
        }
        self.drop_from_queue(id, bead);
        let session = format!("worker-{bead}");
        self.emit(
            id,
            Some(&session),
            Some(bead),
            LoopEventKind::Claimed,
            format!("model {}", entry.model),
        );
        let worktree = self
            .instance_dir
            .join("worktrees")
            .join(&info.space)
            .join(bead);
        let w = Worker {
            supervisor: id.clone(),
            space: info.space.clone(),
            bead: bead.clone(),
            title,
            model: entry.model,
            session,
            worktree,
        };
        let repo = match self.space_repo(&w.space) {
            Ok(repo) => repo,
            Err(e) => {
                self.release(id, &w.space, Some(&w.session), bead, &e).await;
                return true;
            }
        };
        if let Err(e) = make_worktree(&repo, &w.worktree, bead).await {
            let why = format!("cannot make the worktree {}: {e}", w.worktree.display());
            self.release(id, &w.space, Some(&w.session), bead, &why)
                .await;
            return true;
        }
        let pushed = {
            let mut kept = self.loops.kept.lock().unwrap();
            match kept.get_mut(id) {
                None => false,
                Some(kept) => {
                    kept.space = w.space.clone();
                    kept.running.push(WorkerInfo {
                        session: w.session.clone(),
                        bead: w.bead.clone(),
                        title: w.title.clone(),
                        model: w.model.clone(),
                        worktree: w.worktree.display().to_string(),
                        branch: w.bead.clone(),
                        continues: 0,
                        restarts: 0,
                    });
                    true
                }
            }
        };
        if !pushed {
            // The pane closed while the worktree was made.
            let why = "its supervisor's pane closed while the worktree was made";
            self.release(id, &w.space, Some(&w.session), bead, why)
                .await;
            return false;
        }
        // A stop that came after the claim but before the push found no worker to end.
        if !self.supervising(id) {
            if let Some(kept) = self.loops.kept.lock().unwrap().get_mut(id) {
                kept.running.retain(|i| i.session != w.session);
            }
            self.release(
                id,
                &w.space,
                Some(&w.session),
                bead,
                "its supervisor was stopped",
            )
            .await;
            return false;
        }
        let me = self.me.clone();
        tokio::spawn(run_worker(me, w));
        true
    }

    fn drop_from_queue(&self, id: &str, bead: &str) {
        let _ = self.supervisor_change(id, "dequeue", |s: &mut Supervisor| {
            if let Some(queue) = &mut s.queue {
                queue.retain(|e| e.bead != bead);
            }
            Ok(())
        });
    }

    /// The bead's status and title, as `bd show` has them.
    async fn read_bead(&self, space: &str, bead: &str) -> Result<(String, String), String> {
        let out = self.bd(space, vec!["show".into(), bead.into()]).await?;
        let value: Value = serde_json::from_str(&out)
            .map_err(|e| format!("bd show {bead} printed something that is not JSON: {e}"))?;
        let bead_json = if value.is_array() { &value[0] } else { &value };
        let status = bead_json["status"]
            .as_str()
            .ok_or_else(|| format!("bd show {bead} printed a bead with no status"))?;
        let title = bead_json["title"].as_str().unwrap_or_default();
        Ok((status.to_string(), title.to_string()))
    }

    /// Start `w`'s agent session in its worktree: Wheel's own worker configuration, the default
    /// tools, and no pane.
    fn spawn_worker(&self, w: &Worker) -> Result<Arc<Session>, String> {
        let command =
            self.agent_command.lock().unwrap().clone().ok_or(
                "this daemon runs without a lead, so it has no agent command for a worker",
            )?;
        let info = self.supervisor_show(&w.supervisor)?;
        let gate = WorkerGate {
            me: self.me.clone(),
            policy: Policy {
                repo: PathBuf::from(self.space_repo(&w.space)?),
                worktree: w.worktree.clone(),
                bead: w.bead.clone(),
            },
            supervisor: w.supervisor.clone(),
            space: w.space.clone(),
            pane: info.pane,
            session: w.session.clone(),
        };
        let config = SessionConfig {
            command,
            cwd: w.worktree.clone(),
            tools: Vec::new(),
            setting_sources: crate::lead::SETTING_SOURCES.map(String::from).to_vec(),
            home: Some(crate::lead::AgentHome::worker(
                &self.instance_dir,
                self.worker_dir.lock().unwrap().clone(),
            )),
            mcp: None,
            restored: false,
            acp_session: None,
            on_acp_session: None,
            on_auth: None,
            gate: Some(Arc::new(gate)),
            restart: self.agent_timing.lock().unwrap().restart,
        };
        let session = Session::start(
            w.session.clone(),
            None,
            config,
            self.perms.clone(),
            self.global.clone(),
        );
        self.sessions
            .lock()
            .unwrap()
            .insert(w.session.clone(), session.clone());
        Ok(session)
    }

    /// Whether supervisor `id` is still there and not stopped.
    fn supervising(&self, id: &str) -> bool {
        self.supervisor_show(id)
            .is_ok_and(|s| s.supervisor.state != SupervisorState::Stopped)
    }

    fn worker_counts(&self, w: &Worker, continues: u32, restarts: u32) {
        if let Some(kept) = self.loops.kept.lock().unwrap().get_mut(&w.supervisor) {
            if let Some(info) = kept.running.iter_mut().find(|i| i.session == w.session) {
                info.continues = continues;
                info.restarts = restarts;
            }
        }
    }

    /// The worker is done with its slot.
    /// `keep_tracked` leaves the bead in the supervisor's `working` list, for a release that has
    /// not happened yet or failed: a restart tries it again.
    fn worker_done(&self, w: &Worker, keep_tracked: bool) {
        self.end_session(&w.session);
        if let Some(kept) = self.loops.kept.lock().unwrap().get_mut(&w.supervisor) {
            kept.running.retain(|i| i.session != w.session);
        }
        if !keep_tracked {
            self.track(&w.supervisor, &w.bead, false);
        }
    }

    /// What the bead says after a worker's turn.
    async fn judge(&self, w: &Worker) -> Result<Verdict, String> {
        let (status, _) = self.read_bead(&w.space, &w.bead).await?;
        Ok(match status.as_str() {
            "closed" => Verdict::Closed,
            "blocked" => Verdict::Blocked,
            _ => Verdict::Open(status),
        })
    }
}

fn first_message(w: &Worker) -> String {
    format!(
        "You are the worker for bead {id}: {title}\n\
Your working directory is the git worktree {path}, on branch {id}. Read the bead with `bd show {id}`, \
do the work and commit it, then close the bead with `bd close {id}`. If you cannot go on without an \
answer, set the bead blocked with `bd update {id} --status blocked` and write your question in its notes.",
        id = w.bead,
        title = w.title,
        path = w.worktree.display(),
    )
}

fn continue_message(w: &Worker) -> String {
    format!(
        "Bead {id} is still open. Continue working on it: finish the work and commit it, then close \
the bead with `bd close {id}`, or set it blocked and write your question in its notes.",
        id = w.bead
    )
}

fn restart_message(w: &Worker) -> String {
    format!(
        "{}\nYour earlier session on this bead went silent and was restarted. The worktree keeps what \
it had: check `git status` and `git log`, then continue.",
        first_message(w)
    )
}

/// Run one worker from its first message to its slot being freed.
async fn run_worker(me: Weak<AppState>, w: Worker) {
    let (mut continues, mut restarts) = (0, 0);
    // Set when the bead could not be released, so it stays listed for the next start.
    let mut stranded = false;
    let mut message = first_message(&w);
    'session: loop {
        let started = {
            let Some(state) = me.upgrade() else { return };
            state.begin_session(&w, restarts).await
        };
        let session = match started {
            Ok(session) => session,
            Err(e) => {
                if let Some(state) = me.upgrade() {
                    if state.supervising(&w.supervisor) {
                        state.raise(
                            &w,
                            LoopEventKind::Escalated,
                            format!("worker did not start: {e}"),
                        );
                    }
                    state.worker_done(&w, false);
                }
                return;
            }
        };
        let stall = match me.upgrade() {
            Some(state) => state.loop_timing.lock().unwrap().stall,
            None => return,
        };
        loop {
            // No hold on the daemon's state while the turn runs.
            let turn = session_turn(&session, message.clone(), Wait::Idle(stall)).await;
            let Some(state) = me.upgrade() else { return };
            if !state.supervising(&w.supervisor) {
                // The stop releases the bead; until it has, the bead stays listed.
                state.worker_done(&w, true);
                return;
            }
            match turn {
                Err(e) => {
                    state.raise(
                        &w,
                        LoopEventKind::Escalated,
                        format!("worker's turn failed: {e}"),
                    );
                    break 'session;
                }
                Ok(done) if !done.finished => {
                    state.end_session(&w.session);
                    state.emit(
                        &w.supervisor,
                        Some(&w.session),
                        Some(&w.bead),
                        LoopEventKind::Stalled,
                        format!("no word for {}s in turn {}", stall.as_secs_f64(), done.turn),
                    );
                    if restarts >= MAX_RESTARTS {
                        state.raise(
                            &w,
                            LoopEventKind::Escalated,
                            format!(
                                "stalled {} times and restarted {MAX_RESTARTS}",
                                restarts + 1
                            ),
                        );
                        break 'session;
                    }
                    restarts += 1;
                    state.worker_counts(&w, continues, restarts);
                    state.emit(
                        &w.supervisor,
                        Some(&w.session),
                        Some(&w.bead),
                        LoopEventKind::Restarted,
                        format!("restart {restarts} of {MAX_RESTARTS}"),
                    );
                    message = restart_message(&w);
                    continue 'session;
                }
                Ok(done) => match state.judge(&w).await {
                    Err(e) => {
                        state.bd_failed(&w.supervisor, Some(&w.bead), &e);
                        state.raise(
                            &w,
                            LoopEventKind::Escalated,
                            format!("cannot read the bead after turn {}: {e}", done.turn),
                        );
                        stranded = !state
                            .release(
                                &w.supervisor,
                                &w.space,
                                Some(&w.session),
                                &w.bead,
                                "the loop could not read the bead after the worker's turn",
                            )
                            .await;
                        break 'session;
                    }
                    Ok(Verdict::Closed) => {
                        state.bd_worked(&w.supervisor);
                        state.land_closed(&w, &session, stall, done.turn).await;
                        break 'session;
                    }
                    Ok(Verdict::Blocked) => {
                        state.bd_worked(&w.supervisor);
                        state.raise(
                            &w,
                            LoopEventKind::Blocked,
                            format!("bead blocked after turn {}", done.turn),
                        );
                        break 'session;
                    }
                    Ok(Verdict::Open(status)) => {
                        state.bd_worked(&w.supervisor);
                        if continues >= MAX_CONTINUES {
                            state.raise(
                                &w,
                                LoopEventKind::Escalated,
                                format!(
                                    "bead still {status} after turn {} and {MAX_CONTINUES} continue messages",
                                    done.turn
                                ),
                            );
                            break 'session;
                        }
                        continues += 1;
                        state.worker_counts(&w, continues, restarts);
                        state.emit(
                            &w.supervisor,
                            Some(&w.session),
                            Some(&w.bead),
                            LoopEventKind::Continued,
                            format!(
                                "bead {status} after turn {}; continue {continues} of {MAX_CONTINUES}",
                                done.turn
                            ),
                        );
                        message = continue_message(&w);
                    }
                },
            }
        }
    }
    if let Some(state) = me.upgrade() {
        state.worker_done(&w, stranded);
    }
}

fn rebase_message(w: &Worker, check: &str, why: &str) -> String {
    let check = if check.is_empty() {
        String::new()
    } else {
        format!(" and run the project's check, `{check}`, until it passes")
    };
    format!(
        "Your branch {id} did not land on {MAIN}:\n{why}\n\nIn your worktree, rebase onto {MAIN} \
(`git rebase {MAIN}`), resolve what conflicts{check}, and commit. Leave the bead closed.",
        id = w.bead
    )
}

/// The last `KEEP_OUTPUT` bytes of `text`, cut at a character boundary.
fn tail(text: &str) -> &str {
    let mut from = text.len().saturating_sub(KEEP_OUTPUT);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    &text[from..]
}

impl AppState {
    /// Land the closed bead of `w`: the merge, the check, the move of `main`. The first failure
    /// the worker can fix goes back to it as a message and the landing is tried again after its
    /// turn; the second, and any failure it cannot fix, goes to the supervisor's inbox.
    async fn land_closed(
        &self,
        w: &Worker,
        session: &Session,
        stall: Duration,
        turn: impl std::fmt::Display,
    ) {
        let mut sent_back = false;
        loop {
            let why = match self.land(w).await {
                Ok(landed) => {
                    self.emit(
                        &w.supervisor,
                        Some(&w.session),
                        Some(&w.bead),
                        LoopEventKind::Landed,
                        format!(
                            "bead closed after turn {turn}; branch {} landed on {MAIN} at {}; {}",
                            w.bead, landed.commit, landed.note
                        ),
                    );
                    self.remove_worktree(w).await;
                    return;
                }
                Err(LandError::Failed(e)) => {
                    self.raise(w, LoopEventKind::Escalated, format!("cannot land: {e}"));
                    return;
                }
                Err(LandError::Rejected(why)) => why,
            };
            if sent_back {
                self.raise(
                    w,
                    LoopEventKind::Escalated,
                    format!("landing failed again after the worker was sent back: {why}"),
                );
                return;
            }
            sent_back = true;
            self.emit(
                &w.supervisor,
                Some(&w.session),
                Some(&w.bead),
                LoopEventKind::SentBack,
                why.clone(),
            );
            let check = self.setting(CHECK, Some(&w.space)).unwrap_or_default();
            match session_turn(session, rebase_message(w, &check, &why), Wait::Idle(stall)).await {
                Ok(done) if done.finished => {}
                Ok(_) => {
                    self.raise(
                        w,
                        LoopEventKind::Escalated,
                        format!(
                            "worker said nothing for {}s after being sent back",
                            stall.as_secs_f64()
                        ),
                    );
                    return;
                }
                Err(e) => {
                    self.raise(
                        w,
                        LoopEventKind::Escalated,
                        format!("worker's turn failed after being sent back: {e}"),
                    );
                    return;
                }
            }
            if !self.supervising(&w.supervisor) {
                return;
            }
        }
    }

    /// Merge `w`'s branch onto `main` in a worktree wheeld owns, run the check there, and on a pass
    /// move `main` in the space's repo. The person's checkout is touched only by the last step, and
    /// only to fast-forward it.
    async fn land(&self, w: &Worker) -> Result<Landed, LandError> {
        use LandError::{Failed, Rejected};
        let repo = self.space_repo(&w.space).map_err(Failed)?;
        let lock = self
            .loops
            .landing
            .lock()
            .unwrap()
            .entry(repo.clone())
            .or_default()
            .clone();
        let _one_at_a_time = lock.lock().await;
        let check = self.setting(CHECK, Some(&w.space)).map_err(Failed)?;
        let repo = Path::new(&repo);
        let landing = self.instance_dir.join("landing").join(&w.space);
        prepare_landing(repo, &landing).await.map_err(Failed)?;
        let old = git(&landing, &["rev-parse", "HEAD"])
            .await
            .map_err(Failed)?;

        let mut merge = vec![];
        if git(&landing, &["config", "user.name"]).await.is_err() {
            merge.extend([
                "-c",
                "user.name=wheeld",
                "-c",
                "user.email=wheeld@localhost",
            ]);
        }
        merge.extend(["merge", "--no-edit", w.bead.as_str()]);
        if let Err(e) = git(&landing, &merge).await {
            let conflicted = e.contains("CONFLICT");
            let _ = git(&landing, &["merge", "--abort"]).await;
            if !conflicted {
                return Err(Failed(format!("cannot merge {} onto {MAIN}: {e}", w.bead)));
            }
            return Err(Rejected(format!(
                "merging {} onto {MAIN} failed:\n{e}",
                w.bead
            )));
        }
        let new = git(&landing, &["rev-parse", "HEAD"])
            .await
            .map_err(Failed)?;
        if new == old {
            return Ok(Landed {
                commit: new,
                note: format!("{} had nothing {MAIN} lacked, so no check ran", w.bead),
            });
        }
        let note = if check.is_empty() {
            "no project.check is set".to_string()
        } else {
            run_check(&landing, &check).await.map_err(Rejected)?;
            format!("project.check `{check}` passed")
        };
        // The person's checkout moves only by fast-forward; with another branch checked out,
        // only the ref moves.
        let on_main = git(repo, &["symbolic-ref", "--short", "-q", "HEAD"])
            .await
            .is_ok_and(|b| b == MAIN);
        let moved = if on_main {
            git(repo, &["merge", "--ff-only", &new]).await
        } else {
            git(
                repo,
                &["update-ref", &format!("refs/heads/{MAIN}"), &new, &old],
            )
            .await
        };
        moved.map_err(|e| Failed(format!("cannot move {MAIN} to {new}: {e}")))?;
        Ok(Landed { commit: new, note })
    }

    /// Remove the worktree of a landed bead; its branch stays.
    async fn remove_worktree(&self, w: &Worker) {
        let Ok(repo) = self.space_repo(&w.space) else {
            return;
        };
        let at = w.worktree.display().to_string();
        if let Err(e) = git(Path::new(&repo), &["worktree", "remove", "--force", &at]).await {
            tracing::warn!(bead = %w.bead, error = %e, "cannot remove a landed bead's worktree");
        }
    }

    /// Start (or start again) `w`'s session, set its model, and record that it started. The model
    /// the supervisor's agent named is applied when the worker's agent offers it; when it does
    /// not, the event says so and the worker runs on the agent's own.
    async fn begin_session(&self, w: &Worker, restarts: u32) -> Result<Arc<Session>, String> {
        let session = self.spawn_worker(w)?;
        // Checked after the session is in the map, so a stop that lands in between finds it to
        // end, and one that landed before is seen here.
        if !self.supervising(&w.supervisor) {
            self.end_session(&w.session);
            return Err("the supervisor is stopped".into());
        }
        let wait = self.agent_timing.lock().unwrap().start_wait;
        match tokio::time::timeout(wait, session.started()).await {
            Err(_) => {
                return Err(format!(
                    "its agent was not ready within {} seconds",
                    wait.as_secs()
                ))
            }
            Ok(Err(e)) => return Err(e),
            Ok(Ok(())) => {}
        }
        let model = match session.set_config("model".into(), w.model.clone()).await {
            Ok(()) => format!("model {}", w.model),
            Err(e) => format!("model {} not applied: {e}", w.model),
        };
        self.emit(
            &w.supervisor,
            Some(&w.session),
            Some(&w.bead),
            LoopEventKind::Started,
            format!(
                "{model}; worktree {}; start {}",
                w.worktree.display(),
                restarts + 1
            ),
        );
        Ok(session)
    }
}

/// Run `git` in `cwd` and return what it printed.
async fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(GIT_TIMEOUT, command.output()).await {
        Err(_) => {
            return Err(format!(
                "`git {}` ran longer than {}s",
                args.join(" "),
                GIT_TIMEOUT.as_secs()
            ))
        }
        Ok(Err(e)) => return Err(format!("cannot run git: {e}")),
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    tracing::info!(cwd = %cwd.display(), command = %format!("git {}", args.join(" ")), status = ?output.status.code(), %stdout, %stderr, "git ran");
    if !output.status.success() {
        // A merge reports its conflicts on stdout.
        let said = [stderr.as_str(), stdout.as_str()]
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("`git {}` failed: {said}", args.join(" ")));
    }
    Ok(stdout)
}

/// Bring the landing worktree at `path` to `main`, making it first when it is not there.
async fn prepare_landing(repo: &Path, path: &Path) -> Result<(), String> {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{MAIN}"),
        ],
    )
    .await
    .map_err(|_| format!("{} has no branch {MAIN}", repo.display()))?;
    if path.join(".git").exists() {
        git(path, &["checkout", "-q", "--detach", "-f", MAIN]).await?;
        git(path, &["reset", "-q", "--hard", MAIN]).await?;
        git(path, &["clean", "-q", "-fd"]).await?;
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    git(repo, &["worktree", "prune"]).await?;
    let at = path.display().to_string();
    git(repo, &["worktree", "add", "-q", "--detach", &at, MAIN]).await?;
    Ok(())
}

/// Run `check` with `sh -c` in `cwd`; its output's tail is the error when it exits non-zero.
async fn run_check(cwd: &Path, check: &str) -> Result<(), String> {
    let mut command = Command::new("sh");
    command
        .args(["-c", check])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(CHECK_TIMEOUT, command.output()).await {
        Err(_) => {
            return Err(format!(
                "the check `{check}` ran longer than {}s",
                CHECK_TIMEOUT.as_secs()
            ))
        }
        Ok(Err(e)) => return Err(format!("the check `{check}` cannot run: {e}")),
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    tracing::info!(cwd = %cwd.display(), %check, status = ?output.status.code(), %stdout, %stderr, "check ran");
    if output.status.success() {
        return Ok(());
    }
    let status = output
        .status
        .code()
        .map_or("was killed by a signal".to_string(), |c| {
            format!("exited {c}")
        });
    Err(format!(
        "the check `{check}` {status}:\n{}\n{}",
        tail(&stdout),
        tail(&stderr)
    ))
}

/// Make the worktree `path` on a branch named `bead` in `repo`. A worktree already there is kept,
/// and a branch that already exists is checked out rather than made.
async fn make_worktree(repo: &str, path: &Path, bead: &str) -> Result<(), String> {
    if path.join(".git").exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let repo = Path::new(repo);
    let at = path.display().to_string();
    let branch_exists = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{bead}"),
        ],
    )
    .await
    .is_ok();
    if branch_exists {
        git(repo, &["worktree", "add", &at, bead]).await?;
    } else {
        git(repo, &["worktree", "add", "-b", bead, &at]).await?;
    }
    Ok(())
}
