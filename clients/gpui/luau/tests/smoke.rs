use std::collections::HashMap;

use serde_json::json;
use wheel_luau::{Failure, Host, Limits, PaneRuntime};

fn pane(source: &str) -> Result<PaneRuntime, Failure> {
    PaneRuntime::new(source, HashMap::new(), "pane", Limits::default(), Host::default())
}

#[test]
fn a_view_returns_its_tree() {
    let mut rt = pane("return { view = function(s, d) return ui.Column{ ui.Text('hi'), 'there', gap = 4 } end }").unwrap();
    let node = rt.view(&json!({})).unwrap();
    assert_eq!(
        node.to_json(),
        json!({"component": "Column", "gap": 4.0, "children": [
            {"component": "Text", "text": "hi"}, {"component": "Text", "text": "there"}]})
    );
}

#[test]
fn a_loop_stops_at_the_step_budget() {
    let mut rt = pane("return { view = function() while true do end end }").unwrap();
    let e = rt.view(&json!({})).unwrap_err();
    assert!(matches!(e, Failure::StepBudget(ref m) if m.contains("step budget")), "{e:?}");
}

#[test]
fn a_canvas_records_and_measures() {
    let mut rt = pane("return { view = function() return ui.Canvas{ draw = function(ctx, w, h) ctx.font = 'bold 13px sans-serif'; local m = ctx:measureText('ab'); ctx:fillText('ab', m.width, h) end } end }").unwrap();
    let node = rt.view(&json!({})).unwrap();
    let commands = node.props["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 2);
    assert!(commands[1][2].as_f64().unwrap() > 5.0, "{commands:?}");
}
