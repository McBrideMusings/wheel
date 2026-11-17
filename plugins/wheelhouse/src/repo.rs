//! The Repo view's data: every local branch of the project with its worktree, its state against
//! the default branch and its upstream (in `git branch -vv`'s words), its PR (the open one, else the
//! newest merged or closed one), whether it is merged, and whether it is dead (safe to delete).
//! The classification is admin's `ClassifyDead` (internal/git/dead.go): first match wins.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::core::{git, run};

pub mod cli;
pub mod feed;

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
    /// GitHub's state: `OPEN`, `MERGED` or `CLOSED`.
    pub state: String,
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
    /// What `git branch -vv` prints after the upstream: `ahead 5`, `behind 3`, `ahead 2, behind 3`,
    /// `gone` (deleted on origin), or empty when up to date or there is no upstream.
    pub track: String,
    pub upstream_gone: bool,
    pub remote: Option<String>,
    pub unpushed: Option<i64>,
    pub worktree: Option<Worktree>,
    pub pr: Option<Pr>,
    pub pr_merged: bool,
    /// `merged` (its tip is in the default branch, as `git branch --merged` says) or `squash-merged`
    /// (GitHub merged a PR from it, yet its tip is not in the default branch).
    pub merged: Option<&'static str>,
    pub local_only: i64,
    /// Why it is safe to delete: `gone`, `squash-merged`, `merged` or `pushed`.
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
    /// When origin was last fetched (`FETCH_HEAD`'s time), so the pane can say how old the upstream counts are.
    pub fetched: Option<i64>,
    pub pr_error: Option<String>,
    pub error: Option<String>,
    /// The project's `[repo.list]` buttons, run over the listed branches.
    #[serde(skip)]
    pub scripts: Vec<crate::core::buttons::Script>,
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

/// PRs GitHub merged or closed, newest first, each with its head branch. A merged one is the only
/// sight of a squash or rebase merge. Empty when `gh` fails, so a branch then falls through to the
/// git-only checks.
fn closed_prs(top: &str) -> Vec<(String, Pr)> {
    let args = ["pr", "list", "--state", "closed", "--limit", "200", "--json", "number,headRefName,title,state,isDraft,url"];
    let rows = run::gh(top, &args).ok().and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok()).unwrap_or_default();
    rows.iter().map(|p| {
        let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
        (s("headRefName"), Pr { number: p["number"].as_i64().unwrap_or(0), title: s("title"), state: s("state"),
            draft: p["isDraft"].as_bool().unwrap_or(false), url: s("url"), review: None, mergeable: String::new(), ci: None })
    }).collect()
}

fn merged_heads(closed: &[(String, Pr)]) -> Vec<String> {
    closed.iter().filter(|(_, p)| p.state == "MERGED").map(|(h, _)| h.clone()).collect()
}

/// `git fetch --prune origin` in `dir`, so origin's refs (and with them every branch's `track`) say
/// what origin holds now: a branch deleted there reads `gone`. Nothing to do without an origin remote.
/// Never asks for a password: a fetch that needs one fails. A fetch still running after `FETCH_LIMIT`
/// (a stalled connection) is killed and is an error, so the next tick fetches again.
pub fn fetch(dir: &str) -> Result<(), String> {
    let top = git::toplevel(dir)?;
    if git::quiet(&top, &["remote"]).lines().all(|r| r.trim() != "origin") {
        return Ok(());
    }
    let child = std::process::Command::new("/usr/bin/git")
        .args(["fetch", "--prune", "--quiet", "origin"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(&top)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("git fetch: {e}"))?;
    wait_limited(child, FETCH_LIMIT).map_err(|e| format!("git fetch --prune origin in {top}: {e}"))
}

const FETCH_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Wait for `child` up to `limit`: `Ok` when it exits with 0, its stderr when it fails, and a kill
/// with an error naming the limit when it is still running.
fn wait_limited(mut child: std::process::Child, limit: std::time::Duration) -> Result<(), String> {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) if s.success() => return Ok(()),
            Some(_) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                return Err(err.trim().to_string());
            }
            None if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("still running after {} s, stopped", limit.as_secs()));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

