//! Ports PluginActionTests.swift (WheelUI). Skipped:
//! - aHostFailureReachesUpdateAfterTheCallingUpdateReturns: tests `actionFailedLater`, a Swift Task hop out of the
//!   `onAction` callback; the Rust pane records an `Effect::Action` and the host answers afterwards, so there is no
//!   re-entrancy to defer.

mod common;

use std::collections::HashMap;

use common::same;
use serde_json::{Value as Json, json};
use wheel_luau::pane::{Effect, PluginPane};

const SCRIPT: &str = r#"
return {
    init = function(previous) return previous or { shown = "none" } end,
    update = function(state, msg)
        if msg.type == "go" then
            wheel.action("clock.echo", { n = 3, s = "hi" })
        elseif msg.action ~= nil then
            if msg.error ~= nil then
                state.shown = "err " .. msg.action .. " " .. msg.error
            else
                state.shown = "ok " .. msg.action .. " " .. tostring(msg.result.n)
            end
        end
    end,
    view = function(state, data)
        return ui.Column({
            ui.Text(state.shown),
            ui.Button{ child = ui.Text("Go"), action = { type = "go" } },
        })
    end,
}
"#;

fn make() -> PluginPane {
    let mut pane = PluginPane::new();
    pane.load(SCRIPT, HashMap::new(), "t.luau");
    pane
}

fn shown(pane: &PluginPane) -> Option<String> {
    pane.component_texts().iter().find_map(|r| r.get("text").cloned())
}

fn go(pane: &mut PluginPane) {
    let button = pane.tree.all().find(|n| n.component == "Button").unwrap().id.clone();
    assert!(pane.press(&button));
}

#[test]
fn a_script_action_from_update_reaches_the_host() {
    let mut pane = make();
    assert_eq!(pane.error(), None);
    go(&mut pane);
    let actions: Vec<(String, Json)> = pane
        .take_effects()
        .into_iter()
        .filter_map(|e| match e {
            Effect::Action { name, args } => Some((name, args)),
            _ => None,
        })
        .collect();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].0, "clock.echo");
    same(&actions[0].1, json!({"n": 3, "s": "hi"}));
    assert_eq!(pane.error(), None);
}

#[test]
fn a_result_reaches_update_and_the_view_shows_it() {
    let mut pane = make();
    assert_eq!(shown(&pane).as_deref(), Some("none"));
    pane.action_result("clock.echo", json!({"n": 3}));
    assert_eq!(shown(&pane).as_deref(), Some("ok clock.echo 3"));
    assert_eq!(pane.error(), None);
}

#[test]
fn a_failure_reaches_update_and_the_view_shows_it() {
    let mut pane = make();
    pane.action_failed("clock.fail", "boom");
    assert_eq!(shown(&pane).as_deref(), Some("err clock.fail boom"));
    assert_eq!(pane.error(), None);
}
