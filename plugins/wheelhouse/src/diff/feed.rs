//! The Diff pane's data for `wheelhouse backend`: the checkout's three scopes (uncommitted work,
//! the branch against the default branch, the branch against its fork point) as the `diff` feed's
//! value. The backend sends data only; the pane (Luau) decides what to draw.

use serde_json::Value;

/// The `diff` feed's value for the checkout `dir` sits in: a `diff::Snapshot` as JSON.
pub fn value(dir: &str) -> Value {
    serde_json::to_value(super::collect(dir)).unwrap_or(Value::Null)
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
        let dir = std::env::temp_dir().join(format!("wheelhouse-diff-feed-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let v = value(dir.to_str().unwrap());
        assert!(v["error"].as_str().unwrap().contains("is not a git checkout"));
        assert_eq!(v["scopes"], serde_json::json!([]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_edited_and_an_untracked_file_are_listed_with_the_documented_keys() {
        let dir = std::env::temp_dir().join(format!("wheelhouse-diff-feed-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "first"]);
        std::fs::write(dir.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        let v = value(dir.to_str().unwrap());
        let keys: std::collections::BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let want = ["name", "top", "branch", "head", "default", "error", "scopes"];
        assert_eq!(keys, want.into_iter().collect());
        assert_eq!(v["error"], Value::Null);
        let scope = &v["scopes"][0];
        assert_eq!(scope["key"], "uncommitted");
        let files = scope["files"].as_array().unwrap();
        let by = |p: &str| files.iter().find(|f| f["path"] == p).unwrap_or_else(|| panic!("no {p} in {files:?}"));
        assert_eq!(by("a.txt")["status"], "M");
        assert_eq!(by("new.txt")["status"], "?");
        let lines = by("a.txt")["hunks"][0]["lines"].as_array().unwrap();
        let del = lines.iter().find(|l| l["kind"] == "-").unwrap();
        assert_eq!((del["old"].as_u64(), del["new"].clone(), del["text"].as_str()), (Some(2), Value::Null, Some("two")));
        let add = lines.iter().find(|l| l["kind"] == "+").unwrap();
        assert_eq!((add["old"].clone(), add["new"].as_u64(), add["text"].as_str()), (Value::Null, Some(2), Some("TWO")));
        assert!(lines.iter().any(|l| l["kind"] == " "));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
