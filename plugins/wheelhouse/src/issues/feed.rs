//! The Issues pane's data for `wheelhouse backend`: the `issues` feed (the project's GitHub issues,
//! open and closed) and the list buttons. The backend sends data only; the pane (Luau) decides
//! what to draw.

use serde_json::{json, Value};

use super::*;

/// Characters of an issue's body the feed carries.
const BODY_CUT: usize = 3000;

/// The `issues` feed's value for the project in `dir`.
pub fn value(dir: &str) -> Value {
    snapshot_value(&collect(dir), crate::core::page::now())
}

/// `s` as the feed's value read at `at`.
fn snapshot_value(s: &Snapshot, at: i64) -> Value {
    json!({
        "now": at,
        "name": s.name,
        "error": s.error,
        "problems": s.problems,
        "issues": s.issues.iter().map(|i| json!({
            "number": i.number,
            "title": i.title,
            "state": i.state,
            "labels": i.labels,
            "assignees": i.assignees,
            "author": i.author,
            "body": i.body.chars().take(BODY_CUT).collect::<String>(),
            "url": i.url,
            "updated": i.updated,
        })).collect::<Vec<_>>(),
        "scripts": s.scripts.iter().map(|x| json!({ "name": x.name, "label": x.label() })).collect::<Vec<_>>(),
    })
}

/// Run the project's `[issues.list]` button `script` over the issue numbers `ids` for the pane `pane`.
pub fn run_list(dir: &str, script: &str, ids: &[String], pane: &str) -> Result<String, String> {
    let (scripts, _) = crate::core::buttons::scripts(dir, "issues");
    let repo = crate::core::project::name_of(dir);
    let pane = if pane.is_empty() { "default" } else { pane };
    crate::core::buttons::run_list(&scripts, script, ids, "ISSUE_NUMBERS", &[("ISSUE_REPO", repo.as_str()), ("ISSUE_PATH", dir)], dir, pane, "issues")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_issues_value_has_the_documented_keys_and_cuts_a_long_body() {
        let i = Issue { number: 4, title: "t".into(), state: "open".into(), labels: vec!["bug".into()], assignees: vec!["me".into()], author: "a".into(), body: "x".repeat(4000), url: "https://u".into(), updated: 9 };
        let s = Snapshot { name: "t".into(), issues: vec![i], error: None, scripts: vec![], problems: vec!["p".into()] };
        let v = snapshot_value(&s, 100);
        let keys: std::collections::BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["now", "name", "error", "problems", "issues", "scripts"].into_iter().collect());
        assert_eq!((v["now"].as_i64(), v["name"].as_str(), v["problems"].clone()), (Some(100), Some("t"), json!(["p"])));
        let r = &v["issues"][0];
        let ik: std::collections::BTreeSet<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(ik, ["number", "title", "state", "labels", "assignees", "author", "body", "url", "updated"].into_iter().collect());
        assert_eq!((r["number"].as_i64(), r["state"].as_str(), r["updated"].as_i64()), (Some(4), Some("open"), Some(9)));
        assert_eq!((r["labels"].clone(), r["assignees"].clone()), (json!(["bug"]), json!(["me"])));
        assert_eq!(r["body"].as_str().unwrap().chars().count(), 3000);
    }

    #[test]
    fn a_gh_failure_is_passed_through_with_no_issues() {
        let v = snapshot_value(&Snapshot { error: Some("d: gh issue list failed: no".into()), ..Default::default() }, 1);
        assert_eq!(v["error"].as_str(), Some("d: gh issue list failed: no"));
        assert_eq!(v["issues"], json!([]));
        assert!(snapshot_value(&Snapshot::default(), 1)["error"].is_null());
    }

    #[test]
    fn run_list_refuses_an_unknown_script() {
        let dir = std::env::temp_dir().join("wheelhouse-issues-run-list");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(run_list(&dir.to_string_lossy(), "nope", &["1".into()], "p"), Err("no list button nope".into()));
    }
}
