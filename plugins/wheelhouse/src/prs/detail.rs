//! One PR in full: what GitHub holds about it (description, images, comments, reviews, checks,
//! commits, files), the modal that shows it, and what the modal's buttons do: merge, check the
//! branch out in the project's checkout, open it in a worktree with an agent, and the project's own
//! buttons from `[prs.buttons.<name>]` in its wheel.toml.

use std::path::PathBuf;

use serde_json::{json, Map, Value};

use super::*;
use crate::core::agents::Agents;
use crate::core::page::{ago, now, Page};
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

/// A project button: a shell command the project writes in its wheel.toml.
#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub name: String,
    pub label: Option<String>,
    /// `any`, a bucket (`mine`, `needs-me`, `waiting`, `other`), `fix`, `draft` or `ready`.
    pub on: Vec<String>,
    pub run: String,
    /// Run it with `PR_WORKTREE` set to a worktree on the PR's branch, made first when none exists.
    pub worktree: bool,
}

/// The buttons under `[prs.buttons]` in `<dir>/wheel.toml`, and what could not be read.
pub fn buttons(dir: &str) -> (Vec<Button>, Option<String>) {
    let file = PathBuf::from(dir).join("wheel.toml");
    let Ok(text) = std::fs::read_to_string(&file) else { return (vec![], None) };
    let doc: toml_edit::DocumentMut = match text.parse() {
        Ok(t) => t,
        Err(e) => return (vec![], Some(format!("{}: {e}", file.display()))),
    };
    let mut out = vec![];
    if let Some(t) = doc.get("prs").and_then(|b| b.get("buttons")).and_then(|b| b.as_table_like()) {
        for (name, s) in t.iter() {
            out.push(Button {
                name: name.to_string(),
                label: s.get("label").and_then(|v| v.as_str()).map(String::from),
                on: s.get("on").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
                run: s.get("run").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                worktree: s.get("worktree").and_then(|v| v.as_bool()).unwrap_or(false),
            });
        }
    }
    (out, None)
}

pub fn wants(b: &Button, p: &Pr) -> bool {
    b.on.iter().any(|k| match k.as_str() {
        "any" => true,
        "fix" => p.fix,
        "draft" => p.draft,
        "ready" => !p.draft && p.mergeable == "MERGEABLE",
        bucket => p.bucket == bucket,
    })
}

/// A button's label: its `label`, else its name.
pub fn label(b: &Button) -> &str {
    b.label.as_deref().unwrap_or(&b.name)
}

// ---------------------------------------------------------------- the modal

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

/// A block of markdown drawn as the blocks `super::markdown` makes of it.
fn body(p: &mut Page, id: &str, md: &str) -> Vec<String> {
    super::markdown::render(p, id, md)
}

fn fact(p: &mut Page, n: &str, name: &str, value: String) -> String {
    let l = p.label(&format!("pr.f.{n}.k"), name, json!({ "tone": "muted", "size": 12 }));
    p.boxed(&format!("pr.f.{n}"), "h", vec![l, value], json!({ "spacing": 12, "widths": [84, "flex"], "align": "firstTextBaseline" }))
}

fn heading(p: &mut Page, id: &str, title: &str, count: usize) -> String {
    p.runs(id, json!([{ "text": title, "weight": "semibold", "size": 13 }, { "text": format!("  {count}"), "tone": "muted", "size": 13 }]), json!({}))
}

fn names(v: &Value, key: &str, f: impl Fn(&Value) -> Option<String>) -> Vec<String> {
    nodes(v, key).iter().filter_map(f).collect()
}

fn state_tone(s: &str) -> &'static str {
    match s {
        "APPROVED" | "SUCCESS" | "MERGED" | "COMPLETED" => "ok",
        "CHANGES_REQUESTED" | "FAILURE" | "ERROR" | "CLOSED" | "TIMED_OUT" => "bad",
        "PENDING" | "IN_PROGRESS" | "QUEUED" | "EXPECTED" => "warn",
        _ => "muted",
    }
}

