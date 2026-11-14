//! The `beads_graph` feed for `wheelhouse backend`: the Focus or Graph view of one project's beads as a
//! scene for the Canvas, with its caption and the list behind an open "+ N more" node. The pane
//! (Luau) adds the toolbar, the legend and the canvas's own fields; the backend sends data only.

use serde_json::{json, Value};

use super::graph::{focus_scene, whole_scene, Model, Params};
use super::*;
use crate::core::number_of;

/// What a subscription's params ask for; a missing or malformed field takes its default.
fn params(p: &Value) -> Params {
    let text = |k: &str| p.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    let mut out = Params {
        view: if p["view"] == "graph" { "graph".into() } else { "focus".into() },
        sel: text("sel"),
        more: text("more"),
        depth: number_of(&p["depth"]).unwrap_or(2).clamp(1, 3) as usize,
        width: number_of(&p["width"]).map_or(900.0, |w| w as f64).clamp(300.0, 4000.0),
        ..Params::default()
    };
    let list = &p["list"];
    out.list.q = list["q"].as_str().unwrap_or("").to_string();
    if let Some(f) = list["f"].as_object() {
        for (k, v) in f {
            let values = v.as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
            out.list.f.insert(k.clone(), values);
        }
    }
    out.list.not = list["not"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
    out
}

/// The `beads_graph` feed's value for the project in `dir`.
pub fn value(dir: &str, p: &Value) -> Value {
    value_of(&collect(dir), &params(p))
}

fn value_of(snap: &Snapshot, p: &Params) -> Value {
    let m = Model::new(snap, &p.list);
    let layout = m.layout(p);
    let n = m.count();
    let focus = p.view != "graph";
    let empty = if n == 0 { Some("No beads yet".to_string()) } else { layout["empty"].as_str().map(String::from) };
    let scene = if empty.is_some() {
        Value::Null
    } else if focus {
        focus_scene(&layout)
    } else {
        whole_scene(&layout)
    };
    let more = layout["more"].as_array().map(|rows| json!({ "title": format!("{} more beads", rows.len()), "rows": rows }));
    json!({
        "view": p.view,
        "n": n,
        "center": layout["center"]["id"].clone(),
        "caption": layout["caption"].as_str().unwrap_or(""),
        "captionBad": layout["captionBad"].as_bool().unwrap_or(false),
        "cyc": layout["cyc"].as_bool().unwrap_or(false),
        "empty": empty,
        "scene": scene,
        "more": more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bead(id: &str, f: impl Fn(&mut Bead)) -> Bead {
        let mut b = Bead {
            id: id.into(), title: format!("title {id}"), status: "open".into(), kind: "task".into(), priority: Some(2),
            parent: None, labels: vec![], owner: None, updated: String::new(), blocked_by: vec![],
            description: String::new(), ready: false, epic: false,
        };
        f(&mut b);
        b
    }

    fn snap(beads: Vec<Bead>) -> Snapshot {
        Snapshot { name: "t".into(), path: "/t".into(), beads, has_repo: true, ..Default::default() }
    }

    /// How many values a view tree holds the way the Luau runtime counts them: every object, array,
    /// string, number and bool is one.
    pub fn count(v: &Value) -> usize {
        1 + match v {
            Value::Array(a) => a.iter().map(count).sum(),
            Value::Object(o) => o.values().map(count).sum(),
            _ => 0,
        }
    }

    /// 1,500 beads: 30 epics of 50 beads, each bead blocked by the one before it in chains of 5.
    fn big() -> Snapshot {
        let mut beads = vec![];
        for e in 0..30 {
            beads.push(bead(&format!("e{e}"), |b| b.kind = "epic".into()));
        }
        for i in 0..1470 {
            let e = i % 30;
            beads.push(bead(&format!("b{i}"), |b| {
                b.parent = Some(format!("e{e}"));
                b.title = format!("A bead title of a realistic length, number {i}");
                if i % 5 != 0 {
                    b.blocked_by = vec![format!("b{}", i - 30)];
                }
                if i % 7 == 0 {
                    b.status = "in_progress".into();
                }
            }));
        }
        snap(beads)
    }

    #[test]
    fn a_1500_bead_scene_stays_under_the_runtime_limit() {
        let s = big();
        let p = Params { view: "graph".into(), ..Params::default() };
        let graph = count(&value_of(&s, &p));
        let p = Params { sel: Some("b100".into()), width: 1400.0, ..Params::default() };
        let focus = count(&value_of(&s, &p));
        eprintln!("graph {graph} values, focus {focus} values");
        assert!(graph < 80_000, "graph scene is {graph} values");
        assert!(focus < 20_000, "focus scene is {focus} values");
    }

    fn chain() -> Snapshot {
        snap(vec![
            bead("e", |b| b.kind = "epic".into()),
            bead("a", |b| b.parent = Some("e".into())),
            bead("b", |b| { b.parent = Some("e".into()); b.blocked_by = vec!["a".into()] }),
            bead("c", |b| b.blocked_by = vec!["b".into()]),
        ])
    }

    #[test]
    fn the_params_clamp_and_default() {
        let p = params(&json!({ "dir": "/t", "depth": 9, "width": 10, "view": "other", "sel": "", "list": { "q": "x", "f": { "type": ["bug"] }, "not": ["type"] } }));
        assert_eq!((p.view.as_str(), p.depth, p.width, p.sel.clone(), p.more.clone()), ("focus", 3, 300.0, None, None));
        assert_eq!((p.list.q.as_str(), p.list.values("type"), p.list.not.len()), ("x", &["bug".to_string()][..], 1));
        let p = params(&json!({ "dir": "/t" }));
        assert_eq!((p.depth, p.width), (2, 900.0));
        let p = params(&json!({ "depth": 1.0, "width": 700.0 }));
        assert_eq!((p.depth, p.width), (1, 700.0));
        let p = params(&json!({ "depth": 3.0, "width": 5000.0 }));
        assert_eq!((p.depth, p.width), (3, 4000.0));
    }

    #[test]
    fn a_focus_value_has_the_center_a_caption_and_a_scene_of_action_tables() {
        let v = value_of(&chain(), &params(&json!({ "view": "focus", "sel": "b" })));
        assert_eq!((v["view"].as_str(), v["n"].as_u64(), v["center"].as_str()), (Some("focus"), Some(4), Some("b")));
        assert!(v["caption"].as_str().unwrap().contains("1 blocker, 1 unlock"));
        assert_eq!((v["empty"].is_null(), v["more"].is_null(), v["cyc"].as_bool()), (true, true, Some(false)));
        let hits = v["scene"]["hits"].as_array().unwrap();
        let hit = |id: &str| hits.iter().find(|h| h["id"] == id).unwrap();
        assert_eq!(hit("a")["action"], json!({ "type": "focus_on" }));
        assert_eq!(hit("b")["doubleAction"], json!({ "type": "open_bead" }));
        for k in ["id", "component", "minHeight", "escapeAction", "sizeAction", "fitKey"] {
            assert!(v["scene"].get(k).is_none(), "{k}");
        }
        let g = value_of(&chain(), &params(&json!({ "view": "graph", "sel": "b" })));
        assert!(g["center"].is_null());
        assert_eq!(g["scene"]["emptyAction"], json!({ "type": "graph_select" }));
        assert_eq!(g["scene"]["emptyContext"], json!({ "id": "" }));
        assert_eq!(g["scene"]["enterAction"], json!({ "type": "open_bead" }));
        assert_eq!(g["scene"]["enterContext"], json!({ "id": "b" }));
    }

    #[test]
    fn a_directory_with_no_beads_gives_the_empty_value() {
        let v = value("/nonexistent-wheelhouse-dir", &json!({ "dir": "/nonexistent-wheelhouse-dir", "view": "graph" }));
        assert_eq!((v["view"].as_str(), v["n"].as_u64(), v["empty"].as_str()), (Some("graph"), Some(0), Some("No beads yet")));
    }

    #[test]
    fn an_empty_snapshot_says_so_and_has_no_scene() {
        let v = value_of(&snap(vec![]), &params(&json!({})));
        assert_eq!((v["n"].as_u64(), v["empty"].as_str(), v["scene"].is_null(), v["center"].is_null()), (Some(0), Some("No beads yet"), true, true));
    }

    #[test]
    fn an_open_more_key_lists_what_the_node_hides() {
        let mut beads = vec![bead("c0", |_| {})];
        for i in 0..15 {
            beads.push(bead(&format!("d{i}"), |b| b.blocked_by = vec!["c0".into()]));
        }
        let s = snap(beads);
        let v = value_of(&s, &params(&json!({ "sel": "c0", "more": "R1" })));
        let more = &v["more"];
        assert_eq!(more["title"], "2 more beads");
        assert_eq!(more["rows"].as_array().unwrap().len(), 2);
        assert_eq!(more["rows"][0]["word"], "blocked");
        assert_eq!(more["rows"][0]["tone"], "bad");
        let hit = v["scene"]["hits"].as_array().unwrap().iter().find(|h| h["id"] == "R1").unwrap().clone();
        assert_eq!((hit["action"].clone(), hit["context"].clone()), (json!({ "type": "graph_more" }), json!({ "key": "R1" })));
        assert!(value_of(&s, &params(&json!({ "sel": "c0", "more": "nope" })))["more"].is_null());
        assert!(value_of(&s, &params(&json!({ "sel": "c0" })))["more"].is_null());
    }
}
