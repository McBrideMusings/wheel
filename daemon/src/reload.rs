//! `pane_reload`: start a pane again on what it is on. An agent pane's agent and a terminal
//! pane's shell are processes wheeld owns, so wheeld ends and starts them; a web, browser or
//! plugin pane is drawn by each client, so wheeld tells every client to reload it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use wheel_proto::{PaneKind, ServerMsg, TermAgent};

use crate::pty::Term;
use crate::resume::Harness;
use crate::server::{AgentStart, AppState, Owner};
use crate::spaces::{space_find, space_find_mut, PANE_COLS, PANE_ROWS};
use crate::pty::ReadEnd;
use crate::views::{carried_options, Switching, TUI_EXIT};

/// A terminal pane's new shell: the terminal, the directory it started in, the conversation it
/// resumed, and one line saying what happened, for the log.
pub(crate) struct Shell {
    pub(crate) term: Arc<Term>,
    pub(crate) dir: String,
    pub(crate) agent: Option<Box<TermAgent>>,
    pub(crate) outcome: String,
}

impl AppState {
    /// Start `pane` again on what it is on (`ClientMsg::PaneReload`).
    pub async fn pane_reload(&self, pane: &str) -> Result<(), String> {
        let kind = {
            let spaces = self.spaces.lock().unwrap();
            let space = &spaces.spaces[spaces.holding(pane)?];
            space_find(space, pane)
                .cloned()
                .ok_or_else(|| format!("no pane {pane}"))?
        };
        match kind {
            PaneKind::Agent { .. } => self.reload_agent(pane).await,
            PaneKind::Terminal { .. } => self.reload_terminal(pane).await,
            PaneKind::Web { .. } => {
                let _ = self.global.send(ServerMsg::PaneReload { pane: pane.into() });
                // An artifact that declares no refresh command has nothing more to run. The run
                // can wait on a person's answer, so the reload does not wait for it; its record
                // is in `status`.
                if let Ok(run) = self.refresh_pane(pane) {
                    let pane = pane.to_string();
                    tokio::spawn(async move {
                        if let Err(e) = run.await {
                            tracing::warn!(%pane, error = %e, "the reload's refresh did not run");
                        }
                    });
                }
                tracing::info!(%pane, "web pane reloaded");
                Ok(())
            }
            PaneKind::Browser { .. } | PaneKind::Plugin { .. } => {
                let _ = self.global.send(ServerMsg::PaneReload { pane: pane.into() });
                tracing::info!(%pane, "pane reload sent to clients");
                Ok(())
            }
            PaneKind::Empty { .. } => Ok(()),
            PaneKind::Unknown(_) => Err(format!(
                "{pane} is a kind of pane this wheeld does not know, so it cannot reload it"
            )),
        }
    }

