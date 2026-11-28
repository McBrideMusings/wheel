mod common;

use common::*;
use serde_json::{Value as Json, json};

#[test]
fn the_embed_fixture_names_its_spaces_terminals_a_web_view_and_the_first_agent() {
    let row = |id: &str, space: &str, kind: &str| json!({"id": id, "space": space, "kind": kind});
    let panes = json!([row("p1", "s1", "plugin"), row("p2", "s1", "terminal"), row("p3", "s1", "terminal"), row("p4", "s2", "terminal")]);
    let agents = json!([{"pane": "p9", "space": "s2"}, {"pane": "p7", "space": "s1"}]);
    let h = host_feed(move |name, _| match name {
        "wheel.panes" => panes.clone(),
        "wheel.agents" => agents.clone(),
        _ => Json::Null,
    });
    let mut rt = wheel_luau::PaneRuntime::new(&read("tools/fixtures/plugins/embed/main.luau"), Default::default(), "pane", Limits::default(), h).unwrap();
    let data = json!({"space": "s1", "pane": "p1"});
    let mut tree = rt.view(&data).unwrap();
    assert_eq!(tree.children[0].children[0].children.len(), 2);
    assert_eq!(tree.children[0].children[1].component, "Terminal");
    same(&prop(&tree.children[0].children[1], "pane"), json!("p2"));
    assert_eq!(tree.children[1].component, "WebView");
    same(&prop(&tree.children[1], "url"), json!("https://example.com"));
    assert_eq!(tree.children[2].component, "AgentChat");
    same(&prop(&tree.children[2], "pane"), json!("p7"));
    same(&prop(&tree.children[3], "text"), json!("found 2 terminal panes"));
    rt.send(&json!({"type": "select", "pane": "p3"})).unwrap();
    tree = rt.view(&data).unwrap();
    same(&prop(&tree.children[0].children[1], "pane"), json!("p3"));
}
