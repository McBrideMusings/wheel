//! The lead's and the supervisors' own Claude configuration. Each runs with `CLAUDE_CONFIG_DIR` set
//! to its own directory in the instance directory (`lead-home/`, `supervisor-home/`), so nothing
//! from the person's `~/.claude` reaches it. The repo's `lead/` or `supervisor/` folder supplies
//! the brief (`CLAUDE.md`) and the skills, linked into that directory at each start; the agent
//! reads them as its user settings (ADR-0002, ADR-0007).

use std::path::{Path, PathBuf};

use wheel_proto::{LeadConfigInfo, LeadPath};

/// The setting sources the lead's and a supervisor's agent read: only the user's, which `CLAUDE_CONFIG_DIR` points at
/// `lead-home/`. The project and local sources would read the directory the lead works in.
pub const SETTING_SOURCES: [&str; 1] = ["user"];

/// The built-in tools the lead and a supervisor start with. The lead changes the app, the spaces
/// and the beads only through the daemon's MCP tools, which this list does not touch; a supervisor
/// changes nothing itself.
pub const TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// The repo's `lead/` folder, found from where this crate was built; `wheeld --lead-dir` names
/// another.
pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("lead")
}

/// Where an agent's configuration comes from and where it is linked to.
#[derive(Clone, Debug)]
pub struct AgentHome {
    /// Who the agent is, for error messages: `the lead` or `the supervisor`.
    role: &'static str,
    /// `lead-home/` or `supervisor-home/` in the instance directory.
    dir: PathBuf,
    /// The repo's `lead/` or `supervisor/` folder.
    source: PathBuf,
}

impl AgentHome {
    /// The lead's: `lead-home/`, fed by the repo's `lead/` folder.
    pub fn lead(instance_dir: &Path, source: PathBuf) -> AgentHome {
        AgentHome {
            role: "the lead",
            dir: instance_dir.join("lead-home"),
            source,
        }
    }

    /// Every supervisor's: `supervisor-home/`, fed by the repo's `supervisor/` folder.
    pub fn supervisor(instance_dir: &Path, source: PathBuf) -> AgentHome {
        AgentHome {
            role: "the supervisor",
            dir: instance_dir.join("supervisor-home"),
            source,
        }
    }

    /// The directory the agent takes as `CLAUDE_CONFIG_DIR`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn brief(&self) -> PathBuf {
        self.source.join("CLAUDE.md")
    }

    fn skills(&self) -> PathBuf {
        self.source.join("skills")
    }

    /// Link `CLAUDE.md` and `skills` in the config directory to the repo's folder for this role.
    /// Called at every start of the agent; the error names the path that is missing and shows in
    /// the agent's pane.
    pub fn link(&self) -> Result<(), String> {
        if !self.brief().is_file() {
            return Err(format!(
                "{}'s brief {} is missing or is not a file",
                self.role,
                self.brief().display()
            ));
        }
        if !self.skills().is_dir() {
            return Err(format!(
                "{}'s skills folder {} is missing or is not a folder",
                self.role,
                self.skills().display()
            ));
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        for (name, target) in [("CLAUDE.md", self.brief()), ("skills", self.skills())] {
            let link = self.dir.join(name);
            // Only a link this function made is replaced; a real file is the person's to resolve.
            if link.symlink_metadata().is_ok() {
                if !link.is_symlink() {
                    return Err(format!(
                        "{} is not a link to {}'s folder; move it away",
                        link.display(),
                        self.role
                    ));
                }
                // Several supervisors start at once from this one directory: a link that is
                // already right stays, so none of them removes it under another.
                if std::fs::read_link(&link).is_ok_and(|now| now == target) {
                    continue;
                }
                std::fs::remove_file(&link)
                    .map_err(|e| format!("cannot replace {}: {e}", link.display()))?;
            }
            if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
                // Another start made the same link between the check and here.
                if std::fs::read_link(&link).is_ok_and(|now| now == target) {
                    continue;
                }
                return Err(format!(
                    "cannot link {} to {}: {e}",
                    link.display(),
                    target.display()
                ));
            }
        }
        Ok(())
    }

    /// What `wheel lead status` reports.
    pub fn info(&self) -> LeadConfigInfo {
        let path = |p: PathBuf| LeadPath {
            exists: p.exists(),
            path: p.display().to_string(),
        };
        LeadConfigInfo {
            config_dir: path(self.dir.clone()),
            setting_sources: SETTING_SOURCES.map(String::from).to_vec(),
            tools: TOOLS.map(String::from).to_vec(),
            brief: path(self.brief()),
            skills: path(self.skills()),
        }
    }
}
