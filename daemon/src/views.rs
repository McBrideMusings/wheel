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

use wheel_proto::{
    AgentPermissions, AgentPlace, Extra, PaneKind, PaneView, ReportedStatus, TermAgent,
};

use crate::resume::{self, Harness};
use crate::server::{AgentStart, AppState, CarriedView, Owner};
use crate::spaces::{space_find, space_panes, PANE_COLS, PANE_ROWS};

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
            .find_map(|s| space_find(s, pane))
            .ok_or_else(|| format!("no pane {pane}"))?;
        let (view, conversation) = match kind {
            PaneKind::Terminal { agent, .. } => (
                Some(PaneView::Terminal),
                agent.as_ref().map(|a| a.conversation.clone()),
            ),
            PaneKind::Agent {
                agent,
                conversation,
                ..
            } => (
                Some(PaneView::Agent),
                self.agent(agent)
                    .ok()
                    .and_then(|s| s.saved_conversation())
                    .or_else(|| conversation.clone()),
            ),
            _ => (None, None),
        };
        Ok((view, conversation, switch_ms))
    }

    /// The pane's kind, and its space's directory.
    fn kind_and_space(&self, pane: &str) -> Result<(PaneKind, String), String> {
        let spaces = self.spaces.lock().unwrap();
        let space = &spaces.spaces[spaces.holding(pane)?];
        let kind = space_find(space, pane)
            .cloned()
            .ok_or_else(|| format!("no pane {pane}"))?;
        Ok((kind, space.cwd.clone()))
    }

    /// Turn a terminal running Claude Code, Codex or pi into an agent pane on the same conversation.
    async fn view_as_agent(&self, pane: &str) -> Result<(), String> {
        let (kind, space_cwd) = self.kind_and_space(pane)?;
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
        let claimed = self.claimed_agents(&term_id);
        let group = term.foreground();
        let reported_conversation = self.conversation_reports.lock().unwrap().get(&term_id).cloned();
        let sampled = {
            let term = term.clone();
            tokio::task::spawn_blocking(move || {
                resume::sample(
                    term.pid,
                    group,
                    previous.as_ref(),
                    &claimed,
                    None,
                    reported_conversation.as_deref(),
                )
            })
            .await
            .map_err(|e| e.to_string())?
        };
        let (Some(agent), Some(cwd), Some(group)) = (sampled.agent, sampled.cwd, group) else {
            return Err(format!(
                "{pane}'s terminal is not running Claude Code, Codex or pi, so it has no conversation to show as a chat"
            ));
        };
        let kind = Harness::parse(&agent.name).ok_or_else(|| format!("no agent {}", agent.name))?;
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
        // What the agent's own hooks last reported. An agent that reported nothing (started
        // outside Wheel's wrapper) is not known to be mid-turn, so the switch goes ahead.
        let reported = self.reports.lock().unwrap().get(&term_id).map(|r| r.status);
        tracing::info!(%pane, term = %term_id, harness = kind.name(), conversation = %agent.conversation, status = ?reported, "turn check before the agent view");
        if matches!(
            reported,
            Some(ReportedStatus::Working | ReportedStatus::NeedsYou)
        ) {
            let why = if reported == Some(ReportedStatus::NeedsYou) {
                "waiting on you"
            } else {
                "in the middle of a turn"
            };
            return Err(format!("{title} in {pane} is {why}; switch once it ends"));
        }
        tracing::info!(%pane, term = %term_id, group, harness = kind.name(), conversation = %agent.conversation, %cwd, home = %home.display(), "switching to the agent view: stopping {title}");
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
        let name = self.new_id('w');
        let carried = self.view_options.lock().unwrap().get(pane).cloned();
        let (options, permissions) =
            carried.map_or_else(Default::default, |c| (c.options, c.permissions));
        self.spawn_agent(
            &name,
            (pane, kind.name()),
            (&cwd, &space_cwd),
            AgentStart {
                conversation: Some(agent.conversation.clone()),
                home: claude_dir.clone(),
                permissions: permissions.clone(),
                ..Default::default()
            },
        )?;
        let new = PaneKind::Agent {
            agent: name.clone(),
            harness: kind.name().into(),
            conversation: Some(agent.conversation.clone()),
            state: None,
            model: None,
            context: None,
            place: AgentPlace::boxed((cwd != space_cwd).then_some(cwd), claude_dir),
            permissions: AgentPermissions::boxed(permissions.permission_mode, permissions.sandbox),
            extra: Extra::default(),
        };
        self.install(pane, new, None)?;
        self.view_options.lock().unwrap().remove(pane);
        if !options.is_empty() {
            self.carry_options(pane, &name, options);
        }
        tracing::info!(%pane, %name, conversation = %agent.conversation, "pane shows the conversation as a chat");
        Ok(())
    }

    /// Turn an agent pane into a terminal running the same agent on the same conversation.
    async fn view_as_terminal(&self, pane: &str) -> Result<(), String> {
        let (kind, space_cwd) = self.kind_and_space(pane)?;
        let (name, kind, cwd, home) = match kind {
            PaneKind::Terminal { .. } => return Ok(()),
            PaneKind::Agent {
                agent,
                harness,
                place,
                ..
            } => {
                let kind = Harness::parse(&harness).ok_or_else(|| {
                    format!("{pane} runs {harness}, and only Claude Code's, Codex's and pi's conversations resume in a terminal")
                })?;
                let place = place.map(|p| *p).unwrap_or_default();
                (agent, kind, place.cwd, place.home)
            }
            other => return Err(not_switchable(pane, &other)),
        };
        let agent = self.agent(&name)?;
        let conversation = agent.saved_conversation().ok_or_else(|| {
            format!("agent {name} has no saved conversation yet; send it a message first")
        })?;
        // Refuses a running turn, and every message from here on.
        let _held = agent.hold_for_handoff()?;
        let options = carried_options(&agent.attach().config);
        let permissions = agent.permissions();
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
                Some((pane, &space_cwd)),
            )
            .await?;
        tracing::info!(%pane, agent = %name, harness = kind.name(), %conversation, "switching to the terminal view: stopping the agent");
        self.end_agent_wait(&name).await;
        tracing::info!(%pane, ?options, "keeping the agent view's model and mode for the switch back");
        self.view_options.lock().unwrap().insert(
            pane.to_string(),
            CarriedView {
                options,
                permissions,
            },
        );
        let agent = TermAgent {
            name: kind.name().into(),
            conversation: conversation.clone(),
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
        // pane left showing an agent pane with no agent behind it would answer nothing.
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

    /// Give the agent view `agent` the model and mode `pane` had before it was a terminal, as
    /// soon as its agent is up. A choice the agent no longer offers is logged and left at the
    /// agent's default.
    fn carry_options(&self, pane: &str, agent: &str, options: Vec<(String, String)>) {
        let Ok(agent) = self.agent(agent) else {
            return;
        };
        let pane = pane.to_string();
        tokio::spawn(async move {
            for (config, value) in options {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
                let offered = loop {
                    let have = agent.attach().config.iter().any(|o| o.id == config);
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
                match agent.set_config(config.clone(), value.clone()).await {
                    Ok(()) => {
                        tracing::info!(%pane, %config, %value, "agent view takes the option the pane had")
                    }
                    Err(e) => {
                        tracing::warn!(%pane, %config, %value, error = %e, "could not restore the option the pane had")
                    }
                }
            }
        });
    }

    /// The conversations the chat panes and the terminal panes other than `term`'s have saved.
    pub(crate) fn claimed_agents(&self, term: &str) -> HashSet<String> {
        let spaces = self.spaces.lock().unwrap();
        spaces
            .spaces
            .iter()
            .flat_map(|s| space_panes(s))
            .filter_map(|(_, kind)| match kind {
                PaneKind::Terminal {
                    term: t,
                    agent: Some(agent),
                    ..
                } if t != term => Some(agent.conversation.clone()),
                PaneKind::Agent {
                    conversation: Some(conversation),
                    ..
                } => Some(conversation.clone()),
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
