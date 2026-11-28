mod common;

use std::collections::BTreeMap;

use common::*;
use serde_json::{Value as Json, json};

#[allow(clippy::too_many_arguments)]
fn branch(name: &str, upstream: Option<&str>, track: &str, merged: Option<&str>, pr: Json, dead: Option<&str>, blocked: Option<&str>, local_only: i64) -> Json {
    let finished = ["merged", "squash-merged", "gone"].contains(&dead.unwrap_or(""));
    json!({"branch": name, "ts": 100, "subject": "s", "author": "a", "ahead": 1, "behind": 0, "upstream": upstream,
           "track": track, "upstream_gone": track == "gone", "remote": null, "unpushed": null, "worktree": null,
           "pr": pr, "pr_merged": false, "merged": merged, "local_only": local_only,
           "dead": dead, "blocked": blocked, "finished": finished})
}

fn feed() -> Json {
    let pr = |n: i64, state: &str, draft: bool| json!({"number": n, "title": "t", "state": state, "draft": draft, "url": format!("https://x/{n}"), "review": null, "mergeable": "", "ci": null});
    let n = Json::Null;
    let rows = vec![
        branch("open-pr", Some("origin/open-pr"), "", None, pr(19, "OPEN", false), Some("pushed"), None, 0),
        branch("draft-pr", Some("origin/draft-pr"), "", None, pr(20, "OPEN", true), Some("pushed"), None, 0),
        branch("squashed", Some("origin/squashed"), "", Some("squash-merged"), pr(10, "MERGED", false), Some("squash-merged"), None, 0),
        branch("closed-pr", Some("origin/closed-pr"), "", None, pr(7, "CLOSED", false), Some("pushed"), None, 0),
        branch("deleted", Some("origin/deleted"), "gone", Some("merged"), n.clone(), Some("gone"), None, 0),
        branch("dirty", Some("origin/main"), "behind 118", Some("merged"), n.clone(), Some("merged"), Some("uncommitted changes"), 0),
        branch("other-upstream", Some("origin/base"), "ahead 5", None, n.clone(), None, None, 5),
        branch("never-pushed", None, "", None, n, None, None, 6),
    ];
    json!({"now": 400, "name": "p", "top": "/p", "default": "origin/main", "remote": "origin", "head": "main", "branches": rows,
           "stashes": [], "commits": [], "fetched": 280, "pr_error": null, "error": null, "scripts": [],
           "cleanup": {"delete": ["squashed", "deleted"], "keep": [{"branch": "dirty", "why": "blocked: uncommitted changes"}]}})
}

fn repo_pane() -> PaneRuntime {
    wh_pane("repo", "repo", feed(), Json::Null, rec())
}

/// Each row's cell `field` by key.
fn cell_field(tree: &PaneNode, field: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for r in rows(tree) {
        let Some(Json::Array(cells)) = r.row.get("cells") else { continue };
        let mut m = BTreeMap::new();
        for c in cells {
            if let Json::Object(c) = c
                && let Some(k) = s(c.get("key")) {
                    m.insert(k, s(c.get(field)).unwrap_or_default());
                }
        }
        out.insert(r.id, m);
    }
    out
}

#[test]
fn each_branch_is_described_in_gits_words() {
    let tree = repo_pane().view(&wh_data()).unwrap();
    let c = cell_field(&tree, "text");
    let row = |b: &str| -> Vec<String> { ["upstream", "merged", "pr", "delete"].iter().map(|k| c.get(b).and_then(|r| r.get(*k)).cloned().unwrap_or("?".into())).collect() };
    assert_eq!(row("open-pr"), ["up to date", "", "#19 open", "safe"]);
    assert_eq!(row("draft-pr"), ["up to date", "", "#20 draft", "safe"]);
    assert_eq!(row("squashed"), ["up to date", "squash-merged", "#10 merged", "safe"]);
    assert_eq!(row("closed-pr"), ["up to date", "", "#7 closed", "safe"]);
    assert_eq!(row("deleted"), ["gone", "merged", "", "safe"]);
    assert_eq!(row("dirty"), ["origin/main: behind 118", "merged", "", "blocked: uncommitted changes"]);
    assert_eq!(row("other-upstream"), ["origin/base: ahead 5", "", "", "5 unpushed"]);
    assert_eq!(row("never-pushed"), ["no upstream", "", "", "6 unpushed"]);
}

#[test]
fn a_finished_branch_is_faint_in_every_cell_but_delete() {
    let t = cell_field(&repo_pane().view(&wh_data()).unwrap(), "tone");
    for b in ["squashed", "deleted", "dirty"] {
        let row = t.get(b).unwrap_or_else(|| panic!("no row {b}"));
        assert!(row.iter().filter(|(k, _)| *k != "delete").all(|(_, v)| v == "faint"), "{b}: {row:?}");
    }
    assert_eq!(t["squashed"].get("delete").map(String::as_str), Some("ok"));
    assert_eq!(t["deleted"].get("delete").map(String::as_str), Some("ok"));
    assert_eq!(t["dirty"].get("delete").map(String::as_str), Some("warn"));
    for b in ["open-pr", "closed-pr", "other-upstream", "never-pushed"] {
        assert!(!t[b].values().any(|v| v == "faint"), "{b} is not finished");
    }
}

#[test]
fn the_actions_button_is_at_the_right_and_clean_up_comes_first() {
    let mut rt = repo_pane();
    let tree = rt.view(&wh_data()).unwrap();
    let texts = texts_json(&jsonof(&tree));
    assert!(texts.contains(&"Actions ⌄".to_string()));
    assert!(!texts.iter().any(|t| t.contains("listed")));
    let btns = buttons(&tree);
    let clean = btns.iter().position(|b| b.text == "Clean up merged  2").expect("Clean up merged  2");
    let copy = btns.iter().position(|b| b.text == "Copy IDs").expect("Copy IDs");
    assert!(clean < copy);
    assert!(!btns[clean].disabled);
    assert!(btns[clean].help.contains("squashed, deleted"), "{}", btns[clean].help);
    assert!(btns[clean].help.contains("dirty (blocked: uncommitted changes)"), "{}", btns[clean].help);
    assert_eq!(action_type(&btns[clean]).as_deref(), Some("cleanup"));
    rt.send(&wh_msg("list.select", json!({"ids": ["deleted"]}))).unwrap();
    assert!(texts_json(&jsonof(&rt.view(&wh_data()).unwrap())).contains(&"1 selected · Actions ⌄".to_string()));
    // The bar pins the actions to the far right: its two groups are spread apart.
    let bars = all(&rt.view(&wh_data()).unwrap(), "Row").into_iter().filter(|r| s(r.get("justify")).as_deref() == Some("spaceBetween")).count();
    assert_eq!(bars, 1);
}

#[test]
fn the_upstream_header_says_when_origin_was_fetched() {
    let tree = repo_pane().view(&wh_data()).unwrap();
    let tables = all(&tree, "Table");
    let table = tables.first().expect("no table");
    let Some(Json::Array(cols)) = table.get("columns") else { panic!("no table") };
    let labels: Vec<String> = cols.iter().filter_map(|c| if let Json::Object(o) = c { s(o.get("label")) } else { None }).collect();
    assert!(labels.iter().any(|l| l.starts_with("Upstream · fetched ")), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "State"));
}
