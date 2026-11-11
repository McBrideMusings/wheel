//! What a pane is, as labelled values a person copies: one builder behind the clients' ⓘ popover
//! and `wheel pane info`, so every surface shows the same fields.

use wheel_proto::{InfoField, PaneKind, PaneLogin};

use crate::resume::{self, Harness};
use crate::server::AppState;
use crate::spaces::space_panes;

fn field(label: &str, value: impl Into<String>, hint: &str) -> InfoField {
    InfoField {
        label: label.into(),
        value: value.into(),
        hint: Some(hint.into()),
    }
}

impl AppState {
    /// The pane `name` means — a pane id, or an agent pane's agent name — its kind and its fields.
    pub async fn pane_info(&self, name: &str) -> Result<(String, String, Vec<InfoField>, Option<PaneLogin>), String> {
        let (pane, kind, space_cwd, site) = {
            let spaces = self.spaces.lock().unwrap();
            let found = spaces.spaces.iter().find_map(|space| {
                space_panes(space)
                    .into_iter()
                    .find(|(id, kind)| {
                        *id == name
                            || matches!(kind, PaneKind::Agent { agent, .. } if agent == name)
                    })
                    .map(|(id, kind)| {
                        (id.to_string(), kind.clone(), space.cwd.clone(), space.site_of(id))
                    })
            });
            found.ok_or_else(|| format!("no pane {name}"))?
        };
        let pane_field = field(
            "Pane",
            pane.clone(),
            "wheel pane … and wheel agent … take it",
        );
        let mut login = None;
        let (kind_name, fields) = match kind {
            PaneKind::Agent {
                agent,
                harness,
                conversation,
                place,
                ..
            } => {
                let live = self.agent(&agent).ok();
                let conversation = live
                    .as_ref()
                    .and_then(|s| s.saved_conversation().or_else(|| s.conversation_id()))
                    .or(conversation);
                let parsed = Harness::parse(&harness);
                let place = place.map(|p| *p).unwrap_or_default();
                let home = place
                    .home
                    .or_else(|| self.agent_home(&harness).map(|h| h.display().to_string()))
                    .or_else(|| {
                        parsed
                            .and_then(|a| a.daemon_home())
                            .map(|h| h.display().to_string())
                    });
                if let Some(dir) = &home {
                    let account = match parsed {
                        Some(Harness::Claude | Harness::Codex) => self.usage_account_in(std::path::Path::new(dir)),
                        _ => None,
                    };
                    login = Some(PaneLogin {
                        dir: dir.clone(),
                        account,
                    });
                }
                let weekly = login
                    .as_ref()
                    .and_then(|l| l.account.as_ref())
                    .map(|a| match &a.usage.reading {
                        // Only the weekly window; an API-billed login or one with other windows has none.
                        Some(r) => match r.windows.iter().find(|w| w.id == "weekly") {
                            Some(w) => match &w.resets_at {
                                Some(at) => format!("{}% (resets {at})", w.pct.round()),
                                None => format!("{}%", w.pct.round()),
                            },
                            None => match &r.billing {
                                Some(b) => format!("no weekly window ({b})"),
                                None => "no weekly window".into(),
                            },
                        },
                        None => a.usage.why.clone().unwrap_or_else(|| "no reading".into()),
                    })
                    .unwrap_or_else(|| "no usage reading".into());
                let home = home.unwrap_or_else(|| "-".into());
                let mut fields = vec![field(
                    "Program",
                    parsed.map_or(harness.clone(), |a| a.title().to_string()),
                    "which program this pane runs",
                )];
                fields.push(field(
                    "Conversation",
                    conversation.unwrap_or_else(|| "-".into()),
                    &format!(
                        "the agent's own id; {} --resume takes it",
                        parsed.map_or(harness.clone(), |a| a.name().to_string())
                    ),
                ));
                fields.push(pane_field);
                fields.push(field(
                    "Agent",
                    agent,
                    "Wheel's name for this agent",
                ));
                fields.push(field(
                    "Home",
                    home,
                    &format!("agent.{harness}.home, as the agent's config directory"),
                ));
                fields.push(field(
                    "Weekly",
                    weekly,
                    "the weekly limit of the account whose login lives in Home",
                ));
                fields.push(field(
                    "Directory",
                    place.cwd.unwrap_or(space_cwd),
                    "where the agent runs",
                ));
                ("agent", fields)
            }
            PaneKind::Terminal {
                term, cwd, agent, ..
            } => {
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
                        agent.conversation,
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
                let refresh = self.refreshes().await.into_iter().find(|r| r.pane == pane);
                if let Some(refresh) = refresh {
                    fields.push(field(
                        "Refresh",
                        refresh.command,
                        "the command that feeds the page",
                    ));
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
                let mut fields = vec![pane_field, field("Page", url, "what the pane shows now")];
                for (phone, mode) in self.renderers.phone_modes(&pane) {
                    fields.push(field(
                        &format!("Phone {phone}"),
                        if mode == "phone" { "own page" } else { "the Mac's page" },
                        "what this phone shows; --on phone reaches its own page",
                    ));
                }
                ("browser", fields)
            }
            _ => ("empty", vec![pane_field]),
        };
        let mut fields = fields;
        if let Some(site) = site {
            fields.insert(
                1.min(fields.len()),
                field("Place", site.name(), "grid, popped or backgrounded"),
            );
        }
        tracing::info!(pane = %pane, kind = kind_name, fields = fields.len(), "pane info");
        Ok((pane, kind_name.into(), fields, login))
    }
}
