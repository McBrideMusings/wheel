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

/// The remote the project's GitHub repo lives at, whatever it is called: the one whose URL points at the repo `gh`
/// names for `dir`, else the only remote there is. `None` with no remote, or several and none of them the repo.
pub fn project_remote(dir: &str) -> Option<String> {
    choose_remote(dir, crate::prs::repo_of(dir).ok().as_deref())
}

/// [`project_remote`] with the GitHub repo (`owner/name`) already known, when it is.
fn choose_remote(dir: &str, repo: Option<&str>) -> Option<String> {
    if let Some(r) = repo.and_then(|repo| remote_for(dir, repo).ok()) {
        return Some(r);
    }
    let names = quiet(dir, &["remote"]);
    let mut names = names.lines().map(str::trim).filter(|n| !n.is_empty());
    let only = names.next()?;
    names.next().is_none().then(|| only.to_string())
}

/// The ref the repo's default branch lives at: `remote`'s HEAD, else its main|master, else local.
pub fn default_ref(dir: &str, remote: Option<&str>) -> Option<String> {
    if let Some(remote) = remote {
        let r = quiet(dir, &["symbolic-ref", "-q", &format!("refs/remotes/{remote}/HEAD")]);
        if let Some(r) = r.trim().strip_prefix("refs/remotes/") {
            return Some(r.to_string());
        }
    }
    remote.into_iter().flat_map(|r| [format!("{r}/main"), format!("{r}/master")]).chain(["main".to_string(), "master".to_string()])
        .find(|n| !quiet(dir, &["rev-parse", "-q", "--verify", n]).trim().is_empty())
}

/// The name of the remote whose URL points at GitHub repo `repo` (`owner/name`), whatever it is called:
/// the URL's last two path parts, compared without case and without a trailing `.git`.
pub fn remote_for(dir: &str, repo: &str) -> Result<String, String> {
    let want = repo.to_lowercase();
    quiet(dir, &["config", "--get-regexp", r"^remote\..*\.url$"])
        .lines()
        .find_map(|line| {
            let (key, url) = line.split_once(' ')?;
            let url = url.trim().trim_end_matches('/').to_lowercase();
            let mut parts = url.trim_end_matches(".git").rsplit(['/', ':']);
            let (name, owner) = (parts.next()?, parts.next()?);
            (format!("{owner}/{name}") == want).then(|| key.strip_prefix("remote.")?.strip_suffix(".url").map(String::from)).flatten()
        })
        .ok_or_else(|| format!("no git remote in {dir} points at {repo}"))
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

    fn sh(dir: &std::path::Path, args: &[&str]) {
        let out = Command::new("/usr/bin/git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn the_projects_remote_and_its_default_branch_are_found_whatever_the_remote_is_called() {
        let root = std::env::temp_dir().join(format!("wheelhouse-git-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (bare, work) = (root.join("Owner").join("Repo.git"), root.join("work"));
        sh(&root, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        sh(&root, &["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()]);
        sh(&work, &["remote", "rename", "origin", "forevr"]);
        sh(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        sh(&work, &["push", "-q", "forevr", "main"]);
        sh(&work, &["fetch", "-q", "forevr"]);
        let dir = work.to_str().unwrap();
        assert_eq!(choose_remote(dir, None).as_deref(), Some("forevr"), "the only remote");
        assert_eq!(choose_remote(dir, Some("owner/repo")).as_deref(), Some("forevr"));
        assert_eq!(default_ref(dir, Some("forevr")).as_deref(), Some("forevr/main"), "no origin/main exists to find");
        sh(&work, &["remote", "add", "fork", "https://github.com/me/repo.git"]);
        assert_eq!(choose_remote(dir, None), None, "two remotes and no repo to tell them apart");
        assert_eq!(choose_remote(dir, Some("owner/repo")).as_deref(), Some("forevr"), "the one that points at the repo");
        assert_eq!(choose_remote(dir, Some("me/repo")).as_deref(), Some("fork"));
        assert_eq!(choose_remote(dir, Some("x/y")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_count_of_nothing_is_none_and_a_repo_has_a_toplevel() {
        assert_eq!(counts("/", "a", "b"), None);
        let here = env!("CARGO_MANIFEST_DIR");
        assert!(toplevel(here).unwrap().ends_with("wheel") || !toplevel(here).unwrap().is_empty());
        assert!(dirs(here).is_some());
    }
}
