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

/// The remote the project's work lives at, whatever it is called, read from local git alone (no `gh`, no network):
/// the first remote whose `refs/remotes/<r>/HEAD` exists, else the remote the local main (then master) tracks,
/// else the only remote, else one named `origin`. `None` with no remote, or several and none of these.
pub fn project_remote(dir: &str) -> Option<String> {
    let remotes: Vec<String> = quiet(dir, &["remote"]).lines().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
    let tracked = |b: &str| quiet(dir, &["config", "--get", &format!("branch.{b}.remote")]).trim().to_string();
    remotes.iter().find(|r| !quiet(dir, &["symbolic-ref", "-q", &format!("refs/remotes/{r}/HEAD")]).trim().is_empty())
        .or_else(|| ["main", "master"].iter().map(|b| tracked(b)).find_map(|t| remotes.iter().find(|r| **r == t)))
        .or_else(|| remotes.first().filter(|_| remotes.len() == 1))
        .or_else(|| remotes.iter().find(|r| *r == "origin"))
        .cloned()
}

/// The remote for GitHub repo `repo` when its name is known: the one whose URL points at it, else
/// [`project_remote`]'s pick.
pub fn remote_of_repo(dir: &str, repo: &str) -> Result<String, String> {
    remote_for(dir, repo).ok().or_else(|| project_remote(dir)).ok_or_else(|| format!("no git remote in {dir} points at {repo}"))
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
    fn the_projects_remote_is_picked_from_local_git_whatever_it_is_called() {
        let root = std::env::temp_dir().join(format!("wheelhouse-git-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (bare, work) = (root.join("Owner").join("Repo.git"), root.join("work"));
        sh(&root, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        sh(&root, &["init", "-q", work.to_str().unwrap()]);
        let dir = work.to_str().unwrap();
        assert_eq!(project_remote(dir), None, "no remote");
        sh(&work, &["remote", "add", "forevr", bare.to_str().unwrap()]);
        sh(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        sh(&work, &["push", "-q", "forevr", "main"]);
        sh(&work, &["fetch", "-q", "forevr"]);
        sh(&work, &["remote", "set-head", "forevr", "-d"]);
        assert_eq!(project_remote(dir).as_deref(), Some("forevr"), "the only remote");
        assert_eq!(default_ref(dir, Some("forevr")).as_deref(), Some("forevr/main"), "no origin/main exists to find");
        assert_eq!(remote_of_repo(dir, "owner/repo").as_deref(), Ok("forevr"));
        sh(&work, &["remote", "add", "zz-fork", "https://github.com/me/repo.git"]);
        assert_eq!(project_remote(dir), None, "two remotes, none named origin, no HEAD, nothing tracked");
        assert_eq!(remote_of_repo(dir, "owner/repo").as_deref(), Ok("forevr"), "the URL names the repo");
        assert_eq!(remote_of_repo(dir, "me/repo").as_deref(), Ok("zz-fork"));
        assert!(remote_of_repo(dir, "x/y").is_err());
        sh(&work, &["config", "branch.main.remote", "forevr"]);
        assert_eq!(project_remote(dir).as_deref(), Some("forevr"), "the remote main tracks");
        sh(&work, &["config", "--unset", "branch.main.remote"]);
        sh(&work, &["remote", "add", "origin", "https://github.com/me/other.git"]);
        assert_eq!(project_remote(dir).as_deref(), Some("origin"), "a remote named origin");
        sh(&work, &["remote", "set-head", "forevr", "main"]);
        assert_eq!(project_remote(dir).as_deref(), Some("forevr"), "the remote whose HEAD exists beats the name");
        assert_eq!(default_ref(dir, Some("forevr")).as_deref(), Some("forevr/main"));
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
