mod common;

use common::*;
use serde_json::{Value as Json, json};

const PACKAGE: &str = "plugins/wheelhouse";

fn data() -> Json {
    json!({"pane": "p1", "space": "s1", "cwd": "/p"})
}

fn bead(id: &str, status: &str, blocked_by: &[&str], ready: bool) -> Json {
    json!({
        "id": id, "title": format!("Title of {id}"), "status": status, "kind": "task",
        "priority": 2, "blocked_by": blocked_by, "description": format!("About {id}"),
        "ready": ready,
    })
}

fn pane(beads: Vec<Json>) -> PaneRuntime {
    let feed = json!({"beads": beads});
    let h = host_feed(move |_, _| feed.clone());
    PaneRuntime::new(&read(&format!("{PACKAGE}/panes/graph.luau")), modules_of(PACKAGE), "pane", Limits::default(), h).unwrap()
}

/// Open beads `a` <- `b` <- `c` (b waits on a, c on b) and one closed bead that stays out of the drawing.
fn chain() -> PaneRuntime {
    pane(vec![bead("a", "open", &[], true), bead("b", "open", &["a"], false), bead("c", "open", &["b"], false), bead("z", "closed", &[], false)])
}

fn canvas(node: &PaneNode) -> Option<PaneNode> {
    if node.component == "Canvas" {
        return Some(node.clone());
    }
    node.children.iter().find_map(canvas)
}

fn commands(node: &PaneNode) -> Vec<Vec<Json>> {
    let Some(c) = canvas(node) else { return vec![] };
    let Some(Json::Array(list)) = c.props.get("commands") else { return vec![] };
    list.iter().filter_map(|x| x.as_array().cloned()).collect()
}

fn st(rt: &PaneRuntime) -> Json {
    rt.state().unwrap_or(Json::Null)
}

fn field(state: &Json, key: &str) -> Option<Json> {
    state.get(key).cloned()
}

fn number(state: &Json, key: &str) -> Option<f64> {
    field(state, key).and_then(|v| v.as_f64())
}

fn position(rt: &PaneRuntime, id: &str) -> (f64, f64) {
    let state = st(rt);
    let at = &state["layout"][id];
    (at["x"].as_f64().unwrap_or_else(|| panic!("no position for {id}")), at["y"].as_f64().unwrap())
}

fn pointer(rt: &mut PaneRuntime, kind: &str, x: f64, y: f64, dx: f64, dy: f64) {
    rt.send(&json!({"type": "pointer", "kind": kind, "x": x, "y": y, "button": 0, "dx": dx, "dy": dy})).unwrap();
}

fn is_null(v: Option<Json>) -> bool {
    matches!(v, None | Some(Json::Null))
}

#[test]
fn draws_open_beads_and_their_edges_left_to_right_by_dependency() {
    let mut rt = chain();
    let tree = rt.view(&data()).unwrap();
    assert!(texts_children(&tree).contains(&"3 open issues \u{b7} 2 dependencies".to_string()), "{:?}", texts_children(&tree));
    let curves = commands(&tree).into_iter().filter(|c| c.first() == Some(&json!("bezierCurveTo"))).count();
    assert_eq!(curves, 2);
    let (a, b, c) = (position(&rt, "a"), position(&rt, "b"), position(&rt, "c"));
    assert!(a.0 < b.0 && b.0 < c.0, "a blocker sits left of what it blocks: {a:?} {b:?} {c:?}");
    same(&field(&st(&rt), "nodes").unwrap(), json!(3));
    assert!(st(&rt)["layout"].get("z").is_none());
}

#[test]
fn hover_highlights_the_node_and_its_edges() {
    let mut rt = chain();
    let before = commands(&rt.view(&data()).unwrap());
    let a = position(&rt, "a");
    pointer(&mut rt, "move", a.0 + 20.0, a.1 + 15.0, 0.0, 0.0);
    let after = commands(&rt.view(&data()).unwrap());
    same(&field(&st(&rt), "hover").unwrap(), json!("a"));
    assert_ne!(before, after);
    let lit = after.iter().filter(|c| c.len() == 3 && c[1] == json!("strokeStyle") && c[2] == json!("accent")).count();
    assert!(lit >= 1, "the hovered node's edge is stroked in the accent tone");
    // Off every node clears it.
    pointer(&mut rt, "move", 900.0, 480.0, 0.0, 0.0);
    rt.view(&data()).unwrap();
    assert!(is_null(field(&st(&rt), "hover")));
}