/// Download the repo-hosted images the PR's description and comments show, so the modal can draw them.
pub fn prefetch_images(dir: &str, n: &Value) {
    let mut texts: Vec<&str> = vec![n["body"].as_str().unwrap_or("")];
    texts.extend(nodes(n, "comments").iter().chain(nodes(n, "reviews")).filter_map(|c| c["body"].as_str()));
    for t in texts {
        super::markdown::prefetch(dir, t);
    }
}

/// What the person has chosen in the modal, and what the last button said.
#[derive(Clone, Debug, PartialEq)]
pub struct Choices {
    /// `squash`, `merge` or `rebase`.
    pub method: String,
    /// An agent's name, or `default` for the `agent.default` setting.
    pub agent: String,
    /// Where the built-in buttons that work in a worktree open it: `pane` (beside this page) or `space` (a space on the worktree).
    pub open: String,
    /// `merge` or `close`: that button was pressed once and awaits a second press.
    pub confirm: Option<String>,
    pub ran: Option<String>,
    /// The PR's details are still being read: the modal shows what the list holds and a spinner.
    pub loading: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Choices { method: "squash".into(), agent: "default".into(), open: "pane".into(), confirm: None, ran: None, loading: false }
    }
}

/// The agents Wheel can run, by name; empty when `wheel status` cannot say.
pub fn agent_names(dir: &str) -> Vec<String> {
    let Ok(v) = run::json("wheel", dir, &["status", "--json"]) else { return vec![] };
    v["agents"].as_array().into_iter().flatten().filter(|a| a["found"] == true).filter_map(|a| a["name"].as_str().map(String::from)).collect()
}

