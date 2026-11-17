//! One PR in full: what GitHub holds about it (description, images, comments, reviews, checks,
//! commits, files) and what a pane's buttons do with it: merge, check the branch out in the
//! project's checkout, open it in a worktree with an agent, and the buttons `[prs.buttons.<name>]`
//! in the machine's and the project's wheel.toml write.

use std::path::PathBuf;

use serde_json::Value;

use super::*;
use crate::core::agents::Agents;
use crate::core::project::Origin;

const QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){
number title body state isDraft merged mergeable mergeStateStatus headRefName baseRefName isCrossRepository createdAt updatedAt url
additions deletions changedFiles reviewDecision
author{login}
assignees(first:10){nodes{login}}
labels(first:20){nodes{name}}
reviewRequests(first:10){nodes{requestedReviewer{... on User{login} ... on Team{slug}}}}
latestReviews(first:20){nodes{author{login} state}}
reviews(last:30){nodes{author{login} state submittedAt body}}
comments(last:50){nodes{author{login} createdAt body}}
history:commits(last:30){totalCount nodes{commit{abbreviatedOid messageHeadline committedDate}}}
files(first:60){totalCount nodes{path additions deletions}}
checks:commits(last:1){nodes{commit{statusCheckRollup{state contexts(first:40){nodes{__typename ... on CheckRun{name status conclusion} ... on StatusContext{context state}}}}}}}
}}}";

/// The PR `number` as GitHub's GraphQL node.
pub(super) fn fetch(dir: &str, repo: &str, number: i64) -> Result<Value, String> {
    let (owner, name) = repo.split_once('/').ok_or("no owner/name")?;
    let (q, o, n, num) = (format!("query={QUERY}"), format!("owner={owner}"), format!("name={name}"), format!("number={number}"));
    let out = run::gh(dir, &["api", "graphql", "-f", &q, "-F", &o, "-F", &n, "-F", &num])?;
    let v: Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    let pr = v["data"]["repository"]["pullRequest"].clone();
    if pr.is_null() {
        return Err(format!("no PR #{number}"));
    }
    Ok(pr)
}

/// A button on a PR (`[prs.buttons.<name>]`) or a branch of the Repo page (`[repo.buttons.<name>]`): a shell
/// command the machine's or the project's wheel.toml writes, run in the project.
#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub name: String,
    pub label: Option<String>,
    /// A PR button's filter: `any`, a bucket (`mine`, `needs-me`, `waiting`, `other`), `fix`, `draft` or `ready`.
    /// Read for `[prs.buttons]` only: a Repo button's is always empty.
    pub on: Vec<String>,
    pub run: String,
    /// Run it with `PR_WORKTREE` set to a worktree on the PR's or branch's branch, made first when none exists.
    pub worktree: bool,
}

/// The buttons under `[prs.buttons]` in `files`, the machine's first.
pub fn buttons_of(files: &[crate::core::buttons::File]) -> Vec<Button> {
    buttons_in(files, "prs")
}

/// The buttons under `[<view>.buttons]` in `files`, the machine's first.
pub fn buttons_in(files: &[crate::core::buttons::File], view: &str) -> Vec<Button> {
    crate::core::buttons::named(files, &[view, "buttons"])
        .into_iter()
        .map(|(name, s)| Button {
            name,
            label: s.get("label").and_then(|v| v.as_str()).map(String::from),
            on: s.get("on").filter(|_| view == "prs").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
            run: s.get("run").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            worktree: s.get("worktree").and_then(|v| v.as_bool()).unwrap_or(false),
        })
        .collect()
}

/// A button's label: its `label`, else its name.
pub fn label(b: &Button) -> &str {
    b.label.as_deref().unwrap_or(&b.name)
}

