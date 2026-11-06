//! The person's own guidance for agents: two kinds of file beside the machine's `wheel.toml`,
//! `guidance.md` for every agent and `guidance/<agent>.md` for one. Wheel reads them when a pane
//! starts and carries their text after the brief as it is; it parses none of it and acts on
//! none of it (ADR-0017). The paths are fixed, so no setting names a file.

use std::path::PathBuf;

use wheel_proto::GuidanceFile;

use crate::server::AppState;
use crate::settings::agent_names;

impl AppState {
    /// The directory holding the guidance files: the one holding this machine's `wheel.toml`.
    fn guidance_dir(&self) -> PathBuf {
        self.machine_file
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default()
    }

    /// The file for every agent, or for `agent`, which must be one wheel has.
    fn guidance_file(&self, agent: Option<&str>) -> Result<PathBuf, String> {
        match agent {
            None => Ok(self.guidance_dir().join("guidance.md")),
            Some(agent) if agent_names().contains(&agent) => Ok(self
                .guidance_dir()
                .join("guidance")
                .join(format!("{agent}.md"))),
            Some(agent) => Err(format!(
                "no agent {agent}; the agents are {}",
                agent_names().join(", ")
            )),
        }
    }

    /// What a file holds; nothing when it is absent or cannot be read.
    pub(crate) fn guidance_text(&self, agent: Option<&str>) -> String {
        let Ok(path) = self.guidance_file(agent) else {
            return String::new();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "a guidance file cannot be read; it adds nothing");
                String::new()
            }
        }
    }

    /// What goes after the brief for `agent`: the file for every agent, then the agent's own.
    pub(crate) fn guidance_for(&self, agent: &str) -> String {
        crate::brief::guidance_section(
            &self.guidance_text(None),
            agent,
            &self.guidance_text(Some(agent)),
        )
    }

    /// The fixed brief, the file for every agent and a file for each agent wheel has, each with
    /// its path and current text.
    pub(crate) fn guidance_ls(
        &self,
        agent: Option<&str>,
    ) -> Result<(String, String, Vec<GuidanceFile>), String> {
        self.guidance_file(agent)?;
        let given = format!("{}{}", crate::brief::fixed(), self.guidance_for(agent.unwrap_or("")));
        let mut files = Vec::new();
        for agent in std::iter::once(None).chain(agent_names().into_iter().map(Some)) {
            let Ok(path) = self.guidance_file(agent) else {
                continue;
            };
            files.push(GuidanceFile {
                agent: agent.map(String::from),
                heading: crate::brief::guidance_heading(agent),
                path: path.display().to_string(),
                text: self.guidance_text(agent),
            });
        }
        Ok((crate::brief::fixed().to_string(), given, files))
    }

    /// The path of a guidance file, made empty when it is absent so a client can open it for
    /// editing. The server makes it because a client holds no files (ADR-0020).
    pub(crate) fn guidance_path(&self, agent: Option<&str>) -> Result<String, String> {
        let path = self.guidance_file(agent)?;
        let _one = self.settings_write.lock().unwrap();
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&path, "").map_err(|e| format!("{}: {e}", path.display()))?;
            tracing::info!(path = %path.display(), "guidance file created");
        }
        Ok(path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> (std::sync::Arc<AppState>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wheel-guidance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = AppState::with_machine_file(
            "/dev/null".into(),
            "t".into(),
            dir.join("inst"),
            dir.join("home/.wheel/wheel.toml"),
            "/bin/false".into(),
        );
        (state, dir.join("home/.wheel"))
    }

    #[test]
    fn no_files_add_nothing_and_every_agent_is_listed() {
        let (state, _) = state("none");
        assert_eq!(state.guidance_for("claude"), "");
        let (brief, given, files) = state.guidance_ls(None).unwrap();
        assert_eq!(brief, crate::brief::fixed());
        assert_eq!(given, brief);
        let agents: Vec<_> = files.iter().map(|f| f.agent.as_deref()).collect();
        assert_eq!(agents, [None, Some("claude"), Some("codex"), Some("pi")]);
        assert!(files.iter().all(|f| f.text.is_empty()));
        assert!(files[0].path.ends_with("home/.wheel/guidance.md"), "{}", files[0].path);
        assert!(files[2].path.ends_with("home/.wheel/guidance/codex.md"), "{}", files[2].path);
    }

    #[test]
    fn path_makes_the_file_empty_and_keeps_one_that_is_there() {
        let (state, dir) = state("path");
        let path = state.guidance_path(Some("pi")).unwrap();
        assert_eq!(PathBuf::from(&path), dir.join("guidance/pi.md"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        std::fs::write(&path, "keep me").unwrap();
        state.guidance_path(Some("pi")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
        assert_eq!(PathBuf::from(state.guidance_path(None).unwrap()), dir.join("guidance.md"));
    }

    #[test]
    fn a_name_that_is_not_an_agent_is_refused_before_anything_is_made() {
        let (state, dir) = state("refused");
        for name in ["../x", "nope", "", "claude/../../x"] {
            assert!(state.guidance_path(Some(name)).is_err(), "{name}");
        }
        assert!(!dir.join("guidance").exists() && !dir.join("x.md").exists());
    }

    #[test]
    fn an_agent_gets_the_all_agents_text_then_its_own_and_no_other_agents() {
        let (state, dir) = state("order");
        std::fs::create_dir_all(dir.join("guidance")).unwrap();
        std::fs::write(dir.join("guidance.md"), "EVERY '''quoted''' {x}\n").unwrap();
        std::fs::write(dir.join("guidance/codex.md"), "CODEX ONLY").unwrap();
        std::fs::write(dir.join("guidance/pi.md"), "  \n").unwrap();
        let codex = state.guidance_for("codex");
        assert!(codex.find("EVERY '''quoted''' {x}").unwrap() < codex.find("CODEX ONLY").unwrap(), "{codex}");
        let claude = state.guidance_for("claude");
        assert!(claude.contains("EVERY") && !claude.contains("CODEX ONLY"), "{claude}");
        assert!(!state.guidance_for("pi").contains("for pi:"));
    }

    #[test]
    fn a_terminal_pane_gets_a_brief_of_its_own_for_each_agent_with_guidance() {
        let (state, dir) = state("panebrief");
        std::fs::create_dir_all(dir.join("guidance")).unwrap();
        std::fs::write(dir.join("guidance.md"), "EVERY").unwrap();
        std::fs::write(dir.join("guidance/codex.md"), "CODEX").unwrap();
        let space = dir.join("space");
        std::fs::create_dir_all(&space).unwrap();
        state.pane_brief("p4", space.to_str().unwrap());
        let briefs = crate::shell::dir(&state.instance_dir).join("briefs");
        let read = |name: &str| std::fs::read_to_string(briefs.join(name));
        let pane = read("p4.md").unwrap();
        assert!(pane.contains("EVERY") && !pane.contains("CODEX"), "{pane}");
        let codex = read("p4.codex.md").unwrap();
        assert!(codex.find("EVERY").unwrap() < codex.find("CODEX").unwrap(), "{codex}");
        assert!(read("p4.claude.md").is_err() && read("p4.pi.md").is_err());
        std::fs::write(dir.join("guidance/codex.md"), "\n").unwrap();
        state.pane_brief("p4", space.to_str().unwrap());
        assert!(read("p4.codex.md").is_err(), "a blank file leaves no brief of its own");
    }

    #[test]
    fn a_file_that_is_not_text_adds_nothing() {
        let (state, dir) = state("binary");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("guidance.md"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(state.guidance_for("claude"), "");
    }
}
