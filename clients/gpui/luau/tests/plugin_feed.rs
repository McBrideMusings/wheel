//! Ports PluginFeedTests.swift (WheelUI). Skipped:
//! - the JSONPatchTests suite (add, removeAndReplace, moveCopyTest, pointerEscapes, badIndexAndUnknownOp,
//!   failedPatchLeavesDocumentUnchanged, appendToLongArrayIsConstantTime): it tests WheelKit's own `JSONPatch`
//!   type; the Rust pane applies patches with the `json_patch` crate, so there is no code of ours to test. The
//!   pane-level bad-patch behaviour is covered by a_bad_patch_sets_error_and_reads_the_feed_again.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value as Json, json};
use wheel_luau::pane::{Effect, PluginPane};

const SCRIPT: &str = r#"
return {
    init = function(previous) return previous or {} end,
    update = function(state, msg) state.more = (msg.type == "more") end,
    view = function(state, data)
        local a = data.feed("x.y", {})
        local rows = {
            ui.Text("a " .. tostring(if a == nil then "nil" else a.n)),
            ui.Button{ child = ui.Text("More"), action = { type = "more" } },
            ui.Button{ child = ui.Text("Less"), action = { type = "less" } },
        }
        if state.more then table.insert(rows, ui.Text("z " .. tostring(data.feed("x.z", { k = 1 }) ~= nil))) end
        return ui.Column(rows)
    end,
}
"#;

/// The feeds the pane asked to subscribe and unsubscribe, collected from its effects.
#[derive(Default)]
struct Calls {
    sub: Vec<String>,
    unsub: Vec<String>,
}

impl Calls {
    fn drain(&mut self, pane: &mut PluginPane) {
        for effect in pane.take_effects() {
            if let Effect::Feeds { subscribe, unsubscribe } = effect {
                self.sub.extend(subscribe.into_iter().map(|f| f.0));
                self.unsub.extend(unsubscribe.into_iter().map(|f| f.0));
            }
        }
    }
}

fn make(calls: &mut Calls) -> PluginPane {
    let mut pane = PluginPane::new();
    pane.load(SCRIPT, HashMap::new(), "t.luau");
    calls.drain(&mut pane);
    pane
}

fn texts(pane: &PluginPane) -> Vec<String> {
    pane.component_texts().iter().filter_map(|r| r.get("text").cloned()).collect()
}

fn first(pane: &PluginPane) -> Option<String> {
    texts(pane).into_iter().next()
}

fn buttons(pane: &PluginPane) -> Vec<String> {
    let mut ids: Vec<String> = pane.tree.all().filter(|n| n.component == "Button").map(|n| n.id.clone()).collect();
    ids.sort();
    ids
}

fn names(pane: &PluginPane) -> Vec<String> {
    pane.feeds().into_iter().map(|f| f.0).collect()
}

#[test]
fn a_feed_read_subscribes_and_shows_value_then_patch() {
    let mut calls = Calls::default();
    let mut pane = make(&mut calls);
    assert_eq!(pane.error(), None);
    assert_eq!(calls.sub, ["x.y"]);
    assert_eq!(first(&pane).as_deref(), Some("a nil"));
    pane.feed_update("x.y", &json!({}), Some(&json!({"n": 1})), None, false);
    assert_eq!(first(&pane).as_deref(), Some("a 1"));
    pane.feed_update("x.y", &json!({}), None, Some(&json!([{"op": "replace", "path": "/n", "value": 7}])), false);
    assert_eq!(first(&pane).as_deref(), Some("a 7"));
    assert_eq!(pane.error(), None);
    assert_eq!(pane.feed_json(), BTreeMap::from([("x.y {}".to_string(), "{\"n\":7}".to_string())]));
}

