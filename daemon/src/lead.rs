//! The lead's own Claude configuration. The lead's agent runs with `CLAUDE_CONFIG_DIR` set to
//! `lead-home/` in the instance directory, so nothing from the person's `~/.claude` reaches it. The
//! repo's `lead/` folder supplies the brief (`CLAUDE.md`) and the skills, linked into that
//! directory at each start; the agent reads them as its user settings (ADR-0002).

use std::path::{Path, PathBuf};

use wheel_proto::{LeadConfigInfo, LeadPath};

/// The setting sources the lead's agent reads: only the user's, which `CLAUDE_CONFIG_DIR` points at
/// `lead-home/`. The project and local sources would read the directory the lead works in.
pub const SETTING_SOURCES: [&str; 1] = ["user"];

/// The built-in tools the lead starts with. It changes the app, the spaces and the beads only
/// through the daemon's MCP tools, which this list does not touch.
pub const TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// The repo's `lead/` folder, found from where this crate was built; `wheeld --lead-dir` names
/// another.
pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("lead")
}

/// Where the lead's configuration comes from and where it is linked to.
#[derive(Clone, Debug)]
pub struct LeadHome {
    /// `lead-home/` in the instance directory.
    dir: PathBuf,
    /// The repo's `lead/` folder.
    source: PathBuf,
}

impl LeadHome {
    pub fn new(instance_dir: &Path, source: PathBuf) -> LeadHome {
        LeadHome {
            dir: instance_dir.join("lead-home"),
            source,
        }
    }

    /// The directory the lead's agent takes as `CLAUDE_CONFIG_DIR`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn brief(&self) -> PathBuf {
        self.source.join("CLAUDE.md")
    }

    fn skills(&self) -> PathBuf {
        self.source.join("skills")
    }

    /// Link `lead-home/CLAUDE.md` and `lead-home/skills` to the repo's `lead/` folder. Called at
    /// every start of the lead's agent; the error names the path that is missing and shows in the
    /// lead pane.
    pub fn link(&self) -> Result<(), String> {
        if !self.brief().is_file() {
            return Err(format!(
                "the lead's brief {} is missing or is not a file",
                self.brief().display()
            ));
        }
        if !self.skills().is_dir() {
            return Err(format!(
                "the lead's skills folder {} is missing or is not a folder",
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
                        "{} is not a link to the lead folder; move it away",
                        link.display()
                    ));
                }
                std::fs::remove_file(&link)
                    .map_err(|e| format!("cannot replace {}: {e}", link.display()))?;
            }
            std::os::unix::fs::symlink(&target, &link).map_err(|e| {
                format!(
                    "cannot link {} to {}: {e}",
                    link.display(),
                    target.display()
                )
            })?;
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
