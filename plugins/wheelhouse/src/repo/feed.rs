//! The Repo pane's data for `wheelhouse backend`: one project's branches, stashes and recent
//! commits as the `repo` feed's value, and the `branch_delete` and `run_list` actions. The backend
//! sends data only; the pane (Luau) decides what to draw.

use serde_json::{json, Value};

use super::collect;

/// The `repo` feed's value for the project in `dir`.
pub fn value(dir: &str) -> Value {
    let snap = collect(dir);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    json!({
        "now": now,
        "name": snap.name,
        "top": snap.top,
        "default": snap.default,
        "head": snap.head,
        "branches": snap.branches,
        "stashes": snap.stashes.iter().map(|(r, subject, when)| json!({ "ref": r, "subject": subject, "when": when })).collect::<Vec<_>>(),
        "commits": snap.commits,
        "pr_error": snap.pr_error,
        "error": snap.error,
        "scripts": snap.scripts.iter().map(|s| json!({ "name": s.name, "label": s.label() })).collect::<Vec<_>>(),
    })
}

/// Delete branch `branch` of the project in `dir` (and its worktree first); the words for the pane.
pub fn delete(dir: &str, branch: &str) -> Result<String, String> {
    super::delete(dir, branch).map(|did| did.join("; ")).map_err(|e| format!("{branch}: {e}"))
}

/// Run the project's `[repo.list]` button `script` over the branches `ids` for the pane `pane`.
pub fn run_list(dir: &str, script: &str, ids: &[String], pane: &str) -> Result<String, String> {
    let (scripts, _) = crate::core::buttons::scripts(dir, "repo");
    let top = crate::core::git::toplevel(dir).unwrap_or_else(|_| dir.to_string());
    let pane = if pane.is_empty() { "default" } else { pane };
    crate::core::buttons::run_list(&scripts, script, ids, "BRANCHES", &[("REPO_PATH", top.as_str())], dir, pane, "branches")
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
        let want = ["now", "name", "top", "default", "head", "branches", "stashes", "commits", "pr_error", "error", "scripts"];
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
        assert_eq!(v["commits"][0]["subject"], "first");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
