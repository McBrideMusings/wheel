//! Every pane script in the repo, run with recorded feeds, draws the tree WheelLuau draws for the
//! same input. `tests/scripts/cases.json` lists the scripts and the data each view gets; each case's
//! `<name>.feeds.json` holds the feeds it reads (recorded from a live wheeld, then trimmed and
//! scrubbed), and `<name>.tree.json` the tree WheelLuau's `GoldenTreeTests` wrote for it.
//!
//! With `WHEEL_LUAU_ASKS=<dir>` set, each case writes the feeds it asked for and found no recording
//! of to `<dir>/<name>.json`, which `tools/record-feeds.py` reads to record them from a live wheeld.

use std::{cell::RefCell, collections::HashMap, fs, path::{Path, PathBuf}, rc::Rc};

use serde_json::{json, Value as Json};
use wheel_luau::{FeedRead, Host, Limits, PaneRuntime};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scripts")
}

/// Every `.luau` file under the package's `lib/`, keyed by its path in the package, as `plugin_source` carries them.
fn modules(package: &Path) -> HashMap<String, String> {
    fn walk(dir: &Path, rel: &str, out: &mut HashMap<String, String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let key = format!("{rel}/{name}");
            if e.path().is_dir() {
                walk(&e.path(), &key, out);
            } else if name.ends_with(".luau") {
                out.insert(key, fs::read_to_string(e.path()).unwrap());
            }
        }
    }
    let mut out = HashMap::new();
    walk(&package.join("lib"), "lib", &mut out);
    out
}

/// Numbers as doubles, since Luau has no integers and Swift's encoder writes 3.0 as 3.
fn normal(v: &Json) -> Json {
    match v {
        Json::Number(n) => json!(n.as_f64().unwrap()),
        Json::Array(a) => Json::Array(a.iter().map(normal).collect()),
        Json::Object(o) => Json::Object(o.iter().map(|(k, v)| (k.clone(), normal(v))).collect()),
        _ => v.clone(),
    }
}

/// The first place `a` and `b` differ, as a path into the tree.
fn first_difference(a: &Json, b: &Json, at: &str) -> Option<String> {
    match (a, b) {
        (Json::Object(x), Json::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                match (x.get(k), y.get(k)) {
                    (Some(p), Some(q)) => {
                        if let Some(d) = first_difference(p, q, &format!("{at}.{k}")) {
                            return Some(d);
                        }
                    }
                    (p, q) => return Some(format!("{at}.{k}: {p:?} vs {q:?}")),
                }
            }
            None
        }
        (Json::Array(x), Json::Array(y)) if x.len() == y.len() => {
            x.iter().zip(y).enumerate().find_map(|(i, (p, q))| first_difference(p, q, &format!("{at}[{i}]")))
        }
        _ if a == b => None,
        _ => Some(format!("{at}: {} vs {}", short(a), short(b))),
    }
}

fn short(v: &Json) -> String {
    let s = v.to_string();
    if s.chars().count() > 200 { format!("{}…", s.chars().take(200).collect::<String>()) } else { s }
}

#[test]
fn every_pane_script_draws_wheelluaus_tree() {
    let cases: Json = serde_json::from_str(&fs::read_to_string(scripts().join("cases.json")).unwrap()).unwrap();
    let asks_dir = std::env::var_os("WHEEL_LUAU_ASKS").map(PathBuf::from);
    let mut failures = Vec::new();
    let mut checked = 0;
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let package = repo().join(case["package"].as_str().unwrap());
        let source = fs::read_to_string(package.join(case["pane"].as_str().unwrap())).unwrap();
        let feeds: Vec<Json> = fs::read_to_string(scripts().join(format!("{name}.feeds.json")))
            .map(|s| serde_json::from_str(&s).unwrap())
            .unwrap_or_default();
        let asked = Rc::new(RefCell::new(Vec::<Json>::new()));
        let host = Host {
            feed: Box::new({
                let asked = asked.clone();
                move |feed, params| {
                    let hit = feeds.iter().find(|f| f["name"] == feed && f["params"] == *params);
                    match hit {
                        Some(f) => FeedRead { value: f["value"].clone(), stale: f["stale"].as_bool().unwrap_or(false) },
                        None => {
                            let ask = json!({ "name": feed, "params": params });
                            if !asked.borrow().contains(&ask) {
                                asked.borrow_mut().push(ask);
                            }
                            FeedRead::new(Json::Null)
                        }
                    }
                }
            }),
            ..Host::default()
        };
        let tree = PaneRuntime::new(&source, modules(&package), "pane", Limits::default(), host).and_then(|mut rt| {
            for msg in case["messages"].as_array().into_iter().flatten() {
                rt.send(msg)?;
            }
            rt.view(&case["data"])
        });
        if let Some(dir) = &asks_dir {
            fs::create_dir_all(dir).unwrap();
            fs::write(dir.join(format!("{name}.json")), serde_json::to_string_pretty(&*asked.borrow()).unwrap()).unwrap();
        }
        let tree = match tree {
            Ok(t) => t.to_json(),
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        if !asked.borrow().is_empty() {
            failures.push(format!("{name}: no recording of {}", Json::Array(asked.borrow().clone())));
        }
        let Ok(want) = fs::read_to_string(scripts().join(format!("{name}.tree.json"))) else {
            failures.push(format!("{name}: no {name}.tree.json (run GoldenTreeTests with WHEEL_LUAU_GOLDEN=write)"));
            continue;
        };
        let want: Json = serde_json::from_str(&want).unwrap();
        if let Some(d) = first_difference(&normal(&tree), &normal(&want), "root") {
            failures.push(format!("{name}: Rust vs WheelLuau differ at {d}"));
        }
        checked += 1;
    }
    assert!(failures.is_empty(), "{} of {} cases failed:\n{}", failures.len(), cases.as_array().unwrap().len(), failures.join("\n"));
    assert!(checked >= 17, "only {checked} cases checked");
}

/// Each script in `errors.json`, under the default limits, fails with the kind and message
/// WheelLuau gave (`errors.expected.json`, written by `GoldenTreeTests.everyFailureReadsAsRecorded`).
#[test]
fn every_failure_reads_as_wheelluaus() {
    let cases: Json = serde_json::from_str(&fs::read_to_string(scripts().join("errors.json")).unwrap()).unwrap();
    let want: Json = serde_json::from_str(&fs::read_to_string(scripts().join("errors.expected.json")).unwrap()).unwrap();
    let mut got = serde_json::Map::new();
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let run = PaneRuntime::new(case["source"].as_str().unwrap(), HashMap::new(), case["chunk"].as_str().unwrap(), Limits::default(), Host::default())
            .and_then(|mut rt| rt.view(&json!({})));
        let failure = match run {
            Ok(_) => panic!("{name} did not fail"),
            Err(f) => f,
        };
        let kind = match failure {
            wheel_luau::Failure::Syntax(_) => "syntax",
            wheel_luau::Failure::Runtime(_) => "runtime",
            wheel_luau::Failure::StepBudget(_) => "step_budget",
            wheel_luau::Failure::Memory(_) => "memory",
            wheel_luau::Failure::Module(_) => "module",
            wheel_luau::Failure::View(_) => "view",
        };
        got.insert(name.into(), json!({ "kind": kind, "message": failure.message() }));
    }
    assert_eq!(Json::Object(got), want);
}
