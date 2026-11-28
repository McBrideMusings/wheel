// Ports ViewTreeTests.swift (WheelUI). Skipped:
// - aKeystrokeEvaluatesOnlyTheChangedBodies: counts SwiftUI body evaluations in an NSHostingView; no Rust equivalent.
// `===` object identity is a node's id and `serial` together (`ident`).

use std::collections::HashMap;

use serde_json::{Value as Json, json};
use wheel_luau::pane::PluginPane;

const SEARCH: &str = r#"
return {
    init = function(p) return p or { q = "", items = { "apple", "banana", "cherry" } } end,
    update = function(state, msg) if msg.type == "q" then state.q = msg.value end end,
    view = function(state)
        local rows = {}
        for _, it in state.items do
            if string.find(it, state.q, 1, true) then table.insert(rows, ui.Text{ key = it, text = it }) end
        end
        return ui.Column{
            ui.Text("Fruit"),
            ui.TextField{ label = "Search", value = state.q, on_change = { type = "q" } },
            ui.Column(rows),
        }
    end,
}
"#;

fn load(source: &str) -> PluginPane {
    let mut pane = PluginPane::new();
    pane.load(source, HashMap::new(), "t.luau");
    assert_eq!(pane.error(), None);
    pane
}

/// A node's identity: its id and the build it is.
fn ident(pane: &PluginPane, id: &str) -> Option<(String, u64)> {
    pane.tree.node(id).map(|n| (n.id.clone(), n.serial))
}

fn child_ids(pane: &PluginPane, id: &str) -> Vec<String> {
    pane.tree.node(id).unwrap().children.iter().map(|r| r.id.clone()).collect()
}

fn prop(pane: &PluginPane, id: &str, key: &str) -> Option<Json> {
    pane.tree.node(id).and_then(|n| n.props.get(key).cloned())
}

#[test]
fn a_held_pointer_stays_in_the_client() {
    let mut pane = load(SEARCH);
    let renders = pane.perf.renders;
    assert!(pane.hover(Some("root/TextField0")));
    assert_eq!(pane.tree.hover_held.as_deref(), Some("root/TextField0"));
    assert!(!pane.hover(Some("root/nothing")));
    assert_eq!(pane.tree.hover_held.as_deref(), Some("root/TextField0"));
    assert!(pane.hover(None));
    assert_eq!(pane.tree.hover_held, None);
    assert_eq!(pane.perf.renders, renders);
}

#[test]
fn ids_come_from_keys_else_component_and_place() {
    let pane = load(SEARCH);
    let ids: Vec<String> = pane.component_texts().iter().map(|r| r.get("id").cloned().unwrap_or_default()).collect();
    assert_eq!(ids, ["root", "root/Text0", "root/TextField0", "root/Column0", "root/Column0/=apple", "root/Column0/=banana", "root/Column0/=cherry"]);
}

#[test]
fn a_keystroke_writes_only_the_nodes_that_changed() {
    let mut pane = load(SEARCH);
    let heading = ident(&pane, "root/Text0").unwrap();
    let cherry = ident(&pane, "root/Column0/=cherry").unwrap();
    // "an" keeps banana; the field's value comes back equal to its draft, so the field is not written again by the render.
    assert!(pane.set("root/TextField0", json!("an")));
    assert!(pane.tree.node("root/Column0/=banana").is_some());
    assert!(pane.tree.node("root/Column0/=cherry").is_none());
    // Only the list's child order changed: the list node is the one node written.
    assert_eq!(pane.perf.nodes, 1);
    assert_eq!(ident(&pane, "root/Text0").unwrap(), heading);
    // Clearing the search brings cherry back as a new node; the heading is the same object throughout.
    assert!(pane.set("root/TextField0", json!("")));
    assert_ne!(ident(&pane, "root/Column0/=cherry").unwrap(), cherry);
    assert_eq!(pane.perf.nodes, 3);
    assert_eq!(ident(&pane, "root/Text0").unwrap(), heading);
}

#[test]
fn a_render_that_changes_nothing_writes_nothing() {
    let mut pane = load(SEARCH);
    pane.action_result("noop", Json::Null);
    assert_eq!(pane.perf.nodes, 0);
}

