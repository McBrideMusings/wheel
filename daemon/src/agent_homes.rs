//! What the person's login shell sets each agent's home variable to, per primary checkout. Asking
//! means starting `shell -il`, which sources every startup file and takes seconds on a busy Mac, so
//! one shell run answers every harness's variable at once, the answer is kept until a startup file
//! changes, two asks for one checkout share one run, and only [`AT_ONCE`] shells run together.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use crate::resume::Harness;

/// Login shells asked at the same time: a restore asks once per checkout, and starting them all at
/// once on a busy Mac makes every one of them slow.
const AT_ONCE: usize = 2;

/// Files whose change can change the answer: the startup files zsh and bash read, and a direnv
/// file in the checkout itself.
// hack: files the startup files source in turn are not watched, so a change there is seen only
// after wheeld restarts; upgrade when a person routes agent homes from a sourced file.
const STARTUP: [&str; 10] = [
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
    ".bash_profile",
    ".bash_login",
    ".profile",
    ".bashrc",
    ".config/direnv/direnvrc",
    ".direnvrc",
];
const SYSTEM: [&str; 6] = [
    "/etc/zshenv",
    "/etc/zprofile",
    "/etc/zshrc",
    "/etc/zlogin",
    "/etc/profile",
    "/etc/bashrc",
];

/// What every startup file looked like when an answer was given: its modification time, or none.
type Stamp = Vec<Option<SystemTime>>;

#[derive(Clone)]
struct Answer {
    stamp: Stamp,
    /// The ZDOTDIR the shell set, where zsh read its startup files from; empty when unset.
    zdotdir: String,
    /// Each harness's home variable and the value the shell gave it, empty when unset.
    values: HashMap<&'static str, String>,
}

/// The last run for a checkout: its answer, or how it failed and when that run finished, so the
/// asks that queued behind a failed run take its failure rather than each running the shell again.
enum Last {
    Answered(Answer),
    Failed(crate::login_path::Failed, Instant),
}

type Slot = Arc<tokio::sync::Mutex<Option<Last>>>;

pub(crate) struct AgentHomes {
    slots: std::sync::Mutex<HashMap<(String, PathBuf), Slot>>,
    shells: tokio::sync::Semaphore,
}

impl Default for AgentHomes {
    fn default() -> Self {
        AgentHomes {
            slots: Default::default(),
            shells: tokio::sync::Semaphore::new(AT_ONCE),
        }
    }
}

/// The startup files' state for a shell started in `dir`: those under the home directory, those
/// under `zdotdir` (the ZDOTDIR the shell set, read by zsh in place of the home directory's) when
/// set, the system ones, and the checkout's `.envrc`.
fn stamp(dir: &Path, zdotdir: &str) -> Stamp {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let modified = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let relocated = (!zdotdir.is_empty()).then(|| PathBuf::from(zdotdir));
    STARTUP
        .iter()
        .map(|f| home.join(f))
        .chain(relocated.iter().flat_map(|z| STARTUP.iter().map(move |f| z.join(f))))
        .chain(SYSTEM.iter().map(PathBuf::from))
        .chain([dir.join(".envrc")])
        .map(modified)
        .collect()
}

