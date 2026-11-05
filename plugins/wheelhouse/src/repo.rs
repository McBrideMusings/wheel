//! The Repo view's data: every local branch of the project with its worktree, its state against
//! the default branch and its upstream, the open PR on it, and whether it is dead (safe to delete).
//! The classification is admin's `ClassifyDead` (internal/git/dead.go): first match wins.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::core::{git, run};

pub mod cli;
pub mod serve;

const SEP: char = '\x1f';

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Status {
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

impl Status {
    fn dirty(&self) -> bool {
        self.staged + self.unstaged + self.untracked + self.conflicted > 0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Worktree {
    pub path: String,
    pub current: bool,
    pub primary: bool,
    pub status: Status,
    pub op: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pr {
    pub number: i64,
    pub title: String,
    pub draft: bool,
    pub url: String,
    pub review: Option<String>,
    pub mergeable: String,
    pub ci: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Branch {
    pub branch: String,
    pub ts: i64,
    pub subject: String,
    pub author: String,
    pub ahead: Option<i64>,
    pub behind: Option<i64>,
    pub upstream: Option<String>,
    pub upstream_gone: bool,
    pub remote: Option<String>,
    pub unpushed: Option<i64>,
    pub worktree: Option<Worktree>,
    pub pr: Option<Pr>,
    pub pr_merged: bool,
    pub local_only: i64,
    pub dead: Option<&'static str>,
    pub blocked: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub author: String,
    pub ts: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub name: String,
    pub top: String,
    pub default: Option<String>,
    pub head: Option<String>,
    pub branches: Vec<Branch>,
    pub stashes: Vec<(String, String, String)>,
    pub commits: Vec<Commit>,
    pub pr_error: Option<String>,
    pub error: Option<String>,
}

struct Wt {
    path: String,
    branch: Option<String>,
}

fn worktrees(top: &str) -> Vec<Wt> {
    let mut out = vec![];
    let mut cur: Option<Wt> = None;
    let text = git::quiet(top, &["worktree", "list", "--porcelain"]);
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            out.extend(cur.take());
        } else if let Some(p) = line.strip_prefix("worktree ") {
            cur = Some(Wt { path: p.into(), branch: None });
        } else if let (Some(b), Some(w)) = (line.strip_prefix("branch "), cur.as_mut()) {
            w.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).into());
        }
    }
    out
}

fn status_counts(path: &str) -> Status {
    let mut c = Status { staged: 0, unstaged: 0, untracked: 0, conflicted: 0 };
    for line in git::quiet(path, &["status", "--porcelain=v1"]).lines() {
        let b = line.as_bytes();
        if b.len() < 2 {
            continue;
        }
        let (x, y) = (b[0] as char, b[1] as char);
        if x == '?' && y == '?' {
            c.untracked += 1;
        } else if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
            c.conflicted += 1;
        } else {
            c.staged += !" ?".contains(x) as u32;
            c.unstaged += !" ?".contains(y) as u32;
        }
    }
    c
}

fn paused_op(path: &str) -> Option<String> {
    let gd = git::quiet(path, &["rev-parse", "--absolute-git-dir"]);
    let gd = gd.trim();
    [("rebase-merge", "rebase"), ("rebase-apply", "rebase"), ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert"), ("BISECT_LOG", "bisect")]
        .iter()
        .find(|(f, _)| Path::new(gd).join(f).exists())
        .map(|(_, op)| op.to_string())
}

/// Head branches of PRs GitHub merged: the only sight of a squash or rebase merge. Empty when
/// `gh` fails, so a branch then falls through to the git-only checks.
fn merged_pr_heads(top: &str) -> Vec<String> {
    let args = ["pr", "list", "--state", "merged", "--limit", "200", "--json", "headRefName"];
    match run::gh(top, &args).ok().and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok()) {
        Some(rows) => rows.iter().filter_map(|p| p["headRefName"].as_str().map(String::from)).collect(),
        None => vec![],
    }
}

fn open_prs(top: &str) -> Result<Vec<(String, Pr)>, String> {
    let args = ["pr", "list", "--state", "open", "--limit", "100", "--json",
        "number,headRefName,title,isDraft,reviewDecision,mergeable,url,statusCheckRollup"];
    let text = run::gh(top, &args).map_err(|e| e.lines().last().unwrap_or("gh failed").to_string())?;
    let rows: Vec<Value> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(rows.iter().map(|p| {
        let checks: Vec<Option<&str>> = p["statusCheckRollup"].as_array().into_iter().flatten()
            .map(|c| ["conclusion", "state", "status"].iter().filter_map(|k| c[k].as_str()).find(|s| !s.is_empty()))
            .collect();
        let ci = if checks.iter().any(|c| matches!(c, Some("FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED"))) {
            Some("fail")
        } else if checks.iter().any(|c| matches!(c, Some("IN_PROGRESS" | "QUEUED" | "PENDING") | None)) {
            Some("pending")
        } else if !checks.is_empty() {
            Some("pass")
        } else {
            None
        };
        let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
        (s("headRefName"), Pr { number: p["number"].as_i64().unwrap_or(0), title: s("title"),
            draft: p["isDraft"].as_bool().unwrap_or(false), url: s("url"),
            review: Some(s("reviewDecision")).filter(|r| !r.is_empty()), mergeable: s("mergeable"), ci })
    }).collect())
}

