//! The PRs pane's data for `wheelhouse backend`: the `prs` feed (the project's open PRs), the
//! `pr_detail` feed (one PR in full, digested to data), and the PR actions. The backend sends data
//! only; the pane (Luau) decides what to draw.

use serde_json::{json, Value};

use super::detail::{self, Button};
use super::markdown;
use super::*;
use crate::core::number_of;
use crate::core::project::Origin;

/// A Unix time as the local `HH:MM`.
fn clock(ts: i64) -> String {
    run::text("/bin/date", "/", &["-r", &ts.to_string(), "+%H:%M"]).map(|t| t.trim().to_string()).unwrap_or_default()
}

/// The `prs` feed's value for the project in `dir`.
pub fn value(dir: &str) -> Value {
    snapshot_value(&collect(dir), crate::core::now(), clock)
}

/// `s` as the feed's value read at `at`; `clock` writes a Unix time as `HH:MM`.
fn snapshot_value(s: &Snapshot, at: i64, clock: impl Fn(i64) -> String) -> Value {
    let (fetched_clock, next_clock) = if s.fetched > 0 { (clock(s.fetched), clock(s.fetched + 120)) } else { (String::new(), String::new()) };
    json!({
        "now": at,
        "name": s.name,
        "repo": s.repo,
        "me": s.me,
        "error": s.error,
        "fetched": s.fetched,
        "fetched_clock": fetched_clock,
        "next_clock": next_clock,
        "prs": s.prs,
        "buttons": s.buttons.iter().map(|b: &Button| json!({ "name": b.name, "label": detail::label(b), "on": b.on, "worktree": b.worktree })).collect::<Vec<_>>(),
        "scripts": s.scripts.iter().map(|x| json!({ "name": x.name, "label": x.label() })).collect::<Vec<_>>(),
    })
}

/// The `pr_detail` feed's value for PR `number` of the project in `dir`.
pub fn detail_value(dir: &str, number: i64) -> Value {
    match repo_of(dir).and_then(|repo| shared_detail(dir, &repo, number, false)) {
        Ok(n) => {
            detail::prefetch_images(dir, &n);
            digest(&n, number, crate::core::now(), detail::agent_names(dir))
        }
        Err(e) => {
            let mut v = digest(&Value::Null, number, crate::core::now(), vec![]);
            v["error"] = json!(e.chars().take(300).collect::<String>());
            v
        }
    }
}