#[test]
fn a_stale_update_keeps_the_value_and_the_script_reads_the_flag() {
    let mut pane = PluginPane::new();
    pane.load(
        r#"
    return { view = function(state, data)
        local v, stale = data.feed("x.y", {})
        return ui.Text(tostring(if v == nil then "nil" else v.n) .. (if stale then " stale" else ""))
    end }
    "#,
        HashMap::new(),
        "s.luau",
    );
    pane.feed_update("x.y", &json!({}), Some(&json!({"n": 3})), None, false);
    assert_eq!(texts(&pane), ["3"]);
    pane.feed_update("x.y", &json!({}), Some(&json!({"n": 3})), None, true);
    assert_eq!(texts(&pane), ["3 stale"]);
    pane.feed_update("x.y", &json!({}), None, Some(&json!([{"op": "replace", "path": "/n", "value": 4}])), false);
    assert_eq!(texts(&pane), ["4"], "the next update from the backend is fresh");
}

#[test]
fn a_bad_patch_sets_error_and_reads_the_feed_again() {
    let mut calls = Calls::default();
    let mut pane = make(&mut calls);
    pane.feed_update("x.y", &json!({}), Some(&json!({"n": 1})), None, false);
    calls.sub.clear();
    pane.feed_update("x.y", &json!({}), None, Some(&json!([{"op": "remove", "path": "/nope"}])), false);
    calls.drain(&mut pane);
    assert!(pane.error().is_some());
    assert_eq!(first(&pane).as_deref(), Some("a 1"));
    assert!(calls.unsub == ["x.y"] && calls.sub == ["x.y"], "unsubscribe then subscribe, so wheeld sends the whole value");
    assert_eq!(pane.feed_json(), BTreeMap::from([("x.y {}".to_string(), "null".to_string())]), "the value that no longer matches wheeld's is dropped");
    pane.feed_update("x.y", &json!({}), Some(&json!({"n": 2})), None, false);
    assert_eq!(first(&pane).as_deref(), Some("a 2"));
}

#[test]
fn a_refused_subscribe_is_shown_and_subscribed_again_on_the_next_render() {
    let mut calls = Calls::default();
    let mut pane = make(&mut calls);
    assert_eq!(calls.sub, ["x.y"]);
    pane.feed_failed("x.y", &json!({}), "no such feed");
    assert_eq!(pane.error(), Some("no such feed"));
    assert!(pane.feeds().is_empty());
    calls.sub.clear();
    // Any render that reads the feed again subscribes it again: pressing a button renders.
    let ids = buttons(&pane);
    assert!(pane.press(&ids[1]));
    calls.drain(&mut pane);
    assert_eq!(calls.sub, ["x.y"]);
    assert_eq!(names(&pane), ["x.y"]);
}

#[test]
fn a_second_feed_subscribes_and_a_dropped_one_unsubscribes() {
    let mut calls = Calls::default();
    let mut pane = make(&mut calls);
    let ids = buttons(&pane);
    assert_eq!(ids.len(), 2);
    // The first button's action is `more`, the second's `less`, in the order the script lists them.
    assert!(pane.press(&ids[0]));
    calls.drain(&mut pane);
    assert_eq!(calls.sub, ["x.y", "x.z"]);
    assert!(pane.press(&ids[1]));
    calls.drain(&mut pane);
    assert_eq!(calls.unsub, ["x.z"]);
    assert_eq!(names(&pane), ["x.y"]);
}

#[test]
fn close_unsubscribes_everything() {
    let mut calls = Calls::default();
    let mut pane = make(&mut calls);
    pane.close();
    calls.drain(&mut pane);
    assert_eq!(calls.unsub, ["x.y"]);
    assert!(pane.feeds().is_empty());
}

#[test]
fn an_update_for_a_feed_not_read_is_ignored() {
    let mut calls = Calls::default();
    let mut pane = make(&mut calls);
    pane.feed_update("other.f", &Json::Null, Some(&json!(1)), None, false);
    let mut keys: Vec<String> = pane.feed_json().into_keys().collect();
    keys.sort();
    assert_eq!(keys, ["x.y {}"]);
}