/// Commits on `name` that live on no origin ref. A failed read answers 1: never claim a branch is
/// empty of local work.
fn local_only(top: &str, name: &str) -> i64 {
    count_or_one(git::run(top, &["rev-list", "--count", &format!("refs/heads/{name}"), "--not", "--remotes=origin"]))
}

fn count_or_one(r: Result<String, String>) -> i64 {
    r.ok().and_then(|s| s.trim().parse().ok()).unwrap_or(1)
}

fn classify(top: &str, b: &Branch, merged_heads: &[String]) -> (Option<&'static str>, i64) {
    let lo = local_only(top, &b.branch);
    let merged_on_github = merged_heads.contains(&b.branch);
    if lo > 0 && !merged_on_github {
        return (None, lo);
    }
    let ahead_of_upstream = match (&b.upstream, b.upstream_gone) {
        (Some(u), false) => git::counts(top, &b.branch, u).map_or(false, |c| c.0 > 0),
        _ => false,
    };
    let dead = if b.upstream_gone {
        Some("upstream deleted")
    } else if merged_on_github && !ahead_of_upstream {
        Some("PR merged")
    } else if b.ahead == Some(0) {
        Some("merged")
    } else if b.upstream.is_some() && !b.upstream_gone && !ahead_of_upstream {
        Some("on origin")
    } else {
        None
    };
    (dead, lo)
}

/// Why a dead branch still cannot be deleted here.
fn blocked_by(b: &Branch) -> Option<String> {
    let w = b.worktree.as_ref()?;
    Some(if w.primary {
        "checked out in the primary checkout".into()
    } else if w.current {
        "this is the checkout the pane is in".into()
    } else if let Some(op) = &w.op {
        format!("a paused {op}")
    } else if w.status.dirty() {
        "uncommitted changes in its worktree".into()
    } else {
        return None;
    })
}

fn real(p: &str) -> String {
    std::fs::canonicalize(p).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| p.into())
}

fn branch_rows(top: &str, dir: &str, default: &Option<String>, wts: &[Wt], prs: &[(String, Pr)], merged_heads: &[String]) -> Vec<Branch> {
    let fmt = ["%(refname:short)", "%(upstream:short)", "%(upstream:track)", "%(committerdate:unix)", "%(subject)", "%(authorname)"]
        .join(&SEP.to_string());
    let remotes = git::quiet(top, &["for-each-ref", "--format=%(refname:short)", "refs/remotes/origin"]);
    let remotes: Vec<&str> = remotes.split_whitespace().collect();
    let list = git::quiet(top, &["for-each-ref", "--sort=-committerdate", &format!("--format={fmt}"), "refs/heads"]);
    let mut rows = vec![];
    for line in list.lines() {
        let f: Vec<&str> = line.split(SEP).collect();
        if f.len() < 6 {
            continue;
        }
        let name = f[0];
        if default.as_deref().map_or(false, |d| name == d.rsplit('/').next().unwrap_or(d)) {
            continue;
        }
        let (ahead, behind) = match default.as_deref().and_then(|d| git::counts(top, name, d)) {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        };
        let remote = Some(format!("origin/{name}")).filter(|r| remotes.contains(&r.as_str()));
        let worktree = wts.iter().position(|w| w.branch.as_deref() == Some(name)).map(|i| {
            let w = &wts[i];
            Worktree { path: w.path.clone(), current: real(&w.path) == real(dir), primary: i == 0,
                status: status_counts(&w.path), op: paused_op(&w.path) }
        });
        let mut b = Branch {
            branch: name.into(), ts: f[3].parse().unwrap_or(0), subject: f[4].into(), author: f[5].into(),
            ahead, behind, upstream: Some(f[1].to_string()).filter(|u| !u.is_empty()), upstream_gone: f[2].contains("gone"),
            unpushed: remote.as_deref().and_then(|r| git::counts(top, name, r)).map(|c| c.0), remote,
            worktree, pr: prs.iter().find(|(h, _)| h == name).map(|(_, p)| p.clone()),
            pr_merged: merged_heads.iter().any(|h| h == name), local_only: 0, dead: None, blocked: None,
        };
        let (dead, lo) = classify(top, &b, merged_heads);
        b.dead = dead;
        b.local_only = lo;
        b.blocked = if dead.is_some() { blocked_by(&b) } else { None };
        rows.push(b);
    }
    rows
}

