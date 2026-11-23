//! The Repo pane's data for `wheelhouse backend`: one project's branches, stashes and recent
//! commits as the `repo` feed's value, and the `branch_delete`, `branch_cleanup` and `run_list` actions. The backend
//! sends data only; the pane (Luau) decides what to draw.

use serde_json::{json, Value};

use super::{cleanup_plan, collect, Cleanup};

/// The `repo` feed's value for the project in `dir`.
pub fn value(dir: &str) -> Value {
    let snap = collect(dir);
    // Without every open PR the plan cannot know what to keep, so it offers nothing and says why.
    let cleanup = match &snap.open_prs_problem {
        Some(why) => json!({ "delete": [], "keep": [], "off": why }),
        None => {
            let (go, keep) = cleanup_plan(&snap.branches);
            json!({
                "delete": go.iter().map(|b| &b.branch).collect::<Vec<_>>(),
                "keep": keep.iter().map(|(b, why)| json!({ "branch": b.branch, "why": why })).collect::<Vec<_>>(),
            })
        }
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    json!({
        "now": now,
        "name": snap.name,
        "top": snap.top,
        "default": snap.default,
        "remote": snap.remote,
        "head": snap.head,
        "branches": snap.branches,
        "stashes": snap.stashes.iter().map(|(r, subject, when)| json!({ "ref": r, "subject": subject, "when": when })).collect::<Vec<_>>(),
        "commits": snap.commits,
        "fetched": snap.fetched,
        "cleanup": cleanup,
        "pr_error": snap.pr_error,
        "error": snap.error,
        "buttons": snap.buttons.iter().map(|b| json!({ "name": b.name, "label": crate::prs::detail::label(b), "worktree": b.worktree })).collect::<Vec<_>>(),
        "scripts": snap.scripts.iter().map(|s| json!({ "name": s.name, "label": s.label() })).collect::<Vec<_>>(),
    })
}

/// Delete branch `branch` of the project in `dir` (and its worktree first); the words for the pane.
pub fn delete(dir: &str, branch: &str) -> Result<String, String> {
    super::delete(dir, branch).map(|did| did.join("; ")).map_err(|e| format!("{branch}: {e}"))
}

/// Clean up the project in `dir`: delete every finished branch nothing keeps. The words name what
/// was deleted, then what was kept and why; any branch that failed makes the whole an error.
pub fn clean_up(dir: &str) -> Result<String, String> {
    let done = super::clean_up(dir)?;
    for b in &done.deleted {
        eprintln!("wheelhouse backend: branch_cleanup {dir}: deleted {b}");
    }
    for (b, why) in done.kept.iter().chain(&done.failed) {
        eprintln!("wheelhouse backend: branch_cleanup {dir}: kept {b}: {why}");
    }
    let words = cleanup_words(&done);
    if done.failed.is_empty() { Ok(words) } else { Err(words) }
}

fn cleanup_words(c: &Cleanup) -> String {
    let named = |v: &[(String, String)]| v.iter().map(|(b, why)| format!("{b} ({why})")).collect::<Vec<_>>().join(", ");
    let mut parts = vec![if c.deleted.is_empty() {
        "Deleted nothing.".to_string()
    } else {
        format!("Deleted {}: {}.", c.deleted.len(), c.deleted.join(", "))
    }];
    if !c.failed.is_empty() {
        parts.push(format!("Failed {}: {}.", c.failed.len(), named(&c.failed)));
    }
    if !c.kept.is_empty() {
        parts.push(format!("Kept {}: {}.", c.kept.len(), named(&c.kept)));
    }
    parts.join(" ")
}

/// Run the project's `[repo.buttons.<name>]` button for local branch `branch`, in a worktree on it first when the
/// button asks for one. The branch's open PR, when it has one, is the button's `PR_NUMBER`, `PR_TITLE` and `PR_URL`,
/// and a PR from a fork is fetched from its head ref for the worktree. Reads the branch and its PR only, not the page.
pub fn button(dir: &str, pane: &str, branch: &str, name: &str, provider: Option<&str>) -> Result<String, String> {
    let (files, _) = crate::core::buttons::files(dir);
    let buttons = crate::prs::detail::buttons_in(&files, "repo");
    let b = buttons.iter().find(|b| b.name == name).ok_or_else(|| format!("no button {name}"))?;
    let top = crate::core::git::toplevel(dir)?;
    if crate::core::git::quiet(&top, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}")]).trim().is_empty() {
        return Err(format!("no such local branch {branch}"));
    }
    let pr = open_pr(&top, branch);
    let target = crate::prs::detail::Target {
        number: pr.as_ref().map(|p| p.0), title: pr.as_ref().map(|p| p.1.clone()).unwrap_or_default(), branch: branch.to_string(),
        url: pr.as_ref().map(|p| p.2.clone()).unwrap_or_default(),
    };
    let fork_pr = pr.as_ref().filter(|p| p.3).map(|p| p.0);
    let repo = crate::prs::repo_of(dir).unwrap_or_default();
    let origin = crate::core::project::Origin { dir: dir.to_string(), pane: if pane.is_empty() { "default".into() } else { pane.into() } };
    crate::prs::detail::run_button(&origin, "repo", &repo, &target, fork_pr, b, provider)?;
    Ok(format!("{} started for {branch}.", crate::prs::detail::label(b)))
}

