//! A page asking the daemon to run a command: `wheel.run({command, cwd, env})`.
//!
//! The command is a shell string the daemon runs through `/bin/sh -c` in `cwd`, detached: it has
//! no time limit and outlives the request, because what it starts (a worker in a terminal pane, an
//! agent) is meant to keep going. It may call `wheel` or anything else (Wheel's bin directory is
//! first on its `PATH`, as for an agent); the daemon does not look inside it.
//!
//! It is the same approval as a refresh command (ADR-0009, [`crate::refresh::ask_approval`]): the
//! first run of a command, in a directory, for an artifact folder, raises a permission request
//! showing the exact string, and "Always allow" remembers it, together with the hash of every
//! file it names. A page can therefore never run a string nobody has approved. What varies between
//! two runs of one approved command is only `env`, so a page sends the same string with different
//! variables rather than a new string.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::refresh::{ask_approval, reads, Approval, HashCache, Standing};
use crate::server::AppState;

/// The longest command a page may ask for, in bytes.
const COMMAND_MAX: usize = 8 * 1024;

/// The longest value of one variable a page may set, in bytes.
const ENV_VALUE_MAX: usize = 4 * 1024;

/// How many variables a page may set.
const ENV_MAX: usize = 32;

/// How long a request waits for the command to end before answering that it is still running.
/// A command that fails at once (a program not found, a syntax error) is reported with its output;
/// one that outlives this is a worker, and the answer is that it started.
const QUICK: Duration = Duration::from_millis(1500);

/// How much of the end of a quickly failed command's output the answer carries, in bytes.
const OUTPUT_TAIL: usize = 2000;

/// What a page sends: the command, the directory to run it in and the variables for this run.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageRun {
    command: String,
    cwd: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// Whether a page may set variable `name`: upper-case letters, digits and `_`, never one that
/// changes how the shell or the dynamic linker starts the program (`PATH`, `BASH_ENV`,
/// `DYLD_INSERT_LIBRARIES`…), or one Wheel sets itself (`WHEEL_*`). Without this an approved command
/// would run with whatever a page injected around it.
fn env_name_ok(name: &str) -> bool {
    const FORBIDDEN: &[&str] = &[
        "PATH",
        "HOME",
        "SHELL",
        "IFS",
        "ENV",
        "BASH_ENV",
        "PS4",
        "TMPDIR",
        "USER",
        "LOGNAME",
        "PWD",
        "OLDPWD",
        "SHELLOPTS",
        "BASHOPTS",
        "PROMPT_COMMAND",
    ];
    let mut chars = name.chars();
    let shaped = name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    shaped
        && !FORBIDDEN.contains(&name)
        && !["WHEEL_", "DYLD_", "LD_", "BASH_FUNC_"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The end of `bytes` as text, at most [`OUTPUT_TAIL`] bytes of it.
fn tail(bytes: &[u8]) -> String {
    let from = bytes.len().saturating_sub(OUTPUT_TAIL);
    String::from_utf8_lossy(&bytes[from..]).trim().to_string()
}

impl PageRun {
    /// Refuse what no run could use: an empty or oversized command, a directory that is not an
    /// absolute existing one, a variable a page may not set.
    fn check(&self) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err("the command is empty".into());
        }
        if self.command.len() > COMMAND_MAX {
            return Err(format!("the command is over {COMMAND_MAX} bytes"));
        }
        if !Path::new(&self.cwd).is_absolute() || !Path::new(&self.cwd).is_dir() {
            return Err(format!(
                "{} is not an existing absolute directory",
                self.cwd
            ));
        }
        if self.env.len() > ENV_MAX {
            return Err(format!("more than {ENV_MAX} variables"));
        }
        for (name, value) in &self.env {
            if !env_name_ok(name) {
                return Err(format!("a page may not set {name}"));
            }
            if value.len() > ENV_VALUE_MAX || value.contains('\0') {
                return Err(format!("the value of {name} is too long or holds a NUL"));
            }
        }
        Ok(())
    }
}