/// Read project `dir`'s repo. A directory that is no git checkout answers with `error` set.
pub fn collect(dir: &str) -> Snapshot {
    let top = match git::toplevel(dir) {
        Ok(t) => t,
        Err(_) => return Snapshot { name: crate::core::project::name_of(dir), top: dir.into(), error: Some(format!("{dir} is not a git checkout")), ..Default::default() },
    };
    let default = git::default_ref(&top);
    let wts = worktrees(&top);
    let (prs, pr_error) = match open_prs(&top) {
        Ok(p) => (p, None),
        Err(e) => (vec![], Some(e)),
    };
    let stashes = git::quiet(&top, &["stash", "list", &format!("--format=%gd{SEP}%gs{SEP}%cr")])
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(SEP).collect();
            (f.len() == 3).then(|| (f[0].into(), f[1].into(), f[2].into()))
        })
        .collect();
    let commits = default.as_deref().map(|d| {
        git::quiet(&top, &["log", "-n", "15", &format!("--format=%h{SEP}%s{SEP}%an{SEP}%ct"), d])
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(SEP).collect();
                (f.len() == 4).then(|| Commit { sha: f[0].into(), subject: f[1].into(), author: f[2].into(), ts: f[3].parse().unwrap_or(0) })
            })
            .collect()
    });
    let merged = merged_pr_heads(&top);
    Snapshot {
        name: crate::core::project::name_of(&top), head: git::current_branch(&top),
        branches: branch_rows(&top, dir, &default, &wts, &prs, &merged), stashes,
        commits: commits.unwrap_or_default(), pr_error, default, top, error: None,
    }
}

/// Delete one local branch, and its worktree first when it has one, but only when the
/// classification calls it dead right now and nothing blocks it. A worktree goes through
/// `admin worktree rm -y`, so admin's checks and the project's pre_remove hook decide.
/// Returns what was done.
pub fn delete(dir: &str, branch: &str) -> Result<Vec<String>, String> {
    let top = git::toplevel(dir)?;
    let default = git::default_ref(&top);
    let wts = worktrees(&top);
    let rows = branch_rows(&top, dir, &default, &wts, &[], &merged_pr_heads(&top));
    let row = rows.iter().find(|b| b.branch == branch).ok_or("no such local branch")?;
    if row.dead.is_none() {
        return Err(format!("not deletable: {} commit(s) live on no origin ref", row.local_only));
    }
    if let Some(why) = &row.blocked {
        return Err(format!("blocked: {why}"));
    }
    let mut did = vec![];
    if let Some(w) = &row.worktree {
        let admin = find_admin().ok_or("admin not found on PATH; it removes worktrees")?;
        run::text(&admin, &top, &["worktree", "rm", branch, "-y"]).map_err(|e| if e.is_empty() { "admin worktree rm failed".into() } else { e })?;
        did.push(format!("removed worktree {}", w.path));
        if worktrees(&top).iter().any(|w| w.branch.as_deref() == Some(branch)) {
            return Err("the worktree is still there".into());
        }
    }
    if !git::quiet(&top, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}")]).trim().is_empty() {
        git::run(&top, &["branch", "-D", "--", branch])?;
        did.push(format!("deleted branch {branch}"));
    }
    Ok(did)
}

fn find_admin() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = std::env::var("PATH").unwrap_or_default();
    let all = format!("{path}:{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin");
    all.split(':').map(|d| Path::new(d).join("admin")).find(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(f: impl Fn(&mut Branch)) -> Branch {
        let mut b = Branch { branch: "x".into(), ts: 0, subject: String::new(), author: String::new(), ahead: Some(1), behind: Some(0),
            upstream: None, upstream_gone: false, remote: None, unpushed: None, worktree: None, pr: None, pr_merged: false,
            local_only: 0, dead: None, blocked: None };
        f(&mut b);
        b
    }

    fn wt(f: impl Fn(&mut Worktree)) -> Option<Worktree> {
        let mut w = Worktree { path: "/w".into(), current: false, primary: false,
            status: Status { staged: 0, unstaged: 0, untracked: 0, conflicted: 0 }, op: None };
        f(&mut w);
        Some(w)
    }

    #[test]
    fn what_blocks_a_dead_branch() {
        assert_eq!(blocked_by(&branch(|_| {})), None);
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|w| w.primary = true))).unwrap(), "checked out in the primary checkout");
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|w| w.current = true))).unwrap(), "this is the checkout the pane is in");
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|w| w.op = Some("rebase".into())))).unwrap(), "a paused rebase");
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|w| w.status.untracked = 2))).unwrap(), "uncommitted changes in its worktree");
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|_| {}))), None);
    }

    #[test]
    fn a_failed_count_is_never_zero() {
        assert_eq!(count_or_one(Err("x".into())), 1);
        assert_eq!(count_or_one(Ok("0\n".into())), 0);
    }
}