/// A description or comment's text with its images taken out: the text, and each image's URL.
pub fn split_images(md: &str) -> (String, Vec<String>) {
    let (mut text, mut urls) = (String::new(), vec![]);
    let mut rest = md;
    while !rest.is_empty() {
        let md_img = rest.find("![");
        let html_img = rest.find("<img");
        let (at, html) = match (md_img, html_img) {
            (Some(a), Some(b)) if b < a => (b, true),
            (Some(a), _) => (a, false),
            (None, Some(b)) => (b, true),
            (None, None) => break,
        };
        text.push_str(&rest[..at]);
        let tail = &rest[at..];
        let parsed = if html {
            tail.find('>').and_then(|end| {
                let tag = &tail[..=end];
                let src = tag.find("src=\"").map(|i| &tag[i + 5..])?;
                Some((src[..src.find('"')?].to_string(), end + 1))
            })
        } else {
            tail.find("](").and_then(|mid| {
                let after = &tail[mid + 2..];
                let close = after.find(')')?;
                let url = after[..close].split_whitespace().next()?.to_string();
                Some((url, mid + 2 + close + 1))
            })
        };
        match parsed {
            Some((url, used)) => {
                if url.starts_with("http") {
                    urls.push(url);
                }
                rest = &tail[used..];
            }
            None => {
                text.push_str(&tail[..2]);
                rest = &tail[2..];
            }
        }
    }
    text.push_str(rest);
    (text.trim().to_string(), urls)
}

pub(super) fn names(v: &Value, key: &str, f: impl Fn(&Value) -> Option<String>) -> Vec<String> {
    nodes(v, key).iter().filter_map(f).collect()
}

/// Download the repo-hosted images the PR's description and comments show, so the modal can draw them.
pub fn prefetch_images(dir: &str, n: &Value) {
    let mut texts: Vec<&str> = vec![n["body"].as_str().unwrap_or("")];
    texts.extend(nodes(n, "comments").iter().chain(nodes(n, "reviews")).filter_map(|c| c["body"].as_str()));
    for t in texts {
        super::markdown::prefetch(dir, t);
    }
}

/// The agents Wheel can run, by name; empty when `wheel status` cannot say.
pub fn agent_names(dir: &str) -> Vec<String> {
    let Ok(v) = run::json("wheel", dir, &["status", "--json"]) else { return vec![] };
    v["agents"].as_array().into_iter().flatten().filter(|a| a["found"] == true).filter_map(|a| a["name"].as_str().map(String::from)).collect()
}

// ---------------------------------------------------------------- the buttons

fn fail<T>(e: impl Into<String>) -> Result<T, String> {
    Err(e.into())
}

/// `gh pr merge` with `method`. The branch is kept.
pub fn merge(dir: &str, number: i64, method: &str) -> Result<(), String> {
    let flag = match method {
        "squash" => "--squash",
        "merge" => "--merge",
        "rebase" => "--rebase",
        other => return fail(format!("no merge method {other}")),
    };
    run::gh(dir, &["pr", "merge", &number.to_string(), flag]).map(|_| ())
}

/// Take a draft out of draft, or put a PR back into draft.
pub fn set_draft(dir: &str, number: i64, draft: bool) -> Result<(), String> {
    let n = number.to_string();
    let mut args = vec!["pr", "ready", n.as_str()];
    if draft {
        args.push("--undo");
    }
    run::gh(dir, &args).map(|_| ())
}

/// Close a PR without merging it.
pub fn close(dir: &str, number: i64) -> Result<(), String> {
    run::gh(dir, &["pr", "close", &number.to_string()]).map(|_| ())
}

/// Switch the project's own checkout to the PR's branch (`gh pr checkout`), which refuses when
/// the working tree has changes it would lose.
pub fn checkout(dir: &str, number: i64) -> Result<(), String> {
    run::gh(dir, &["pr", "checkout", &number.to_string()]).map(|_| ())
}

