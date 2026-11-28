//! Ports PluginPaneTests.swift (WheelUI). Nothing skipped.

mod common;

use std::collections::HashMap;

use common::same;
use serde_json::{Value as Json, json};
use wheel_luau::pane::PluginPane;

const SAMPLE: &str = r#"
local function init(previous)
    return previous or { count = 0, name = "" }
end
local function update(state, msg)
    if msg.type == "add" then
        state.count += 1
    elseif msg.type == "name" then
        state.name = msg.value
    end
end
local function view(state, data)
    return ui.Column{
        modifiers = { { padding = 16 } },
        ui.Text{ text = "Count " .. state.count, variant = "h2" },
        ui.Button{ child = ui.Text("Add one"), action = { type = "add" } },
        ui.TextField{ label = "Name", value = state.name, on_change = { type = "name" } },
        ui.Text(if state.name == "" then "Type a name" else "Hello, " .. state.name),
    }
end
return { init = init, update = update, view = view }
"#;

fn pane_with(source: &str, chunk: &str) -> PluginPane {
    let mut pane = PluginPane::new();
    pane.load(source, HashMap::new(), chunk);
    pane
}

fn texts(pane: &PluginPane) -> Vec<String> {
    pane.component_texts().iter().filter_map(|r| r.get("text").cloned()).collect()
}

fn button(pane: &PluginPane) -> Option<String> {
    let mut buttons: Vec<String> = pane.tree.all().filter(|n| n.component == "Button").map(|n| n.id.clone()).collect();
    buttons.sort();
    buttons.into_iter().next()
}

fn component(pane: &PluginPane, kind: &str) -> String {
    pane.tree.all().find(|n| n.component == kind).unwrap_or_else(|| panic!("no {kind}")).id.clone()
}

#[test]
fn a_load_draws_the_scripts_view() {
    let pane = pane_with(SAMPLE, "sample/main.luau");
    assert_eq!(pane.error(), None);
    assert_eq!(texts(&pane), ["Count 0", "Add one", "Type a name"]);
    assert!(pane.component_texts().iter().any(|r| r.get("type").map(String::as_str) == Some("TextField") && r.get("label").map(String::as_str) == Some("Name")));
}

