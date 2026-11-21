//! `pane_reload`: start a pane again on what it is on. An agent pane's agent and a terminal
//! pane's shell are processes wheeld owns, so wheeld ends and starts them; a web, browser or
//! plugin pane is drawn by each client, so wheeld tells every client to reload it. An agent or
//! terminal pane's reload can change the environment its new process starts with ([`EnvChange`]).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wheel_proto::{PaneKind, ServerMsg, TermAgent};

use crate::agent::{Agent, Handoff, ReloadTurn};
use crate::pty::Term;
use crate::resume::Harness;
use crate::carry::Carried;
use crate::server::{AgentStart, AppState, Owner};
use crate::spaces::{picked_besides_home, space_find, space_find_mut, PANE_COLS, PANE_ROWS};
use crate::pty::ReadEnd;
use crate::views::{carried_options, Switching, TUI_EXIT};

/// A terminal pane's new shell: the terminal, the directory it started in, the conversation it
/// resumed, the command it typed again, and one line saying what happened, for the log.
pub(crate) struct Shell {
    pub(crate) term: Arc<Term>,
    pub(crate) dir: String,
    pub(crate) agent: Option<Box<TermAgent>>,
    pub(crate) command: Option<String>,
    pub(crate) outcome: String,
}

/// What a terminal pane ran before its shell ended, which its new shell starts again: the
/// directory it was in, the coding agent whose conversation it resumes, the command `pane_run` or
/// a runner started it with, and the message a resumed agent takes first (a restore's word that
/// its turn ended).
#[derive(Default)]
pub(crate) struct Before {
    pub(crate) cwd: Option<String>,
    pub(crate) agent: Option<Box<TermAgent>>,
    pub(crate) command: Option<String>,
    pub(crate) message: Option<String>,
}

/// What a reload did: started the pane again, or (an agent pane's while it is busy) queued the
/// reload on the pane's agent as `turn`.
pub enum Reloaded {
    Done,
    Queued { agent: String, turn: u64 },
}

/// What [`AppState::ready_reload`] read and checked for an agent pane's reload, which holds the
/// pane against a view switch until it drops.
struct ReadyReload<'a> {
    _switching: Switching<'a>,
    space_cwd: String,
    harness: String,
    model: Option<String>,
    context: Option<wheel_proto::ContextUse>,
    place: wheel_proto::AgentPlace,
    cwd: String,
    picked: BTreeMap<String, String>,
    home: Option<String>,
    conversation: Option<String>,
    kind: Harness,
    orchestrator: Option<wheel_proto::OrchestratorScope>,
}

/// How a reload changes the environment values picked for a pane's process: `set` picks values,
/// `unset` drops picked ones, so the login shell decides them again.
#[derive(Debug, Default)]
pub(crate) struct EnvChange {
    set: BTreeMap<String, String>,
    unset: Vec<String>,
}

impl EnvChange {
    /// The change, or why a name or value cannot be one: a name must be a shell variable's name
    /// that Wheel's own shell startup does not use (`ZDOTDIR`, `WHEEL_…`), and no name is both set
    /// and unset.
    pub(crate) fn new(set: BTreeMap<String, String>, unset: Vec<String>) -> Result<Self, String> {
        let variable = |name: &str| {
            let mut chars = name.chars();
            chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        };
        if let Some(bad) = set.keys().chain(&unset).find(|n| !variable(n)) {
            return Err(format!(
                "{bad:?} is not a variable name: use letters, digits and _, not starting with a digit"
            ));
        }
        if let Some(owned) = set.keys().chain(&unset).find(|n| *n == "ZDOTDIR" || n.starts_with("WHEEL_")) {
            return Err(format!("{owned} is Wheel's own: its shells and agents set it, so a pane cannot pick it"));
        }
        if let Some(both) = unset.iter().find(|n| set.contains_key(*n)) {
            return Err(format!("{both} is both set and unset; name it once"));
        }
        if let Some((name, _)) = set.iter().find(|(_, v)| v.contains('\0')) {
            return Err(format!("the value for {name} holds a NUL byte, which no environment can carry"));
        }
        Ok(EnvChange { set, unset })
    }

