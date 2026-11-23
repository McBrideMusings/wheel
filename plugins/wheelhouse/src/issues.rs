//! The Issues view's data: the project's GitHub issues, read, created and closed through `gh`.

use serde_json::Value;

use crate::core::run;

pub mod cli;
pub mod feed;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Issue {
    pub number: i64,
    pub title: String,
    /// `open` or `closed`.
    pub state: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub has_pr: bool,
    pub author: String,
    pub body: String,
    pub url: String,
    /// Unix time of the last update.
    pub updated: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub issues: Vec<Issue>,
    pub error: Option<String>,
    /// The list buttons under `[issues.list]` in the machine's and the project's wheel.toml.
    pub scripts: Vec<crate::core::buttons::Script>,
    pub buttons: Vec<crate::prs::detail::Button>,
    pub viewer: String,
    /// What was wrong with the wheel.toml, shown on the page without hiding the issues.
    pub problems: Vec<String>,
}

const FIELDS: &str = "number,title,state,labels,assignees,author,body,url,updatedAt,closedByPullRequestsReferences";

fn issue(v: &Value) -> Issue {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let names = |k: &str, name: &str| -> Vec<String> {
        v[k].as_array().map(|a| a.iter().filter_map(|x| x[name].as_str().map(String::from)).collect()).unwrap_or_default()
    };
    Issue {
        number: v["number"].as_i64().unwrap_or(0),
        title: s("title"),
        state: s("state").to_lowercase(),
        labels: names("labels", "name"),
        assignees: names("assignees", "login"),
        has_pr: v["closedByPullRequestsReferences"].as_array().is_some_and(|prs| !prs.is_empty()),
        author: v["author"]["login"].as_str().unwrap_or("").to_string(),
        body: s("body"),
        url: s("url"),
        updated: crate::beads::epoch(&s("updatedAt")),
    }
}

/// Every issue of the GitHub repository `dir` is a checkout of (newest 500, closed ones included).
pub fn list(dir: &str) -> Result<Vec<Issue>, String> {
    let out = run::gh(dir, &["issue", "list", "--state", "all", "--limit", "500", "--json", FIELDS])
        .map_err(|e| format!("{dir}: gh issue list failed: {e}"))?;
    let rows: Vec<Value> = serde_json::from_str(&out).map_err(|e| format!("gh issue list: not JSON: {e}"))?;
    Ok(rows.iter().map(issue).collect())
}

/// Open an issue titled `title`; the number it got.
pub fn create(dir: &str, title: &str) -> Result<i64, String> {
    if title.trim().is_empty() {
        return Err("an issue needs a title".into());
    }
    let url = run::gh(dir, &["issue", "create", "--title", title, "--body", ""])?;
    url.trim().rsplit('/').next().and_then(|n| n.parse().ok()).ok_or_else(|| format!("gh issue create printed {}", url.trim()))
}

/// Close issue `number`.
pub fn close(dir: &str, number: i64) -> Result<(), String> {
    run::gh(dir, &["issue", "close", &number.to_string()]).map(|_| ())
}

pub fn collect(dir: &str) -> Snapshot {
    let mut s = Snapshot { name: crate::core::project::name_of(dir), ..Default::default() };
    let (files, problems) = crate::core::buttons::files(dir);
    s.scripts = crate::core::buttons::scripts_of(&files, "issues");
    s.buttons = crate::prs::detail::buttons_in(&files, "issues");
    if !s.buttons.is_empty() {
        s.viewer = run::gh(dir, &["api", "user", "--jq", ".login"]).unwrap_or_default().trim().to_string();
    }
    s.problems = problems;
    match list(dir) {
        Ok(i) => s.issues = i,
        Err(e) => s.error = Some(e),
    }
    s
}
