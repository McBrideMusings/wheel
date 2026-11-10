//! The PRs view's data: every open PR of the project's GitHub repo, bucketed the way admin's PRs
//! tab buckets them (internal/gh/gh.go): mine, needs me, waiting on others, other.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::beads::epoch;
use crate::core::run;

pub mod cli;
pub mod detail;
pub mod markdown;
pub mod serve;

const QUERY: &str = "query($owner:String!,$name:String!,$after:String){repository(owner:$owner,name:$name){pullRequests(states:OPEN,first:25,after:$after,orderBy:{field:UPDATED_AT,direction:DESC}){pageInfo{hasNextPage endCursor} nodes{
number title bodyText headRefName isDraft mergeable updatedAt reviewDecision url additions deletions changedFiles
closingIssuesReferences(first:3){nodes{number}}
author{login __typename}
assignees(first:10){nodes{login}}
reviewRequests(first:10){nodes{requestedReviewer{... on User{login} ... on Team{slug}}}}
labels(first:20){nodes{name}}
reviews(last:50){nodes{author{login __typename} state submittedAt}}
comments(last:50){nodes{author{login __typename} createdAt body}}
reviewThreads(first:50){nodes{isResolved comments(last:20){nodes{author{login __typename} createdAt}}}}
commits(last:1){totalCount nodes{commit{committedDate statusCheckRollup{state}}}}
}}}}";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pr {
    pub number: i64,
    pub title: String,
    pub author: String,
    pub branch: String,
    pub draft: bool,
    pub mergeable: String,
    /// passing, pending, failing or none.
    pub checks: &'static str,
    /// changes-requested, needs-response, approved or in-review (the author's view).
    pub review: &'static str,
    /// mine, needs-me, waiting or other.
    pub bucket: &'static str,
    /// The viewer's standing on a PR they did not write.
    pub my: Option<&'static str>,
    /// A for no assignee, R for no reviewer, L for no label.
    pub gaps: String,
    pub updated: i64,
    pub url: String,
    pub fix: bool,
    pub additions: i64,
    pub deletions: i64,
    pub files: i64,
    pub commits: i64,
    pub labels: Vec<String>,
    pub reviewers: Vec<String>,
    /// What has to happen next, in a few words.
    pub next: String,
    /// The start of the description, one line.
    pub summary: String,
    /// The issues the PR closes.
    pub issues: Vec<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub name: String,
    pub repo: Option<String>,
    pub me: Option<String>,
    pub prs: Vec<Pr>,
    pub error: Option<String>,
    /// When GitHub was asked for these PRs (Unix seconds).
    pub fetched: i64,
    /// The project's `[prs.buttons]`; the CLI does not print them.
    #[serde(skip)]
    pub buttons: Vec<detail::Button>,
}

fn login(a: &Value) -> Option<&str> {
    a["login"].as_str()
}

fn bot(a: &Value) -> bool {
    a["__typename"] == "Bot" || login(a).map_or(false, |l| l.ends_with("[bot]"))
}

fn eq(a: Option<&str>, b: Option<&str>) -> bool {
    a.map(|s| s.to_lowercase()) == b.map(|s| s.to_lowercase())
}

fn nodes<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key]["nodes"].as_array().map_or(&[], |a| a.as_slice())
}

fn ts(v: &Value) -> Option<i64> {
    v.as_str().filter(|s| !s.is_empty()).map(epoch)
}

fn is_me(a: &Value, me: &str) -> bool {
    eq(login(a), Some(me))
}

