//! The lead's bead tools: wheeld runs `bd` in a space's directory and hands back what it printed.
//! `bd` is found on `PATH`; a call that cannot run it, or that it refuses, is an error the lead
//! reads, never an empty list.

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::server::AppState;

/// The longest one `bd` call may run.
const BD_TIMEOUT: Duration = Duration::from_secs(60);

/// The most `bd` output a tool returns; a longer answer is an error, since cut JSON is unreadable.
const MAX_BD_BYTES: usize = 256 * 1024;

impl AppState {
    /// The directory of space `space`: the repo its beads live in.
    pub(crate) fn space_repo(&self, space: &str) -> Result<String, String> {
        self.spaces()
            .spaces
            .iter()
            .find(|s| s.id == space)
            .map(|s| s.cwd.clone())
            .ok_or_else(|| format!("no space {space}"))
    }

    /// Run `bd <subcommand> --json <rest>` in `space`'s directory (`args` is the subcommand then
    /// the rest) and return its output. A non-zero exit is an error carrying what `bd` said.
    pub async fn bd(&self, space: &str, args: Vec<String>) -> Result<String, String> {
        let (subcommand, rest) = args.split_first().ok_or("no bd subcommand")?;
        let cwd = self.space_repo(space)?;
        let mut command = Command::new("bd");
        command
            .arg(subcommand)
            .arg("--json")
            .args(rest)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let shown = format!("bd {}", args.join(" "));
        let output = match tokio::time::timeout(BD_TIMEOUT, command.output()).await {
            Err(_) => {
                return Err(format!(
                    "`{shown}` ran longer than {}s and was stopped",
                    BD_TIMEOUT.as_secs()
                ))
            }
            Ok(Err(e)) => return Err(format!("cannot run bd: {e}; is it on wheeld's PATH?")),
            Ok(Ok(output)) => output,
        };
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        tracing::info!(
            %space, %cwd, command = %shown, status = ?output.status.code(),
            stdout_bytes = stdout.len(), stderr_bytes = stderr.len(), "bd ran"
        );
        if !output.status.success() {
            let said = [stderr, stdout]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let status = match output.status.code() {
                Some(code) => format!("exit {code}"),
                None => "killed by a signal".to_string(),
            };
            return Err(format!("`{shown}` failed ({status}): {said}"));
        }
        if stdout.len() > MAX_BD_BYTES {
            return Err(format!(
                "`{shown}` printed {} bytes, over the {MAX_BD_BYTES} a tool returns; narrow the filters or set a limit",
                stdout.len()
            ));
        }
        Ok(stdout)
    }
}

/// A bead id as `bd` takes it positionally: never empty, never a flag.
pub(crate) fn bead_id(id: &str) -> Result<String, String> {
    if id.is_empty() || id.starts_with('-') || id.contains(char::is_whitespace) {
        return Err(format!("{id:?} is not a bead id"));
    }
    Ok(id.to_string())
}

/// Push `--name=value` onto `args` for each flag that has a value.
pub(crate) fn push_flags(
    args: &mut Vec<String>,
    flags: impl IntoIterator<Item = (&'static str, Option<String>)>,
) {
    for (name, value) in flags {
        if let Some(value) = value {
            args.push(format!("--{name}={value}"));
        }
    }
}