    /// End agent pane `pane`'s agent between turns and start a new one on the same conversation,
    /// directory, model, mode and permissions. The new agent has a new name, as a view switch's
    /// does, so every client attaches to it when the layout names it; the pane keeps its declared
    /// work. The home is the one the pane picked, else the login shell's answer now, asked before
    /// the agent ends so a shell that gives none leaves the agent running.
    async fn reload_agent(&self, pane: &str) -> Result<(), String> {
        let _switching = Switching::begin(self, pane)?;
        let (space_cwd, old, harness, model, context, place, pane_conversation) = {
            let spaces = self.spaces.lock().unwrap();
            let space = &spaces.spaces[spaces.holding(pane)?];
            match space_find(space, pane) {
                Some(PaneKind::Agent {
                    agent,
                    harness,
                    model,
                    context,
                    place,
                    conversation,
                    ..
                }) => (
                    space.cwd.clone(),
                    agent.clone(),
                    harness.clone(),
                    model.clone(),
                    *context,
                    place.as_deref().cloned().unwrap_or_default(),
                    conversation.clone(),
                ),
                _ => return Err(format!("{pane} no longer shows an agent")),
            }
        };
        let kind = Harness::parse(&harness).ok_or_else(|| format!("no agent {harness}"))?;
        self.agent_ready(&harness)?;
        let live = self.agent(&old)?;
        // Refuses a running turn or a waiting message, and every message until the agent is gone.
        let held = live.hold_for_handoff()?;
        let cwd = place.cwd.clone().unwrap_or_else(|| space_cwd.clone());
        let home = match place.env.get(kind.home_var()) {
            Some(picked) => (!picked.is_empty()).then(|| picked.clone()),
            None => self.shell_agent_home(&harness, &cwd).await?,
        };
        // An agent that has not saved its conversation yet (one restored on it, say) is still on
        // the one the pane saved.
        let conversation = live.saved_conversation().or(pane_conversation);
        let options = carried_options(&live.attach().config);
        let permissions = live.permissions();
        tracing::info!(%pane, agent = %old, ?conversation, ?home, "reloading the agent: stopping it");
        self.end_agent_wait(&old).await;
        drop(held);
        drop(live);
        let name = self.new_id('w');
        let started = self.spawn_agent(
            &name,
            (pane, &harness),
            (&cwd, &space_cwd),
            AgentStart {
                conversation: conversation.clone(),
                fork: None,
                home,
                context,
                model,
                permissions,
            },
        );
        {
            let mut spaces = self.spaces.lock().unwrap();
            let index = spaces.holding(pane).ok();
            let slot = index.and_then(|i| space_find_mut(&mut spaces.spaces[i], pane));
            match (slot, &started) {
                // Only the agent is new; the pane keeps its place, permissions and work.
                (Some(PaneKind::Agent { agent, conversation: saved, .. }), Ok(())) if *agent == old => {
                    *agent = name.clone();
                    if conversation.is_some() {
                        *saved = conversation.clone();
                    }
                }
                // The agent's program went missing: the pane keeps naming the agent it saved,
                // which starts once the program is found, as a restored pane's does.
                (Some(PaneKind::Agent { agent, conversation: saved, .. }), Err(e)) if *agent == old => {
                    if conversation.is_some() {
                        *saved = conversation.clone();
                    }
                    self.kept_agents.lock().unwrap().insert(old.clone());
                    let message = format!("agent {old} in pane {pane} stopped and could not start again: {e}");
                    self.changed(&spaces);
                    return Err(message);
                }
                // The pane closed or changed while the agent started: the new agent is not its.
                _ => {
                    self.end_agent(&name);
                    return Err(match &started {
                        Err(e) => format!("agent {old} in pane {pane} stopped and could not start again: {e}"),
                        Ok(()) => format!("{pane} closed or changed while its agent started again"),
                    });
                }
            }
            self.sync_conversations(&mut spaces);
            self.changed(&spaces);
        }
        if !options.is_empty() {
            self.carry_options(pane, &name, options);
        }
        let live = self.agent(&name)?;
        let start_wait = self.agent_timing.lock().unwrap().start_wait;
        match tokio::time::timeout(start_wait, live.started()).await {
            Ok(Ok(())) => {
                tracing::info!(%pane, old = %old, agent = %name, ?conversation, "agent reloaded");
                Ok(())
            }
            Ok(Err(e)) => Err(format!("agent {name} in pane {pane} could not start: {e}. Its pane stays open showing the error")),
            Err(_) => {
                tracing::info!(%pane, agent = %name, "agent reloaded, still starting");
                Ok(())
            }
        }
    }