fn replied(me: &str, n: &Value, since: i64) -> bool {
    for th in nodes(n, "reviewThreads") {
        if th["isResolved"] == true {
            continue;
        }
        let cs: &[Value] = th["comments"]["nodes"].as_array().map_or(&[], |a| a.as_slice());
        if !cs.iter().any(|c| is_me(&c["author"], me)) {
            continue;
        }
        if cs.iter().any(|c| !is_me(&c["author"], me) && !bot(&c["author"]) && ts(&c["createdAt"]).map_or(false, |t| t > since)) {
            return true;
        }
    }
    let mention = format!("@{}", me.to_lowercase());
    nodes(n, "comments").iter().any(|c| {
        !is_me(&c["author"], me) && !bot(&c["author"]) && ts(&c["createdAt"]).map_or(false, |t| t > since)
            && c["body"].as_str().unwrap_or("").to_lowercase().contains(&mention)
    })
}

/// admin's classifyInboxState: the viewer's standing on a PR they did not author.
fn my_state(me: &str, n: &Value) -> &'static str {
    let mut latest: Option<i64> = None;
    let mut verdict: Option<(i64, &str)> = None;
    let mut bump = |t: Option<i64>| {
        if let Some(t) = t {
            latest = Some(latest.map_or(t, |l| l.max(t)));
        }
    };
    for r in nodes(n, "reviews") {
        if !is_me(&r["author"], me) {
            continue;
        }
        let t = ts(&r["submittedAt"]);
        bump(t);
        if let (Some(t), Some(s @ ("APPROVED" | "CHANGES_REQUESTED"))) = (t, r["state"].as_str()) {
            if verdict.map_or(true, |(v, _)| t > v) {
                verdict = Some((t, s));
            }
        }
    }
    for c in nodes(n, "comments") {
        if is_me(&c["author"], me) {
            bump(ts(&c["createdAt"]));
        }
    }
    for th in nodes(n, "reviewThreads") {
        for c in th["comments"]["nodes"].as_array().into_iter().flatten() {
            if is_me(&c["author"], me) {
                bump(ts(&c["createdAt"]));
            }
        }
    }
    let Some(latest) = latest else { return "not-reviewed" };
    if let Some(c) = nodes(n, "commits").first() {
        if ts(&c["commit"]["committedDate"]).map_or(false, |t| t > latest) {
            return "updated";
        }
    }
    if replied(me, n, latest) {
        return "replied";
    }
    match verdict.map(|v| v.1) {
        Some("APPROVED") => "approved",
        Some("CHANGES_REQUESTED") => "changes",
        _ => "commented",
    }
}

/// admin's classifyReviewClass with the PR's own author as the viewer.
fn author_class(n: &Value) -> &'static str {
    let me = login(&n["author"]).unwrap_or("").to_string();
    let (mut mine, mut formal, mut informal): (Option<i64>, Option<i64>, Option<i64>) = (None, None, None);
    let bump = |cur: &mut Option<i64>, t: Option<i64>| {
        if let Some(t) = t {
            *cur = Some(cur.map_or(t, |c| c.max(t)));
        }
    };
    for r in nodes(n, "reviews") {
        let t = ts(&r["submittedAt"]);
        if is_me(&r["author"], &me) {
            bump(&mut mine, t);
        } else if bot(&r["author"]) {
        } else if r["state"] == "CHANGES_REQUESTED" {
            bump(&mut formal, t);
        } else if r["state"] == "COMMENTED" {
            bump(&mut informal, t);
        }
    }
    for c in nodes(n, "comments") {
        let t = ts(&c["createdAt"]);
        if is_me(&c["author"], &me) {
            bump(&mut mine, t);
        } else if !bot(&c["author"]) {
            bump(&mut informal, t);
        }
    }
    for th in nodes(n, "reviewThreads") {
        if th["isResolved"] == true {
            continue;
        }
        for c in th["comments"]["nodes"].as_array().into_iter().flatten() {
            let t = ts(&c["createdAt"]);
            if is_me(&c["author"], &me) {
                bump(&mut mine, t);
            } else if !bot(&c["author"]) {
                bump(&mut informal, t);
            }
        }
    }
    if let Some(c) = nodes(n, "commits").first() {
        bump(&mut mine, ts(&c["commit"]["committedDate"]));
    }
    let after = |x: Option<i64>| x.map_or(false, |x| mine.map_or(true, |m| x > m));
    if after(formal) {
        "changes-requested"
    } else if after(informal) {
        "needs-response"
    } else if n["reviewDecision"] == "APPROVED" {
        "approved"
    } else {
        "in-review"
    }
}

