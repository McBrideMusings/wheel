//! What a pane is, as labelled values a person copies: one builder behind the clients' ⓘ popover
//! and `wheel pane info`, so every surface shows the same fields.

use wheel_proto::{InfoField, PaneKind};

use crate::resume::{self, Agent};
use crate::server::AppState;
use crate::spaces::panes;

fn field(label: &str, value: impl Into<String>, hint: &str) -> InfoField {
    InfoField {
        label: label.into(),
        value: value.into(),
        hint: Some(hint.into()),
    }
}

impl AppState {
    /// The pane `name` means — a pane id, or an agent pane's session name — its kind and its fields.
    pub async fn pane_info(&self, name: &str) -> Result<(String, String, Vec<InfoField>), String> {
        let (pane, kind, space_cwd) = {
            let spaces = self.spaces.lock().unwrap();
            let found = spaces.spaces.iter().find_map(|space| {
                panes(&space.root)
                    .into_iter()
                    .find(|(id, kind)| {
                        *id == name
                            || matches!(kind, PaneKind::Agent { session, .. } if session == name)
                    })
                    .map(|(id, kind)| (id.to_string(), kind.clone(), space.cwd.clone()))
            });
            found.ok_or_else(|| format!("no pane {name}"))?
        };
        let pane_field = field("Pane", pane.clone(), "wheel pane … and wheel agent … take it");
        let (kind_name, fields) = match kind {
            PaneKind::Agent {
                session,
                agent,
                conversation,
                place,
                ..
            } => {
                let live = self.session(&session).ok();
                let conversation = live
                    .as_ref()
                    .and_then(|s| s.saved_conversation().or_else(|| s.conversation_id()))
                    .or(conversation);
                let parsed = Agent::parse(&agent);
                let place = place.map(|p| *p).unwrap_or_default();
                let home = place
                    .home
                    .or_else(|| self.agent_home(&agent).map(|h| h.display().to_string()))
                    .or_else(|| {
                        parsed.and_then(|a| a.daemon_home()).map(|h| h.display().to_string())
                    })
                    .unwrap_or_else(|| "-".into());
                let mut fields = vec![field(
                    "Agent",
                    parsed.map_or(agent.clone(), |a| a.title().to_string()),
                    "which program this pane runs",
                )];
                fields.push(field(
                    "Conversation",
                    conversation.unwrap_or_else(|| "-".into()),
                    &format!(
                        "the agent's own id; {} --resume takes it",
                        parsed.map_or(agent.clone(), |a| a.name().to_string())
                    ),
                ));
                fields.push(pane_field);
                fields.push(field("Session", session, "Wheel's name for this chat session"));
                fields.push(field(
                    "Home",
                    home,
                    &format!("agent.{agent}.home, as the agent's config directory"),
                ));
                fields.push(field(
                    "Directory",
                    place.cwd.unwrap_or(space_cwd),
                    "where the agent runs",
                ));
                ("agent", fields)
            }
            PaneKind::Terminal { term, cwd, agent, .. } => {
                let live = self.term(&term);
                let foreground = live.as_ref().and_then(|t| t.foreground());
                let directory = foreground
                    .and_then(resume::process_cwd)
                    .or(cwd)
                    .unwrap_or(space_cwd);
                let mut fields = vec![
                    pane_field,
                    field("Directory", directory, "where the shell is now"),
                    field(
                        "Program",
                        foreground
                            .and_then(resume::process_name)
                            .unwrap_or_else(|| "-".into()),
                        "what holds the terminal's foreground",
                    ),
                ];
                if let Some(agent) = agent {
                    fields.push(field(
                        "Conversation",
                        agent.session,
                        &format!("the agent's own id; {} --resume takes it", agent.name),
                    ));
                }
                ("terminal", fields)
            }
            PaneKind::Web { artifact, .. } => {
                let folder = self
                    .artifact_entry(&artifact, &space_cwd)
                    .ok()
                    .flatten()
                    .and_then(|e| e.watch_dir())
                    .map_or_else(|| "-".into(), |d| d.display().to_string());
                let mut fields = vec![
                    pane_field,
                    field(
                        "Artifact",
                        format!("{} {}", artifact.scope().name(), artifact.name()),
                        "scope and name",
                    ),
                    field("Folder", folder, "where the page is"),
                ];
                let refresh = self
                    .refreshes()
                    .await
                    .into_iter()
                    .find(|r| r.pane == pane);
                if let Some(refresh) = refresh {
                    fields.push(field("Refresh", refresh.command, "the command that feeds the page"));
                    let last = match &refresh.last {
                        None => "not run yet".to_string(),
                        Some(run) => match (run.exit, &run.error) {
                            (Some(code), _) => format!("exit {code}"),
                            (None, Some(why)) => why.clone(),
                            (None, None) => "no exit status".into(),
                        },
                    };
                    fields.push(field("Last exit", last, "how the last run ended"));
                }
                ("web", fields)
            }
            PaneKind::Browser { url, .. } => {
                ("browser", vec![pane_field, field("Page", url, "what the pane shows now")])
            }
            _ => ("empty", vec![pane_field]),
        };
        tracing::info!(pane = %pane, kind = kind_name, fields = fields.len(), "pane info");
        Ok((pane, kind_name.into(), fields))
    }
}