impl AgentHomes {
    /// The value `shell` started in `dir` gives `var`, from the kept answer while no startup file
    /// changed, else from one new shell run that answers every harness's variable. `fresh` reports
    /// whether a shell ran, for the log.
    pub(crate) async fn value(
        &self,
        shell: &str,
        dir: &Path,
        var: &'static str,
    ) -> Result<(String, bool), crate::login_path::Failed> {
        let slot = self
            .slots
            .lock()
            .unwrap()
            .entry((shell.to_string(), dir.to_path_buf()))
            .or_default()
            .clone();
        let asked_at = Instant::now();
        let mut held = slot.lock().await;
        match held.as_ref() {
            Some(Last::Answered(answer)) if answer.stamp == stamp(dir, &answer.zdotdir) => {
                return Ok((answer.values.get(var).cloned().unwrap_or_default(), false));
            }
            Some(Last::Failed(failed, finished)) if *finished > asked_at => return Err(failed.clone()),
            _ => {}
        }
        let _shell = self.shells.acquire().await.expect("the semaphore is never closed");
        let vars: Vec<&'static str> = Harness::ALL.iter().map(|h| h.home_var()).collect();
        let mut asked = vars.clone();
        asked.push("ZDOTDIR");
        let (program, at) = (shell.to_string(), dir.to_path_buf());
        let ran = tokio::task::spawn_blocking(move || {
            crate::login_path::probe_in(&program, &at, &asked, crate::login_path::LIMIT)
        })
        .await
        .map_err(|e| crate::login_path::Failed {
            why: format!("asking {shell} failed: {e}"),
            timed_out: false,
        })
        .and_then(|r| r);
        let mut values = match ran {
            Ok(values) => values,
            Err(failed) => {
                *held = Some(Last::Failed(failed.clone(), Instant::now()));
                return Err(failed);
            }
        };
        let zdotdir = values.pop().unwrap_or_default();
        let answer = Answer {
            stamp: stamp(dir, &zdotdir),
            zdotdir,
            values: vars.into_iter().zip(values).collect(),
        };
        let value = answer.values.get(var).cloned().unwrap_or_default();
        *held = Some(Last::Answered(answer));
        Ok((value, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[tokio::test]
    async fn one_shell_run_answers_every_ask_until_a_startup_file_changes() {
        let root = std::env::temp_dir().join(format!("wheel-agent-homes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let runs = root.join("runs");
        let shell = root.join("shell");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\necho x >> '{}'\nexport CLAUDE_CONFIG_DIR=/c CODEX_HOME=/x ZDOTDIR='{}'\neval \"$2\"\n",
                runs.display(),
                root.join("zd").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shell = shell.display().to_string();
        let homes = AgentHomes::default();
        let count = || std::fs::read_to_string(&runs).map_or(0, |s| s.lines().count());

        let asks = futures_util::future::join_all((0..4).map(|_| homes.value(&shell, &root, "CLAUDE_CONFIG_DIR"))).await;
        for ask in asks {
            assert_eq!(ask.unwrap().0, "/c");
        }
        assert_eq!(homes.value(&shell, &root, "CODEX_HOME").await.unwrap(), ("/x".into(), false));
        assert_eq!(homes.value(&shell, &root, "PI_CODING_AGENT_DIR").await.unwrap(), (String::new(), false));
        assert_eq!(count(), 1, "five asks for one checkout ran the shell more than once");

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join(".envrc"), "").unwrap();
        assert_eq!(homes.value(&shell, &root, "CLAUDE_CONFIG_DIR").await.unwrap(), ("/c".into(), true));
        assert_eq!(count(), 2, "a changed startup file did not ask the shell again");

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(root.join("zd")).unwrap();
        std::fs::write(root.join("zd").join(".zshrc"), "").unwrap();
        assert_eq!(homes.value(&shell, &root, "CLAUDE_CONFIG_DIR").await.unwrap(), ("/c".into(), true));
        assert_eq!(count(), 3, "a changed .zshrc under the shell's ZDOTDIR did not ask the shell again");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn asks_queued_behind_a_failed_run_take_its_failure() {
        let root = std::env::temp_dir().join(format!("wheel-agent-homes-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let runs = root.join("runs");
        let shell = root.join("shell");
        std::fs::write(&shell, format!("#!/bin/sh\necho x >> '{}'\nsleep 0.3\nexit 1\n", runs.display())).unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shell = shell.display().to_string();
        let homes = AgentHomes::default();

        let asks = futures_util::future::join_all((0..3).map(|_| homes.value(&shell, &root, "CLAUDE_CONFIG_DIR"))).await;
        assert!(asks.iter().all(Result::is_err));
        let count = std::fs::read_to_string(&runs).map_or(0, |s| s.lines().count());
        assert_eq!(count, 1, "asks queued behind a failed run each ran the shell");
        assert!(homes.value(&shell, &root, "CLAUDE_CONFIG_DIR").await.is_err());
        let count = std::fs::read_to_string(&runs).map_or(0, |s| s.lines().count());
        assert_eq!(count, 2, "an ask after the failure did not try the shell again");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