fn has_reviewer(n: &Value) -> bool {
    // A requested team counts: it is a {slug} with no login.
    if nodes(n, "reviewRequests").iter().any(|r| !r["requestedReviewer"].is_null()) {
        return true;
    }
    nodes(n, "reviews").iter().any(|r| !bot(&r["author"]) && !eq(login(&r["author"]), login(&n["author"])))
}

/// One GraphQL node as a PR, bucketed for `me`.
pub fn convert(n: &Value, me: &str) -> Pr {
    let author = login(&n["author"]).unwrap_or("ghost").to_string();
    let asked = nodes(n, "assignees").iter().any(|a| eq(login(a), Some(me)))
        || nodes(n, "reviewRequests").iter().any(|r| eq(login(&r["requestedReviewer"]), Some(me)));
    let (bucket, my) = if eq(Some(&author), Some(me)) {
        ("mine", None)
    } else {
        let my = my_state(me, n);
        let b = match my {
            "not-reviewed" if asked => "needs-me",
            "not-reviewed" => "other",
            "updated" | "replied" => "needs-me",
            _ => "waiting",
        };
        (b, Some(my))
    };
    let state = nodes(n, "commits").first().and_then(|c| c["commit"]["statusCheckRollup"]["state"].as_str());
    let checks = match state {
        Some("SUCCESS") => "passing",
        Some("PENDING" | "EXPECTED") => "pending",
        Some("FAILURE" | "ERROR") => "failing",
        _ => "none",
    };
    let gaps: String = [('A', nodes(n, "assignees").is_empty()), ('R', !has_reviewer(n)), ('L', nodes(n, "labels").is_empty())]
        .iter().filter(|(_, missing)| *missing).map(|(k, _)| *k).collect();
    let review = author_class(n);
    // Any PR with red checks or conflicts needs a fix; review feedback only counts on mine.
    let fix = checks == "failing" || n["mergeable"] == "CONFLICTING" || (bucket == "mine" && matches!(review, "changes-requested" | "needs-response"));
    let next = next_step(n, bucket, my, checks, review, fix);
    let summary: String = n["bodyText"].as_str().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
    let issues = nodes(n, "closingIssuesReferences").iter().filter_map(|i| i["number"].as_i64()).collect();
    let num = |k: &str| n[k].as_i64().unwrap_or(0);
    let text = |k: &str| n[k].as_str().unwrap_or("").to_string();
    Pr {
        number: num("number"), title: text("title"), author, branch: text("headRefName"), draft: n["isDraft"] == true,
        mergeable: text("mergeable"), checks, review, bucket, my, gaps, updated: ts(&n["updatedAt"]).unwrap_or(0), url: text("url"), fix,
        additions: num("additions"), deletions: num("deletions"), files: num("changedFiles"), commits: n["commits"]["totalCount"].as_i64().unwrap_or(0),
        labels: nodes(n, "labels").iter().filter_map(|l| l["name"].as_str().map(String::from)).collect(),
        reviewers: nodes(n, "reviewRequests").iter().filter_map(|r| {
            let x = &r["requestedReviewer"];
            login(x).or_else(|| x["slug"].as_str()).map(String::from)
        }).collect(),
        next, summary, issues,
    }
}

