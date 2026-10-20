//! A Claude Code conversation shown either way in one pane: wheel's agent chat (an agent pane
//! running `claude-agent-acp`) or the Claude Code TUI in a terminal pane. Both programs read and
//! write the same per-directory store (ADR-0006), so a pane moves between them by changing kind
//! and keeping the conversation's id.
//!
//! One process drives a conversation at a time. A switch happens only between turns, stops the
//! view it leaves — the agent's process group killed and reaped, or the TUI's foreground job sent
//! SIGTERM and waited for until the shell has the terminal back — and only then starts the view it
//! goes to on that conversation.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use wheel_proto::{AgentPlace, Extra, PaneKind, PaneView, TermAgent};

use crate::resume::{self, Agent};
use crate::server::{AppState, Owner};
use crate::spaces::{find_pane, panes, PANE_COLS, PANE_ROWS};

/// How long Claude Code in a terminal has to quit after SIGTERM before the switch gives up.
const TUI_EXIT: Duration = Duration::from_secs(10);

impl AppState {
    /// Show `pane`'s conversation as `to` (when given), and say which view the pane is in and the
    /// conversation it shows.
    pub async fn pane_view(
        &self,
        pane: &str,
        to: Option<PaneView>,
    ) -> Result<(Option<PaneView>, Option<String>, Option<u64>), String> {
        let mut switch_ms = None;
        if let Some(to) = to {
            let _switching = Switching::begin(self, pane)?;
            let started = std::time::Instant::now();
            match to {
                PaneView::Agent => self.view_as_agent(pane).await?,
                PaneView::Terminal => self.view_as_terminal(pane).await?,
            }
            let ms = started.elapsed().as_millis() as u64;
            tracing::info!(%pane, ?to, switch_ms = ms, "pane view switched");
            switch_ms = Some(ms);
        }
        let spaces = self.spaces();
        let kind = spaces
            .spaces
            .iter()
            .find_map(|s| find_pane(&s.root, pane))
            .ok_or_else(|| format!("no pane {pane}"))?;
        let (view, conversation) = match kind {
            PaneKind::Terminal { agent, .. } => (
                Some(PaneView::Terminal),
                agent
                    .as_ref()
                    .filter(|a| a.name == Agent::Claude.name())
                    .map(|a| a.session.clone()),
            ),
            PaneKind::Agent {
                session,
                acp_session,
                ..
            } => (
                Some(PaneView::Agent),
                self.session(session)
                    .ok()
                    .and_then(|s| s.saved_conversation())
                    .or_else(|| acp_session.clone()),
            ),
            _ => (None, None),
        };
        Ok((view, conversation, switch_ms))
    }

    /// The pane's kind, and its space's directory.
    fn kind_and_space(&self, pane: &str) -> Result<(PaneKind, String, String), String> {
        let spaces = self.spaces.lock().unwrap();
        let space = &spaces.spaces[spaces.holding(pane)?];
        let kind = find_pane(&space.root, pane)
            .cloned()
            .ok_or_else(|| format!("no pane {pane}"))?;
        Ok((kind, space.id.clone(), space.cwd.clone()))
    }