    fn is_empty(&self) -> bool {
        self.set.is_empty() && self.unset.is_empty()
    }

    /// What a client shows for the reload while it waits in an agent's queue: `Reload with` and
    /// each variable it sets, a value under the home directory written from `~`; `Reload` alone
    /// when it sets none.
    fn queued_text(&self) -> String {
        let home = std::env::var("HOME").ok().filter(|h| !h.is_empty());
        let shown = |value: &str| match home.as_deref().and_then(|h| value.strip_prefix(h)) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
            _ => value.to_string(),
        };
        if self.set.is_empty() {
            return "Reload".into();
        }
        let set: Vec<String> = self.set.iter().map(|(name, value)| format!("{name}={}", shown(value))).collect();
        format!("Reload with {}", set.join(" "))
    }

    /// `picked` with this change made.
    fn apply(&self, picked: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut env = picked.clone();
        for name in &self.unset {
            env.remove(name);
        }
        env.extend(self.set.clone());
        env
    }
}

impl AppState {
    /// Start `pane` again on what it is on (`ClientMsg::PaneReload`), with the environment values
    /// picked for an agent or terminal pane changed by `set` and `unset`.
    pub async fn pane_reload(
        self: &Arc<Self>,
        pane: &str,
        set: BTreeMap<String, String>,
        unset: Vec<String>,
    ) -> Result<Reloaded, String> {
        let change = EnvChange::new(set, unset)?;
        let kind = {
            let spaces = self.spaces.lock().unwrap();
            let space = &spaces.spaces[spaces.holding(pane)?];
            space_find(space, pane)
                .cloned()
                .ok_or_else(|| format!("no pane {pane}"))?
        };
        if !change.is_empty() {
            let why = match &kind {
                PaneKind::Agent { .. } | PaneKind::Terminal { .. } => None,
                PaneKind::Web { .. } => Some("a web pane runs no process of its own: its artifact's refresh command runs per artifact"),
                PaneKind::Plugin { .. } => Some("a plugin pane runs no process of its own: its script runs in each client, per plugin"),
                PaneKind::Browser { .. } => Some("a browser pane runs no process for wheeld to start"),
                PaneKind::Empty { .. } => Some("an empty pane runs no process"),
                PaneKind::Mirror { .. } => None,
                PaneKind::Unknown(_) => Some("wheeld does not know this kind of pane"),
            };
            if let Some(why) = why {
                return Err(format!("{pane} takes no --env or --unset-env: {why}"));
            }
        }
        match kind {
            PaneKind::Agent { .. } => self.reload_agent(pane, change).await,
            PaneKind::Terminal { .. } => self.reload_terminal(pane, &change).await.map(|()| Reloaded::Done),
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
                Ok(Reloaded::Done)
            }
            PaneKind::Browser { .. } | PaneKind::Plugin { .. } => {
                let _ = self.global.send(ServerMsg::PaneReload { pane: pane.into() });
                tracing::info!(%pane, "pane reload sent to clients");
                Ok(Reloaded::Done)
            }
            PaneKind::Empty { .. } => Ok(Reloaded::Done),
            PaneKind::Mirror { source, .. } => Err(format!(
                "{pane} is a mirror and holds nothing to reload; reload its source, {source}"
            )),
            PaneKind::Unknown(_) => Err(format!(
                "{pane} is a kind of pane this wheeld does not know, so it cannot reload it"
            )),
        }
    }

    /// Reload agent pane `pane`'s agent ([`AppState::reload_held`]): at once when it is between
    /// turns with nothing queued, else once the turn running and the messages queued before this
    /// ask are done. A queued reload waits in the agent's queue as a turn of its own, which
    /// `agent_unsend`, `agent_send_now` and `agent_clear_queue` act on as on a message; the messages
    /// queued behind it go to the agent it starts.
    async fn reload_agent(self: &Arc<Self>, pane: &str, change: EnvChange) -> Result<Reloaded, String> {
        let old = {
            let spaces = self.spaces.lock().unwrap();
            let space = &spaces.spaces[spaces.holding(pane)?];
            match space_find(space, pane) {
                Some(PaneKind::Agent { agent, .. }) => agent.clone(),
                _ => return Err(format!("{pane} no longer shows an agent")),
            }
        };
        let live = self.agent(&old)?;
        let turn = live.hold_or_queue_reload(change.queued_text())?;
        match turn {
            ReloadTurn::Now(held) => {
                self.reload_held(pane, &change, &live, held, Vec::new()).await?;
                Ok(Reloaded::Done)
            }
            ReloadTurn::Queued { turn, reached } => {
                let (state, pane, waiting) = (self.clone(), pane.to_string(), live.clone());
                tokio::spawn(async move {
                    let Ok(behind) = reached.await else {
                        tracing::info!(%pane, agent = %waiting.name, turn, "queued reload taken back");
                        return;
                    };
                    let held = waiting.adopt_reload_hold();
                    if let Err(e) = state.reload_held(&pane, &change, &waiting, held, behind).await {
                        tracing::warn!(%pane, agent = %waiting.name, turn, error = %e, "queued reload failed");
                        waiting.set_notice(Some(format!("The reload did not run: {e}")));
                    }
                });
                Ok(Reloaded::Queued { agent: old, turn })
            }
        }
    }

    /// End agent `live`, which `held` holds between turns, and start a new one in its pane on the
    /// same conversation, directory, model, mode and permissions, then send it `behind`, the texts
    /// of the messages a queued reload left queued behind it. The new agent has a new name, as a
    /// view switch's does, so every client attaches to it when the layout names it; the pane keeps
    /// its declared work. Its picked environment is the pane's with `change` made, saved on the
    /// pane once the new agent starts. The home is the one picked, else the login shell's answer
    /// now, asked before the agent ends so a shell that gives none leaves the agent running. When
    /// the home differs from the one the agent ran in, the conversation's file is carried there
    /// first ([`crate::carry`]); a carry that fails leaves the agent running. A reload refused
    /// before the agent ends lets `live` go on, with `behind` queued on it again.
    async fn reload_held(
        &self,
        pane: &str,
        change: &EnvChange,
        live: &Arc<Agent>,
        held: Handoff<'_>,
        behind: Vec<String>,
    ) -> Result<(), String> {
        let ready = match self.ready_reload(pane, change, live).await {
            Ok(ready) => ready,
            Err(e) => {
                drop(held);
                for text in behind {
                    if let Err(lost) = live.send(text) {
                        tracing::warn!(%pane, agent = %live.name, error = %lost, "a message queued behind the reload was lost");
                    }
                }
                return Err(e);
            }
        };
        let ReadyReload { _switching, space_cwd, harness, model, context, place, cwd, picked, home, conversation, kind, orchestrator } = ready;
        let old = live.name.clone();
        let options = carried_options(&live.attach().config);
        let permissions = live.permissions();
        tracing::info!(%pane, agent = %old, ?conversation, ?home, "reloading the agent: stopping it");
        self.end_agent_wait(&old).await;
        drop(held);
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
                env: picked_besides_home(picked.clone(), kind),
                orchestrator,
            },
        );
        {
            let mut spaces = self.spaces.lock().unwrap();
            let index = spaces.holding(pane).ok();
            let slot = index.and_then(|i| space_find_mut(&mut spaces.spaces[i], pane));
            match (slot, &started) {
                // Only the agent and its picked environment are new; the pane keeps its directory,
                // permissions and work.
                (Some(PaneKind::Agent { agent, conversation: saved, place: saved_place, .. }), Ok(())) if *agent == old => {
                    *agent = name.clone();
                    if conversation.is_some() {
                        *saved = conversation.clone();
                    }
                    *saved_place = wheel_proto::AgentPlace::boxed(place.cwd.clone(), picked.clone());
                }
                // The agent's program went missing: the pane keeps naming the agent it saved,
                // which starts once the program is found, as a restored pane's does.
                (Some(PaneKind::Agent { agent, conversation: saved, .. }), Err(e)) if *agent == old => {
                    if conversation.is_some() {
                        *saved = conversation.clone();
                    }
                    self.kept_agents.lock().unwrap().insert(old.clone());
                    let message = format!("agent {old} in pane {pane} stopped and could not start again: {e}");
                    self.changed(&mut spaces);
                    lost_behind(pane, &behind);
                    return Err(message);
                }
                // The pane closed or changed while the agent started: the new agent is not its.
                _ => {
                    self.end_agent(&name);
                    lost_behind(pane, &behind);
                    return Err(match &started {
                        Err(e) => format!("agent {old} in pane {pane} stopped and could not start again: {e}"),
                        Ok(()) => format!("{pane} closed or changed while its agent started again"),
                    });
                }
            }
            self.sync_conversations(&mut spaces);
            self.changed(&mut spaces);
        }
        if !options.is_empty() {
            self.carry_options(pane, &name, options);
        }
        let live = self.agent(&name)?;
        // Queued on the new agent at once: it takes them up once it is ready, in this order.
        for text in behind {
            if let Err(e) = live.send(text) {
                tracing::warn!(%pane, agent = %name, error = %e, "a message queued behind the reload was lost");
            }
        }
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

    /// What reloading agent `live` in `pane` needs, read and checked before the agent ends: the
    /// pane's settings, the home the new agent runs in, and its conversation carried there. Fails
    /// when the pane no longer shows `live`, the agent's program is missing, the login shell gives
    /// no home, or the carry would fail.
    async fn ready_reload(&self, pane: &str, change: &EnvChange, live: &Agent) -> Result<ReadyReload<'_>, String> {
        let _switching = Switching::begin(self, pane)?;
        let (space_cwd, harness, model, context, place, pane_conversation, orchestrator) = {
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
                    orchestrator,
                    ..
                }) if *agent == live.name => (
                    space.cwd.clone(),
                    harness.clone(),
                    model.clone(),
                    *context,
                    place.as_deref().cloned().unwrap_or_default(),
                    conversation.clone(),
                    *orchestrator,
                ),
                _ => return Err(format!("{pane} no longer shows agent {}", live.name)),
            }
        };
        let kind = Harness::parse(&harness).ok_or_else(|| format!("no agent {harness}"))?;
        self.agent_ready(&harness)?;
        let cwd = place.cwd.clone().unwrap_or_else(|| space_cwd.clone());
        let picked = change.apply(&place.env);
        let home = match picked.get(kind.home_var()) {
            Some(picked) => (!picked.is_empty()).then(|| picked.clone()),
            None => self.shell_agent_home(&harness, &cwd).await?,
        };
        // An agent that has not saved its conversation yet (one restored on it, say) is still on
        // the one the pane saved.
        let conversation = live.saved_conversation().or(pane_conversation);
        let homes = (
            live.home().map(Path::to_path_buf).or_else(|| kind.daemon_home()),
            home.as_ref().map(PathBuf::from).or_else(|| self.agent_home(&harness)).or_else(|| kind.daemon_home()),
        );
        if let (Some(conversation), (Some(old_home), Some(new_home))) = (&conversation, &homes) {
            if old_home != new_home {
                // Between turns the agent writes nothing, so its file is whole.
                let carried = crate::carry::carry(kind, (old_home, new_home), &cwd, conversation)
                    .map_err(|e| format!("{pane} was not reloaded: {e}"))?;
                log_carry(pane, conversation, old_home, new_home, &carried);
            }
        }
        Ok(ReadyReload { _switching, space_cwd, harness, model, context, place, cwd, picked, home, conversation, kind, orchestrator })
    }

    /// End terminal pane `pane`'s shell and start a new one in the directory it was last in,
    /// resuming the conversation of the coding agent it ran. A coding agent running in the old
    /// shell is asked to quit, as a view switch asks it, and the old terminal must be gone before
    /// the new shell resumes the conversation, so the two never write it at once. The new shell
    /// starts with the pane's picked environment changed by `change`; when that sets the home
    /// variable of the agent the shell ran, the conversation's file is carried to the new home
    /// once the old shell is gone, and a carry that would fail refuses the reload before it ends.
    async fn reload_terminal(&self, pane: &str, change: &EnvChange) -> Result<(), String> {
        let _switching = Switching::begin(self, pane)?;
        // The pane's saved directory and agent are sampled every two seconds; sample now so a `cd`
        // just made counts.
        if let Some(state) = self.me.upgrade() {
            let _ = tokio::task::spawn_blocking(move || state.sample_terminals()).await;
        }
        let (space_cwd, old, cwd, mut agent, command, picked) = {
            let spaces = self.spaces.lock().unwrap();
            let space = &spaces.spaces[spaces.holding(pane)?];
            match space_find(space, pane) {
                Some(PaneKind::Terminal {
                    term, cwd, agent, env, command, ..
                }) => (space.cwd.clone(), term.clone(), cwd.clone(), agent.clone(), command.clone(), change.apply(env)),
                _ => return Err(format!("{pane} no longer shows a terminal")),
            }
        };
        let dir = cwd.clone().unwrap_or_else(|| space_cwd.clone());
        // The home the shell's agent resumes in: the one set, else for one unset the login
        // shell's answer now, as a new agent pane asks it.
        let mut carry = None;
        if let Some((a, kind)) = agent.as_deref().and_then(|a| Some((a, Harness::parse(&a.name)?))) {
            let var = kind.home_var();
            let new = match change.set.get(var) {
                Some(set) => Some((!set.is_empty()).then(|| set.clone())),
                None if change.unset.iter().any(|n| n == var) => Some(self.shell_agent_home(kind.name(), &dir).await?),
                None => None,
            };
            if let Some(new) = new {
                let old_home = a.home.as_ref().map(PathBuf::from).or_else(|| kind.daemon_home());
                let new_home = new.as_ref().map(PathBuf::from).or_else(|| kind.daemon_home());
                if let (Some(old_home), Some(new_home)) = (old_home, new_home) {
                    if old_home != new_home {
                        carry = Some((kind, old_home, new_home, new));
                    }
                }
            }
        }
        if let (Some((kind, old_home, new_home, _)), Some(a)) = (&carry, &agent) {
            crate::carry::plan(*kind, (old_home, new_home), &dir, &a.conversation)
                .map_err(|e| format!("{pane} was not reloaded: {e}"))?;
        }
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
        let mut picked = picked;
        let mut kept_home = None;
        if let (Some((kind, old_home, new_home, new)), Some(a)) = (&carry, agent.as_deref_mut()) {
            match crate::carry::carry(*kind, (old_home, new_home), &dir, &a.conversation) {
                Ok(carried) => {
                    log_carry(pane, &a.conversation, old_home, new_home, &carried);
                    a.home = new.clone();
                }
                // The file changed or could not be written since the check, and the old shell is
                // gone: the new one resumes the conversation where it is, with the home it ran in.
                Err(e) => {
                    tracing::warn!(%pane, error = %e, "the conversation stays in its home");
                    let var = kind.home_var().to_string();
                    match &a.home {
                        Some(home) => picked.insert(var, home.clone()),
                        None => picked.remove(&var),
                    };
                    kept_home = Some(format!(
                        "{pane}'s shell started again in {}, the home its conversation is still in, because the carry failed: {e}",
                        old_home.display()
                    ));
                }
            }
        }
        let shell = self
            .reopen_shell(pane, &space_cwd, Before { cwd, agent, command, message: None }, &picked, &mut HashSet::new())
            .await;
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane).ok();
        let slot = index.and_then(|i| space_find_mut(&mut spaces.spaces[i], pane));
        let result = match (slot, shell) {
            (
                Some(PaneKind::Terminal {
                    term, cwd, agent, env, command, ..
                }),
                Ok(shell),
            ) if *term == old => {
                tracing::info!(%pane, term = %shell.term.id, outcome = %shell.outcome, ?picked, "terminal reloaded");
                *term = shell.term.id.clone();
                *cwd = Some(shell.dir);
                *agent = shell.agent;
                *command = shell.command;
                *env = picked;
                kept_home.map_or(Ok(()), Err)
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
        // The new shell may have set its title before the pane held it.
        self.sync_conversations(&mut spaces);
        self.changed(&mut spaces);
        result
    }

    /// Start a new shell for terminal pane `pane` of the space in `space_cwd`, with the environment
    /// values `picked` for it: in `before.cwd` when it still exists, else the space's directory,
    /// typing the resume command of `before.agent`'s conversation (with `before.message` as its
    /// first message) when the conversation is still there and `resumed` does not hold it already.
    /// A pane with no agent types `before.command` again; a pane that ran an agent leaves that to
    /// the resume, so the agent never starts twice.
    pub(crate) async fn reopen_shell(
        &self,
        pane: &str,
        space_cwd: &str,
        Before { cwd, agent, command, message }: Before,
        picked: &BTreeMap<String, String>,
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
                (PANE_COLS, PANE_ROWS),
                Owner::Pane,
                Some((pane, space_cwd)),
                picked,
            )
            .await?;
        let mut kept = None;
        let mut kept_command = None;
        let ran_agent = agent.is_some();
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
            } else if let Some(line) = crate::resume::resume_line(&agent, message.as_deref()) {
                match term.type_line(&line) {
                    Ok(()) => {
                        outcome = format!(
                            "{outcome}; resumed {} conversation {}",
                            agent.name, agent.conversation
                        );
                        if message.is_some() {
                            outcome = format!("{outcome}, told its turn ended");
                        }
                        resumed.insert(key);
                        kept = Some(agent);
                        // Still the pane's command while the agent it started runs.
                        kept_command = command.clone();
                    }
                    Err(e) => {
                        tracing::warn!(%pane, error = %e, "could not type the agent's resume command");
                        outcome = format!("{outcome}; could not resume {}", agent.name);
                    }
                }
            }
        }
        if let (false, Some(command)) = (ran_agent, command) {
            match term.type_line(&command) {
                Ok(()) => {
                    outcome = format!("{outcome}; reran {command}");
                    kept_command = Some(command);
                }
                Err(e) => {
                    tracing::warn!(%pane, %command, error = %e, "could not type the pane's command again");
                    outcome = format!("{outcome}; could not rerun {command}");
                }
            }
        }
        Ok(Shell {
            term,
            dir,
            agent: kept,
            command: kept_command,
            outcome,
        })
    }
}