/// What has to happen to this PR next, from the viewer's side.
fn next_step(n: &Value, bucket: &str, my: Option<&str>, checks: &str, review: &str, fix: bool) -> String {
    let words = if n["isDraft"] == true {
        "Mark ready"
    } else if n["mergeable"] == "CONFLICTING" {
        "Resolve conflicts"
    } else if checks == "failing" {
        "Fix failing checks"
    } else if bucket == "mine" && review == "changes-requested" {
        "Address requested changes"
    } else if bucket == "mine" && review == "needs-response" {
        "Reply to comments"
    } else if checks == "pending" {
        "Wait for checks"
    } else if bucket == "mine" && review == "approved" {
        "Merge"
    } else if bucket == "mine" && !has_reviewer(n) {
        "Request a review"
    } else if bucket == "mine" {
        "Waiting on a reviewer"
    } else {
        match my {
            Some("not-reviewed") => "Review",
            Some("updated") => "Re-review",
            Some("replied") => "Reply",
            Some("approved") => "Approved: waiting on author",
            Some("changes") => "Waiting on author",
            _ if fix => "Waiting on author",
            _ => "Waiting on author",
        }
    };
    words.to_string()
}

/// How long a read of a repo's PRs, or of one PR, is shared by every pane and project asking for it.
const SHARE: Duration = Duration::from_secs(30);

/// What GitHub said, kept for every pane and every directory of the same repo: one read of the repo's PRs
/// serves them all, and one PR's details are read once however many panes show it.
#[derive(Default)]
struct Cache {
    /// Project directory -> `owner/name`.
    repos: HashMap<String, String>,
    me: Option<String>,
    /// `owner/name` -> when it was read, the Unix second, the PR nodes.
    nodes: HashMap<String, (Instant, i64, Vec<Value>)>,
    details: HashMap<(String, i64), (Instant, Value)>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);
/// One gate per repo: a caller that waits at it finds the read the one before it made, and does not make its own.
static GATES: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = LazyLock::new(Mutex::default);

fn gate(repo: &str) -> Arc<Mutex<()>> {
    GATES.lock().unwrap().entry(repo.to_string()).or_default().clone()
}

/// Whether an entry read at `at` serves a caller that arrived at `arrived`: always one read after the caller came
/// (it asked for fresh data and someone just got it), otherwise one younger than [`SHARE`] unless `force`.
fn serves(at: Instant, arrived: Instant, force: bool) -> bool {
    at >= arrived || (!force && at.elapsed() < SHARE)
}