#[test]
fn a_keyed_row_keeps_its_draft_when_the_list_reorders() {
    let mut pane = load(
        r#"
    return {
        init = function(p) return p or { order = { "a", "b" } } end,
        update = function(state, msg) if msg.type == "flip" then state.order = { state.order[2], state.order[1] } end end,
        view = function(state)
            local rows = {}
            for _, k in state.order do table.insert(rows, ui.TextField{ key = k, label = k }) end
            rows.key = "list"
            return ui.Column{ ui.Column(rows), ui.Button{ child = ui.Text("Flip"), action = { type = "flip" } } }
        end,
    }
    "#,
    );
    let b = ident(&pane, "root/=list/=b").unwrap();
    assert!(pane.set("root/=list/=b", json!("typed")));
    assert!(pane.press("root/Button0"));
    assert_eq!(child_ids(&pane, "root/=list"), ["root/=list/=b", "root/=list/=a"]);
    assert_eq!(ident(&pane, "root/=list/=b").unwrap(), b);
    assert_eq!(pane.tree.node("root/=list/=b").unwrap().value(), Some(&json!("typed")));
}

#[test]
fn an_unkeyed_insertion_before_siblings_rebuilds_the_later_ones() {
    let mut pane = load(
        r#"
    return {
        init = function(p) return p or { first = false } end,
        update = function(state, msg) state.first = true end,
        view = function(state)
            local rows = {}
            if state.first then table.insert(rows, ui.TextField{ label = "new" }) end
            table.insert(rows, ui.TextField{ label = "old" })
            table.insert(rows, ui.Button{ child = ui.Text("Add"), action = { type = "add" } })
            return ui.Column(rows)
        end,
    }
    "#,
    );
    assert!(pane.set("root/TextField0", json!("draft")));
    assert!(pane.press("root/Button0"));
    // The old field is now the second TextField: a new node, its draft gone; the first is the one inserted.
    assert_eq!(prop(&pane, "root/TextField1", "label"), Some(json!("old")));
    assert_eq!(pane.tree.node("root/TextField1").unwrap().value(), Some(&json!("")));
    assert_eq!(prop(&pane, "root/TextField0", "label"), Some(json!("new")));
}

#[test]
fn a_duplicate_key_draws_the_first_and_errors() {
    let mut pane = PluginPane::new();
    pane.load(
        r#"
    return { view = function() return ui.Column{ ui.Text{ key = "x", text = "one" }, ui.Text{ key = "x", text = "two" } } end }
    "#,
        HashMap::new(),
        "dup.luau",
    );
    assert!(pane.error().is_some_and(|e| e.contains("duplicate key root x")));
    assert_eq!(child_ids(&pane, "root"), ["root/=x"]);
    assert_eq!(prop(&pane, "root/=x", "text"), Some(json!("one")));
}

#[test]
fn a_key_never_takes_a_generated_id_or_a_path() {
    let pane = load(
        r#"
    return { view = function() return ui.Column{ ui.Text("plain"), ui.Text{ key = "Text0", text = "keyed" }, ui.Text{ key = "a/b@c", text = "odd" } } end }
    "#,
    );
    assert_eq!(child_ids(&pane, "root"), ["root/Text0", "root/=Text0", "root/=a%2Fb%40c"]);
    assert_eq!(prop(&pane, "root/Text0", "text"), Some(json!("plain")));
    assert_eq!(prop(&pane, "root/=Text0", "text"), Some(json!("keyed")));
}

#[test]
fn presented_modals_stack_in_the_order_the_view_gives_them() {
    let pane = load(
        r#"
    return { view = function()
        local cs = {}
        for i = 1, 11 do table.insert(cs, ui.Modal{ presented = true, ui.Text("m" .. i) }) end
        table.insert(cs, ui.Modal{ key = "a", presented = true, ui.Text("keyed") })
        return ui.Column(cs)
    end }
    "#,
    );
    let mut expected: Vec<String> = (0..11).map(|i| format!("root/Modal{i}")).collect();
    expected.push("root/=a".into());
    let got: Vec<String> = pane.tree.modals().map(|m| m.id.clone()).collect();
    assert_eq!(got, expected);
}

/// What a pointer entering and leaving raises: the script's own table with `hovering` set, and the view the script draws back.
#[test]
fn on_hover_raises_its_table_with_hovering() {
    let mut pane = load(
        r#"
    return {
        init = function(p) return p or {} end,
        update = function(state, msg) if msg.type == "hover" then state.over = msg.hovering end end,
        view = function(state)
            return ui.Column{ ui.Text("Box"), modifiers = {
                { opacity = if state.over then 1 else 0.2 },
                { animation = { curve = "easeOut", duration = 0.4, delay = if state.over then 0 else 3, value = state.over == true } },
                { onHover = { type = "hover" } },
            } }
        end,
    }
    "#,
    );
    fn opacity(pane: &PluginPane) -> Option<f64> {
        pane.tree.node("root")?.props.get("modifiers")?.as_array()?.first()?.as_object()?.get("opacity")?.as_f64()
    }
    assert_eq!(opacity(&pane), Some(0.2));
    assert!(pane.raise("root", "modifiers.2.onHover", &json!({"hovering": true})));
    assert_eq!(opacity(&pane), Some(1.0));
    assert!(pane.raise("root", "modifiers.2.onHover", &json!({"hovering": false})));
    assert_eq!(opacity(&pane), Some(0.2));
}