/// The path of a worktree on `branch`, a PR's or a branch with none: the one that holds it already,
/// fast-forwarded to the remote's branch when it can be, else a new one at `~/.worktrees/<project>/<branch>`,
/// made on the local branch, else from the branch on the remote that points at `repo`. `fork_pr` is the number of
/// a PR from a fork: its head ref is fetched, then `gh pr checkout` runs inside the worktree, which sets up the
/// fork's remote.
pub fn worktree(dir: &str, repo: &str, branch: &str, fork_pr: Option<i64>) -> Result<String, String> {
    let remote = crate::core::git::remote_of_repo(dir, repo);
    let list = crate::core::git::run(dir, &["worktree", "list", "--porcelain"])?;
    let want = format!("branch refs/heads/{branch}");
    let mut path = None;
    let mut held = None;
    for line in list.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
        } else if line == want {
            held = path.clone();
        }
    }
    if let Some(p) = held {
        if let Ok(remote) = &remote {
            let _ = crate::core::git::run(&p, &["merge", "-q", "--ff-only", &format!("{remote}/{branch}")]);
        }
        return Ok(p);
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    let wt = format!("{home}/.worktrees/{}/{}", crate::core::project::name_of(dir), branch.replace('/', "-"));
    let local = !crate::core::git::quiet(dir, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}")]).trim().is_empty();
    if let Some(number) = fork_pr {
        let remote = remote?;
        let n = number.to_string();
        let head = format!("pull/{n}/head");
        crate::core::git::run(dir, &["fetch", "-q", &remote, &head])?;
        crate::core::git::run(dir, &["worktree", "add", "-q", "--detach", &wt, "FETCH_HEAD"])?;
        run::gh(&wt, &["pr", "checkout", &n])?;
    } else if local {
        crate::core::git::run(dir, &["worktree", "add", "-q", &wt, branch])?;
    } else {
        let remote = remote?;
        crate::core::git::run(dir, &["fetch", "-q", &remote, branch])?;
        crate::core::git::run(dir, &["worktree", "add", "-q", "-b", branch, &wt, &format!("{remote}/{branch}")])?;
    }
    // The project's environment file comes along, as the Beads buttons do it.
    let envrc = PathBuf::from(dir).join(".envrc");
    if envrc.is_file() && !PathBuf::from(&wt).join(".envrc").exists() {
        let _ = std::fs::copy(&envrc, PathBuf::from(&wt).join(".envrc"));
        let _ = run::text("direnv", &wt, &["allow", &wt]);
    }
    Ok(wt)
}

/// A new terminal pane beside the asking pane.
pub fn terminal(dir: &str, pane: &str) -> Result<String, String> {
    if pane == "default" {
        return fail("opening a pane needs the asking pane");
    }
    let out = run::text("wheel", dir, &["pane", "split", pane, "right"])?;
    let new = out.split_whitespace().next().ok_or("wheel pane split printed no pane id")?.to_string();
    run::text("wheel", dir, &["pane", "start", &new, "terminal"])?;
    Ok(new)
}

/// What an agent is told when it opens a PR in a worktree.
pub fn brief(pr: &Pr, wt: &str) -> String {
    format!(
        "Work on PR #{} \"{}\" ({}), branch {}. Work only in its worktree, {wt}: this pane's directory is the main checkout, so use git -C {wt} and absolute paths under {wt}.",
        pr.number, pr.title, pr.url, pr.branch
    )
}

/// The space whose folder is `dir`, made in the background when none is open; its first pane's id.
fn space_pane(origin: &Origin, dir: &str) -> Result<String, String> {
    let real = std::fs::canonicalize(dir).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| dir.to_string());
    let spaces = run::json("wheel", &origin.dir, &["space", "ls", "--json"])?;
    let held = spaces.as_array().into_iter().flatten().find(|s| s["cwd"].as_str().map(|c| c.trim_end_matches('/')) == Some(real.as_str()));
    let panes = match held {
        Some(s) => s["panes"].clone(),
        None => {
            let mut args = vec!["space", "new", real.as_str(), "--background"];
            if origin.pane != "default" {
                args.extend(["--pane", origin.pane.as_str()]);
            }
            let id = run::text("wheel", &origin.dir, &args)?.split_whitespace().next().ok_or("wheel space new printed no space id")?.to_string();
            run::json("wheel", &origin.dir, &["space", "ls", "--json"])?.as_array().into_iter().flatten().find(|s| s["id"] == id.as_str()).map(|s| s["panes"].clone()).unwrap_or(Value::Null)
        }
    };
    panes.as_array().and_then(|p| p.first()).and_then(|p| p.as_str()).map(String::from).ok_or_else(|| format!("no pane in the space on {real}"))
}

