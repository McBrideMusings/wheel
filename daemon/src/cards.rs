//! Cards in an agent's transcript (`agent_card`): an agent names a pane or an artifact, and a
//! client draws it live where the card sits in the conversation. wheeld checks the name when the
//! card is posted and writes what a client needs to open it; the work it shows stays in its own
//! pane or page.
//!
//! A plugin pane's embedded artifacts (`plugin_embed`) are pages of the same kind, kept per plugin
//! pane instead of per agent.

use wheel_proto::{AgentEvent, ArtifactRef, ArtifactScope, CardRef};

use crate::server::AppState;

/// How many artifact cards of one agent keep their page; an older card's page ends.
pub(crate) const CARDS_KEPT: usize = 8;

/// An artifact card's page: what it shows, the directory of the space it was named in, and the
/// agent whose transcript holds the card.
pub(crate) struct CardPage {
    pub artifact: ArtifactRef,
    pub cwd: String,
    pub agent: String,
}

impl AppState {
    /// Put a card for `pane` or `artifact` (exactly one) in the transcript of the agent `target`
    /// names. An artifact is looked for in the project of the agent's space, then the machine's;
    /// an agent with no space (the daemon's orchestrator) can name only a machine artifact.
    pub(crate) fn agent_card(
        &self,
        target: &str,
        pane: Option<String>,
        artifact: Option<String>,
    ) -> Result<(String, CardRef), String> {
        let found = self.resolve_agent(target)?;
        let card = match (pane, artifact) {
            (Some(pane), None) => {
                self.spaces.lock().unwrap().holding(&pane)?;
                CardRef::Pane { pane }
            }
            (None, Some(name)) => {
                let space = match &found.pane {
                    Some(pane) => Some(self.space_holding(pane)?),
                    None => self.orchestrator_space(&found.agent.name),
                };
                let (artifact, cwd, space) = match space {
                    Some(space) => {
                        let cwd = self.artifact_space(Some(&space))?;
                        let artifact = self.artifact_ref(None, &name, &cwd)?;
                        let space = (artifact.scope() == ArtifactScope::Project).then_some(space);
                        (artifact, cwd, space)
                    }
                    None => {
                        let artifact = ArtifactRef::new(ArtifactScope::Machine, &name)?;
                        if self.artifact_entry(&artifact, "/")?.is_none() {
                            return Err(format!(
                                "no machine artifact {name}; {} has no space, so it names only machine artifacts",
                                found.agent.name
                            ));
                        }
                        (artifact, "/".to_string(), None)
                    }
                };
                let id = self.new_id('k');
                let scope = artifact.scope();
                self.card_pages.lock().unwrap().insert(id.clone(), CardPage { artifact: artifact.clone(), cwd: cwd.clone(), agent: found.agent.name.clone() });
                // Watched and refreshed as a web pane's page is, so the card draws the page as it is now.
                self.show_artifact(&id, &artifact, &cwd);
                self.drop_card_pages(&found.agent.name, CARDS_KEPT);
                CardRef::Artifact { id, name, scope, space }
            }
            _ => return Err("a card names exactly one of a pane and an artifact".into()),
        };
        found.agent.post(AgentEvent::Card { card: card.clone() });
        Ok((found.agent.name.clone(), card))
    }

    /// End the pages of `agent`'s artifact cards past the newest `keep`: their watches, refresh,
    /// page store and data directory go as a closed web pane's do, and a client then finds the page
    /// gone. 0 ends them all, for an agent that ended.
    pub(crate) fn drop_card_pages(&self, agent: &str, keep: usize) {
        let dropped: Vec<String> = {
            let mut pages = self.card_pages.lock().unwrap();
            let mut ids: Vec<(u64, String)> = pages
                .iter()
                .filter(|(_, p)| p.agent == agent)
                .map(|(id, _)| (id[1..].parse().unwrap_or(0), id.clone()))
                .collect();
            ids.sort();
            let over = ids.len().saturating_sub(keep);
            ids.into_iter().take(over).map(|(_, id)| id).inspect(|id| { pages.remove(id); }).collect()
        };
        for id in dropped {
            tracing::info!(%agent, page = %id, "card page dropped");
            self.drop_artifact(&id);
        }
    }

