mod common;

use std::collections::BTreeMap;

use common::*;
use serde_json::{Map, Value as Json, json};

/// Every node of the gallery's tree (children and nodes inside properties) listed by its path.
fn nodes() -> BTreeMap<String, PaneNode> {
    fn walk(n: &PaneNode, id: &str, out: &mut BTreeMap<String, PaneNode>) {
        out.insert(id.to_string(), n.clone());
        for (i, c) in n.children.iter().enumerate() {
            walk(c, &format!("{id}.{i}"), out);
        }
        for (k, v) in &n.props {
            lift(v, &format!("{id}@{k}"), out);
        }
    }
    fn lift(v: &Json, id: &str, out: &mut BTreeMap<String, PaneNode>) {
        if let Some(n) = PaneNode::from_json(v.clone()) {
            walk(&n, id, out);
            return;
        }
        match v {
            Json::Array(a) => a.iter().enumerate().for_each(|(i, x)| lift(x, &format!("{id}.{i}"), out)),
            Json::Object(o) => o.iter().for_each(|(k, x)| lift(x, &format!("{id}.{k}"), out)),
            _ => {}
        }
    }
    let mut rt = new(&read("plugins/gallery/panes/gallery.luau"));
    let mut out = BTreeMap::new();
    walk(&rt.view(&Json::Null).unwrap(), "root", &mut out);
    out
}

fn components() -> BTreeMap<String, Map<String, Json>> {
    nodes()
        .into_iter()
        .map(|(id, n)| {
            let mut m = n.props.clone();
            m.entry("component").or_insert(json!(n.component));
            (id, m)
        })
        .collect()
}

#[test]
fn draws_every_class() {
    let all = components();
    let texts: Vec<&str> = all.values().filter_map(|f| f.get("text").and_then(|t| t.as_str())).collect();
    for heading in [
        "Component gallery", "Content", "Buttons", "Selection and input", "Containers and layout",
        "Navigation and presentation", "SwiftUI controls", "Table", "Modifiers", "Wheel's own component",
    ] {
        assert!(texts.contains(&heading), "no {heading} heading");
    }
}

/// A vertical List or a Table with no frame makes the view fill the pane, and a page that fills never
/// scrolls past its first screen.
#[test]
fn the_gallery_scrolls_as_a_whole() {
    for (id, f) in components() {
        let scrolls_itself =
            f.get("component") == Some(&json!("Table")) || (f.get("component") == Some(&json!("List")) && f.get("direction") != Some(&json!("horizontal")));
        if !scrolls_itself {
            continue;
        }
        let Some(Json::Array(mods)) = f.get("modifiers") else { panic!("{id} has no modifiers") };
        assert!(mods.iter().any(|m| matches!(m, Json::Object(m) if m.contains_key("frame"))), "{id} has no frame");
    }
}

#[test]
fn every_node_a_property_names_exists() {
    let all = nodes();
    let tabs = all.values().find(|n| n.component == "Tabs").expect("a Tabs node");
    let Some(Json::Array(list)) = tabs.props.get("tabs") else { panic!("Tabs has no tabs") };
    for tab in list {
        let Json::Object(tab) = tab else { continue };
        let Some(child) = tab.get("child") else { panic!("a tab has no child: {tab:?}") };
        assert!(PaneNode::from_json(child.clone()).is_some(), "tab child {child} is not a component");
    }
    let pinned = all.values().find(|n| n.props.contains_key("scrollTo")).expect("a node with scrollTo");
    let Some(Json::Number(place)) = pinned.props.get("scrollTo") else { panic!("scrollTo is not a place") };
    let place = place.as_f64().unwrap() as i64;
    let target = (place - 1) as usize;
    assert!(
        pinned.children.get(target).is_some_and(|c| c.component == "Section"),
        "scrollTo names place {place}, not a Section"
    );
}