/// An agent in worktree `wt`, placed by `open`: a new pane beside the asking pane (`pane`), or the first pane of the
/// space on the worktree (`space`), where the agent's own folder is the worktree. Answers the pane's id.
pub fn place_agent(origin: &Origin, pr: &Pr, wt: &str, agent: &str, open: &str) -> Result<String, String> {
    if open != "space" {
        return start_agent(origin, pr, wt, agent);
    }
    let pane = space_pane(origin, wt)?;
    let mut start = vec!["pane", "start", pane.as_str(), "agent"];
    if agent != "default" {
        start.extend(["--agent", agent]);
    }
    run::text("wheel", &origin.dir, &start)?;
    let brief = format!("Work on PR #{} \"{}\" ({}), branch {}. This pane's directory is its worktree.", pr.number, pr.title, pr.url, pr.branch);
    run::text_or_exit("wheel", &origin.dir, &["agent", "send", &pane, &brief, "--timeout", "1"], &[3])?;
    Ok(pane)
}

/// An agent in a new pane, told to work in worktree `wt`. `agent` is `default` for the setting's choice.
pub fn start_agent(origin: &Origin, pr: &Pr, wt: &str, agent: &str) -> Result<String, String> {
    Agents::new("wheel").start(origin, &brief(pr, wt), Some(agent).filter(|a| *a != "default"), None).or_else(fail)
}

/// What a button runs for: a PR, or a branch that may have none.
pub struct Target {
    pub number: Option<i64>,
    pub title: String,
    pub branch: String,
    pub url: String,
}

impl Target {
    pub fn of_pr(pr: &Pr) -> Target {
        Target { number: Some(pr.number), title: pr.title.clone(), branch: pr.branch.clone(), url: pr.url.clone() }
    }
}