/// The modal for PR `pr` whose GraphQL node is `n`, as a `Modal` component named `detail`.
pub fn modal(p: &mut Page, pr: &Pr, n: &Value, agents: &[String], buttons: &[Button], ch: &Choices) {
    let at = now();
    let mut c: Vec<String> = vec![];
    c.push(p.label("pr.title", &format!("#{} {}", pr.number, pr.title), json!({ "size": 17, "weight": "semibold" })));

    let state = if n["merged"] == true { "MERGED" } else { n["state"].as_str().unwrap_or("OPEN") };
    let state_text = if n["isDraft"] == true { "draft".to_string() } else { state.to_lowercase() };
    let mut chips = vec![p.label("pr.c.state", &state_text, json!({ "chip": true, "size": 11, "tone": if n["isDraft"] == true { "muted" } else { state_tone(state) } }))];
    if pr.checks != "none" {
        chips.push(p.label("pr.c.checks", &format!("checks {}", pr.checks), json!({ "chip": true, "size": 11, "tone": match pr.checks { "passing" => "ok", "failing" => "bad", _ => "warn" } })));
    }
    let merge_state = n["mergeStateStatus"].as_str().unwrap_or("UNKNOWN");
    chips.push(p.label("pr.c.merge", &merge_state.to_lowercase().replace('_', " "), json!({ "chip": true, "size": 11, "tone": match merge_state { "CLEAN" | "HAS_HOOKS" => "ok", "DIRTY" | "BLOCKED" => "bad", "BEHIND" | "UNSTABLE" => "warn", _ => "muted" } })));
    chips.push(p.label("pr.c.review", pr.review, json!({ "chip": true, "size": 11, "tone": "muted" })));
    c.push(p.boxed("pr.chips", "h", chips, json!({ "spacing": 6, "align": "center" })));

    let v = p.label("pr.f.br.v", &format!("{} → {}", pr.branch, n["baseRefName"].as_str().unwrap_or("?")), json!({ "mono": true, "size": 12 }));
    c.push(fact(p, "br", "Branch", v));
    let v = p.label("pr.f.au.v", &pr.author, json!({}));
    c.push(fact(p, "au", "Author", v));
    if ch.loading {
        c.push(p.label("pr.loading", "Loading details…", json!({ "symbol": "spinner", "tone": "muted", "size": 12 })));
        if !pr.summary.is_empty() {
            let t = p.label("pr.loading.t", &pr.summary, json!({ "size": 13, "tone": "muted" }));
            c.push(p.boxed("pr.loading.b", "v", vec![t], json!({ "padding": [10, 12], "fill": "soft", "radius": 8, "align": "start" })));
        }
    } else {
    let assignees = names(n, "assignees", |a| login(a).map(String::from));
    let v = p.label("pr.f.as.v", &if assignees.is_empty() { "none".into() } else { assignees.join(", ") }, json!({ "tone": if assignees.is_empty() { "muted" } else { "fg" } }));
    c.push(fact(p, "as", "Assignee", v));
    // A requested reviewer, or one who reviewed, with where their review stands.
    let mut reviewers: Vec<String> = names(n, "reviewRequests", |r| {
        let x = &r["requestedReviewer"];
        login(x).or_else(|| x["slug"].as_str()).map(|l| format!("{l} (requested)"))
    });
    reviewers.extend(names(n, "latestReviews", |r| login(&r["author"]).map(|l| format!("{l} ({})", r["state"].as_str().unwrap_or("").to_lowercase().replace('_', " ")))));
    let v = p.label("pr.f.rv.v", &if reviewers.is_empty() { "none".into() } else { reviewers.join(", ") }, json!({ "tone": if reviewers.is_empty() { "muted" } else { "fg" } }));
    c.push(fact(p, "rv", "Reviewers", v));
    let labels = names(n, "labels", |l| l["name"].as_str().map(String::from));
    let v = p.label("pr.f.lb.v", &if labels.is_empty() { "none".into() } else { labels.join("  ·  ") }, json!({ "tone": if labels.is_empty() { "muted" } else { "fg" } }));
    c.push(fact(p, "lb", "Labels", v));
    let v = p.label("pr.f.sz.v", &format!("{} commits · {} files · +{} −{}", pr.commits, pr.files, pr.additions, pr.deletions), json!({ "tabular": true }));
    c.push(fact(p, "sz", "Size", v));
    let v = p.label("pr.f.up.v", &format!("opened {} ago · updated {} ago", ago(ts(&n["createdAt"]).unwrap_or(0), at), ago(ts(&n["updatedAt"]).unwrap_or(0), at)), json!({}));
    c.push(fact(p, "up", "Dates", v));
    p.put(json!({ "id": "pr.f.url.v", "component": "Link", "title": pr.url, "destination": pr.url }));
    let v = "pr.f.url.v".to_string();
    c.push(fact(p, "url", "GitHub", v));

    // What the buttons below use.
    let methods = json!([{ "value": "squash", "text": "Squash" }, { "value": "merge", "text": "Merge commit" }, { "value": "rebase", "text": "Rebase" }]);
    let m = p.choose("pr.method", "/set_method", &ch.method, methods, true, false);
    let mut options = vec![json!({ "value": "default", "text": "Default agent" })];
    options.extend(agents.iter().map(|a| json!({ "value": a, "text": a })));
    let a = p.choose("pr.agent", "/set_agent", &ch.agent, options, false, false);
    let places = json!([{ "value": "pane", "text": "This space" }, { "value": "space", "text": "New space" }]);
    let o = p.choose("pr.open", "/set_open", &ch.open, places, true, false);
    let ol = p.label("pr.open.k", "Open in", json!({ "tone": "muted", "size": 12 }));
    let ml = p.label("pr.method.k", "Merge with", json!({ "tone": "muted", "size": 12 }));
    let al = p.label("pr.agent.k", "Open with", json!({ "tone": "muted", "size": 12 }));
    c.push(p.boxed("pr.opts", "h", vec![ml, m, al, a, ol, o], json!({ "spacing": 8, "align": "center", "padding": [4, 0, 0, 0] })));

    let md = n["body"].as_str().unwrap_or("").trim();
    let kids = if md.is_empty() { vec![p.label("pr.desc.none", "No description.", json!({ "tone": "muted", "size": 13 }))] } else { body(p, "pr.desc", md) };
    let inner = p.boxed("pr.desc.in", "v", kids, json!({ "spacing": 8, "align": "start" }));
    c.push(p.boxed("pr.desc.b", "v", vec![inner], json!({ "padding": [10, 12], "fill": "soft", "radius": 8 })));

    // Checks.
    let rollup = &n["checks"]["nodes"][0]["commit"]["statusCheckRollup"];
    let contexts = nodes(rollup, "contexts");
    if !contexts.is_empty() {
        let mut rows = vec![heading(p, "pr.ck.h", "Checks", contexts.len())];
        for (i, k) in contexts.iter().enumerate() {
            let (name, st) = if k["__typename"] == "CheckRun" {
                (k["name"].as_str().unwrap_or(""), k["conclusion"].as_str().or(k["status"].as_str()).unwrap_or(""))
            } else {
                (k["context"].as_str().unwrap_or(""), k["state"].as_str().unwrap_or(""))
            };
            let chip = p.label(&format!("pr.ck.{i}.s"), &st.to_lowercase().replace('_', " "), json!({ "chip": true, "size": 11, "tone": state_tone(st) }));
            let l = p.label(&format!("pr.ck.{i}.n"), name, json!({ "size": 12, "lineLimit": 1 }));
            rows.push(p.boxed(&format!("pr.ck.{i}"), "h", vec![chip, l], json!({ "spacing": 8, "align": "center", "widths": [90, "flex"] })));
        }
        c.push(p.boxed("pr.ck", "v", rows, json!({ "spacing": 3, "padding": [8, 0, 0, 0] })));
    }

    // Conversation: reviews that say something, then comments, oldest first.
    let mut talk: Vec<(i64, String, String, String)> = vec![];
    for r in nodes(n, "reviews") {
        let text = r["body"].as_str().unwrap_or("").trim();
        let st = r["state"].as_str().unwrap_or("");
        if text.is_empty() && st == "COMMENTED" {
            continue;
        }
        talk.push((ts(&r["submittedAt"]).unwrap_or(0), login(&r["author"]).unwrap_or("ghost").into(), format!("review: {}", st.to_lowercase().replace('_', " ")), text.into()));
    }
    for cm in nodes(n, "comments") {
        talk.push((ts(&cm["createdAt"]).unwrap_or(0), login(&cm["author"]).unwrap_or("ghost").into(), "commented".into(), cm["body"].as_str().unwrap_or("").into()));
    }
    talk.sort_by_key(|t| t.0);
    if !talk.is_empty() {
        let mut rows = vec![heading(p, "pr.cm.h", "Conversation", talk.len())];
        for (i, (t, who, what, text)) in talk.iter().enumerate() {
            let head = p.runs(&format!("pr.cm.{i}.h"), json!([{ "text": who, "weight": "semibold", "size": 12 }, { "text": format!("  {what}  ·  {} ago", ago(*t, at)), "tone": "muted", "size": 12 }]), json!({}));
            let mut kids = vec![head];
            kids.extend(body(p, &format!("pr.cm.{i}"), text));
            rows.push(p.boxed(&format!("pr.cm.{i}.b"), "v", kids, json!({ "spacing": 4, "padding": [8, 12], "fill": "soft", "radius": 8, "align": "start" })));
        }
        c.push(p.boxed("pr.cm", "v", rows, json!({ "spacing": 6, "padding": [8, 0, 0, 0], "align": "start" })));
    }

    // History: the last commits, newest first.
    let commits = nodes(n, "history");
    if !commits.is_empty() {
        let total = n["history"]["totalCount"].as_u64().unwrap_or(commits.len() as u64) as usize;
        let mut rows = vec![heading(p, "pr.hi.h", "Commits", total)];
        for (i, k) in commits.iter().rev().enumerate() {
            let k = &k["commit"];
            let sha = p.label(&format!("pr.hi.{i}.s"), k["abbreviatedOid"].as_str().unwrap_or(""), json!({ "mono": true, "size": 12, "tone": "muted" }));
            let msg = p.label(&format!("pr.hi.{i}.m"), k["messageHeadline"].as_str().unwrap_or(""), json!({ "size": 12, "lineLimit": 1 }));
            let age = p.label(&format!("pr.hi.{i}.a"), &ago(ts(&k["committedDate"]).unwrap_or(0), at), json!({ "size": 12, "tone": "muted", "tabular": true, "align": "end" }));
            rows.push(p.boxed(&format!("pr.hi.{i}"), "h", vec![sha, msg, age], json!({ "spacing": 8, "align": "center", "widths": [64, "flex", 56] })));
        }
        if total > commits.len() {
            rows.push(p.label("pr.hi.more", &format!("… {} older commits", total - commits.len()), json!({ "tone": "muted", "size": 12 })));
        }
        c.push(p.boxed("pr.hi", "v", rows, json!({ "spacing": 3, "padding": [8, 0, 0, 0] })));
    }

    // Files.
    let files = nodes(n, "files");
    if !files.is_empty() {
        let total = n["files"]["totalCount"].as_u64().unwrap_or(files.len() as u64) as usize;
        let mut rows = vec![heading(p, "pr.fl.h", "Files", total)];
        for (i, f) in files.iter().enumerate() {
            let path = p.label(&format!("pr.fl.{i}.p"), f["path"].as_str().unwrap_or(""), json!({ "mono": true, "size": 12, "lineLimit": 1 }));
            let delta = p.label(&format!("pr.fl.{i}.d"), &format!("+{} −{}", f["additions"].as_i64().unwrap_or(0), f["deletions"].as_i64().unwrap_or(0)), json!({ "mono": true, "size": 12, "tone": "muted", "tabular": true, "align": "end" }));
            rows.push(p.boxed(&format!("pr.fl.{i}"), "h", vec![path, delta], json!({ "spacing": 8, "align": "center", "widths": ["flex", 96] })));
        }
        if total > files.len() {
            rows.push(p.label("pr.fl.more", &format!("… {} more files", total - files.len()), json!({ "tone": "muted", "size": 12 })));
        }
        c.push(p.boxed("pr.fl", "v", rows, json!({ "spacing": 3, "padding": [8, 0, 0, 0] })));
    }

    }

    // Footer: the result of the last button, then every button, always in the same place: one that does not apply is
    // disabled, never missing. Merge and Close PR take two clicks: the first turns that button into Confirm.
    let body_id = p.boxed("pr.body", "v", c, json!({ "spacing": 12, "align": "start" }));
    let mut footer = vec![];
    if let Some(r) = &ch.ran {
        footer.push(p.label("pr.ran", r, json!({ "tone": "ok", "weight": "semibold" })));
    }
    let open = state == "OPEN" && !ch.loading;
    let draft = n["isDraft"] == true;
    let mergeable = open && !draft && n["mergeable"] == "MERGEABLE";
    // The project's own buttons (wheel.toml) first, each with a terminal glyph; a button whose `on` does not match this PR is disabled.
    for b in buttons {
        let id = format!("pr.act.{}", crate::beads::sane(&b.name));
        let on = wants(b, pr);
        let off = if on { json!({}) } else { json!({ "disabled": true }) };
        p.put(json!({ "id": format!("{id}.i"), "component": "Icon", "name": "terminal" }));
        p.put(json!({ "id": format!("{id}.t"), "component": "Text", "text": label(b) }));
        p.put(json!({ "id": format!("{id}.r"), "component": "Row", "children": [format!("{id}.i"), format!("{id}.t")], "align": "center", "justify": "start" }));
        let mut mods = vec![json!({ "help": format!("{}: runs the command in wheel.toml [prs.buttons.{}]{}", label(b), b.name, if on { "" } else { " (does not apply to this PR)" }) })];
        if off["disabled"] == true {
            mods.push(off);
        }
        p.put(json!({ "id": id, "component": "Button", "child": format!("{id}.r"), "modifiers": mods,
            "action": { "event": { "name": "run_button", "context": { "name": b.name } } } }));
        footer.push(id);
    }
    if !buttons.is_empty() {
        p.put(json!({ "id": "pr.sep", "component": "Divider", "axis": "vertical" }));
        footer.push("pr.sep".into());
    }
    footer.push(p.button("pr.checkout", "Terminal on branch", "checkout_pr", json!({}), json!({ "disabled": ch.loading, "help": "Switch this project's checkout to the PR's branch and open a terminal in a new pane" })));
    footer.push(p.button("pr.worktree", "Agent in worktree", "worktree_pr", json!({}), json!({ "disabled": ch.loading, "help": "Open the branch in its own worktree and start the agent chosen under Open with, in the place chosen under Open in" })));
    footer.push(p.button("pr.draft", if draft { "Mark ready" } else { "Convert to draft" }, "toggle_draft", json!({}),
        json!({ "disabled": !open, "help": if draft { "Take this PR out of draft" } else { "Put this PR back into draft" } })));
    let asking = |what: &str| ch.confirm.as_deref() == Some(what);
    footer.push(p.button("pr.close", if asking("close") { "Confirm" } else { "Close PR" }, if asking("close") { "close_pr_now" } else { "ask_close" }, json!({}),
        json!({ "disabled": !open, "tone": "red", "help": if asking("close") { format!("Click again to close #{} without merging", pr.number) } else { "Close this PR without merging".to_string() } })));
    footer.push(p.button("pr.merge", if asking("merge") { "Confirm" } else { "Merge" }, if asking("merge") { "merge_pr" } else { "ask_merge" }, json!({}),
        json!({ "style": "primary", "tint": if asking("merge") { "green" } else { "tint" }, "disabled": !mergeable, "help": if !mergeable { "Not mergeable: closed, a draft, conflicting or not yet computed".to_string() } else if asking("merge") { format!("Click again to merge #{} ({})", pr.number, ch.method) } else { "Merge this PR with the method chosen under Merge with".to_string() } })));
    p.put(json!({ "id": "detail", "component": "Modal", "presented": true, "dismissAction": "close_pr", "children": [body_id], "actions": footer }));
}