/// One PR's GraphQL node as the `pr_detail` value; a null node gives the empty value.
fn digest(n: &Value, number: i64, at: i64, agents: Vec<String>) -> Value {
    let state = if n["merged"] == true { "MERGED" } else { n["state"].as_str().unwrap_or("OPEN") };
    let secs = |v: &Value| ts(v).unwrap_or(0);
    let mut reviewers = detail::names(n, "reviewRequests", |r| {
        let x = &r["requestedReviewer"];
        login(x).or_else(|| x["slug"].as_str()).map(|l| format!("{l} (requested)"))
    });
    reviewers.extend(detail::names(n, "latestReviews", |r| login(&r["author"]).map(|l| format!("{l} ({})", r["state"].as_str().unwrap_or("").to_lowercase().replace('_', " ")))));
    let contexts = nodes(&n["checks"]["nodes"][0]["commit"]["statusCheckRollup"], "contexts");
    let checks: Vec<Value> = contexts
        .iter()
        .map(|k| {
            let (name, st) = if k["__typename"] == "CheckRun" {
                (k["name"].as_str().unwrap_or(""), k["conclusion"].as_str().or(k["status"].as_str()).unwrap_or(""))
            } else {
                (k["context"].as_str().unwrap_or(""), k["state"].as_str().unwrap_or(""))
            };
            json!({ "name": name, "state": st })
        })
        .collect();
    // Reviews that say something, then comments, oldest first.
    let mut talk: Vec<(i64, String, String, String)> = vec![];
    for r in nodes(n, "reviews") {
        let text = r["body"].as_str().unwrap_or("").trim();
        let st = r["state"].as_str().unwrap_or("");
        if text.is_empty() && st == "COMMENTED" {
            continue;
        }
        talk.push((secs(&r["submittedAt"]), login(&r["author"]).unwrap_or("ghost").into(), format!("review: {}", st.to_lowercase().replace('_', " ")), text.into()));
    }
    for cm in nodes(n, "comments") {
        talk.push((secs(&cm["createdAt"]), login(&cm["author"]).unwrap_or("ghost").into(), "commented".into(), cm["body"].as_str().unwrap_or("").into()));
    }
    talk.sort_by_key(|t| t.0);
    let history = nodes(n, "history");
    let files = nodes(n, "files");
    json!({
        "now": at,
        "number": number,
        "error": Value::Null,
        "state": state,
        "draft": n["isDraft"] == true,
        "mergeable": n["mergeable"].as_str().unwrap_or(""),
        "merge_state": n["mergeStateStatus"].as_str().unwrap_or(""),
        "base": n["baseRefName"].as_str().unwrap_or(""),
        "cross": n["isCrossRepository"] == true,
        "created": secs(&n["createdAt"]),
        "updated": secs(&n["updatedAt"]),
        "url": n["url"].as_str().unwrap_or(""),
        "assignees": detail::names(n, "assignees", |a| login(a).map(String::from)),
        "reviewers": reviewers,
        "labels": detail::names(n, "labels", |l| l["name"].as_str().map(String::from)),
        "body": markdown::blocks(n["body"].as_str().unwrap_or("").trim()),
        "checks": checks,
        "talk": talk.iter().map(|(t, who, what, text)| json!({ "at": t, "who": who, "what": what, "blocks": markdown::blocks(text) })).collect::<Vec<_>>(),
        "commits_total": n["history"]["totalCount"].as_u64().unwrap_or(history.len() as u64),
        "commits": history.iter().rev().map(|k| {
            let k = &k["commit"];
            json!({ "sha": k["abbreviatedOid"].as_str().unwrap_or(""), "headline": k["messageHeadline"].as_str().unwrap_or(""), "at": secs(&k["committedDate"]) })
        }).collect::<Vec<_>>(),
        "files_total": n["files"]["totalCount"].as_u64().unwrap_or(files.len() as u64),
        "files": files.iter().map(|f| json!({ "path": f["path"].as_str().unwrap_or(""), "additions": f["additions"].as_i64().unwrap_or(0), "deletions": f["deletions"].as_i64().unwrap_or(0) })).collect::<Vec<_>>(),
        "agents": agents,
    })
}

/// Run PR action `action` (`pr_merge`, `pr_close`, `pr_draft`, `pr_checkout`, `pr_worktree`, `pr_button` or
/// `pr_refresh`) on the project in `dir` for the pane `pane`; the words for the pane.
pub fn act(action: &str, dir: &str, pane: &str, args: &Value) -> Result<String, String> {
    if action == "pr_refresh" {
        forget(dir);
        return Ok(String::new());
    }
    let number = number_of(&args["number"]).ok_or_else(|| format!("{action} needs args.number"))?;
    let text = |k: &str| args[k].as_str().map(String::from);
    let snap = collect(dir);
    if let (Some(e), true) = (&snap.error, snap.prs.is_empty()) {
        return Err(e.clone());
    }
    let pr = snap.prs.iter().find(|p| p.number == number).cloned().ok_or_else(|| format!("no open PR #{number}"))?;
    let origin = Origin { dir: dir.to_string(), pane: if pane.is_empty() { "default".into() } else { pane.into() } };
    let cross = || {
        let repo = snap.repo.as_deref().unwrap_or("");
        shared_detail(dir, repo, number, false).map(|d| d["isCrossRepository"] == true).unwrap_or(false)
    };
    match action {
        "pr_merge" => {
            let method = text("method").unwrap_or_else(|| "squash".into());
            detail::merge(dir, number, &method)?;
            Ok(format!("Merged #{number} ({method})."))
        }
        "pr_close" => {
            detail::close(dir, number)?;
            Ok(format!("Closed #{number}."))
        }
        "pr_draft" => {
            let draft = args["draft"].as_bool().ok_or("pr_draft needs args.draft, true or false")?;
            detail::set_draft(dir, number, draft)?;
            Ok(if draft { format!("#{number} is a draft.") } else { format!("#{number} is ready for review.") })
        }
        "pr_checkout" => {
            detail::checkout(dir, number)?;
            let p = detail::terminal(dir, &origin.pane)?;
            Ok(format!("Checked out {} here; terminal {p} opened.", pr.branch))
        }
        "pr_worktree" => {
            let wt = detail::worktree(dir, number, &pr.branch, cross())?;
            let agent = text("agent").unwrap_or_else(|| "default".into());
            let open = text("open").unwrap_or_else(|| "pane".into());
            let p = detail::place_agent(&origin, &pr, &wt, &agent, &open)?;
            Ok(format!("{} is in {wt}; agent pane {p} started.", pr.branch))
        }
        "pr_button" => {
            let name = text("name").ok_or("pr_button needs args.name")?;
            let b = snap.buttons.iter().find(|b| b.name == name).ok_or_else(|| format!("no button {name}"))?;
            detail::run_button(&origin, snap.repo.as_deref().unwrap_or(""), &pr, cross(), b)?;
            Ok(format!("{} started for #{number}. See the new pane.", detail::label(b)))
        }
        other => Err(format!("no PR action {other}")),
    }
}