#[test]
fn pressing_the_button_increments_the_count() {
    let mut pane = pane_with(SAMPLE, "sample/main.luau");
    let id = button(&pane).unwrap();
    assert!(pane.press(&id));
    assert!(pane.press(&id));
    assert_eq!(pane.state_json(), r#"{"count":2,"name":""}"#);
    assert_eq!(texts(&pane).first().map(String::as_str), Some("Count 2"));
}

#[test]
fn a_text_field_change_updates_the_state() {
    let mut pane = pane_with(SAMPLE, "sample/main.luau");
    let field = component(&pane, "TextField");
    pane.set(&field, json!("Ada"));
    assert_eq!(pane.state_json(), r#"{"count":0,"name":"Ada"}"#);
    assert_eq!(texts(&pane).last().map(String::as_str), Some("Hello, Ada"));
}

#[test]
fn a_reload_keeps_the_state() {
    let mut pane = pane_with(SAMPLE, "sample/main.luau");
    let id = button(&pane).unwrap();
    pane.press(&id);
    pane.load(&SAMPLE.replace("Add one", "Add 1"), HashMap::new(), "sample/main.luau");
    assert_eq!(pane.reloads(), 1);
    assert_eq!(texts(&pane), ["Count 1", "Add 1", "Type a name"]);
    assert_eq!(pane.state_json(), r#"{"count":1,"name":""}"#);
}

#[test]
fn a_table_action_naming_event_still_reaches_update() {
    let mut pane = pane_with(
        r#"
    return {
        init = function(p) return p or { on = false } end,
        update = function(state, msg) if msg.event == "toggle" then state.on = not state.on end end,
        view = function(state)
            return ui.Button{ child = ui.Text("Toggle"), action = { event = "toggle" } }
        end,
    }
    "#,
        "toggle.luau",
    );
    assert_eq!(pane.error(), None);
    let id = button(&pane).unwrap();
    assert!(pane.press(&id));
    assert_eq!(pane.state_json(), r#"{"on":true}"#);
}

const CONTROLS: &str = r#"
return {
    init = function(p) return p or { log = {} } end,
    update = function(state, msg)
        local keys = {}
        for k, v in msg do table.insert(keys, k .. "=" .. (if type(v) == "table" then "[" .. #v .. "]" else tostring(v))) end
        table.sort(keys)
        table.insert(state.log, table.concat(keys, ","))
    end,
    view = function(state, data)
        return ui.Column{
            ui.Text("pane " .. tostring(data.pane) .. " cwd " .. tostring(data.cwd) .. " " .. tostring(data.plugin) .. "/" .. tostring(data.plugin_pane)),
            ui.Table{
                columns = { { key = "t", label = "T" } },
                sections = { { rows = { { id = "a", cells = { { key = "t", text = "A" } }, menu = { { label = "Drop", action = { type = "drop", id = "a" } } } } } } },
                rowAction = { type = "open" }, sortAction = { type = "sort" }, foldAction = { type = "fold" },
            },
            ui.Canvas{ ops = {}, hits = { { id = "n1", rect = { 0, 0, 10, 10 }, action = { type = "hit" }, doubleAction = { type = "open", id = "n1" } } } },
            ui.CheckBox{ label = "Done", value = false, on_change = { type = "done" } },
            ui.ChoicePicker{ options = { { label = "A", value = "a" } }, value = {}, on_change = { type = "pick" } },
            ui.Modal{ dismissAction = { type = "dismiss" }, ui.Text("modal") },
        }
    end,
}
"#;

#[test]
fn view_gets_the_panes_context() {
    let mut pane = PluginPane::new();
    for (k, v) in [("pane", "p7"), ("cwd", "/tmp/proj"), ("plugin", "wh"), ("plugin_pane", "beads")] {
        pane.context.insert(k.into(), json!(v));
    }
    pane.load(CONTROLS, HashMap::new(), "controls.luau");
    assert_eq!(pane.error(), None);
    assert_eq!(texts(&pane).first().map(String::as_str), Some("pane p7 cwd /tmp/proj wh/beads"));
}

#[test]
fn every_action_key_reaches_update_with_the_events_fields() {
    let mut pane = pane_with(CONTROLS, "controls.luau");
    let table = component(&pane, "Table");
    let canvas = component(&pane, "Canvas");
    let modal = component(&pane, "Modal");
    assert!(pane.raise(&table, "rowAction", &json!({"id": "a"})));
    assert!(pane.raise(&table, "sortAction", &json!({"key": "t", "asc": false})));
    assert!(pane.raise(&table, "foldAction", &json!({"path": "a"})));
    assert!(pane.raise(&format!("{table}/a"), "menu.0.action", &json!({})));
    assert!(pane.raise(&canvas, "hits.0.action", &json!({"id": "n1"})));
    assert!(pane.raise(&canvas, "hits.0.doubleAction", &json!({"id": "other"})));
    assert!(pane.raise(&modal, "dismissAction", &Json::Null));
    assert!(!pane.raise(&table, "selectAction", &Json::Null));
    assert_eq!(
        pane.state_json(),
        r#"{"log":["id=a,type=open","asc=false,key=t,type=sort","path=a,type=fold","id=a,type=drop","id=n1,type=hit","id=n1,type=open","type=dismiss"]}"#
    );
    // The Table keeps the tables the script wrote, and each row is its own node.
    let props = pane.tree.node(&table).unwrap().props.clone();
    assert_eq!(props["rowAction"], json!({"type": "open"}));
    same(&props["sections"][0]["rows"], json!([format!("{table}/a")]));
}

#[test]
fn every_bound_control_sends_its_on_change() {
    let mut pane = pane_with(CONTROLS, "controls.luau");
    let check = component(&pane, "CheckBox");
    pane.set(&check, json!(true));
    let picker = component(&pane, "ChoicePicker");
    pane.set(&picker, json!(["a"]));
    assert_eq!(pane.state_json(), r#"{"log":["type=done,value=true","type=pick,value=[1]"]}"#);
}

#[test]
fn a_control_the_script_does_not_echo_keeps_the_edit() {
    let mut pane = pane_with(
        r#"
    return {
        init = function(p) return p or { picks = 0, reset = false } end,
        update = function(state, msg) if msg.type == "pick" then state.picks += 1 elseif msg.type == "reset" then state.reset = true end end,
        view = function(state)
            return ui.Column{
                ui.ChoicePicker{ options = { { label = "A", value = "a" } }, on_change = { type = "pick" } },
                ui.TextField{ label = "Name", value = if state.reset then "fresh" else "", on_change = { type = "name" } },
                ui.Button{ child = ui.Text("Reset"), action = { type = "reset" } },
            }
        end,
    }
    "#,
        "echo.luau",
    );
    let picker = component(&pane, "ChoicePicker");
    let field = component(&pane, "TextField");
    pane.set(&picker, json!(["a"]));
    pane.set(&field, json!("Ada"));
    assert_eq!(pane.tree.node(&picker).unwrap().value(), Some(&json!(["a"])));
    assert_eq!(pane.tree.node(&field).unwrap().value(), Some(&json!("Ada")));
    // A value the script changes is written over the edit.
    let reset = component(&pane, "Button");
    assert!(pane.press(&reset));
    assert_eq!(pane.tree.node(&field).unwrap().value(), Some(&json!("fresh")));
}

#[test]
fn data_under_an_action_key_and_a_copy_stay_as_written() {
    let pane = pane_with(
        r#"
    return {
        view = function()
            return ui.Column{
                ui.Canvas{ ops = {}, hits = { { id = "n", rect = { 0, 0, 1, 1 }, action = { type = "hit" }, context = { action = { type = "data" } } } } },
                ui.Button{ child = ui.Text("Copy"), action = { copy = { text = "wheel-1" } } },
            }
        end,
    }
    "#,
        "data.luau",
    );
    let canvas = pane.tree.node(&component(&pane, "Canvas")).unwrap().props.clone();
    let hit = &canvas["hits"][0];
    assert_eq!(hit["action"], json!({"type": "hit"}));
    assert_eq!(hit["context"]["action"]["type"], json!("data"));
    let button = pane.tree.node(&component(&pane, "Button")).unwrap().props.clone();
    assert_eq!(button["action"]["copy"]["text"], json!("wheel-1"));
}

#[test]
fn a_syntax_error_keeps_the_last_view_and_fixing_clears_the_error() {
    let mut pane = pane_with(SAMPLE, "sample/main.luau");
    pane.load(&format!("{SAMPLE}\nlocal = ="), HashMap::new(), "sample/main.luau");
    assert!(pane.error().is_some_and(|e| e.contains("sample/main.luau")));
    assert!(pane.error().is_some_and(|e| !e.contains('\n')));
    assert_eq!(texts(&pane), ["Count 0", "Add one", "Type a name"]);
    assert_eq!(pane.reloads(), 0);
    // A message the old script handles does not clear the error: the file is still broken.
    let id = button(&pane).unwrap_or_default();
    pane.press(&id);
    assert!(pane.error().is_some());
    assert_eq!(texts(&pane).first().map(String::as_str), Some("Count 1"));
    pane.load(SAMPLE, HashMap::new(), "sample/main.luau");
    assert_eq!(pane.error(), None);
    assert_eq!(pane.reloads(), 1);
}

#[test]
fn a_required_module_draws_and_an_edited_one_reloads_and_a_bad_path_shows_in_the_error() {
    let main = r#"
    local lib = require("lib/ui")
    return { view = function() return lib.heading() end }
    "#;
    let module = |word: &str| HashMap::from([("lib/ui.luau".to_string(), format!("return {{ heading = function() return ui.Text(\"{word}\") end }}"))]);
    let mut pane = PluginPane::new();
    pane.load(main, module("one"), "demo/main.luau");
    assert_eq!(pane.error(), None);
    assert_eq!(texts(&pane), ["one"]);
    pane.load(main, module("two"), "demo/main.luau");
    assert_eq!(texts(&pane), ["two"]);
    assert_eq!(pane.reloads(), 1);
    pane.load(&format!("require(\"../x\")\n{main}"), HashMap::new(), "demo/main.luau");
    assert!(pane.error().is_some_and(|e| e.contains("demo/main.luau:1:")));
    assert!(pane.error().is_some_and(|e| e.contains("the path leaves the package")));
    assert_eq!(texts(&pane), ["two"]);
}

#[test]
fn the_component_list_holds_modal_actions_table_rows_and_disabled_buttons() {
    let pane = pane_with(
        r#"
    return {
        init = function(p) return p end,
        update = function() end,
        view = function()
            return ui.Column{
                ui.Table{
                    columns = { { key = "a", label = "A" }, { key = "b", label = "B" } },
                    sections = { { title = "Open", rows = { { id = "r1", cells = { { key = "b", text = "two" }, { key = "a", text = "one" } } } } } },
                },
                ui.Button{ child = ui.Text("Prev"), action = { type = "prev" }, modifiers = { { disabled = true } } },
                ui.Modal{ presented = true, actions = { ui.Button{ child = ui.Text("Press"), action = { type = "go" } } }, ui.Text("body") },
            }
        end,
    }
    "#,
        "list.luau",
    );
    assert_eq!(pane.error(), None);
    let all = pane.component_texts();
    let row = all.iter().find(|r| r.get("type").map(String::as_str) == Some("TableRow")).unwrap();
    assert_eq!(row.get("id").map(String::as_str), Some("r1"));
    assert_eq!(row.get("cells").map(String::as_str), Some("one | two"));
    assert_eq!(row.get("section").map(String::as_str), Some("Open"));
    assert!(all.iter().any(|r| r.get("type").map(String::as_str) == Some("Button") && r.get("disabled").map(String::as_str) == Some("true")));
    let texts: Vec<&String> = all.iter().filter_map(|r| r.get("text")).collect();
    assert!(texts.iter().any(|t| *t == "Press") && texts.iter().any(|t| *t == "body"));
}