/// Run button `b` of `view` (`prs` or `repo`) for `target`, in a worktree first when the button asks for one. `fork_pr`
/// is the target's PR number when that PR comes from a fork.
pub fn run_button(origin: &Origin, view: &str, repo: &str, target: &Target, fork_pr: Option<i64>, b: &Button) -> Result<(), String> {
    let wt = if b.worktree { Some(worktree(&origin.dir, repo, &target.branch, fork_pr)?) } else { None };
    let number = target.number.map(|n| n.to_string()).unwrap_or_default();
    let (title, branch, url) = (target.title.clone(), target.branch.clone(), target.url.clone());
    let mut env = vec![("PR_NUMBER", number.as_str()), ("PR_TITLE", title.as_str()), ("PR_BRANCH", branch.as_str()), ("PR_URL", url.as_str()),
        ("PR_REPO", repo), ("PR_PATH", origin.dir.as_str())];
    if let Some(w) = &wt {
        env.push(("PR_WORKTREE", w.as_str()));
    }
    let log = std::env::temp_dir().join(format!("wheel-{view}-{}-{}.log", if view == "repo" { crate::beads::sane(&branch) } else { number.clone() }, crate::beads::sane(&b.name)));
    run::button(label(b), &b.run, &origin.dir, &origin.pane, &env, &log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_taken_out_of_markdown_and_html() {
        let (text, urls) = split_images("Look ![shot](https://x/a.png) and <img width=\"3\" src=\"https://x/b.png\" /> done ![no](rel.png)");
        assert_eq!(urls, vec!["https://x/a.png", "https://x/b.png"]);
        assert_eq!(text, "Look  and  done");
    }

    #[test]
    fn a_button_is_read_from_wheel_toml_and_chosen_by_its_on_list() {
        let (root, machine, dir) = crate::core::buttons::tests::fixture("prs", "", "[prs.buttons.fix]\nlabel = \"Make mergeable\"\non = [\"fix\"]\nworktree = true\nrun = \"true\"\n[prs.buttons.all]\non = [\"any\"]\nrun = \"true\"\n");
        let (files, problems) = crate::core::buttons::files_from(&machine, dir.to_str().unwrap());
        assert!(problems.is_empty());
        let b = buttons_of(&files);
        assert_eq!(b.len(), 2);
        let fix = b.iter().find(|x| x.name == "fix").unwrap();
        assert!(fix.worktree && label(fix) == "Make mergeable");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_machines_pr_buttons_apply_in_a_project_that_writes_none_and_a_project_overrides_by_name() {
        let machine = "[prs.buttons.review]\nlabel = \"Review\"\non = [\"needs-me\"]\nrun = \"/bin/bash ~/.wheel/lib/pr-agent.sh review\"\n[prs.buttons.mergeable]\nlabel = \"Make mergeable\"\non = [\"fix\"]\nworktree = true\nrun = \"/bin/bash ~/.wheel/lib/pr-agent.sh mergeable\"\n";
        let (root, machine, dir) = crate::core::buttons::tests::fixture("prs-machine", machine, "");
        let (files, _) = crate::core::buttons::files_from(&machine, dir.to_str().unwrap());
        assert_eq!(buttons_of(&files).iter().map(label).collect::<Vec<_>>(), ["Review", "Make mergeable"], "a project with no wheel.toml gets the machine's buttons");
        std::fs::write(dir.join("wheel.toml"), "[prs.buttons.review]\nenabled = false\n[prs.buttons.mergeable]\non = [\"any\"]\nrun = \"make fix\"\n").unwrap();
        let (files, _) = crate::core::buttons::files_from(&machine, dir.to_str().unwrap());
        let b = buttons_of(&files);
        assert_eq!(b.len(), 1, "review is dropped");
        assert_eq!((b[0].name.as_str(), label(&b[0]), b[0].on.clone(), b[0].run.as_str(), b[0].worktree), ("mergeable", "mergeable", vec!["any".to_string()], "make fix", false),
            "the project's table replaces the machine's whole: no label, worktree or on carry over");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_worktree_is_made_on_the_prs_branch_once_and_found_again() {
        let root = std::env::temp_dir().join(format!("wheelhouse-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let sh = |dir: &std::path::Path, args: &[&str]| crate::core::git::run(dir.to_str().unwrap(), args).unwrap();
        let (bare, work) = (root.join("Owner").join("Repo.git"), root.join("proj"));
        sh(&root, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        sh(&root, &["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()]);
        sh(&work, &["remote", "rename", "origin", "upstream"]);
        for a in [["config", "user.email", "t@t"], ["config", "user.name", "t"]] {
            sh(&work, &a);
        }
        sh(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        sh(&work, &["push", "-q", "upstream", "HEAD:refs/heads/feat/x"]);
        let home = root.join("home");
        std::env::set_var("HOME", &home);
        let dir = work.to_str().unwrap();
        let a = worktree(dir, "owner/repo", "feat/x", None).unwrap();
        assert_eq!(a, format!("{}/.worktrees/proj/feat-x", home.display()));
        assert_eq!(crate::core::git::current_branch(&a).as_deref(), Some("feat/x"));
        sh(&work, &["commit", "-q", "--allow-empty", "-m", "two"]);
        sh(&work, &["push", "-q", "upstream", "HEAD:refs/heads/feat/x"]);
        let again = worktree(dir, "owner/repo", "feat/x", None).unwrap();
        assert_eq!(std::fs::canonicalize(&again).unwrap(), std::fs::canonicalize(&a).unwrap(), "the second call finds the first");
        assert_eq!(sh(std::path::Path::new(&again),&["log", "-1", "--format=%s"]).trim(), "two", "the second call fast-forwards to the remote's branch");
        sh(&work, &["branch", "local-only"]);
        let l = worktree(dir, "owner/repo", "local-only", None).unwrap();
        assert_eq!(crate::core::git::current_branch(&l).as_deref(), Some("local-only"), "a branch with no PR and no remote branch gets a worktree on the local branch");
        assert!(worktree(dir, "owner/repo", "nowhere", None).is_err(), "a branch neither local nor on the remote has none to make");
        let _ = std::fs::remove_dir_all(&root);
    }
}
