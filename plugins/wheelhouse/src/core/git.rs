//! Git reads shared by the Repo and Diff views. Every call here is read-only and runs in the
//! project's directory with `core.quotepath` off, so a path prints as it is written.

use std::process::Command;

/// `git <args>` in `dir`: standard output, or the first 300 characters of the error.
pub fn run(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("/usr/bin/git")
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).chars().take(300).collect());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git <args>` in `dir`, an empty string on failure.
pub fn quiet(dir: &str, args: &[&str]) -> String {
    run(dir, args).unwrap_or_default()
}

pub fn toplevel(dir: &str) -> Result<String, String> {
    run(dir, &["rev-parse", "--show-toplevel"]).map(|s| s.trim().to_string())
}

/// The ref the repo's default branch lives at: origin/HEAD, else origin/main|master, else local.
pub fn default_ref(dir: &str) -> Option<String> {
    let r = quiet(dir, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"]);
    if let Some(r) = r.trim().strip_prefix("refs/remotes/") {
        return Some(r.to_string());
    }
    ["origin/main", "origin/master", "main", "master"]
        .iter()
        .find(|n| !quiet(dir, &["rev-parse", "-q", "--verify", n]).trim().is_empty())
        .map(|n| n.to_string())
}

pub fn current_branch(dir: &str) -> Option<String> {
    let b = quiet(dir, &["symbolic-ref", "-q", "--short", "HEAD"]);
    (!b.trim().is_empty()).then(|| b.trim().to_string())
}

/// (commits only in `left`, commits only in `right`); `None` when git cannot say.
pub fn counts(dir: &str, left: &str, right: &str) -> Option<(i64, i64)> {
    let out = quiet(dir, &["rev-list", "--left-right", "--count", &format!("{left}...{right}")]);
    let mut it = out.split_whitespace().filter_map(|n| n.parse().ok());
    Some((it.next()?, it.next()?))
}

/// How many commits `range` holds.
pub fn count(dir: &str, range: &[&str]) -> Option<i64> {
    let mut args = vec!["rev-list", "--count"];
    args.extend(range);
    run(dir, &args).ok()?.trim().parse().ok()
}

/// Where git keeps this checkout's own files (HEAD, index, logs) and the files every worktree shares.
pub fn dirs(dir: &str) -> Option<(String, String)> {
    let own = run(dir, &["rev-parse", "--absolute-git-dir"]).ok()?.trim().to_string();
    let common = run(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok()?.trim().to_string();
    Some((own, common))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_of_nothing_is_none_and_a_repo_has_a_toplevel() {
        assert_eq!(counts("/", "a", "b"), None);
        let here = env!("CARGO_MANIFEST_DIR");
        assert!(toplevel(here).unwrap().ends_with("wheel") || !toplevel(here).unwrap().is_empty());
        assert!(dirs(here).is_some());
    }
}
