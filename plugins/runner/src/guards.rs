//! Guards: the preconditions a command's `requires` names. They run in order, in the directory the
//! command will run in, and the first that fails stops the command before it starts.

use std::path::Path;
use std::process::Command;

use crate::config;

/// How long a user guard or the fetch of `git-pushed` may take.
const LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// A `requires` that is not a list of names: refused, so a typo never drops a precondition.
pub const INVALID: &str = "\u{0}invalid-requires";

/// Run every guard in `names`. `Err` carries the first failure's message.
pub fn check(names: &[String], cwd: &Path, machine: &Path, project: &str) -> Result<(), String> {
    let user = config::guards(machine, project);
    for name in names {
        match name.as_str() {
            "git-clean" => git_clean(cwd)?,
            "git-pushed" => git_pushed(cwd)?,
            INVALID => return Err("requires must be a list of guard names".into()),
            other => {
                let guard = user.get(other).ok_or_else(|| format!("unknown guard {other}"))?;
                match crate::check::bounded(&guard.run, cwd, LIMIT) {
                    Ok(b) if b.code == Some(0) => {}
                    Ok(b) if b.timed_out => return Err(format!("{other}: timed out after {}s ({})", LIMIT.as_secs(), guard.fail)),
                    _ => return Err(format!("{other}: {}", guard.fail)),
                }
            }
        }
    }
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).env("GIT_TERMINAL_PROMPT", "0").stdin(std::process::Stdio::null()).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn in_repo(name: &str, cwd: &Path) -> Result<(), String> {
    match git(cwd, &["rev-parse", "--is-inside-work-tree"]).as_deref() {
        Ok("true") => Ok(()),
        _ => Err(format!("{name}: {} is not a git repository", cwd.display())),
    }
}

/// Fails on uncommitted or untracked changes.
fn git_clean(cwd: &Path) -> Result<(), String> {
    in_repo("git-clean", cwd)?;
    let status = git(cwd, &["status", "--porcelain"]).map_err(|e| format!("git-clean: could not read git status: {e}"))?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(format!("git-clean: the working tree has uncommitted or untracked changes ({} paths); commit or stash them (git status)", status.lines().count()))
    }
}

/// Fails when HEAD has commits its configured upstream lacks. The upstream's remote is fetched
/// first, so a stale remote-tracking ref cannot read as pushed.
fn git_pushed(cwd: &Path) -> Result<(), String> {
    in_repo("git-pushed", cwd)?;
    let branch = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let upstream = git(cwd, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])
        .map_err(|_| format!("git-pushed: branch {branch} has no upstream; push it with -u"))?;
    if let Ok(remote) = git(cwd, &["config", &format!("branch.{branch}.remote")]) {
        git(cwd, &["fetch", "--quiet", &remote]).map_err(|e| format!("git-pushed: could not fetch {remote}: {e}"))?;
    }
    let ahead = git(cwd, &["rev-list", "--count", &format!("{upstream}..HEAD")])?;
    if ahead == "0" {
        Ok(())
    } else {
        Err(format!("git-pushed: {ahead} commit(s) on {branch} are not on {upstream}; push them"))
    }
}
