//! The lead's bead tools: wheeld runs `bd` in a space's directory and hands back what it printed.
//! `bd` is the `beads.bd` setting, else the first one on wheeld's `PATH`; a call that cannot run
//! it, or that it refuses, is an error the lead reads, never an empty list.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use tokio::process::Command;
use wheel_proto::{BdInfo, SettingOrigin};

use crate::server::AppState;
use crate::settings::BD;

/// How long `bd --version` gets to answer when status asks.
const VERSION_WAIT: Duration = Duration::from_secs(5);

const WHY: &str = "the lead's bead tools (list_beads, show_bead, create_bead, update_bead and the rest) run bd in a space's directory";
const INSTALL: &str = "install bd from https://github.com/steveyegge/beads, or set its absolute path with `wheel config set beads.bd <path>`";

/// The first executable file named `name` in the directories of `path_var`.
fn find_in(path_var: &OsStr, name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|p| {
            std::fs::metadata(p)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

/// What `<bd> --version` printed, by path and modification time, so status asking again does not
/// run it again but a reinstalled bd is asked afresh.
type VersionCache = Mutex<HashMap<(PathBuf, Option<SystemTime>), Result<String, String>>>;

fn versions() -> &'static VersionCache {
    static VERSIONS: OnceLock<VersionCache> = OnceLock::new();
    VERSIONS.get_or_init(Mutex::default)
}

fn version_of(bd: &Path) -> Result<String, String> {
    let modified = std::fs::metadata(bd).and_then(|m| m.modified()).ok();
    let cache_key = (bd.to_path_buf(), modified);
    if let Some(known) = versions().lock().unwrap().get(&cache_key) {
        return known.clone();
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let path = bd.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(
            std::process::Command::new(&path)
                .arg("--version")
                .stdin(Stdio::null())
                .output(),
        );
    });
    let found = match rx.recv_timeout(VERSION_WAIT) {
        Err(_) => Err(format!(
            "`{} --version` gave no answer within {}s",
            bd.display(),
            VERSION_WAIT.as_secs()
        )),
        Ok(Err(e)) => Err(format!("`{} --version` cannot run: {e}", bd.display())),
        Ok(Ok(out)) if !out.status.success() => Err(format!(
            "`{} --version` failed: {}",
            bd.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Ok(Ok(out)) => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
    };
    // Only an answer is remembered: a failure is asked again at the next status.
    if let Ok(version) = &found {
        versions()
            .lock()
            .unwrap()
            .insert(cache_key, Ok(version.clone()));
    }
    found
}

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

    /// The `bd` to run for `space` (the active space when None) and whether the `beads.bd` setting
    /// named it: the setting, else the first `bd` on wheeld's `PATH`. An error names the setting.
    pub(crate) fn resolve_bd(&self, space: Option<&str>) -> Result<(PathBuf, bool), String> {
        let (_, settings) = self.settings_ls(space)?;
        if let Some(setting) = settings.into_iter().find(|s| s.key == BD) {
            if setting.origin != SettingOrigin::Default {
                return Ok((PathBuf::from(setting.value), true));
            }
            if let Some(problem) = setting.problem {
                return Err(format!("the {BD} setting cannot be used: {problem}"));
            }
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        find_in(&path, "bd").map(|p| (p, false)).ok_or_else(|| {
            format!("bd is not on wheeld's PATH and the {BD} setting is not set; {INSTALL}")
        })
    }

    /// The `bd` the bead tools run for `space` (the active space when None), as `wheel status`
    /// reports it: found with its path and version, or missing and why.
    pub fn bd_info(&self, space: Option<&str>) -> BdInfo {
        let mut info = BdInfo {
            why: WHY.to_string(),
            install: INSTALL.to_string(),
            ..BdInfo::default()
        };
        match self.resolve_bd(space) {
            Err(problem) => info.problem = Some(problem),
            Ok((path, from_setting)) => {
                info.from_setting = from_setting;
                info.path = Some(path.display().to_string());
                // The wait for `--version` blocks its thread, so a multi-thread runtime is told to
                // move its other tasks off this one first.
                let asked = match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor())
                {
                    Ok(tokio::runtime::RuntimeFlavor::MultiThread) => {
                        tokio::task::block_in_place(|| version_of(&path))
                    }
                    _ => version_of(&path),
                };
                match asked {
                    Ok(version) => {
                        info.found = true;
                        info.version = Some(version);
                    }
                    Err(problem) => info.problem = Some(format!("{problem}; {INSTALL}")),
                }
            }
        }
        info
    }

    /// Run `bd <subcommand> --json <rest>` in `space`'s directory (`args` is the subcommand then
    /// the rest) and return its output. A non-zero exit is an error carrying what `bd` said.
    pub async fn bd(&self, space: &str, args: Vec<String>) -> Result<String, String> {
        let (subcommand, rest) = args.split_first().ok_or("no bd subcommand")?;
        let cwd = self.space_repo(space)?;
        let (bd, _) = self.resolve_bd(Some(space))?;
        let mut command = Command::new(&bd);
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
            Ok(Err(e)) => {
                return Err(format!(
                    "cannot run {}: {e}; check the {BD} setting",
                    bd.display()
                ))
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_in_returns_the_first_executable_and_skips_the_rest() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("wheel-find-bd-{}", std::process::id()));
        let (plain, exec, empty) = (root.join("a"), root.join("b"), root.join("c"));
        for d in [&plain, &exec, &empty] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(plain.join("bd"), "").unwrap();
        std::fs::write(exec.join("bd"), "").unwrap();
        std::fs::set_permissions(exec.join("bd"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&empty, &plain, &exec]).unwrap();
        assert_eq!(find_in(&path, "bd"), Some(exec.join("bd")));
        assert_eq!(
            find_in(&std::env::join_paths([&empty, &plain]).unwrap(), "bd"),
            None
        );
        assert_eq!(find_in(OsStr::new(""), "bd"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