#[test]
fn a_click_opens_the_bead_and_close_shuts_it() {
    let mut rt = chain();
    rt.view(&data()).unwrap();
    let b = position(&rt, "b");
    pointer(&mut rt, "down", b.0 + 20.0, b.1 + 15.0, 0.0, 0.0);
    pointer(&mut rt, "up", b.0 + 20.0, b.1 + 15.0, 0.0, 0.0);
    let tree = rt.view(&data()).unwrap();
    same(&field(&st(&rt), "open").unwrap(), json!("b"));
    assert!(texts_children(&tree).contains(&"Blocked by a \u{b7} Blocks c".to_string()), "{:?}", texts_children(&tree));
    rt.send(&json!({"type": "close"})).unwrap();
    rt.view(&data()).unwrap();
    assert!(is_null(field(&st(&rt), "open")));
}

#[test]
fn a_drag_pans_and_opens_nothing() {
    let mut rt = chain();
    rt.view(&data()).unwrap();
    let a = position(&rt, "a");
    // A drag that starts on a node pans too, and its release is not a click.
    pointer(&mut rt, "down", a.0 + 20.0, a.1 + 15.0, 0.0, 0.0);
    pointer(&mut rt, "drag", a.0 + 60.0, a.1 + 40.0, 40.0, 25.0);
    pointer(&mut rt, "up", a.0 + 60.0, a.1 + 40.0, 0.0, 0.0);
    let tree = rt.view(&data()).unwrap();
    assert!(number(&st(&rt), "ox") == Some(40.0) && number(&st(&rt), "oy") == Some(25.0), "{:?}", st(&rt));
    assert!(is_null(field(&st(&rt), "open")));
    // The drawing is translated by the pan.
    assert!(commands(&tree).iter().any(|c| c.first() == Some(&json!("translate")) && c.len() == 3 && c[1] == json!(40.0) && c[2] == json!(25.0)));
    // A tap inside the slop is a click, not a pan.
    pointer(&mut rt, "down", a.0 + 60.0, a.1 + 40.0, 0.0, 0.0);
    pointer(&mut rt, "drag", a.0 + 62.0, a.1 + 41.0, 2.0, 1.0);
    pointer(&mut rt, "up", a.0 + 62.0, a.1 + 41.0, 0.0, 0.0);
    rt.view(&data()).unwrap();
    assert!(number(&st(&rt), "ox") == Some(40.0) && field(&st(&rt), "open") == Some(json!("a")), "{:?}", st(&rt));
}

#[test]
fn the_first_drag_past_the_slop_pans_by_everything_since_the_press() {
    let mut rt = chain();
    rt.view(&data()).unwrap();
    pointer(&mut rt, "down", 700.0, 400.0, 0.0, 0.0);
    pointer(&mut rt, "drag", 702.0, 400.0, 2.0, 0.0);
    assert_eq!(number(&st(&rt), "ox"), Some(0.0));
    pointer(&mut rt, "drag", 705.0, 400.0, 3.0, 0.0);
    assert_eq!(number(&st(&rt), "ox"), Some(5.0));
    pointer(&mut rt, "drag", 707.0, 401.0, 2.0, 1.0);
    assert!(number(&st(&rt), "ox") == Some(7.0) && number(&st(&rt), "oy") == Some(1.0), "{:?}", st(&rt));
}

#[test]
fn a_cycle_still_lays_out() {
    let mut rt = pane(vec![bead("a", "open", &["b"], false), bead("b", "open", &["a"], false)]);
    let tree = rt.view(&data()).unwrap();
    assert!(texts_children(&tree).contains(&"2 open issues \u{b7} 2 dependencies".to_string()), "{:?}", texts_children(&tree));
}

#[test]
fn no_open_beads_says_so_and_draws_no_canvas() {
    let mut rt = pane(vec![bead("z", "closed", &[], false)]);
    let tree = rt.view(&data()).unwrap();
    assert!(texts_children(&tree).contains(&"No open issues".to_string()));
    assert!(canvas(&tree).is_none());
}
