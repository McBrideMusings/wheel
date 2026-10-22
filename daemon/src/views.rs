//! A Claude Code, Codex or pi conversation shown either way in one pane: wheel's agent chat (an agent
//! pane running `claude --print`, `codex app-server` or `pi --mode rpc`) or the agent's TUI in a
//! terminal pane.
//! Both programs of an agent read and write the same store (ADR-0006), so a pane moves between
//! them by changing kind and keeping the conversation's id.
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
                agent.as_ref().map(|a| a.session.clone()),
            ),
            PaneKind::Agent {
                session,
                conversation,
                ..
            } => (
                Some(PaneView::Agent),
                self.session(session)
                    .ok()
                    .and_then(|s| s.saved_conversation())
                    .or_else(|| conversation.clone()),
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

    /// Turn a terminal running Claude Code, Codex or pi into an agent pane on the same conversation.
    async fn view_as_agent(&self, pane: &str) -> Result<(), String> {
        let (kind, space_id, space_cwd) = self.kind_and_space(pane)?;
        let (term_id, previous) = match kind {
            PaneKind::Agent { .. } => return Ok(()),
            PaneKind::Terminal { term, agent, .. } => (term, agent.map(|a| *a)),
            other => return Err(not_switchable(pane, &other)),
        };
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
        let (Some(agent), Some(cwd), Some(group)) = (sampled.agent, sampled.cwd, group) else {
            return Err(format!(
                "{pane}'s terminal is not running Claude Code, Codex or pi, so it has no conversation to show as a chat"
            ));
        };
        let kind = Agent::parse(&agent.name).ok_or_else(|| format!("no agent {}", agent.name))?;
        let title = kind.title();
        // Before the TUI is stopped, so a program that is missing leaves the terminal as it was.
        self.agent_ready(kind.name())
            .map_err(|why| format!("{pane}'s conversation cannot be shown as a chat: {why}"))?;
        let home = agent
            .home
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| kind.default_home())
            .ok_or_else(|| format!("no home directory to find {title}'s conversations in"))?;
        // Reads up to the whole rollout, so off the async runtime.
        let open = {
            let (home, cwd, session) = (home.clone(), cwd.clone(), agent.session.clone());
            tokio::task::spawn_blocking(move || resume::turn_open(kind, &home, &cwd, &session))
                .await
                .map_err(|e| format!("reading {title}'s transcript: {e}"))?
        };
        tracing::debug!(%pane, agent = kind.name(), session = %agent.session, open, "turn check before the agent view");
        if open {
            return Err(format!(
                "{title} in {pane} is in the middle of a turn; switch once it ends"
            ));
        }
        tracing::info!(%pane, term = %term_id, group, agent = kind.name(), session = %agent.session, %cwd, home = %home.display(), "switching to the agent view: stopping {title}");
        resume::stop_foreground(group)?;
        let deadline = tokio::time::Instant::now() + TUI_EXIT;
        // Over only once the shell holds the terminal again, or the terminal is gone: a moment
        // where no foreground group can be read is not Claude Code having quit.
        let back = |term: &crate::pty::Term| {
            !term.info().alive || term.foreground().is_some_and(|g| g != group)
        };
        while !back(&term) {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(%pane, group, "{title} did not quit; the pane stays a terminal");
                return Err(format!(
                    "{title} in {pane} did not quit within {}s, so the pane stays a terminal",
                    TUI_EXIT.as_secs()
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tracing::info!(%pane, group, "{title} quit");
        // The agent runs with the config directory the TUI used only when the daemon's own is
        // another one; the pane saves both so a restart reopens the conversation in the same place.
        let configured = self.agent_home(kind.name()).or_else(|| kind.daemon_home());
        let claude_dir = (Some(&home) != configured.as_ref()).then(|| home.display().to_string());
        let session = self.new_id('w');
        let carried = self.view_options.lock().unwrap().get(pane).cloned();
        self.spawn_agent(
            &session,
            (&space_id, pane, kind.name()),
            &cwd,
            Some(agent.session.clone()),
            claude_dir.clone(),
            None,
        )?;
        let new = PaneKind::Agent {
            session: session.clone(),
            agent: kind.name().into(),
            conversation: Some(agent.session.clone()),
            state: None,
            context: None,
            place: AgentPlace::boxed((cwd != space_cwd).then_some(cwd), claude_dir),
            extra: Extra::default(),
        };
        self.install(pane, new, None)?;
        self.view_options.lock().unwrap().remove(pane);
        if let Some(options) = carried {
            self.carry_options(pane, &session, options);
        }
        tracing::info!(%pane, %session, conversation = %agent.session, "pane shows the conversation as a chat");
        Ok(())
    }

    /// Turn an agent pane into a terminal running the same agent on the same conversation.
    async fn view_as_terminal(&self, pane: &str) -> Result<(), String> {
        let (kind, space_id, space_cwd) = self.kind_and_space(pane)?;
        let (name, kind, cwd, home) = match kind {
            PaneKind::Terminal { .. } => return Ok(()),
            PaneKind::Agent {
                session,
                agent,
                place,
                ..
            } => {
                let kind = Agent::parse(&agent).ok_or_else(|| {
                    format!("{pane} runs {agent}, and only Claude Code's, Codex's and pi's conversations resume in a terminal")
                })?;
                let place = place.map(|p| *p).unwrap_or_default();
                (session, kind, place.cwd, place.home)
            }
            other => return Err(not_switchable(pane, &other)),
        };
        let session = self.session(&name)?;
        let conversation = session.saved_conversation().ok_or_else(|| {
            format!("agent {name} has no saved conversation yet; send it a message first")
        })?;
        // Refuses a running turn, and every message from here on.
        let _held = session.hold_for_handoff()?;
        let options = carried_options(&session.attach().config);
        // The directory the agent ran with: the pane's own, else the agent's `agent.<name>.home`;
        // none means it inherited the daemon's environment.
        let home = home
            .map(PathBuf::from)
            .or_else(|| self.agent_home(kind.name()))
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
        tracing::info!(%pane, session = %name, agent = kind.name(), %conversation, "switching to the terminal view: stopping the agent");
        self.end_session_wait(&name).await;
        tracing::info!(%pane, ?options, "keeping the agent view's model and mode for the switch back");
        self.view_options
            .lock()
            .unwrap()
            .insert(pane.to_string(), options);
        let agent = TermAgent {
            name: kind.name().into(),
            session: conversation.clone(),
            home,
            context: None,
        };
        let line = resume::switch_line(&agent)
            .ok_or_else(|| format!("no resume command for {}", kind.name()))?;
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

    /// Give the agent view `session` the model and mode `pane` had before it was a terminal, as
    /// soon as its agent is up. A choice the agent no longer offers is logged and left at the
    /// agent's default.
    fn carry_options(&self, pane: &str, session: &str, options: Vec<(String, String)>) {
        let Ok(session) = self.session(session) else {
            return;
        };
        let pane = pane.to_string();
        tokio::spawn(async move {
            for (config, value) in options {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
                let offered = loop {
                    let have = session.attach().config.iter().any(|o| o.id == config);
                    if have {
                        break true;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        break false;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                };
                if !offered {
                    tracing::warn!(%pane, %config, %value, "agent view never offered the option; not restored");
                    continue;
                }
                match session.set_config(config.clone(), value.clone()).await {
                    Ok(()) => tracing::info!(%pane, %config, %value, "agent view takes the option the pane had"),
                    Err(e) => tracing::warn!(%pane, %config, %value, error = %e, "could not restore the option the pane had"),
                }
            }
        });
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

/// The model and mode of an agent view as (option id, current value).
fn carried_options(config: &[wheel_proto::ConfigOption]) -> Vec<(String, String)> {
    config
        .iter()
        .filter(|o| matches!(o.category.as_deref(), Some("model" | "mode")))
        .filter_map(|o| match &o.kind {
            wheel_proto::ConfigKind::Select { current, .. } => {
                Some((o.id.clone(), current.clone()))
            }
            _ => None,
        })
        .collect()
}

fn not_switchable(pane: &str, kind: &PaneKind) -> String {
    let name = serde_json::to_value(kind)
        .ok()
        .and_then(|v| v["kind"].as_str().map(str::to_owned))
        .unwrap_or_default();
    format!(
        "{pane} is a {name} pane; only a terminal running Claude Code, Codex or pi or an agent pane switches view"
    )
}