    /// Turn a terminal running Claude Code into an agent pane on the same conversation.
    async fn view_as_agent(&self, pane: &str) -> Result<(), String> {
        let (kind, space_id, space_cwd) = self.kind_and_space(pane)?;
        let (term_id, previous) = match kind {
            PaneKind::Agent { .. } => return Ok(()),
            PaneKind::Terminal { term, agent, .. } => (term, agent.map(|a| *a)),
            other => return Err(not_switchable(pane, &other)),
        };
        if !self.agent_is_claude() {
            return Err(format!(
                "this daemon's agent panes do not run Claude Code, so {pane}'s conversation cannot be shown as a chat"
            ));
        }
        let term = self
            .term(&term_id)
            .ok_or_else(|| format!("{pane}'s terminal {term_id} is gone"))?;
        // Sampled now rather than read from the pane, which can be two seconds behind: the
        // process signalled below must be the Claude Code found here.
        let claimed = self.claimed_sessions(&term_id);
        let group = term.foreground();
        let sampled = {
            let term = term.clone();
            tokio::task::spawn_blocking(move || {
                resume::sample(term.pid, group, previous.as_ref(), &claimed)
            })
            .await
            .map_err(|e| e.to_string())?
        };
        let (Some(agent), Some(cwd), Some(group)) = (
            sampled.agent.filter(|a| a.name == Agent::Claude.name()),
            sampled.cwd,
            group,
        ) else {
            return Err(format!(
                "{pane}'s terminal is not running Claude Code, so it has no conversation to show as a chat"
            ));
        };
        let home = agent
            .home
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| Agent::Claude.default_home())
            .ok_or("no home directory to find Claude Code's conversations in")?;
        if resume::claude_turn_open(&home, &cwd, &agent.session) {
            return Err(format!(
                "Claude Code in {pane} is in the middle of a turn; switch once it ends"
            ));
        }
        tracing::info!(%pane, term = %term_id, group, session = %agent.session, %cwd, home = %home.display(), "switching to the agent view: stopping Claude Code");
        resume::stop_foreground(group)?;
        let deadline = tokio::time::Instant::now() + TUI_EXIT;
        // Over only once the shell holds the terminal again, or the terminal is gone: a moment
        // where no foreground group can be read is not Claude Code having quit.
        let back = |term: &crate::pty::Term| {
            !term.info().alive || term.foreground().is_some_and(|g| g != group)
        };
        while !back(&term) {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(%pane, group, "Claude Code did not quit; the pane stays a terminal");
                return Err(format!(
                    "Claude Code in {pane} did not quit within {}s, so the pane stays a terminal",
                    TUI_EXIT.as_secs()
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tracing::info!(%pane, group, "Claude Code quit");
        // The agent runs with the config directory the TUI used only when the daemon's own is
        // another one; the pane saves both so a restart reopens the conversation in the same place.
        let claude_dir = (Some(&home) != Agent::Claude.daemon_home().as_ref())
            .then(|| home.display().to_string());
        let session = self.new_id('w');
        self.spawn_agent(
            &session,
            (&space_id, pane),
            &cwd,
            Some(agent.session.clone()),
            claude_dir.clone(),
            None,
        )?;
        let new = PaneKind::Agent {
            session: session.clone(),
            acp_session: Some(agent.session.clone()),
            state: None,
            context: None,
            place: AgentPlace::boxed((cwd != space_cwd).then_some(cwd), claude_dir),
            extra: Extra::default(),
        };
        self.install(pane, new, None)?;
        tracing::info!(%pane, %session, conversation = %agent.session, "pane shows the conversation as a chat");
        Ok(())
    }

    /// Turn an agent pane into a terminal running Claude Code on the same conversation.
    async fn view_as_terminal(&self, pane: &str) -> Result<(), String> {
        let (kind, space_id, space_cwd) = self.kind_and_space(pane)?;
        let (name, cwd, home) = match kind {
            PaneKind::Terminal { .. } => return Ok(()),
            PaneKind::Agent { session, place, .. } => {
                let place = place.map(|p| *p).unwrap_or_default();
                (session, place.cwd, place.home)
            }
            other => return Err(not_switchable(pane, &other)),
        };
        if !self.agent_is_claude() {
            return Err(format!(
                "{pane}'s agent is not Claude Code, so its conversation cannot be shown in a terminal"
            ));
        }
        let session = self.session(&name)?;
        let conversation = session.saved_conversation().ok_or_else(|| {
            format!("agent {name} has no saved conversation yet; send it a message first")
        })?;
        // Refuses a running turn, and every message from here on.
        let _held = session.hold_for_handoff()?;
        // The directory the agent ran with: the pane's own, else the one set for every agent;
        // none means it inherited the daemon's environment.
        let home = home
            .map(PathBuf::from)
            .or_else(|| self.agent_config_dir())
            .map(|h| h.display().to_string());
        let dir = cwd.unwrap_or_else(|| space_cwd.clone());
        let term = self
            .open_term(
                dir.clone(),
                self.shell_in(&space_cwd),
                PANE_COLS,
                PANE_ROWS,
                Owner::Pane,
                Some((&space_id, pane, Some(&space_cwd))),
            )
            .await?;
        tracing::info!(%pane, session = %name, %conversation, "switching to the terminal view: stopping the agent");
        self.end_session_wait(&name).await;
        let agent = TermAgent {
            name: Agent::Claude.name().into(),
            session: conversation.clone(),
            home,
            context: None,
        };
        let line = resume::switch_line(&agent).ok_or("no resume command for claude")?;
        let new = PaneKind::Terminal {
            term: term.id.clone(),
            cwd: Some(dir),
            agent: Some(Box::new(agent)),
            extra: Extra::default(),
        };
        // The agent is gone, so the pane becomes the terminal before anything else can fail: a
        // pane left showing an agent with no session behind it would answer nothing.
        self.install(pane, new, None)?;
        if let Err(e) = term.write(format!("{line}\n").into_bytes()) {
            tracing::warn!(%pane, error = %e, "could not type the resume command");
            return Err(format!(
                "the agent stopped and {pane} is a terminal, but typing `{line}` into it failed: {e}"
            ));
        }
        tracing::info!(%pane, term = %term.id, %conversation, %line, "pane shows the conversation in a terminal");
        Ok(())
    }

    /// The agent sessions terminal panes other than `term`'s have saved.
    fn claimed_sessions(&self, term: &str) -> HashSet<String> {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .spaces
            .iter()
            .flat_map(|s| panes(&s.root))
            .filter_map(|(_, kind)| match kind {
                PaneKind::Terminal {
                    term: t,
                    agent: Some(agent),
                    ..
                } if t != term => Some(agent.session.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Marks a pane as mid-switch for as long as it lives.
struct Switching<'a> {
    state: &'a AppState,
    pane: String,
}

impl<'a> Switching<'a> {
    fn begin(state: &'a AppState, pane: &str) -> Result<Switching<'a>, String> {
        if !state
            .switching_views
            .lock()
            .unwrap()
            .insert(pane.to_string())
        {
            return Err(format!(
                "{pane} is already switching view; try again once that is done"
            ));
        }
        Ok(Switching {
            state,
            pane: pane.to_string(),
        })
    }
}

impl Drop for Switching<'_> {
    fn drop(&mut self) {
        self.state
            .switching_views
            .lock()
            .unwrap()
            .remove(&self.pane);
    }
}

fn not_switchable(pane: &str, kind: &PaneKind) -> String {
    let name = serde_json::to_value(kind)
        .ok()
        .and_then(|v| v["kind"].as_str().map(str::to_owned))
        .unwrap_or_default();
    format!(
        "{pane} is a {name} pane; only a terminal running Claude Code or an agent pane switches view"
    )
}
