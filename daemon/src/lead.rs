//! The supervisors' and workers' own Claude configuration. Each runs with `CLAUDE_CONFIG_DIR` set
//! to its own directory in the instance directory (`supervisor-home/`, `worker-home/`), so nothing
//! from the person's `~/.claude` reaches it. The repo's `supervisor/` or `worker/` folder supplies
//! the brief (`CLAUDE.md`) and the skills, linked into that directory at each start; the agent
//! reads them as its user settings (ADR-0007). The lead and agent panes keep the person's
//! configuration and get Wheel's brief from `crate::brief` instead.

use std::path::{Path, PathBuf};

/// The setting sources a supervisor's and a worker's agent read: only the user's, which
/// `CLAUDE_CONFIG_DIR` points at its home. The project and local sources would read the directory
/// it works in.
pub const SETTING_SOURCES: [&str; 1] = ["user"];

/// The built-in tools a supervisor starts with: it changes nothing itself, only through its MCP
/// tools, which this list does not touch.
pub const TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// The `worker/` folder this daemon ships with: `Resources/worker` in the app bundle that holds it
/// (`Contents/MacOS/wheeld` is the executable), else the repo's folder next to the source this
/// crate was built from. `wheeld --worker-dir` names another. With neither present it returns the
/// build-time path, which `AgentHome::link` then reports as missing.
pub fn worker_default_dir() -> PathBuf {
    folder_dir("worker")
}

/// The `supervisor/` folder this daemon ships with, found the way the worker's is:
/// `Resources/supervisor` in the app bundle, else the repo's folder.
pub fn supervisor_default_dir() -> PathBuf {
    folder_dir("supervisor")
}

fn folder_dir(name: &str) -> PathBuf {
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    match std::env::current_exe() {
        Ok(exe) => resolve_dir(&exe, name, built_from),
        Err(_) => built_from,
    }
}

fn resolve_dir(exe: &Path, name: &str, built_from: PathBuf) -> PathBuf {
    match exe.parent().map(|dir| dir.join("../Resources").join(name)) {
        Some(bundled) if bundled.join("CLAUDE.md").is_file() => bundled,
        _ => built_from,
    }
}

/// Where an agent's configuration comes from and where it is linked to.
#[derive(Clone, Debug)]
pub struct AgentHome {
    /// Who the agent is, for error messages: `the supervisor` or `the worker`.
    role: &'static str,
    /// `supervisor-home/` or `worker-home/` in the instance directory.
    dir: PathBuf,
    /// The `supervisor/` or `worker/` folder: the repo's, or the app bundle's copy.
    source: PathBuf,
}

impl AgentHome {
    /// Every supervisor's: `supervisor-home/`, fed by the repo's `supervisor/` folder.
    pub fn supervisor(instance_dir: &Path, source: PathBuf) -> AgentHome {
        AgentHome {
            role: "the supervisor",
            dir: instance_dir.join("supervisor-home"),
            source,
        }
    }

    /// Every worker's: `worker-home/`, fed by the repo's `worker/` folder.
    pub fn worker(instance_dir: &Path, source: PathBuf) -> AgentHome {
        AgentHome {
            role: "the worker",
            dir: instance_dir.join("worker-home"),
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
                        "{} is not a link to {} folder; move it away",
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role_folder(dir: &Path) {
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
        role_folder(&root.join("Contents/Resources/worker"));
        let exe = root.join("Contents/MacOS/wheeld");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let found = resolve_dir(&exe, "worker", root.join("src/worker"));
        assert_eq!(found, root.join("Contents/MacOS/../Resources/worker"));
    }

    #[test]
    fn a_bundled_supervisor_folder_wins_over_the_build_path() {
        let root = scratch("bundle-supervisor");
        role_folder(&root.join("Contents/Resources/supervisor"));
        let exe = root.join("Contents/MacOS/wheeld");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let found = resolve_dir(&exe, "supervisor", root.join("src/supervisor"));
        assert_eq!(found, root.join("Contents/MacOS/../Resources/supervisor"));
    }

    #[test]
    fn a_daemon_outside_a_bundle_uses_the_build_path() {
        let root = scratch("missing");
        let found = resolve_dir(&root.join("bin/wheeld"), "worker", root.join("src/worker"));
        assert_eq!(found, root.join("src/worker"));
    }

    #[test]
    fn link_points_at_the_folder_and_relinks_after_a_move() {
        let root = scratch("relink");
        role_folder(&root.join("a"));
        role_folder(&root.join("b"));
        AgentHome::supervisor(&root.join("inst"), root.join("a"))
            .link()
            .unwrap();
        let home = AgentHome::supervisor(&root.join("inst"), root.join("b"));
        home.link().unwrap();
        let link = root.join("inst/supervisor-home");
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
        role_folder(&root.join("a"));
        std::fs::create_dir_all(root.join("inst/supervisor-home")).unwrap();
        std::fs::write(root.join("inst/supervisor-home/CLAUDE.md"), "mine").unwrap();
        let err = AgentHome::supervisor(&root.join("inst"), root.join("a"))
            .link()
            .unwrap_err();
        assert!(
            err.contains("is not a link to the supervisor folder"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("inst/supervisor-home/CLAUDE.md")).unwrap(),
            "mine"
        );
    }
}