#[test]
fn raise_on_a_dropped_node_returns_false() {
    let mut pane = load(
        r#"
    return {
        init = function(p) return p or { shown = true } end,
        update = function(state, msg) state.shown = false end,
        view = function(state)
            if state.shown then return ui.Column{ ui.Button{ child = ui.Text("Hide"), action = { type = "hide" } } } end
            return ui.Column{}
        end,
    }
    "#,
    );
    assert!(pane.tree.node("root/Button0").is_some());
    assert!(pane.raise("root/Button0", "action", &Json::Null));
    assert!(pane.tree.node("root/Button0").is_none());
    assert!(!pane.raise("root/Button0", "action", &Json::Null));
}

#[test]
fn a_changed_table_row_is_the_only_row_written() {
    let mut pane = load(
        r#"
    return {
        init = function(p) return p or { n = 0 } end,
        update = function(state, msg) state.n += 1 end,
        view = function(state)
            local rows = {}
            for i = 1, 50 do
                local text = if i == 7 then "seven " .. state.n else "row " .. i
                table.insert(rows, { id = "r" .. i, cells = { { key = "t", text = text } } })
            end
            return ui.Column{
                ui.Button{ child = ui.Text("Bump"), action = { type = "bump" } },
                ui.Table{ columns = { { key = "t", label = "T" } }, sections = { { rows = rows } } },
            }
        end,
    }
    "#,
    );
    let row1 = ident(&pane, "root/Table0/r1").unwrap();
    assert!(pane.press("root/Button0"));
    assert_eq!(pane.perf.nodes, 1);
    assert_eq!(ident(&pane, "root/Table0/r1").unwrap(), row1);
    let text = prop(&pane, "root/Table0/r7", "cells").and_then(|c| c.as_array()?.first()?.as_object()?.get("text").cloned());
    assert_eq!(text, Some(json!("seven 1")));
}

#[test]
fn a_cells_view_is_its_rows_embedded_node() {
    let mut pane = load(
        r#"
    return {
        init = function(p) return p or { n = 0 } end,
        update = function(state, msg) state.n += 1 end,
        view = function(state)
            local rows = {}
            for i = 1, 3 do
                local label = if i == 2 then "two " .. state.n else "row " .. i
                table.insert(rows, { id = "r" .. i, cells = { { key = "t", text = "row " .. i, view = ui.Text(label) } } })
            end
            return ui.Column{
                ui.Button{ child = ui.Text("Bump"), action = { type = "bump" } },
                ui.Table{ columns = { { key = "t", label = "T" } }, sections = { { rows = rows } } },
            }
        end,
    }
    "#,
    );
    assert!(pane.tree.node("root/Table0").unwrap().embedded.is_empty());
    let cell_ref = pane.tree.node("root/Table0/r2").unwrap().embedded.get("cells.0.view").cloned().unwrap();
    assert_eq!(cell_ref.id, "root/Table0/r2@cells.0.view");
    let cell = ident(&pane, &cell_ref.id).unwrap();
    assert_eq!(cell.1, cell_ref.serial);
    assert_eq!(prop(&pane, &cell_ref.id, "text"), Some(json!("two 0")));
    assert!(pane.press("root/Button0"));
    // Only the cell's own Text changed: the row node and the table keep their props.
    assert_eq!(pane.perf.nodes, 1);
    let after = pane.tree.node("root/Table0/r2").unwrap().embedded.get("cells.0.view").cloned().unwrap();
    assert_eq!(after, cell_ref);
    assert_eq!(ident(&pane, &after.id).unwrap(), cell);
    assert_eq!(prop(&pane, &cell_ref.id, "text"), Some(json!("two 1")));
}

#[test]
fn set_writes_one_prop_only_when_it_differs() {
    let mut pane = load(SEARCH);
    pane.tree.set("root/Text0", "text", Some(json!("Fruit")));
    assert_eq!(pane.tree.written(), 0);
    pane.tree.set("root/Text0", "text", Some(json!("Veg")));
    assert_eq!(pane.tree.written(), 1);
    assert_eq!(prop(&pane, "root/Text0", "text"), Some(json!("Veg")));
}