/// Say in the log what carrying `conversation` from `old` to `new` did.
/// Say in the log that the messages a queued reload left behind it were lost with the agent that
/// did not start again.
fn lost_behind(pane: &str, behind: &[String]) {
    if !behind.is_empty() {
        tracing::warn!(%pane, lost = behind.len(), "messages queued behind the reload were lost: the agent did not start again");
    }
}

fn log_carry(pane: &str, conversation: &str, old: &Path, new: &Path, carried: &Carried) {
    let (old, new) = (old.display(), new.display());
    match carried {
        Carried::Same => tracing::info!(%pane, %conversation, %old, %new, "the new home already holds the conversation's file"),
        Carried::Copied(file) => tracing::info!(%pane, %conversation, %old, %new, file = %file.display(), "copied the conversation into the new home"),
        Carried::Replaced(file) => tracing::info!(%pane, %conversation, %old, %new, file = %file.display(), "replaced the new home's earlier copy of the conversation"),
        Carried::Nothing => tracing::info!(%pane, %conversation, %old, "the conversation has no file to carry yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(set: &[(&str, &str)], unset: &[&str]) -> Result<EnvChange, String> {
        EnvChange::new(
            set.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            unset.iter().map(|n| n.to_string()).collect(),
        )
    }

    #[test]
    fn a_change_names_only_variables_a_pane_may_pick() {
        assert!(change(&[("CLAUDE_CONFIG_DIR", "/h"), ("_X1", "")], &["CODEX_HOME"]).is_ok());
        for (set, unset) in [
            (&[("1X", "a")][..], &[][..]),
            (&[("A-B", "a")], &[]),
            (&[], &[""]),
            (&[("ZDOTDIR", "/z")], &[]),
            (&[], &["WHEEL_PICKED"]),
            (&[("A", "a")], &["A"]),
            (&[("A", "a\0b")], &[]),
        ] {
            assert!(change(set, unset).is_err(), "{set:?} {unset:?}");
        }
    }

    #[test]
    fn a_change_sets_and_drops_picked_values() {
        let picked = [("A".to_string(), "1".to_string()), ("B".to_string(), "2".to_string())].into();
        let changed = change(&[("A", "3"), ("C", "")], &["B"]).unwrap().apply(&picked);
        assert_eq!(changed, [("A".to_string(), "3".to_string()), ("C".to_string(), String::new())].into());
    }
}
