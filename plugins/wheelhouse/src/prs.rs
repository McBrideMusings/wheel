//! The PRs view's data: every open PR of the project's GitHub repo, bucketed the way admin's PRs
//! tab buckets them (internal/gh/gh.go): mine, needs me, waiting on others, other.

use serde::Serialize;
use serde_json::Value;

use crate::beads::epoch;
use crate::core::run;

pub mod cli;
pub mod serve;

const QUERY: &str = "query($owner:String!,$name:String!,$after:String){repository(owner:$owner,name:$name){pullRequests(states:OPEN,first:25,after:$after,orderBy:{field:UPDATED_AT,direction:DESC}){pageInfo{hasNextPage endCursor} nodes{
number title headRefName isDraft mergeable updatedAt reviewDecision url additions deletions changedFiles
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
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub name: String,
    pub repo: Option<String>,
    pub me: Option<String>,
    pub prs: Vec<Pr>,
    pub error: Option<String>,
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
    }
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
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
    unreachable!()
}

/// Read project `dir`'s open PRs. A failure is in `error`, never a panic.
pub fn collect(dir: &str) -> Snapshot {
    let mut s = Snapshot { name: crate::core::project::name_of(dir), ..Default::default() };
    let repo = match run::gh(dir, &["repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"]) {
        Ok(r) => r.trim().to_string(),
        Err(e) => {
            s.error = Some(format!("{dir} is not a GitHub repository: {e}"));
            return s;
        }
    };
    let me = match run::gh(dir, &["api", "user", "-q", ".login"]) {
        Ok(m) => m.trim().to_string(),
        Err(e) => {
            s.error = Some(format!("gh api user failed: {e}"));
            return s;
        }
    };
    match pulls(dir, &repo) {
        Ok(nodes) => s.prs = nodes.iter().map(|n| convert(n, &me)).collect(),
        Err(e) => s.error = Some(e.chars().take(300).collect()),
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