/// A modal that is not presented: the client keeps every component it was sent, so only this
/// replaces an open one.
pub fn closed(p: &mut Page) {
    p.put(json!({ "id": "detail", "component": "Modal", "presented": false, "dismissAction": "close_pr" }));
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

/// The path of a worktree on the PR's branch: the one that holds it already, else a new one at
/// `~/.worktrees/<project>/<branch>`, made from the branch on origin (a fork's PR from its head ref,
/// then `gh pr checkout` inside it, which sets up the fork's remote).
pub fn worktree(dir: &str, number: i64, branch: &str, cross: bool) -> Result<String, String> {
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
        return Ok(p);
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    let wt = format!("{home}/.worktrees/{}/{}", crate::core::project::name_of(dir), branch.replace('/', "-"));
    let n = number.to_string();
    if cross {
        let head = format!("pull/{n}/head");
        crate::core::git::run(dir, &["fetch", "-q", "origin", &head])?;
        crate::core::git::run(dir, &["worktree", "add", "-q", "--detach", &wt, "FETCH_HEAD"])?;
        run::gh(&wt, &["pr", "checkout", &n])?;
    } else {
        crate::core::git::run(dir, &["fetch", "-q", "origin", branch])?;
        let local = !crate::core::git::quiet(dir, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}")]).trim().is_empty();
        if local {
            crate::core::git::run(dir, &["worktree", "add", "-q", &wt, branch])?;
        } else {
            crate::core::git::run(dir, &["worktree", "add", "-q", "-b", branch, &wt, &format!("origin/{branch}")])?;
        }
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
        return fail("opening a pane needs the asking pane (_meta wheel/pane)");
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
    let mut args = Map::new();
    args.insert("brief".into(), json!(brief(pr, wt)));
    if agent != "default" {
        args.insert("agent".into(), json!(agent));
    }
    let r = Agents::new("wheel").spawn(origin, &args);
    let text = r.content.first().and_then(|c| serde_json::to_value(c).ok()).and_then(|c| c["text"].as_str().map(String::from)).unwrap_or_default();
    if r.is_error == Some(true) {
        fail(text)
    } else {
        Ok(text)
    }
}

/// Run project button `b` for `pr`, in a worktree first when the button asks for one.
pub fn run_button(origin: &Origin, repo: &str, pr: &Pr, cross: bool, b: &Button) -> Result<(), String> {
    let wt = if b.worktree { Some(worktree(&origin.dir, pr.number, &pr.branch, cross)?) } else { None };
    let (number, title, branch, url) = (pr.number.to_string(), pr.title.clone(), pr.branch.clone(), pr.url.clone());
    let mut env = vec![("PR_NUMBER", number.as_str()), ("PR_TITLE", title.as_str()), ("PR_BRANCH", branch.as_str()), ("PR_URL", url.as_str()),
        ("PR_REPO", repo), ("PR_PATH", origin.dir.as_str())];
    if let Some(w) = &wt {
        env.push(("PR_WORKTREE", w.as_str()));
    }
    if let Some(d) = &origin.data {
        env.push(("WHEEL_DATA_DIR", d.as_str()));
    }
    let log = std::env::temp_dir().join(format!("wheel-prs-{}-{}.log", pr.number, crate::beads::sane(&b.name)));
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
        let dir = std::env::temp_dir().join(format!("wheelhouse-prs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("wheel.toml"), "[prs.buttons.fix]\nlabel = \"Make mergeable\"\non = [\"fix\"]\nworktree = true\nrun = \"true\"\n[prs.buttons.all]\non = [\"any\"]\nrun = \"true\"\n").unwrap();
        let (b, problem) = buttons(dir.to_str().unwrap());
        assert!(problem.is_none());
        assert_eq!(b.len(), 2);
        let mut pr = Pr { number: 1, title: "t".into(), author: "a".into(), branch: "b".into(), draft: false, mergeable: "MERGEABLE".into(), checks: "passing", review: "in-review",
            bucket: "mine", my: None, gaps: String::new(), updated: 0, url: "u".into(), fix: false, additions: 0, deletions: 0, files: 0, commits: 0, labels: vec![], reviewers: vec![], next: String::new(), summary: String::new(), issues: vec![] };
        let fix = b.iter().find(|x| x.name == "fix").unwrap();
        assert!(fix.worktree && label(fix) == "Make mergeable");
        assert!(!wants(fix, &pr));
        pr.fix = true;
        assert!(wants(fix, &pr));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_modal_shows_description_images_conversation_checks_commits_files_and_buttons() {
        let n = json!({ "state": "OPEN", "isDraft": false, "merged": false, "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "baseRefName": "main",
            "createdAt": "2026-11-01T00:00:00Z", "updatedAt": "2026-11-02T00:00:00Z", "body": "Fixes it\n![shot](https://x/a.png)",
            "assignees": { "nodes": [{ "login": "me" }] }, "labels": { "nodes": [{ "name": "bug" }] },
            "reviewRequests": { "nodes": [{ "requestedReviewer": { "login": "you" } }] },
            "latestReviews": { "nodes": [{ "author": { "login": "zed" }, "state": "APPROVED" }] },
            "reviews": { "nodes": [{ "author": { "login": "zed" }, "state": "APPROVED", "submittedAt": "2026-11-02T00:00:00Z", "body": "ok" }] },
            "comments": { "nodes": [{ "author": { "login": "you" }, "createdAt": "2026-11-02T01:00:00Z", "body": "see ![](https://x/b.png)" }] },
            "history": { "totalCount": 1, "nodes": [{ "commit": { "abbreviatedOid": "abc1234", "messageHeadline": "fix", "committedDate": "2026-11-01T00:00:00Z" } }] },
            "files": { "totalCount": 1, "nodes": [{ "path": "a.rs", "additions": 3, "deletions": 1 }] },
            "checks": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS", "contexts": { "nodes": [{ "__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "SUCCESS" }] } } } }] } });
        let pr = Pr { number: 7, title: "t".into(), author: "a".into(), branch: "b".into(), draft: false, mergeable: "MERGEABLE".into(), checks: "passing", review: "approved",
            bucket: "mine", my: None, gaps: String::new(), updated: 0, url: "https://u".into(), fix: true, additions: 3, deletions: 1, files: 1, commits: 1, labels: vec![], reviewers: vec![], next: String::new(), summary: String::new(), issues: vec![] };
        let b = Button { name: "fix".into(), label: Some("Make mergeable".into()), on: vec!["fix".into()], run: "true".into(), worktree: true };
        let mut p = Page { out: vec![] };
        modal(&mut p, &pr, &n, &["claude".into(), "codex".into()], &[b], &Choices::default());
        let has = |id: &str| p.out.iter().any(|c| c["id"] == id);
        for id in ["pr.ck.0", "pr.hi.0", "pr.fl.0", "pr.method", "pr.agent", "pr.f.url.v"] {
            assert!(has(id), "missing {id}");
        }
        let images: Vec<&str> = p.out.iter().filter(|c| c["component"] == "Image").filter_map(|c| c["url"].as_str()).collect();
        assert_eq!(images, vec!["https://x/a.png", "https://x/b.png"], "the description's and the comment's images are drawn");
        let texts: Vec<&str> = p.out.iter().filter(|c| c["component"] == "Text").filter_map(|c| c["text"].as_str()).collect();
        assert!(texts.contains(&"Fixes it") && texts.contains(&"see"));
        let m = p.out.iter().find(|c| c["id"] == "detail").unwrap();
        assert_eq!(m["actions"], json!(["pr.act.fix", "pr.sep", "pr.checkout", "pr.worktree", "pr.draft", "pr.close", "pr.merge"]), "every button is always there");
        let mut p = Page { out: vec![] };
        modal(&mut p, &pr, &n, &[], &[], &Choices { confirm: Some("merge".into()), ..Default::default() });
        let merge = p.out.iter().find(|c| c["id"] == "pr.merge").unwrap();
        assert_eq!(merge["action"]["event"]["name"], "merge_pr", "the second click merges");
        assert!(p.out.iter().any(|c| c["id"] == "pr.merge.t" && c["text"] == "Confirm"));
        assert!(p.out.iter().any(|c| c["id"] == "pr.checkout"), "the other buttons stay while one asks");
    }

    #[test]
    fn a_worktree_is_made_on_the_prs_branch_once_and_found_again() {
        let root = std::env::temp_dir().join(format!("wheelhouse-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let sh = |dir: &std::path::Path, args: &[&str]| crate::core::git::run(dir.to_str().unwrap(), args).unwrap();
        let (origin, work) = (root.join("origin.git"), root.join("proj"));
        sh(&root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        sh(&root, &["clone", "-q", origin.to_str().unwrap(), work.to_str().unwrap()]);
        for a in [["config", "user.email", "t@t"], ["config", "user.name", "t"]] {
            sh(&work, &a);
        }
        sh(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        sh(&work, &["push", "-q", "origin", "HEAD:refs/heads/feat/x"]);
        let home = root.join("home");
        std::env::set_var("HOME", &home);
        let a = worktree(work.to_str().unwrap(), 5, "feat/x", false).unwrap();
        assert_eq!(a, format!("{}/.worktrees/proj/feat-x", home.display()));
        assert_eq!(crate::core::git::current_branch(&a).as_deref(), Some("feat/x"));
        let again = worktree(work.to_str().unwrap(), 5, "feat/x", false).unwrap();
        assert_eq!(std::fs::canonicalize(&again).unwrap(), std::fs::canonicalize(&a).unwrap(), "the second call finds the first");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pr_still_loading_shows_the_list_data_a_spinner_and_disabled_buttons() {
        let pr = Pr { number: 7, title: "t".into(), author: "a".into(), branch: "b".into(), draft: false, mergeable: "MERGEABLE".into(), checks: "passing", review: "approved",
            bucket: "mine", my: None, gaps: String::new(), updated: 0, url: "https://u".into(), fix: false, additions: 3, deletions: 1, files: 1, commits: 1, labels: vec![], reviewers: vec![],
            next: String::new(), summary: "The start of the body".into(), issues: vec![] };
        let mut p = Page { out: vec![] };
        modal(&mut p, &pr, &Value::Null, &[], &[], &Choices { loading: true, ..Default::default() });
        let has = |id: &str| p.out.iter().any(|c| c["id"] == id);
        assert!(has("pr.loading") && has("pr.loading.t"), "a spinner and the list's description");
        assert!(!has("pr.f.as.v"), "nothing the list lacks is shown as if it were known");
        let disabled = |id: &str| p.out.iter().find(|c| c["id"] == id).unwrap()["modifiers"].to_string().contains("disabled");
        assert!(disabled("pr.merge") && disabled("pr.worktree") && disabled("pr.draft"));
    }
}
