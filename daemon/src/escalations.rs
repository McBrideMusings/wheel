//! What a supervisor's agent does with its inbox (ADR-0007). The loop wakes the agent with one
//! inbox item at a time and the worker's last turn, and again when the supervisor's scope runs dry,
//! when the agent reads the scope's ready beads once more. During a wake the agent settles an item
//! through its MCP tools (`supervisor_mcp`): answer the worker, close the item, or forward it to the
//! lead's inbox. The methods here are what those tools call.

use std::time::Duration;

use wheel_proto::{LoopEventKind, SupervisorInboxItem, SupervisorState, WorkerInfo};

use crate::agents::{session_turn, Wait};
use crate::loops::{first_message, make_worktree, run_worker, Worker};
use crate::server::AppState;

/// How long the agent has to settle one inbox item.
const WAKE_WAIT: Duration = Duration::from_secs(15 * 60);

/// What wakes the agent.
enum Wake {
    Item(SupervisorInboxItem),
    Dry,
}

fn wake_prompt(item: &SupervisorInboxItem) -> String {
    let last = if item.last_turn.trim().is_empty() {
        "(the worker said nothing)"
    } else {
        item.last_turn.trim()
    };
    format!(
        "Inbox item {id} from worker {worker} on bead {bead}: {kind:?}. {detail}\n\n\
The worker's last turn:\n{last}\n\n\
Settle it with one of your tools: message_worker answers the worker, which starts again with your \
answer; close_item drops the item when nothing needs doing; forward_to_lead puts a one-line summary \
in the lead's inbox when only the person or the lead can settle it.",
        id = item.id,
        worker = item.worker,
        bead = item.bead,
        kind = item.kind,
        detail = item.detail,
    )
}