/// The open PR from `branch`: (number, title, url, from a fork), or none when it has none or `gh` cannot say.
fn open_pr(top: &str, branch: &str) -> Option<(i64, String, String, bool)> {
    let out = crate::core::run::gh(top, &["pr", "list", "--head", branch, "--state", "open", "--limit", "1", "--json", "number,title,url,isCrossRepository"]).ok()?;
    let rows: Vec<Value> = serde_json::from_str(&out).ok()?;
    let p = rows.first()?;
    Some((p["number"].as_i64()?, p["title"].as_str()?.to_string(), p["url"].as_str()?.to_string(), p["isCrossRepository"].as_bool().unwrap_or(false)))
}

/// Run the project's `[repo.list]` button `script` over the branches `ids` for the pane `pane`.
pub fn run_list(dir: &str, script: &str, ids: &[String], pane: &str, provider: Option<&str>) -> Result<String, String> {
    let (scripts, _) = crate::core::buttons::scripts(dir, "repo");
    let top = crate::core::git::toplevel(dir).unwrap_or_else(|_| dir.to_string());
    let pane = if pane.is_empty() { "default" } else { pane };
    crate::core::buttons::run_list(&scripts, script, ids, "BRANCHES", &[("REPO_PATH", top.as_str())], dir, pane, "branches", provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("/usr/bin/git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn the_cleanup_words_name_what_went_what_stayed_and_why() {
        let c = Cleanup {
            deleted: vec!["a".into(), "b".into()],
            kept: vec![("c".into(), "blocked: uncommitted changes".into())],
            failed: vec![],
        };
        assert_eq!(cleanup_words(&c), "Deleted 2: a, b. Kept 1: c (blocked: uncommitted changes).");
        assert_eq!(cleanup_words(&Cleanup::default()), "Deleted nothing.");
        let f = Cleanup { failed: vec![("d".into(), "the worktree is still there".into())], ..Default::default() };
        assert_eq!(cleanup_words(&f), "Deleted nothing. Failed 1: d (the worktree is still there).");
    }

    #[test]
    fn a_repo_button_with_a_worktree_runs_in_the_one_holding_the_branch_and_needs_no_pr_or_remote() {
        let dir = std::env::temp_dir().join(format!("wheelhouse-repo-feed-button-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let toml = r#"[repo.buttons.t-wt]
worktree = true
run = 'printf "%s|%s|%s" "$PR_WORKTREE" "$PR_NUMBER" "$PR_BRANCH" > out.txt'
[repo.buttons.t-plain]
run = "true"
"#;
        std::fs::write(dir.join("wheel.toml"), toml).unwrap();
        let d = dir.to_str().unwrap();
        assert!(button(d, "", "nope", "t-wt", None).unwrap_err().contains("no such local branch"));
        assert!(button(d, "", "main", "missing", None).unwrap_err().contains("no button"));
        button(d, "", "main", "t-wt", None).unwrap();
        let out = std::fs::read_to_string(dir.join("out.txt")).unwrap();
        let (wt, rest) = out.split_once('|').unwrap();
        assert_eq!(std::fs::canonicalize(wt).unwrap(), std::fs::canonicalize(&dir).unwrap());
        assert_eq!(rest, "|main", "no open PR, so PR_NUMBER is empty");
        button(d, "", "main", "t-plain", None).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_that_is_no_checkout_sets_error() {
        let dir = std::env::temp_dir().join(format!("wheelhouse-repo-feed-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let v = value(dir.to_str().unwrap());
        assert!(v["error"].as_str().unwrap().contains("is not a git checkout"));
        assert_eq!(v["branches"], json!([]));
        assert_eq!(v["stashes"], json!([]));
        assert_eq!(v["commits"], json!([]));
        assert!(v["now"].as_u64().unwrap() > 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_repo_with_one_extra_branch_lists_it_with_the_documented_keys() {
        let dir = std::env::temp_dir().join(format!("wheelhouse-repo-feed-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(&dir, &["branch", "extra"]);
        let v = value(dir.to_str().unwrap());
        let keys: std::collections::BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let want = ["now", "name", "top", "default", "remote", "head", "branches", "stashes", "commits", "fetched", "cleanup", "pr_error", "error", "buttons", "scripts"];
        assert_eq!(keys, want.into_iter().collect());
        assert_eq!(v["error"], Value::Null);
        assert_eq!(v["head"], "main");
        assert_eq!(v["default"], "main");
        let branches = v["branches"].as_array().unwrap();
        assert_eq!(branches.len(), 1, "the default branch is not listed");
        let b = &branches[0];
        assert_eq!(b["branch"], "extra");
        assert_eq!(b["ahead"], 0);
        assert_eq!(b["local_only"], 1, "no origin ref holds it, so it is never called dead");
        assert_eq!(v["cleanup"]["delete"], json!([]), "with no origin gh reads no open PRs, so nothing is offered");
        assert_eq!(v["commits"][0]["subject"], "first");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