impl AppState {
    /// Run `req` for web pane `pane`, asking the person first unless the command is already
    /// trusted for this artifact folder and directory. Answers once the command has started: with
    /// its exit status and output when it ended within [`QUICK`], otherwise with its pid. An error
    /// when the pane shows no artifact, the request is unusable or the person refuses.
    pub(crate) async fn page_run(
        self: &Arc<Self>,
        pane: &str,
        req: PageRun,
    ) -> Result<Value, String> {
        req.check()?;
        self.page(pane)?;
        let (artifact, space_dir) = self
            .web_pane_artifact(pane)
            .ok_or_else(|| format!("{pane} is not a web pane"))?;
        let entry = self
            .artifact_entry(&artifact, &space_dir)?
            .ok_or_else(|| format!("no entry for {}", artifact.name()))?;
        let dir = entry.locate()?.root;
        let data = self.pane_data_dir(pane)?;
        let panes = data.parent().unwrap_or(&data).to_path_buf();

        let (command, cwd) = (req.command.clone(), req.cwd.clone());
        let named = {
            let (command, cwd, dir, panes) = (
                command.clone(),
                cwd.clone(),
                dir.display().to_string(),
                panes.clone(),
            );
            tokio::task::spawn_blocking(move || {
                reads(&command, &dir, &cwd, &panes, &HashCache::default())
            })
            .await
            .map_err(|e| format!("hashing the files the command names: {e}"))?
        };
        let standing: Standing =
            self.trust
                .lock()
                .unwrap()
                .standing(&dir.display().to_string(), &cwd, &command, &named);
        let title = format!(
            "A page of the {} artifact {} asks to run {command:?} in {cwd}",
            artifact.scope().name(),
            artifact.name()
        );
        let of = Approval {
            pane,
            scope: artifact.scope(),
            name: artifact.name(),
            dir: &dir,
            cwd: &cwd,
            command: &command,
        };
        if !ask_approval(&self.perms, &self.trust, &of, title, standing, named).await? {
            return Err("the person refused this command".into());
        }

        let log = data.join("runs");
        std::fs::create_dir_all(&log).map_err(|e| format!("make {}: {e}", log.display()))?;
        let log = log.join(format!("{}.log", unix_ms()));
        let out =
            std::fs::File::create(&log).map_err(|e| format!("make {}: {e}", log.display()))?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let mut shell = tokio::process::Command::new("/bin/sh");
        shell
            .arg("-c")
            .arg(&command)
            .current_dir(&cwd)
            // Wheel's bin first, as for an agent, so a command can call `wheel`.
            .env(
                "PATH",
                crate::shell::agent_path(&self.instance_dir, &self.path()),
            )
            .env("WHEEL_PANE", pane)
            .env("WHEEL_SPACE_DIR", &space_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            // Its own group and not killed with this request: a worker outlives the click.
            .process_group(0)
            .kill_on_drop(false);
        for (name, value) in &req.env {
            shell.env(name, value);
        }
        let mut child = shell.spawn().map_err(|e| format!("run {command:?}: {e}"))?;
        let pid = child.id();
        tracing::info!(%pane, name = artifact.name(), %command, %cwd, ?pid, vars = ?req.env.keys().collect::<Vec<_>>(), log = %log.display(), "page run started");
        let name = artifact.name().to_string();
        let ended = tokio::spawn(async move {
            let status = child.wait().await;
            tracing::info!(name, ?pid, ?status, "page run ended");
            status
        });
        match tokio::time::timeout(QUICK, ended).await {
            Ok(Ok(Ok(status))) => {
                let output = tail(&std::fs::read(&log).unwrap_or_default());
                Ok(
                    json!({"started": true, "pid": pid, "exit": status.code(), "output": output, "log": log}),
                )
            }
            _ => Ok(json!({"started": true, "pid": pid, "running": true, "log": log})),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page that could set `PATH`, `BASH_ENV` or `DYLD_INSERT_LIBRARIES` would run code of its own
    /// inside a command the person approved by its text alone.
    #[test]
    fn a_page_cannot_change_how_an_approved_command_starts() {
        for name in [
            "PATH",
            "BASH_ENV",
            "ENV",
            "IFS",
            "DYLD_INSERT_LIBRARIES",
            "LD_PRELOAD",
            "WHEEL_PANE",
            "BASH_FUNC_x%%",
            "lower",
            "1ABC",
            "",
        ] {
            assert!(!env_name_ok(name), "{name} must be refused");
        }
        for name in ["PR_NUMBER", "PR_BRANCH", "A1"] {
            assert!(env_name_ok(name), "{name} must be allowed");
        }
    }
}
