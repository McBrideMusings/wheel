//! The folders an agent could run with as its home (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
//! `PI_CODING_AGENT_DIR`): the ones sitting in the daemon user's home folder, and what a given
//! folder holds. Every client and the CLI read the same answer here.

use std::path::{Path, PathBuf};

use wheel_proto::{AgentHome, HomeState};

/// What one agent keeps in its home, and where its home is when nothing sets one.
struct Kind {
    /// The folder name every candidate starts with, in the user's home folder.
    prefix: &'static str,
    /// The agent's own default home, relative to the user's home folder.
    default: &'static str,
    /// Entries that only a folder this agent has run in holds: any one makes the folder valid.
    marks: &'static [&'static str],
}

fn kind(agent: &str) -> Option<Kind> {
    Some(match agent {
        "claude" => Kind {
            prefix: ".claude",
            default: ".claude",
            marks: &[
                "projects",
                "settings.json",
                ".credentials.json",
                ".claude.json",
            ],
        },
        "codex" => Kind {
            prefix: ".codex",
            default: ".codex",
            marks: &["auth.json", "config.toml", "sessions"],
        },
        "pi" => Kind {
            prefix: ".pi",
            default: ".pi/agent",
            marks: &["settings.json", "auth.json", "sessions"],
        },
        _ => return None,
    })
}

/// What `dir` holds for an agent whose marks are `marks`.
fn state_of(dir: &Path, marks: &[&str]) -> HomeState {
    match std::fs::metadata(dir) {
        Err(_) => HomeState::Missing,
        Ok(meta) if !meta.is_dir() => HomeState::NotFolder,
        Ok(_) if marks.iter().any(|m| dir.join(m).exists()) => HomeState::Valid,
        Ok(_) => HomeState::Empty,
    }
}

/// The candidate folders for `agent` under `home`, its default first when it is there, then the
/// rest by name, and `path`'s own reading (the default's when `path` is empty). `None` when
/// `agent` is not one wheel runs.
pub(crate) fn agent_homes(
    agent: &str,
    path: Option<&str>,
    home: &Path,
) -> Option<(Vec<AgentHome>, Option<AgentHome>)> {
    let kind = kind(agent)?;
    let default = home.join(kind.default);
    let mut found: Vec<PathBuf> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name == kind.prefix || name.starts_with(&format!("{}-", kind.prefix))
        })
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        // pi keeps its home in an `agent` folder inside each `~/.pi*`.
        .map(
            |p| match kind.default.contains('/') && p.join("agent").is_dir() {
                true => p.join("agent"),
                false => p,
            },
        )
        .collect();
    found.sort();
    if let Some(at) = found.iter().position(|p| *p == default) {
        let default = found.remove(at);
        found.insert(0, default);
    }
    let homes = found
        .into_iter()
        .map(|p| AgentHome {
            state: state_of(&p, kind.marks),
            path: p.to_string_lossy().into_owned(),
        })
        .collect();
    let checked = path.map(|p| {
        let p = p.trim();
        let dir = match p.is_empty() {
            true => default.clone(),
            false => PathBuf::from(crate::dirs::expand_home(p)),
        };
        AgentHome {
            state: match dir.is_absolute() {
                true => state_of(&dir, kind.marks),
                false => HomeState::Missing,
            },
            path: dir.to_string_lossy().into_owned(),
        }
    });
    Some((homes, checked))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-homes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_is_valid_empty_missing_or_a_file() {
        let home = scratch("claude");
        touch(&home.join(".claude/settings.json"));
        std::fs::create_dir_all(home.join(".claude-new")).unwrap();
        std::fs::create_dir_all(home.join(".claude-work/projects")).unwrap();
        touch(&home.join(".claude.json"));
        std::fs::create_dir_all(home.join("Documents")).unwrap();

        let (homes, _) = agent_homes("claude", None, &home).unwrap();
        let seen: Vec<(String, HomeState)> = homes
            .iter()
            .map(|h| {
                (
                    h.path
                        .trim_start_matches(home.to_str().unwrap())
                        .to_string(),
                    h.state,
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("/.claude".into(), HomeState::Valid),
                ("/.claude-new".into(), HomeState::Empty),
                ("/.claude-work".into(), HomeState::Valid),
            ]
        );

        let check = |p: &str| {
            agent_homes("claude", Some(p), &home)
                .unwrap()
                .1
                .unwrap()
                .state
        };
        assert_eq!(
            check(""),
            HomeState::Valid,
            "an empty path is the default folder"
        );
        assert_eq!(
            check(&format!("{}/.claude.json", home.display())),
            HomeState::NotFolder
        );
        assert_eq!(
            check(&format!("{}/nope", home.display())),
            HomeState::Missing
        );
        assert_eq!(check("relative"), HomeState::Missing);
        assert!(agent_homes("gemini", None, &home).is_none());
    }

    #[test]
    fn pi_homes_are_the_agent_folder_inside_each_pi_folder() {
        let home = scratch("pi");
        touch(&home.join(".pi/agent/auth.json"));
        std::fs::create_dir_all(home.join(".pi-work/agent")).unwrap();
        let (homes, _) = agent_homes("pi", None, &home).unwrap();
        assert_eq!(homes.len(), 2);
        assert!(homes[0].path.ends_with("/.pi/agent"));
        assert_eq!(homes[0].state, HomeState::Valid);
        assert_eq!(homes[1].state, HomeState::Empty);
    }
}