    /// What the page with id `id` shows and the directory of its space: a web pane's artifact, or
    /// an artifact card's. None for any other id.
    pub(crate) fn page_artifact(&self, id: &str) -> Option<(ArtifactRef, String)> {
        let pane = self.spaces.lock().unwrap().spaces.iter().find_map(|s| match crate::spaces::space_find(s, id) {
            Some(wheel_proto::PaneKind::Web { artifact, .. }) => Some((artifact.clone(), s.cwd.clone())),
            _ => None,
        });
        pane.or_else(|| self.card_pages.lock().unwrap().get(id).map(|p| (p.artifact.clone(), p.cwd.clone())))
            .or_else(|| self.embed_pages.lock().unwrap().get(id).map(|p| (p.artifact.clone(), p.cwd.clone())))
    }

    /// The page plugin pane `pane` shows artifact `name` in (`plugin_embed`, ADR-0046): the name
    /// resolved in the pane's space, project then machine, and a page for it kept and served as an
    /// artifact card's is. The same pane and artifact get the page they already have, which counts
    /// as its newest use; past `EMBEDS_KEPT` the least recently asked for ends.
    pub(crate) fn plugin_embed(&self, pane: &str, name: &str) -> Result<(ArtifactRef, String), String> {
        let cwd = {
            let spaces = self.spaces.lock().unwrap();
            let (space, kind) = spaces
                .spaces
                .iter()
                .find_map(|s| crate::spaces::space_find(s, pane).map(|k| (s, k)))
                .ok_or_else(|| format!("no pane {pane}"))?;
            if crate::plugins::shown_plugin(kind).is_none() {
                return Err(format!("{pane} shows no plugin, so it embeds no artifact"));
            }
            space.cwd.clone()
        };
        let artifact = self.artifact_ref(None, name, &cwd)?;
        let seq = self.embed_seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (id, fresh) = {
            let mut pages = self.embed_pages.lock().unwrap();
            match pages.iter_mut().find(|(_, p)| p.pane == pane && p.artifact == artifact) {
                Some((id, page)) => {
                    page.used = seq;
                    (id.clone(), false)
                }
                None => {
                    let id = self.new_id('e');
                    pages.insert(id.clone(), EmbedPage { artifact: artifact.clone(), cwd: cwd.clone(), pane: pane.to_string(), used: seq });
                    (id, true)
                }
            }
        };
        if fresh {
            tracing::info!(%pane, page = %id, scope = artifact.scope().name(), name = artifact.name(), "embed page opened");
            self.show_artifact(&id, &artifact, &cwd);
            self.drop_embed_pages(pane, EMBEDS_KEPT);
        }
        Ok((artifact, id))
    }

    /// End the pages plugin pane `pane` embeds past the `keep` it used last, as a closed web
    /// pane's page ends. 0 ends them all, for a pane that closed or shows something else now.
    pub(crate) fn drop_embed_pages(&self, pane: &str, keep: usize) {
        let dropped: Vec<String> = {
            let mut pages = self.embed_pages.lock().unwrap();
            let mut ids: Vec<(u64, String)> = pages.iter().filter(|(_, p)| p.pane == pane).map(|(id, p)| (p.used, id.clone())).collect();
            ids.sort();
            let over = ids.len().saturating_sub(keep);
            ids.into_iter().take(over).map(|(_, id)| id).inspect(|id| { pages.remove(id); }).collect()
        };
        for id in dropped {
            tracing::info!(%pane, page = %id, "embed page dropped");
            self.drop_artifact(&id);
        }
    }
}

/// How many pages one plugin pane keeps embedded; each holds two folder watches.
pub(crate) const EMBEDS_KEPT: usize = 16;

/// A page a plugin pane embeds: the artifact, its space's directory, the plugin pane, and when the
/// pane last asked for it.
pub(crate) struct EmbedPage {
    pub artifact: ArtifactRef,
    pub cwd: String,
    pub pane: String,
    pub used: u64,
}