fn pulls(dir: &str, repo: &str) -> Result<Vec<Value>, String> {
    let (owner, short) = repo.split_once('/').ok_or("no owner/name")?;
    // GitHub computes `mergeable` on demand: after a push to the base branch every PR reads
    // UNKNOWN until the first query starts the computation. Ask again until none is UNKNOWN (at
    // most 4 asks, 3 seconds apart).
    for attempt in 0..4 {
        let mut all: Vec<Value> = vec![];
        let mut after: Option<String> = None;
        loop {
            let q = format!("query={QUERY}");
            let (o, nm) = (format!("owner={owner}"), format!("name={short}"));
            let mut args = vec!["api", "graphql", "-f", q.as_str(), "-F", o.as_str(), "-F", nm.as_str()];
            let cursor = after.as_ref().map(|a| format!("after={a}"));
            if let Some(c) = &cursor {
                args.extend(["-f", c.as_str()]);
            }
            let page: Value = serde_json::from_str(&run::gh(dir, &args)?).map_err(|e| e.to_string())?;
            let prs = &page["data"]["repository"]["pullRequests"];
            all.extend(prs["nodes"].as_array().cloned().unwrap_or_default());
            if prs["pageInfo"]["hasNextPage"] != true {
                break;
            }
            after = prs["pageInfo"]["endCursor"].as_str().map(String::from);
        }
        if attempt == 3 || !all.iter().any(|n| n["mergeable"] == "UNKNOWN") {
            return Ok(all);
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    unreachable!()
}

/// The repo's PR nodes and when they were read: the shared read when one serves, else a new one.
fn shared_pulls(dir: &str, repo: &str, force: bool) -> Result<(i64, Vec<Value>), String> {
    let arrived = Instant::now();
    let g = gate(repo);
    let _turn = g.lock().unwrap();
    if let Some((at, when, nodes)) = CACHE.lock().unwrap().nodes.get(repo) {
        if serves(*at, arrived, force) {
            return Ok((*when, nodes.clone()));
        }
    }
    let nodes = pulls(dir, repo)?;
    let when = crate::core::page::now();
    CACHE.lock().unwrap().nodes.insert(repo.to_string(), (Instant::now(), when, nodes.clone()));
    Ok((when, nodes))
}

/// One PR in full when a read of it is younger than [`SHARE`]; never asks GitHub.
pub fn cached_detail(repo: &str, number: i64) -> Option<Value> {
    CACHE.lock().unwrap().details.get(&(repo.to_string(), number)).filter(|(at, _)| at.elapsed() < SHARE).map(|(_, v)| v.clone())
}

/// One PR in full, shared the same way.
pub fn shared_detail(dir: &str, repo: &str, number: i64, force: bool) -> Result<Value, String> {
    let arrived = Instant::now();
    let g = gate(repo);
    let _turn = g.lock().unwrap();
    let key = (repo.to_string(), number);
    if let Some((at, v)) = CACHE.lock().unwrap().details.get(&key) {
        if serves(*at, arrived, force) {
            return Ok(v.clone());
        }
    }
    let v = detail::fetch(dir, repo, number)?;
    CACHE.lock().unwrap().details.insert(key, (Instant::now(), v.clone()));
    Ok(v)
}

fn repo_of(dir: &str) -> Result<String, String> {
    if let Some(r) = CACHE.lock().unwrap().repos.get(dir) {
        return Ok(r.clone());
    }
    let r = run::gh(dir, &["repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"]).map_err(|e| format!("{dir} is not a GitHub repository: {e}"))?;
    let r = r.trim().to_string();
    CACHE.lock().unwrap().repos.insert(dir.to_string(), r.clone());
    Ok(r)
}

fn viewer(dir: &str) -> Result<String, String> {
    if let Some(m) = CACHE.lock().unwrap().me.clone() {
        return Ok(m);
    }
    let m = run::gh(dir, &["api", "user", "-q", ".login"]).map_err(|e| format!("gh api user failed: {e}"))?.trim().to_string();
    CACHE.lock().unwrap().me = Some(m.clone());
    Ok(m)
}

/// Read project `dir`'s open PRs, from what another pane or project of the same repo read in the last 30 seconds
/// when that serves. A failure is in `error`, never a panic.
pub fn collect(dir: &str) -> Snapshot {
    collect_with(dir, false)
}

/// As [`collect`]; `force` asks GitHub again unless a read began after this call did.
pub fn collect_with(dir: &str, force: bool) -> Snapshot {
    let mut s = Snapshot { name: crate::core::project::name_of(dir), ..Default::default() };
    let (buttons, problem) = detail::buttons(dir);
    s.buttons = buttons;
    let repo = match repo_of(dir) {
        Ok(r) => r,
        Err(e) => {
            s.error = Some(e);
            return s;
        }
    };
    let me = match viewer(dir) {
        Ok(m) => m,
        Err(e) => {
            s.error = Some(e);
            return s;
        }
    };
    match shared_pulls(dir, &repo, force) {
        Ok((when, nodes)) => {
            s.fetched = when;
            s.prs = nodes.iter().map(|n| convert(n, &me)).collect();
        }
        Err(e) => s.error = Some(e.chars().take(300).collect()),
    }
    if s.error.is_none() {
        s.error = problem;
    }
    s.repo = Some(repo);
    s.me = Some(me);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(author: &str, f: impl Fn(&mut Value)) -> Value {
        let mut n = json!({ "number": 1, "title": "t", "headRefName": "b", "isDraft": false, "mergeable": "MERGEABLE",
            "updatedAt": "2026-11-01T00:00:00Z", "reviewDecision": null, "url": "u", "additions": 1, "deletions": 0, "changedFiles": 1,
            "author": { "login": author, "__typename": "User" }, "assignees": { "nodes": [] }, "reviewRequests": { "nodes": [] },
            "labels": { "nodes": [] }, "reviews": { "nodes": [] }, "comments": { "nodes": [] }, "reviewThreads": { "nodes": [] },
            "commits": { "totalCount": 1, "nodes": [{ "commit": { "committedDate": "2026-11-01T00:00:00Z", "statusCheckRollup": { "state": "SUCCESS" } } }] } });
        f(&mut n);
        n
    }

    #[test]
    fn a_read_serves_later_callers_for_thirty_seconds_and_a_forced_one_only_if_it_began_after_the_call() {
        let now = Instant::now();
        let old = now - Duration::from_secs(40);
        assert!(serves(now, now, false), "a fresh read serves");
        assert!(!serves(old, now, false), "a read older than 30s does not");
        assert!(serves(now - Duration::from_secs(5), now, false));
        assert!(!serves(now - Duration::from_secs(5), now, true), "a forced call wants a read that began after it arrived");
        assert!(serves(now + Duration::from_millis(1), now, true), "one that did is shared");
    }

    #[test]
    fn the_next_step_follows_the_pr_and_who_is_looking() {
        let step = |author: &str, f: &dyn Fn(&mut Value)| convert(&node(author, |n| f(n)), "me").next;
        assert_eq!(step("me", &|n| n["isDraft"] = json!(true)), "Mark ready");
        assert_eq!(step("me", &|n| n["mergeable"] = json!("CONFLICTING")), "Resolve conflicts");
        assert_eq!(step("me", &|n| n["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"] = json!("FAILURE")), "Fix failing checks");
        assert_eq!(step("me", &|n| n["reviewDecision"] = json!("APPROVED")), "Merge");
        assert_eq!(step("you", &|n| n["reviewRequests"]["nodes"] = json!([{ "requestedReviewer": { "login": "me" } }])), "Review");
        let p = convert(&node("me", |n| { n["bodyText"] = json!("A  long\n description"); n["closingIssuesReferences"] = json!({ "nodes": [{ "number": 12 }] }); }), "me");
        assert_eq!((p.summary.as_str(), p.issues), ("A long description", vec![12]));
    }

    #[test]
    fn my_pr_is_mine_and_red_checks_need_a_fix() {
        let p = convert(&node("me", |_| {}), "me");
        assert_eq!((p.bucket, p.checks, p.fix, p.gaps.as_str()), ("mine", "passing", false, "ARL"));
        let p = convert(&node("me", |n| n["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"] = json!("FAILURE")), "me");
        assert!(p.fix && p.checks == "failing");
    }

    #[test]
    fn someone_elses_pr_is_needs_me_when_asked_else_other() {
        let asked = node("you", |n| n["reviewRequests"]["nodes"] = json!([{ "requestedReviewer": { "login": "me" } }]));
        assert_eq!(convert(&asked, "me").bucket, "needs-me");
        assert_eq!(convert(&node("you", |_| {}), "me").bucket, "other");
    }

    #[test]
    fn a_review_i_gave_makes_it_waiting_until_new_commits_land() {
        let reviewed = |commit: &str| node("you", |n| {
            n["reviews"]["nodes"] = json!([{ "author": { "login": "me" }, "state": "APPROVED", "submittedAt": "2026-11-02T00:00:00Z" }]);
            n["commits"]["nodes"][0]["commit"]["committedDate"] = json!(commit);
        });
        let p = convert(&reviewed("2026-11-01T00:00:00Z"), "me");
        assert_eq!((p.bucket, p.my), ("waiting", Some("approved")));
        let p = convert(&reviewed("2026-11-03T00:00:00Z"), "me");
        assert_eq!((p.bucket, p.my), ("needs-me", Some("updated")));
    }
}