/// When origin was last fetched: the time of the shared `FETCH_HEAD`.
fn fetched_at(top: &str) -> Option<i64> {
    let (_, common) = git::dirs(top)?;
    let t = std::fs::metadata(Path::new(&common).join("FETCH_HEAD")).ok()?.modified().ok()?;
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

/// One line naming why `gh` failed, from its stderr: a signed-out `gh` says so in its own words, any
/// other failure is gh's first non-empty line.
fn gh_problem(stderr: &str) -> String {
    if stderr.contains("To get started with GitHub CLI") || stderr.contains("not logged into") {
        return "gh is not signed in: open PRs and checks are not shown".into();
    }
    stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("gh failed").to_string()
}

fn open_prs(top: &str) -> Result<Vec<(String, Pr)>, String> {
    let args = ["pr", "list", "--state", "open", "--limit", "100", "--json",
        "number,headRefName,title,isDraft,reviewDecision,mergeable,url,statusCheckRollup"];
    let text = run::gh(top, &args).map_err(|e| gh_problem(&e))?;
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
        (s("headRefName"), Pr { number: p["number"].as_i64().unwrap_or(0), title: s("title"), state: "OPEN".into(),
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
        Some("gone")
    } else if b.ahead == Some(0) {
        Some("merged")
    } else if merged_on_github && !ahead_of_upstream {
        Some("squash-merged")
    } else if b.upstream.is_some() && !b.upstream_gone && !ahead_of_upstream {
        Some("pushed")
    } else {
        None
    };
    (dead, lo)
}

/// `merged` when no commit of the branch is missing from the default branch, `squash-merged` when
/// GitHub merged a PR from it anyway.
fn merged_label(ahead: Option<i64>, pr_merged: bool) -> Option<&'static str> {
    if ahead == Some(0) {
        Some("merged")
    } else if pr_merged {
        Some("squash-merged")
    } else {
        None
    }
}

/// Why a dead branch still cannot be deleted here.
fn blocked_by(b: &Branch) -> Option<String> {
    let w = b.worktree.as_ref()?;
    Some(if w.primary {
        "primary checkout".into()
    } else if w.current {
        "this pane's checkout".into()
    } else if let Some(op) = &w.op {
        format!("paused {op}")
    } else if w.status.dirty() {
        "uncommitted changes".into()
    } else {
        return None;
    })
}

fn real(p: &str) -> String {
    std::fs::canonicalize(p).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| p.into())
}