/// One line: whitespace runs become a single space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl AppState {
    /// Wake supervisor `id`'s agent when it has an inbox item it has not seen, or when `dry` (its
    /// queue is empty) and nothing runs. One wake at a time, off the caller's task.
    pub(crate) fn wake_agent(&self, id: &str, dry: bool) {
        let wake = {
            let mut kept = self.loops.kept.lock().unwrap();
            let Some(kept) = kept.get_mut(id) else { return };
            if kept.waking {
                return;
            }
            // An item whose worker is still being wound down waits for the next look.
            let next = kept.inbox.iter().find(|i| {
                !kept.woken.contains(&i.id) && !kept.running.iter().any(|w| w.bead == i.bead)
            });
            if let Some(item) = next.cloned() {
                kept.woken.insert(item.id.clone());
                kept.waking = true;
                Wake::Item(item)
            } else if dry && kept.running.is_empty() && kept.inbox.is_empty() {
                if kept.dry_woken {
                    return;
                }
                kept.dry_woken = true;
                kept.waking = true;
                Wake::Dry
            } else {
                // A worker runs again: the next dry spell wakes the agent anew.
                if !kept.running.is_empty() {
                    kept.dry_woken = false;
                }
                return;
            }
        };
        let (me, id) = (self.me.clone(), id.to_string());
        tokio::spawn(async move {
            let Some(state) = me.upgrade() else { return };
            match wake {
                Wake::Item(item) => state.wake_with_item(&id, item).await,
                Wake::Dry => state.wake_dry(&id).await,
            }
            if let Some(kept) = state.loops.kept.lock().unwrap().get_mut(&id) {
                kept.waking = false;
            };
        });
    }

    async fn wake_with_item(&self, id: &str, item: SupervisorInboxItem) {
        self.emit(
            id,
            Some(&item.worker),
            Some(&item.bead),
            LoopEventKind::Woken,
            format!(
                "item {} ({:?}): {}; last turn {} chars",
                item.id,
                item.kind,
                item.detail,
                item.last_turn.len()
            ),
        );
        let turn = async {
            let info = self.supervisor_show(id)?;
            let session = self.session(&info.supervisor.session)?;
            session_turn(&session, wake_prompt(&item), Wait::Total(WAKE_WAIT)).await
        }
        .await;
        let still_there = self
            .loops
            .kept
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|k| k.inbox.iter().any(|i| i.id == item.id));
        let why = match turn {
            Ok(done) if !done.finished => "the agent did not settle it in time".to_string(),
            Ok(done) => {
                tracing::info!(supervisor = %id, item = %item.id, turn = done.turn, reply_chars = done.reply.len(), "supervisor agent's wake turn ended");
                "the agent's turn ended without settling it".to_string()
            }
            Err(e) => format!("the agent's turn failed: {e}"),
        };
        if still_there && self.supervising(id) {
            self.emit(
                id,
                Some(&item.worker),
                Some(&item.bead),
                LoopEventKind::Unhandled,
                format!("item {} stays in the inbox: {why}", item.id),
            );
        }
    }

    async fn wake_dry(&self, id: &str) {
        self.emit(
            id,
            None,
            None,
            LoopEventKind::Dry,
            "queue empty and no worker running; the agent reads the scope again",
        );
        let planned = self.plan_queue(id).await;
        self.store_plan(id, planned);
    }

    /// Take inbox item `item` of supervisor `id` out of its inbox.
    fn take_item(&self, id: &str, item: &str) -> Result<SupervisorInboxItem, String> {
        let mut kept = self.loops.kept.lock().unwrap();
        let kept = kept
            .get_mut(id)
            .ok_or_else(|| format!("no supervisor {id}"))?;
        let at = kept
            .inbox
            .iter()
            .position(|i| i.id == item)
            .ok_or_else(|| format!("no item {item} in the inbox"))?;
        kept.resume.remove(item);
        Ok(kept.inbox.remove(at))
    }

    /// Answer the worker behind `item`: its bead is open again and the worker starts in its
    /// worktree with `text`. The item leaves the inbox.
    pub(crate) async fn supervisor_answer(
        &self,
        id: &str,
        item: &str,
        text: String,
    ) -> Result<String, String> {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("the answer is empty".into());
        }
        let info = self.supervisor_show(id)?;
        if info.supervisor.state != SupervisorState::Running {
            return Err(format!(
                "supervisor {id} is not running; the worker starts again when it is"
            ));
        }
        let (entry, resume) = {
            let kept = self.loops.kept.lock().unwrap();
            let kept = kept.get(id).ok_or_else(|| format!("no supervisor {id}"))?;
            let entry = kept
                .inbox
                .iter()
                .find(|i| i.id == item)
                .cloned()
                .ok_or_else(|| format!("no item {item} in the inbox"))?;
            let resume = kept
                .resume
                .get(item)
                .cloned()
                .ok_or_else(|| format!("item {item} has no worker to answer"))?;
            if kept.running.iter().any(|w| w.bead == resume.bead) {
                return Err(format!("bead {} has a worker running", resume.bead));
            }
            (entry, resume)
        };
        let repo = self.space_repo(&resume.space)?;
        make_worktree(&repo, &resume.worktree, &resume.bead)
            .await
            .map_err(|e| {
                format!(
                    "cannot make the worktree {}: {e}",
                    resume.worktree.display()
                )
            })?;
        // Claimed before the bead changes, so a second settling of the item fails with no effect.
        self.take_item(id, item)?;
        let reopened = async {
            let (status, _) = self.read_bead(&resume.space, &resume.bead).await?;
            if status == "blocked" {
                self.bd(
                    &resume.space,
                    vec![
                        "update".into(),
                        resume.bead.clone(),
                        "--status".into(),
                        "in_progress".into(),
                    ],
                )
                .await?;
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(e) = reopened {
            // The item goes back, so the agent or the person can settle it another way.
            if let Some(kept) = self.loops.kept.lock().unwrap().get_mut(id) {
                kept.inbox.insert(0, entry);
                kept.resume.insert(item.to_string(), resume);
            }
            return Err(e);
        }
        let w = Worker::resumed(id, &resume);
        let message = format!(
            "{}\n\nYou stopped with: {}\nYour supervisor answered: {text}\n\nCarry on from there: finish the \
work and commit it, then close the bead with `bd close {}` (or leave it closed if it is).",
            first_message(&w),
            entry.detail,
            w.bead,
        );
        {
            let mut kept = self.loops.kept.lock().unwrap();
            let kept = kept
                .get_mut(id)
                .ok_or_else(|| format!("no supervisor {id}"))?;
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
        }
        self.emit(
            id,
            Some(&w.session),
            Some(&w.bead),
            LoopEventKind::Answered,
            format!("item {item}: {text}"),
        );
        let me = self.me.clone();
        tokio::spawn(run_worker(me, w, message));
        Ok(format!(
            "item {item} answered; the worker for {} starts again",
            resume.bead
        ))
    }

    /// Drop `item` from the inbox; `note` is what the agent says about it.
    pub(crate) fn supervisor_close_item(
        &self,
        id: &str,
        item: &str,
        note: Option<String>,
    ) -> Result<String, String> {
        let gone = self.take_item(id, item)?;
        self.emit(
            id,
            Some(&gone.worker),
            Some(&gone.bead),
            LoopEventKind::Closed,
            format!(
                "item {item}: {}",
                note.map(|n| one_line(&n)).unwrap_or_default()
            ),
        );
        Ok(format!("item {item} closed"))
    }

    /// Put `summary` in the lead's inbox as an escalation from supervisor `id`, and drop `item`.
    pub(crate) fn supervisor_forward(
        &self,
        id: &str,
        item: &str,
        summary: String,
    ) -> Result<String, String> {
        let summary = one_line(&summary);
        if summary.is_empty() {
            return Err("the summary is empty".into());
        }
        let info = self.supervisor_show(id)?;
        let gone = self.take_item(id, item)?;
        let forwarded = self.inbox_escalate(&info, summary.clone());
        self.emit(
            id,
            Some(&gone.worker),
            Some(&gone.bead),
            LoopEventKind::Forwarded,
            format!("item {item} -> lead inbox {}: {summary}", forwarded.id),
        );
        Ok(format!(
            "item {item} forwarded to the lead as inbox item {}",
            forwarded.id
        ))
    }
}
