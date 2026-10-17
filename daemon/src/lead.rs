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

/// The `lead/` folder this daemon ships with: `Resources/lead` in the app bundle that holds it
/// (`Contents/MacOS/wheeld` is the executable), else the repo's folder next to the source this
/// crate was built from. `wheeld --lead-dir` names another. With neither present it returns the
/// build-time path, which `LeadHome::link` then reports as missing in the lead pane.
pub fn default_dir() -> PathBuf {
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("lead");
    match std::env::current_exe() {
        Ok(exe) => resolve_dir(&exe, built_from),
        Err(_) => built_from,
    }
}

fn resolve_dir(exe: &Path, built_from: PathBuf) -> PathBuf {
    match exe.parent().map(|dir| dir.join("../Resources/lead")) {
        Some(bundled) if bundled.join("CLAUDE.md").is_file() => bundled,
        _ => built_from,
    }
}

/// Where the lead's configuration comes from and where it is linked to.
#[derive(Clone, Debug)]
pub struct LeadHome {
    /// `lead-home/` in the instance directory.
    dir: PathBuf,
    /// The `lead/` folder: the repo's, or the app bundle's copy.
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

    /// Link `lead-home/CLAUDE.md` and `lead-home/skills` to the `lead/` folder. Called at
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lead_folder(dir: &Path) {
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "brief").unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("wheel-lead-unit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_bundled_folder_wins_over_the_build_path() {
        let root = scratch("bundle");
        lead_folder(&root.join("Contents/Resources/lead"));
        let exe = root.join("Contents/MacOS/wheeld");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let found = resolve_dir(&exe, root.join("src/lead"));
        assert_eq!(found, root.join("Contents/MacOS/../Resources/lead"));
    }

    #[test]
    fn a_daemon_outside_a_bundle_uses_the_build_path() {
        let root = scratch("missing");
        let found = resolve_dir(&root.join("bin/wheeld"), root.join("src/lead"));
        assert_eq!(found, root.join("src/lead"));
    }

    #[test]
    fn link_points_at_the_folder_and_relinks_after_a_move() {
        let root = scratch("relink");
        lead_folder(&root.join("a"));
        lead_folder(&root.join("b"));
        LeadHome::new(&root.join("inst"), root.join("a"))
            .link()
            .unwrap();
        let home = LeadHome::new(&root.join("inst"), root.join("b"));
        home.link().unwrap();
        let link = root.join("inst/lead-home");
        assert_eq!(
            std::fs::read_link(link.join("CLAUDE.md")).unwrap(),
            root.join("b/CLAUDE.md")
        );
        assert_eq!(
            std::fs::read_link(link.join("skills")).unwrap(),
            root.join("b/skills")
        );
    }

    #[test]
    fn link_refuses_a_real_file_and_leaves_it() {
        let root = scratch("real");
        lead_folder(&root.join("a"));
        std::fs::create_dir_all(root.join("inst/lead-home")).unwrap();
        std::fs::write(root.join("inst/lead-home/CLAUDE.md"), "mine").unwrap();
        let err = LeadHome::new(&root.join("inst"), root.join("a"))
            .link()
            .unwrap_err();
        assert!(err.contains("is not a link to the lead folder"), "{err}");
        assert_eq!(
            std::fs::read_to_string(root.join("inst/lead-home/CLAUDE.md")).unwrap(),
            "mine"
        );
    }
}