    /// End terminal pane `pane`'s shell and start a new one in the directory it was last in,
    /// resuming the conversation of the coding agent it ran. A coding agent running in the old
    /// shell is asked to quit, as a view switch asks it, and the old terminal must be gone before
    /// the new shell resumes the conversation, so the two never write it at once.
    async fn reload_terminal(&self, pane: &str) -> Result<(), String> {
        let _switching = Switching::begin(self, pane)?;
        // The pane's saved directory and agent are sampled every two seconds; sample now so a `cd`
        // just made counts.
        if let Some(state) = self.me.upgrade() {
            let _ = tokio::task::spawn_blocking(move || state.sample_terminals()).await;
        }
        let (space_cwd, old, cwd, agent) = {
            let spaces = self.spaces.lock().unwrap();
            let space = &spaces.spaces[spaces.holding(pane)?];
            match space_find(space, pane) {
                Some(PaneKind::Terminal {
                    term, cwd, agent, ..
                }) => (space.cwd.clone(), term.clone(), cwd.clone(), agent.clone()),
                _ => return Err(format!("{pane} no longer shows a terminal")),
            }
        };
        if let Some(term) = self.term(&old) {
            if let Some(group) = term.foreground().filter(|g| Some(*g) != term.pid) {
                tracing::info!(%pane, term = %old, group, "reloading the terminal: asking its foreground job to quit");
                let _ = crate::resume::stop_foreground(group);
            }
            term.kill();
            // The terminal ends once every process holding it has; its output then stops.
            let (_, end) = term.read_text(None, TUI_EXIT).await;
            if !matches!(end, ReadEnd::Exited(_)) {
                tracing::warn!(%pane, term = %old, "the old terminal did not end; the pane keeps it");
                return Err(format!(
                    "{pane}'s shell or the program in it did not end within {}s, so no new shell was started",
                    TUI_EXIT.as_secs()
                ));
            }
            tracing::info!(%pane, term = %old, "reloading the terminal: the old one ended");
        }
        self.terms.lock().unwrap().remove(&old);
        let shell = self
            .reopen_shell(pane, &space_cwd, cwd, agent, &mut HashSet::new())
            .await;
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane).ok();
        let slot = index.and_then(|i| space_find_mut(&mut spaces.spaces[i], pane));
        let result = match (slot, shell) {
            (
                Some(PaneKind::Terminal {
                    term, cwd, agent, ..
                }),
                Ok(shell),
            ) if *term == old => {
                tracing::info!(%pane, term = %shell.term.id, outcome = %shell.outcome, "terminal reloaded");
                *term = shell.term.id.clone();
                *cwd = Some(shell.dir);
                *agent = shell.agent;
                Ok(())
            }
            // The old shell is gone and no new one started: the pane is empty, as a restore
            // leaves a pane whose shell cannot start.
            (Some(slot), Err(e)) if matches!(&*slot, PaneKind::Terminal { term, .. } if *term == old) => {
                tracing::warn!(%pane, error = %e, "could not start the pane's shell again; it is empty now");
                *slot = PaneKind::empty();
                Err(format!("{pane}'s shell could not start again, so the pane is empty: {e}"))
            }
            // The pane closed or changed while the shell started: the new shell is not its.
            (_, Ok(shell)) => {
                if let Some(held) = self.terms.lock().unwrap().remove(&shell.term.id) {
                    held.term.kill();
                }
                return Err(format!("{pane} closed or changed while its shell started again"));
            }
            (_, Err(e)) => return Err(format!("{pane} closed or changed while its shell started again: {e}")),
        };
        self.changed(&spaces);
        result
    }

    /// Start a new shell for terminal pane `pane` of the space in `space_cwd`: in `cwd` when it
    /// still exists, else the space's directory, typing the resume command of `agent`'s
    /// conversation when the conversation is still there and `resumed` does not hold it already.
    pub(crate) async fn reopen_shell(
        &self,
        pane: &str,
        space_cwd: &str,
        cwd: Option<String>,
        agent: Option<Box<TermAgent>>,
        resumed: &mut HashSet<(String, String)>,
    ) -> Result<Shell, String> {
        let mut outcome = "shell in the space's directory".to_string();
        let dir = match cwd {
            Some(dir) if Path::new(&dir).is_dir() => {
                outcome = format!("shell in {dir}");
                dir
            }
            Some(dir) => {
                tracing::warn!(%pane, %dir, "the pane's last directory is gone; its shell starts in the space's directory");
                space_cwd.to_string()
            }
            None => space_cwd.to_string(),
        };
        let term = self
            .open_term(
                dir.clone(),
                self.shell_in(space_cwd),
                PANE_COLS,
                PANE_ROWS,
                Owner::Pane,
                Some((pane, space_cwd)),
            )
            .await?;
        let mut kept = None;
        if let Some(agent) = agent {
            let key = (agent.name.clone(), agent.conversation.clone());
            if resumed.contains(&key) {
                outcome = format!("{outcome}; {} conversation {} already resumed in another pane, so a plain shell", agent.name, agent.conversation);
            } else if !crate::resume::conversation_exists(&agent, &dir) {
                tracing::warn!(%pane, agent = %agent.name, conversation = %agent.conversation, %dir, "the agent's conversation is gone; the pane starts a plain shell");
                outcome = format!(
                    "{outcome}; {} conversation {} is gone, so a plain shell",
                    agent.name, agent.conversation
                );
            } else if let Some(line) = crate::resume::resume_line(&agent) {
                match term.write(format!("{line}\n").into_bytes()) {
                    Ok(()) => {
                        outcome = format!(
                            "{outcome}; resumed {} conversation {}",
                            agent.name, agent.conversation
                        );
                        resumed.insert(key);
                        kept = Some(agent);
                    }
                    Err(e) => {
                        tracing::warn!(%pane, error = %e, "could not type the agent's resume command");
                        outcome = format!("{outcome}; could not resume {}", agent.name);
                    }
                }
            }
        }
        Ok(Shell {
            term,
            dir,
            agent: kept,
            outcome,
        })
    }
}