fn branch_rows(top: &str, dir: &str, default: &Option<String>, wts: &[Wt], prs: &[(String, Pr)], closed: &[(String, Pr)]) -> Vec<Branch> {
    let merged_heads = merged_heads(closed);
    let fmt = ["%(refname:short)", "%(upstream:short)", "%(upstream:track,nobracket)", "%(committerdate:unix)", "%(subject)", "%(authorname)"]
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
            ahead, behind, upstream: Some(f[1].to_string()).filter(|u| !u.is_empty()), track: f[2].to_string(), upstream_gone: f[2] == "gone",
            unpushed: remote.as_deref().and_then(|r| git::counts(top, name, r)).map(|c| c.0), remote,
            worktree, pr: prs.iter().chain(closed).find(|(h, _)| h == name).map(|(_, p)| p.clone()),
            pr_merged: merged_heads.iter().any(|h| h == name), merged: None, local_only: 0, dead: None, blocked: None,
        };
        b.merged = merged_label(b.ahead, b.pr_merged);
        let (dead, lo) = classify(top, &b, &merged_heads);
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
    let closed = closed_prs(&top);
    let (scripts, problems) = crate::core::buttons::scripts(dir, "repo");
    let pr_error = pr_error.or(problems.into_iter().next());
    Snapshot {
        scripts, fetched: fetched_at(&top),
        name: crate::core::project::name_of(&top), head: git::current_branch(&top),
        branches: branch_rows(&top, dir, &default, &wts, &prs, &closed), stashes,
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
    let rows = branch_rows(&top, dir, &default, &wts, &[], &closed_prs(&top));
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
            upstream: None, track: String::new(), upstream_gone: false, remote: None, unpushed: None, worktree: None, pr: None,
            pr_merged: false, merged: None, local_only: 0, dead: None, blocked: None };
        f(&mut b);
        b
    }

    fn wt(f: impl Fn(&mut Worktree)) -> Option<Worktree> {
        let mut w = Worktree { path: "/w".into(), current: false, primary: false,
            status: Status { staged: 0, unstaged: 0, untracked: 0, conflicted: 0 }, op: None };
        f(&mut w);
        Some(w)
    }

    fn sh(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("/usr/bin/git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn a_branch_deleted_on_origin_reads_gone_after_fetch() {
        let root = std::env::temp_dir().join(format!("wheelhouse-repo-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        sh(&root, &["init", "-q", "--bare", "origin.git"]);
        sh(&root, &["clone", "-q", "origin.git", "work"]);
        let work = root.join("work");
        sh(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        sh(&work, &["push", "-q", "-u", "origin", "main"]);
        sh(&work, &["checkout", "-q", "-b", "feature"]);
        sh(&work, &["commit", "-q", "--allow-empty", "-m", "feature work"]);
        sh(&work, &["push", "-q", "-u", "origin", "feature"]);
        sh(&work, &["checkout", "-q", "-b", "unmerged", "main"]);
        sh(&work, &["commit", "-q", "--allow-empty", "-m", "unmerged work"]);
        sh(&work, &["push", "-q", "-u", "origin", "unmerged"]);
        sh(&work, &["checkout", "-q", "main"]);
        sh(&work, &["merge", "-q", "--ff-only", "feature"]);
        sh(&work, &["push", "-q", "origin", "main"]);
        let dir = work.to_str().unwrap();
        let row = |name: &str| collect(dir).branches.into_iter().find(|b| b.branch == name).unwrap();
        let before = row("feature");
        assert_eq!((before.track.as_str(), before.dead, before.merged), ("", Some("merged"), Some("merged")));
        sh(&root.join("origin.git"), &["branch", "-q", "-D", "feature", "unmerged"]);
        assert_eq!(row("feature").track, "", "without a fetch the local origin/feature still stands");
        fetch(dir).unwrap();
        let merged = row("feature");
        assert_eq!((merged.track.as_str(), merged.upstream_gone, merged.dead), ("gone", true, Some("gone")));
        let unmerged = row("unmerged");
        assert_eq!((unmerged.track.as_str(), unmerged.dead, unmerged.local_only), ("gone", None, 1),
            "its one commit now lives on no origin ref, so it is not safe to delete");
        assert!(collect(dir).fetched.is_some());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn what_blocks_a_dead_branch() {
        assert_eq!(blocked_by(&branch(|_| {})), None);
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|w| w.primary = true))).unwrap(), "primary checkout");
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|w| w.current = true))).unwrap(), "this pane's checkout");
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|w| w.op = Some("rebase".into())))).unwrap(), "paused rebase");
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|w| w.status.untracked = 2))).unwrap(), "uncommitted changes");
        assert_eq!(blocked_by(&branch(|b| b.worktree = wt(|_| {}))), None);
    }

    #[test]
    fn merged_is_in_the_default_branch_and_squash_merged_is_a_merged_pr_that_is_not() {
        assert_eq!(merged_label(Some(0), false), Some("merged"));
        assert_eq!(merged_label(Some(0), true), Some("merged"));
        assert_eq!(merged_label(Some(3), true), Some("squash-merged"));
        assert_eq!(merged_label(Some(3), false), None);
        assert_eq!(merged_label(None, false), None);
    }

    #[test]
    fn a_process_past_its_limit_is_stopped_and_named() {
        let cmd = |sh: &str| std::process::Command::new("/bin/sh").args(["-c", sh])
            .stderr(std::process::Stdio::piped()).spawn().unwrap();
        let limit = std::time::Duration::from_millis(300);
        assert_eq!(wait_limited(cmd("exit 0"), limit), Ok(()));
        assert_eq!(wait_limited(cmd("echo boom >&2; exit 1"), limit), Err("boom".into()));
        let started = std::time::Instant::now();
        assert_eq!(wait_limited(cmd("sleep 30"), limit), Err("still running after 0 s, stopped".into()));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn gh_problem_names_the_cause() {
        let signed_out = "To get started with GitHub CLI, please run:  gh auth login\nAlternatively, populate the GH_TOKEN environment variable with a GitHub API authentication token.\n";
        assert_eq!(gh_problem(signed_out), "gh is not signed in: open PRs and checks are not shown");
        let no_remote = "none of the git remotes configured for this repository point to a known GitHub host. To tell gh about a new GitHub host, please use `gh auth login`\n";
        assert_eq!(gh_problem(no_remote), no_remote.trim());
        assert_eq!(gh_problem(""), "gh failed");
    }

    #[test]
    fn a_failed_count_is_never_zero() {
        assert_eq!(count_or_one(Err("x".into())), 1);
        assert_eq!(count_or_one(Ok("0\n".into())), 0);
    }
}
