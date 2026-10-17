//! What a supervisor's agent does with its inbox (ADR-0007). The loop wakes the agent with one
//! inbox item at a time and the worker's last turn, and again when the supervisor's scope runs dry,
//! when the agent reads the scope's ready beads once more. During a wake the agent settles an item
//! through its MCP tools (`supervisor_mcp`): answer the worker, close the item, or forward it to the
//! lead's inbox. The methods here are what those tools call.

use std::time::Duration;

use wheel_proto::{
    unix_ms_now, Answer, AnswerFrom, InboxKind, InboxState, LoopEventKind, SupervisorInboxItem,
    SupervisorState, WorkerInfo, WorkerStage,
};

use crate::agents::{quiet_turn, Wait};
use crate::loops::{first_message, make_worktree, run_worker, Resume, Worker};
use crate::server::AppState;
use crate::supervisors::answers_block;

/// How long the agent has to settle one inbox item.
const WAKE_WAIT: Duration = Duration::from_secs(15 * 60);

/// Where an answered worker's item sits until the answer settles it.
enum Origin {
    /// The supervisor's own inbox, by item id.
    Inbox(String),
    /// Forwarded to the lead, by the id of the lead's inbox item.
    Forwarded(String),
}

impl Origin {
    fn item(&self) -> &str {
        match self {
            Origin::Inbox(i) | Origin::Forwarded(i) => i,
        }
    }
}

/// What wakes the agent.
enum Wake {
    Item(SupervisorInboxItem),
    Dry,
}