/// Run the project's `[prs.list]` button `script` over the PR numbers `ids` for the pane `pane`.
pub fn run_list(dir: &str, script: &str, ids: &[String], pane: &str) -> Result<String, String> {
    let (scripts, _) = crate::core::buttons::scripts(dir, "prs");
    let repo = repo_of(dir).unwrap_or_default();
    let pane = if pane.is_empty() { "default" } else { pane };
    crate::core::buttons::run_list(&scripts, script, ids, "PR_NUMBERS", &[("PR_REPO", repo.as_str()), ("PR_PATH", dir)], dir, pane, "PRs")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> Pr {
        Pr { number: 7, title: "t".into(), author: "a".into(), branch: "b".into(), draft: false, mergeable: "MERGEABLE".into(), checks: "passing", review: "approved",
            bucket: "mine", my: None, standing: "waiting-reviewer", updated: 5, url: "https://u".into(), fix: true, additions: 3, deletions: 1, files: 1, commits: 1,
            labels: vec![], reviewers: vec![], summary: String::new() }
    }

    #[test]
    fn the_prs_value_has_the_documented_keys_and_clock_times() {
        let b = Button { name: "fix".into(), label: Some("Make mergeable".into()), on: vec!["fix".into()], run: "true".into(), worktree: true };
        let s = Snapshot { name: "t".into(), repo: Some("o/t".into()), me: Some("me".into()), prs: vec![pr()], error: None, fetched: 1000, buttons: vec![b], scripts: vec![] };
        let v = snapshot_value(&s, 1100, |t| format!("c{t}"));
        let keys: std::collections::BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let want = ["now", "name", "repo", "me", "error", "fetched", "fetched_clock", "next_clock", "prs", "buttons", "scripts"];
        assert_eq!(keys, want.into_iter().collect());
        assert_eq!((v["fetched"].as_i64(), v["fetched_clock"].as_str(), v["next_clock"].as_str()), (Some(1000), Some("c1000"), Some("c1120")));
        assert_eq!(v["buttons"], json!([{ "name": "fix", "label": "Make mergeable", "on": ["fix"], "worktree": true }]));
        let p = &v["prs"][0];
        assert_eq!((p["number"].as_i64(), p["bucket"].as_str(), p["standing"].as_str(), p["fix"].as_bool()), (Some(7), Some("mine"), Some("waiting-reviewer"), Some(true)));
        assert!(p["my"].is_null() && p["labels"].is_array() && p["summary"].is_string());
        let never = snapshot_value(&Snapshot { error: Some("no gh".into()), ..Default::default() }, 1, |_| unreachable!());
        assert_eq!((never["fetched"].as_i64(), never["fetched_clock"].as_str(), never["next_clock"].as_str(), never["error"].as_str()), (Some(0), Some(""), Some(""), Some("no gh")));
        assert!(never["repo"].is_null());
    }

    #[test]
    fn the_detail_value_digests_a_node() {
        let n = json!({ "state": "OPEN", "isDraft": false, "merged": false, "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "baseRefName": "main",
            "isCrossRepository": true, "url": "https://u", "createdAt": "2026-11-01T00:00:00Z", "updatedAt": "2026-11-02T00:00:00Z", "body": "Fixes it\n![shot](https://x/a.png)",
            "assignees": { "nodes": [{ "login": "me" }] }, "labels": { "nodes": [{ "name": "bug" }] },
            "reviewRequests": { "nodes": [{ "requestedReviewer": { "login": "you" } }, { "requestedReviewer": { "slug": "core" } }] },
            "latestReviews": { "nodes": [{ "author": { "login": "zed" }, "state": "CHANGES_REQUESTED" }] },
            "reviews": { "nodes": [
                { "author": { "login": "zed" }, "state": "APPROVED", "submittedAt": "2026-11-02T00:00:00Z", "body": "ok" },
                { "author": { "login": "quiet" }, "state": "COMMENTED", "submittedAt": "2026-11-02T00:30:00Z", "body": "" }] },
            "comments": { "nodes": [{ "author": { "login": "you" }, "createdAt": "2026-11-02T01:00:00Z", "body": "see ![](https://x/b.png)" }] },
            "history": { "totalCount": 31, "nodes": [
                { "commit": { "abbreviatedOid": "aaa", "messageHeadline": "first", "committedDate": "2026-11-01T00:00:00Z" } },
                { "commit": { "abbreviatedOid": "bbb", "messageHeadline": "second", "committedDate": "2026-11-01T01:00:00Z" } }] },
            "files": { "totalCount": 61, "nodes": [{ "path": "a.rs", "additions": 3, "deletions": 1 }] },
            "checks": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS", "contexts": { "nodes": [
                { "__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "SUCCESS" },
                { "__typename": "CheckRun", "name": "slow", "status": "IN_PROGRESS", "conclusion": null },
                { "__typename": "StatusContext", "context": "lint", "state": "FAILURE" }] } } } }] } });
        let v = digest(&n, 7, 99, vec!["claude".into()]);
        let keys: std::collections::BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let want = ["now", "number", "error", "state", "draft", "mergeable", "merge_state", "base", "cross", "created", "updated", "url", "assignees", "reviewers", "labels",
            "body", "checks", "talk", "commits_total", "commits", "files_total", "files", "agents"];
        assert_eq!(keys, want.into_iter().collect());
        assert_eq!((v["now"].as_i64(), v["number"].as_i64(), v["state"].as_str(), v["cross"].as_bool(), v["base"].as_str()), (Some(99), Some(7), Some("OPEN"), Some(true), Some("main")));
        assert_eq!(v["created"], 1793491200);
        assert_eq!(v["reviewers"], json!(["you (requested)", "core (requested)", "zed (changes requested)"]));
        assert_eq!(v["assignees"], json!(["me"]));
        assert_eq!(v["checks"], json!([{ "name": "ci", "state": "SUCCESS" }, { "name": "slow", "state": "IN_PROGRESS" }, { "name": "lint", "state": "FAILURE" }]));
        assert_eq!(v["body"][0]["text"], "Fixes it");
        assert_eq!(v["body"][1]["url"], "https://x/a.png");
        let who: Vec<(&str, &str)> = v["talk"].as_array().unwrap().iter().map(|t| (t["who"].as_str().unwrap(), t["what"].as_str().unwrap())).collect();
        assert_eq!(who, vec![("zed", "review: approved"), ("you", "commented")], "an empty COMMENTED review is left out, oldest first");
        assert_eq!(v["talk"][1]["blocks"][0]["text"], "see");
        assert_eq!(v["talk"][1]["blocks"][1]["kind"], "image");
        assert_eq!((v["commits_total"].as_u64(), v["files_total"].as_u64()), (Some(31), Some(61)));
        assert_eq!(v["commits"][0], json!({ "sha": "bbb", "headline": "second", "at": 1793494800 }), "newest first");
        assert_eq!(v["files"][0], json!({ "path": "a.rs", "additions": 3, "deletions": 1 }));
        assert_eq!(v["agents"], json!(["claude"]));
        let merged = digest(&json!({ "state": "CLOSED", "merged": true }), 7, 1, vec![]);
        assert_eq!(merged["state"], "MERGED");
        let none = digest(&Value::Null, 9, 1, vec![]);
        assert_eq!((none["number"].as_i64(), none["state"].as_str(), none["body"].clone(), none["talk"].clone()), (Some(9), Some("OPEN"), json!([]), json!([])));
    }
}