fn wake_prompt(item: &SupervisorInboxItem, open_beads: Option<&str>, answers: &[Answer]) -> String {
    let known = answers_block(answers);
    if let Some(open) = open_beads {
        return format!(
            "Inbox item {id} from worker {worker} on bead {bead}: {kind:?}. {detail}\n\n\
The scope's open beads:\n{open}\n{known}\n\
Settle it with one of your tools: file_bead files the follow-up as a new bead (title, body, labels; \
it records that it was discovered while working {bead}) when no open bead above covers it; close_item \
drops the item when an open bead above already covers it (name that bead); forward_to_lead puts a \
one-line summary in the lead's inbox when only the person or the lead can decide.",
            id = item.id,
            worker = item.worker,
            bead = item.bead,
            kind = item.kind,
            detail = item.detail,
            open = if open.trim().is_empty() { "(none)" } else { open },
        );
    }
    let last = if item.last_turn.trim().is_empty() {
        "(the worker said nothing)"
    } else {
        item.last_turn.trim()
    };
    format!(
        "Inbox item {id} from worker {worker} on bead {bead}: {kind:?}. {detail}\n\n\
The worker's last turn:\n{last}\n{known}\n\
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

/// Refuse a label the repo's `.beads/labels.toml` does not list. A repo with no such file lists
/// none, so every label passes.
fn check_labels(repo: &str, labels: &[String]) -> Result<(), String> {
    let path = std::path::Path::new(repo).join(".beads/labels.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?;
    let known: Vec<&str> = table
        .get("labels")
        .and_then(|l| l.as_table())
        .map(|l| l.keys().map(String::as_str).collect())
        .unwrap_or_default();
    match labels.iter().find(|l| !known.contains(&l.as_str())) {
        Some(bad) => Err(format!(
            "label {bad:?} is not in {}; use one of {}",
            path.display(),
            known.join(", ")
        )),
        None => Ok(()),
    }
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
            let open = if item.kind == LoopEventKind::FollowUp {
                Some(self.open_beads(id).await?)
            } else {
                None
            };
            quiet_turn(
                &session,
                wake_prompt(&item, open.as_deref(), &info.supervisor.answers),
                Wait::Total(WAKE_WAIT),
            )
            .await
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

    /// Take the item an answer settles out of where it waits.
    fn claim(&self, id: &str, origin: &Origin) -> Result<(), String> {
        match origin {
            Origin::Inbox(item) => self.take_item(id, item).map(|_| ()),
            Origin::Forwarded(item) => self
                .loops
                .kept
                .lock()
                .unwrap()
                .get_mut(id)
                .and_then(|k| k.forwarded.remove(item))
                .map(|_| ())
                .ok_or_else(|| format!("inbox item {item} has no worker to answer")),
        }
    }

    /// Put back an item whose answer failed, so it can be settled another way.
    fn unclaim(&self, id: &str, origin: &Origin, entry: SupervisorInboxItem, resume: Resume) {
        if let Some(kept) = self.loops.kept.lock().unwrap().get_mut(id) {
            match origin {
                Origin::Inbox(item) => {
                    kept.inbox.insert(0, entry);
                    kept.resume.insert(item.clone(), resume);
                }
                Origin::Forwarded(item) => {
                    kept.forwarded.insert(item.clone(), (entry, resume));
                }
            }
        }
    }

    /// Answer the worker behind `item`: its bead is open again and the worker starts in its
    /// worktree with `text`. The item leaves the inbox.
    pub(crate) async fn supervisor_answer(
        &self,
        id: &str,
        item: &str,
        text: String,
    ) -> Result<String, String> {
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
            (entry, resume)
        };
        self.answer_worker(
            id,
            Origin::Inbox(item.to_string()),
            entry,
            resume,
            text,
            AnswerFrom::Supervisor,
        )
        .await
    }

    /// Answer the worker behind the lead's inbox item `item`, which supervisor `id` forwarded: the
    /// same path `message_worker` takes, recorded as given `from` the lead or the person. Closes
    /// the lead's item and the supervisor's together.
    pub(crate) async fn answer_forward(
        &self,
        item: &str,
        answer: String,
        source: Option<String>,
        from: AnswerFrom,
    ) -> Result<String, String> {
        let source = source.map(|s| one_line(&s)).filter(|s| !s.is_empty());
        if from == AnswerFrom::Lead && source.is_none() {
            return Err("source is required when from is lead: name the decision bead, ADR or earlier words of the person the answer rests on".into());
        }
        if from == AnswerFrom::Supervisor {
            return Err("from must be lead or person".into());
        }
        let lead_item = self
            .inbox_list()
            .into_iter()
            .find(|i| i.id == item && i.state != InboxState::Closed)
            .ok_or_else(|| format!("no open inbox item {item}"))?;
        if lead_item.kind != InboxKind::Escalation {
            return Err(format!("inbox item {item} is not an escalation"));
        }
        let id = lead_item
            .supervisor
            .clone()
            .ok_or_else(|| format!("inbox item {item} names no supervisor"))?;
        let (entry, resume) = self
            .loops
            .kept
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|k| k.forwarded.get(item).cloned())
            .ok_or_else(|| format!("inbox item {item} has no worker to answer"))?;
        let bead = entry.bead.clone();
        let done = self
            .answer_worker(
                &id,
                Origin::Forwarded(item.to_string()),
                entry,
                resume,
                answer,
                from,
            )
            .await?;
        self.inbox_close(item)?;
        tracing::info!(
            "forward answered item={item} bead={bead} from={} source={}",
            from.name(),
            source.as_deref().unwrap_or("-")
        );
        Ok(done)
    }

    /// Start the worker of `entry` again with `text`, record the answer, and settle the item it
    /// came from.
    async fn answer_worker(
        &self,
        id: &str,
        origin: Origin,
        entry: SupervisorInboxItem,
        resume: Resume,
        text: String,
        from: AnswerFrom,
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
        if self
            .loops
            .kept
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|k| k.running.iter().any(|w| w.bead == resume.bead))
        {
            return Err(format!("bead {} has a worker running", resume.bead));
        }
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
        self.claim(id, &origin)?;
        let answer = one_line(&text);
        let reopened = async {
            let question = self
                .bead_question(&resume.space, &resume.bead)
                .await?
                .unwrap_or_else(|| one_line(&entry.detail));
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
            // The bead's notes keep the answer where the worker and any later reader can see it.
            self.bd(
                &resume.space,
                vec![
                    "update".into(),
                    resume.bead.clone(),
                    "--append-notes".into(),
                    format!("answered: {question} -> {answer}"),
                ],
            )
            .await?;
            Ok::<String, String>(question)
        }
        .await;
        let question = match reopened {
            Ok(question) => question,
            Err(e) => {
                // The item goes back, so the agent or the person can settle it another way.
                self.unclaim(id, &origin, entry, resume);
                return Err(e);
            }
        };
        // The worker starts either way: a supervisor that vanished meanwhile keeps nothing to record.
        if let Err(e) = self.record_answer(id, &resume.bead, question, answer, from) {
            tracing::warn!(supervisor = %id, error = %e, "answer not recorded");
        }
        let w = Worker::resumed(id, &resume);
        let message = format!(
            "{}\n\nYou stopped with: {}\n{who} answered: {text}\n\nCarry on from there: finish the \
work and commit it, then close the bead with `bd close {}` (or leave it closed if it is).",
            first_message(&w),
            entry.detail,
            w.bead,
            who = from.who(),
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
                stage: WorkerStage::Starting,
                pane: None,
            });
        }
        self.emit(
            id,
            Some(&w.session),
            Some(&w.bead),
            LoopEventKind::Answered,
            format!("item {}: {text}", origin.item()),
        );
        let me = self.me.clone();
        tokio::spawn(run_worker(me, w, message));
        Ok(format!(
            "item {} answered; the worker for {} starts again",
            origin.item(),
            resume.bead
        ))
    }

    /// The question a blocked worker left on `bead`: the text of the last `question:` line in its
    /// notes since the last `answered:` line. None when the notes hold none.
    async fn bead_question(&self, space: &str, bead: &str) -> Result<Option<String>, String> {
        let out = self.bd(space, vec!["show".into(), bead.into()]).await?;
        let value: serde_json::Value = serde_json::from_str(&out)
            .map_err(|e| format!("bd show {bead} printed something that is not JSON: {e}"))?;
        let bead_json = if value.is_array() { &value[0] } else { &value };
        Ok(bead_json["notes"].as_str().and_then(|notes| {
            // Only a question written since the last answer belongs to this block.
            notes
                .lines()
                .rev()
                .take_while(|l| !l.trim().starts_with("answered:"))
                .find_map(|l| l.trim().strip_prefix("question:"))
                .map(one_line)
                .filter(|q| !q.is_empty())
        }))
    }

    /// Keep an answer to a worker's question in supervisor `id`'s saved state, where every later
    /// wake of its agent lists it.
    pub(crate) fn record_answer(
        &self,
        id: &str,
        bead: &str,
        question: String,
        answer: String,
        from: AnswerFrom,
    ) -> Result<(), String> {
        let answer = Answer {
            bead: bead.to_string(),
            question,
            answer,
            at: unix_ms_now(),
            from,
        };
        tracing::info!(supervisor = %id, bead = %answer.bead, question = %answer.question, answer = %answer.answer, from = ?answer.from, "answer recorded");
        self.supervisor_change(id, "record answer", |s| {
            s.answers.push(answer);
            Ok(())
        })
        .map(|_| ())
    }

    /// The open beads in supervisor `id`'s scope, one `<id> <title>` per line.
    async fn open_beads(&self, id: &str) -> Result<String, String> {
        let info = self.supervisor_show(id)?;
        let mut args = vec![
            "list".to_string(),
            "--status=open".to_string(),
            "--limit=0".to_string(),
        ];
        if let Some(goal) = &info.supervisor.goal {
            args.push(format!("--parent={goal}"));
        }
        if let Some(focus) = &info.supervisor.focus {
            args.push(format!("--label={focus}"));
        }
        let out = self.bd(&info.space, args).await?;
        let beads: Vec<serde_json::Value> = serde_json::from_str(&out)
            .map_err(|e| format!("bd list printed something that is not a list of beads: {e}"))?;
        Ok(beads
            .iter()
            .map(|b| {
                format!(
                    "{} {}",
                    b["id"].as_str().unwrap_or_default(),
                    b["title"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// File inbox item `item` as a new bead in the supervisor's space, discovered while working the
    /// item's bead, and drop the item. A label the repo's `.beads/labels.toml` does not list is
    /// refused.
    pub(crate) async fn supervisor_file_bead(
        &self,
        id: &str,
        item: &str,
        title: String,
        body: String,
        labels: Vec<String>,
    ) -> Result<String, String> {
        let title = one_line(&title);
        if title.is_empty() {
            return Err("the title is empty".into());
        }
        let info = self.supervisor_show(id)?;
        let entry = self
            .loops
            .kept
            .lock()
            .unwrap()
            .get(id)
            .and_then(|k| k.inbox.iter().find(|i| i.id == item).cloned())
            .ok_or_else(|| format!("no item {item} in the inbox"))?;
        if entry.kind != LoopEventKind::FollowUp {
            return Err(format!(
                "item {item} is not a follow-up; settle it with message_worker, close_item or forward_to_lead"
            ));
        }
        let repo = self.space_repo(&info.space)?;
        check_labels(&repo, &labels)?;
        let mut args = vec![
            "create".to_string(),
            title.clone(),
            "--deps".into(),
            format!("discovered-from:{}", entry.bead),
        ];
        if !body.trim().is_empty() {
            args.push("--description".into());
            args.push(body);
        }
        if !labels.is_empty() {
            args.push("--labels".into());
            args.push(labels.join(","));
        }
        let out = self.bd(&info.space, args).await?;
        let value: serde_json::Value = serde_json::from_str(&out)
            .map_err(|e| format!("bd create printed something that is not JSON: {e}"))?;
        let created = if value.is_array() { &value[0] } else { &value };
        let new = created["id"]
            .as_str()
            .ok_or("bd create printed a bead with no id")?
            .to_string();
        self.take_item(id, item)?;
        self.emit(
            id,
            Some(&entry.worker),
            Some(&entry.bead),
            LoopEventKind::Filed,
            format!("item {item} filed as {new}: {title}"),
        );
        Ok(format!(
            "item {item} filed as {new}, discovered from {}",
            entry.bead
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
        let resume = self
            .loops
            .kept
            .lock()
            .unwrap()
            .get(id)
            .and_then(|k| k.resume.get(item).cloned());
        let gone = self.take_item(id, item)?;
        let forwarded = self.inbox_escalate(&info, summary.clone());
        // The answer to the lead's item needs the worker the supervisor's item stood for.
        if let Some(resume) = resume {
            if let Some(kept) = self.loops.kept.lock().unwrap().get_mut(id) {
                kept.forwarded
                    .insert(forwarded.id.clone(), (gone.clone(), resume));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::check_labels;

    #[test]
    fn a_label_missing_from_labels_toml_is_refused_and_a_listed_one_passes() {
        let dir = std::env::temp_dir().join(format!("wheel-labels-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".beads")).unwrap();
        std::fs::write(
            dir.join(".beads/labels.toml"),
            "required = [\"daemon\"]\n[labels.daemon]\nabout = \"d\"\n[labels.cli]\nabout = \"c\"\n",
        )
        .unwrap();
        let repo = dir.to_str().unwrap();
        assert!(check_labels(repo, &["daemon".into(), "cli".into()]).is_ok());
        let err = check_labels(repo, &["daemon".into(), "nope".into()]).unwrap_err();
        assert!(
            err.contains("\"nope\"") && err.contains("cli, daemon"),
            "{err}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
