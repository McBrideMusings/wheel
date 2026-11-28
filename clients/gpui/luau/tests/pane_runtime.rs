mod common;

use std::collections::HashMap;

use common::*;
use serde_json::json;

const COUNTER: &str = r#"
local function init(previous)
    return previous or { count = 0 }
end
local function update(state, msg)
    if msg.type == "inc" then state.count += msg.by or 1 end
    return state
end
local function view(state, data)
    return ui.VStack{
        ui.Text("Count " .. state.count),
        ui.Button{ label = "Add", action = { type = "inc" } },
    }
end
return { init = init, update = update, view = view }
"#;

const USES_UI: &str = r#"
local ui_lib = require("lib/ui")
return { view = function(state) return ui_lib.title("Hello") end }
"#;

fn null() -> serde_json::Value {
    serde_json::Value::Null
}

// PaneRuntimeTests

#[test]
fn counter_increments_on_a_message_and_view_shows_the_count() {
    let mut pane = new(COUNTER);
    assert_eq!(text(&pane.view(&null()).unwrap().children[0]).as_deref(), Some("Count 0"));
    pane.send(&json!({"type": "inc"})).unwrap();
    pane.send(&json!({"type": "inc", "by": 2})).unwrap();
    let tree = pane.view(&null()).unwrap();
    assert_eq!(tree.component, "VStack");
    assert_eq!(tree.children.iter().map(|c| c.component.as_str()).collect::<Vec<_>>(), ["Text", "Button"]);
    assert_eq!(text(&tree.children[0]).as_deref(), Some("Count 3"));
    same(&prop(&tree.children[1], "action"), json!({"type": "inc"}));
}

#[test]
fn reload_keeps_the_state_through_init_previous() {
    let mut pane = new(COUNTER);
    pane.send(&json!({"type": "inc"})).unwrap();
    pane.send(&json!({"type": "inc"})).unwrap();
    pane.reload(&COUNTER.replace("\"Count \"", "\"Total \""), HashMap::new()).unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap().children[0]).as_deref(), Some("Total 2"));
    same(&pane.state().unwrap(), json!({"count": 2}));
}

#[test]
fn an_infinite_loop_is_stopped_by_the_step_budget() {
    let mut pane = new_with(
        r#"
        return {
            update = function(state, msg)
                while true do
                    pcall(function() while true do end end)
                end
            end,
            view = function() return ui.Text("alive") end,
        }
        "#,
        Limits { steps: 100_000, ..Limits::default() },
    );
    let e = err(pane.send(&null()));
    assert!(matches!(&e, Failure::StepBudget(m) if m.contains("step budget")), "{e:?}");
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("alive"));
}

#[test]
fn allocating_past_the_memory_limit_is_stopped() {
    let mut pane = new_with(
        r#"
        return {
            update = function(state, msg)
                local t = {}
                for i = 1, 10000000 do t[i] = string.rep("x", 64) .. i end
            end,
            view = function() return ui.Text("alive") end,
        }
        "#,
        Limits { memory: 4 << 20, steps: 100_000_000 },
    );
    assert_eq!(err(pane.send(&null())), Failure::Memory("the script ran past its memory limit of 4194304 bytes".into()));
    assert!(pane.memory_used() < 4 << 20, "{}", pane.memory_used());
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("alive"));
}

#[test]
fn io_and_os_are_absent_and_require_is_the_package_one() {
    let mut pane = new(
        r#"
        return {
            view = function()
                return ui.Text(table.concat({ type(io), type(os), type(require), type(getfenv and getfenv().os) }, " "))
            end,
        }
        "#,
    );
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("nil nil function nil"));
}

#[test]
fn globals_and_libraries_are_frozen() {
    let mut pane = new(
        r#"
        return {
            view = function()
                local ok = pcall(function() string.rep = nil end)
                local uiOk = pcall(function() ui.Text = 1 end)
                return ui.Text(tostring(ok) .. " " .. tostring(uiOk))
            end,
        }
        "#,
    );
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("false false"));
}

#[test]
fn a_syntax_error_on_reload_leaves_the_previous_module_running() {
    let mut pane = new(COUNTER);
    pane.send(&json!({"type": "inc"})).unwrap();
    let e = err(pane.reload("return { view = function( }", HashMap::new()));
    assert!(matches!(e, Failure::Syntax(_)), "{e:?}");
    assert_eq!(text(&pane.view(&null()).unwrap().children[0]).as_deref(), Some("Count 1"));
    pane.send(&json!({"type": "inc"})).unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap().children[0]).as_deref(), Some("Count 2"));
}

#[test]
fn a_runtime_error_in_init_on_reload_leaves_the_previous_module_running() {
    let mut pane = new(COUNTER);
    pane.reload("return { init = function() error('boom') end, view = function() return ui.Text('new') end }", HashMap::new()).unwrap_err();
    assert_eq!(text(&pane.view(&null()).unwrap().children[0]).as_deref(), Some("Count 0"));
}

#[test]
fn a_module_without_view_is_refused() {
    assert_eq!(err(try_new("return { init = function() return {} end }")), Failure::Module("the module has no view function".into()));
}

#[test]
fn view_returning_a_non_tree_is_an_error() {
    let mut pane = new("return { view = function() return 42 end }");
    assert_eq!(err(pane.view(&null())), Failure::View("view returned something that is not a component tree".into()));
}

#[test]
fn feed_and_action_reach_the_host() {
    let actions = rec();
    let h = host(
        |name, params| if name == "build" { json!({"progress": 0.5, "target": params}) } else { null() },
        actions.clone(),
    );
    let mut pane = PaneRuntime::new(
        r#"
        return {
            update = function(state, msg) wheel.action("rebuild", { target = msg.target }) end,
            view = function(state, data)
                local build = data.feed("build", "wheeld")
                return ui.ProgressView{ value = build.progress, label = build.target .. " " .. data.title }
            end,
        }
        "#,
        HashMap::new(),
        "pane",
        Limits::default(),
        h,
    )
    .unwrap();
    let tree = pane.view(&json!({"title": "Build"})).unwrap();
    same(&prop(&tree, "value"), json!(0.5));
    same(&prop(&tree, "label"), json!("wheeld Build"));
    pane.send(&json!({"target": "wheeld"})).unwrap();
    assert_eq!(*actions.borrow(), vec![("rebuild".to_string(), json!({"target": "wheeld"}))]);
}

// Swift prints the params with JSONValue's enum description, `bool(true)`; Rust's Debug of the
// same value is `Bool(true)`.
#[test]
fn a_host_call_from_a_coroutine_reads_the_coroutines_arguments() {
    let h = host_feed(|name, params| json!(format!("{name}:{params:?}")));
    let mut pane = PaneRuntime::new(
        r#"
        return { view = function(state, data)
            local co = coroutine.wrap(function(feedName) return data.feed(feedName, true) end)
            return ui.Text(co("log"))
        end }
        "#,
        HashMap::new(),
        "pane",
        Limits::default(),
        h,
    )
    .unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("log:Bool(true)"));
}

#[test]
fn a_feed_read_returns_its_value_and_whether_it_is_stale() {
    let h = Host {
        feed: Box::new(|name, _| FeedRead { value: json!(name), stale: name == "down" }),
        ..Host::default()
    };
    let mut pane = PaneRuntime::new(
        r#"
        return { view = function(state, data)
            local up, upStale = data.feed("up")
            local down, downStale = data.feed("down")
            return ui.Text(up .. tostring(upStale) .. " " .. down .. tostring(downStale))
        end }
        "#,
        HashMap::new(),
        "pane",
        Limits::default(),
        h,
    )
    .unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("upfalse downtrue"));
}

#[test]
fn a_host_call_without_a_name_is_a_script_error() {
    let mut pane = new("return { view = function(state, data) return ui.Text(data.feed(1)) end }");
    err(pane.view(&null()));
}

// PaneNodeTests

#[test]
fn a_view_is_a_tree_with_nodes_inside_properties() {
    let mut pane = new(
        r#"
        return { view = function()
            return ui.VStack{
                modifiers = { { padding = 12 } },
                "Hello",
                ui.Button{ child = ui.Text("Go"), action = { type = "go" } },
            }
        end }
        "#,
    );
    let root = pane.view(&null()).unwrap();
    assert_eq!(root.component, "VStack");
    same(&prop(&root, "modifiers"), json!([{"padding": 12}]));
    assert_eq!(root.children.iter().map(|c| c.component.as_str()).collect::<Vec<_>>(), ["Text", "Button"]);
    same(&prop(&root.children[0], "text"), json!("Hello"));
    let button = &root.children[1];
    same(&prop(button, "action"), json!({"type": "go"}));
    let child = PaneNode::from_json(prop(button, "child"));
    assert_eq!(child, Some(PaneNode { component: "Text".into(), props: serde_json::from_value(json!({"text": "Go"})).unwrap(), children: vec![] }));
}

// PaneRuntimeHostileScriptTests

#[test]
fn a_module_with_a_metatable_runs_no_script_outside_the_limits() {
    let no_view = Failure::Module("the module has no view function".into());
    assert_eq!(err(try_new("return setmetatable({}, { __index = function() error('boom') end })")), no_view);
    assert_eq!(err(try_new("return setmetatable({}, { __index = function() while true do end end })")), no_view);
    let mut pane = new(
        r#"
        return setmetatable({ view = function() return ui.Text("ok") end },
            { __index = function() while true do end end })
        "#,
    );
    pane.send(&null()).unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("ok"));
}

#[test]
fn a_shared_or_cyclic_table_in_view_is_an_error_not_a_hang() {
    let mut shared = new(
        r#"
        return { view = function()
            local t = ui.Text("x")
            for i = 1, 40 do t = ui.VStack{ t, t } end
            return t
        end }
        "#,
    );
    err(shared.view(&null()));
    let mut cyclic = new(
        r#"
        return { view = function()
            local t = { component = "Text" }
            t.a = t
            t.b = t
            return t
        end }
        "#,
    );
    err(cyclic.view(&null()));
    let mut feed_args = new(
        r#"
        return { view = function(state, data)
            local t = {}
            t.self = t
            return ui.Text(tostring(data.feed("x", t)))
        end }
        "#,
    );
    err(feed_args.view(&null()));
}

#[test]
fn garbage_from_an_earlier_entry_does_not_count_against_the_next() {
    let mut pane = new_with(
        r#"
        return {
            init = function()
                local t = {}
                for i = 1, 200000 do t[i] = string.rep("x", 100) .. i end
                return { n = #t }
            end,
            update = function(state) state.n += #string.rep("y", 15 * 1024 * 1024) - 15 * 1024 * 1024 + 1 end,
            view = function(state) return ui.Text(tostring(state.n)) end,
        }
        "#,
        Limits { memory: 40 << 20, steps: 100_000_000 },
    );
    pane.send(&null()).unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("200001"));
}

#[test]
fn tables_convert_without_dropping_keys() {
    let pane = new(
        r#"
        return {
            init = function() return { list = {}, sparse = { 1, 2, nil, 4 }, mixed = { 1, x = 2 } } end,
            view = function() return ui.Text("x") end,
        }
        "#,
    );
    same(
        &pane.state().unwrap(),
        json!({"list": [], "sparse": {"1": 1, "2": 2, "4": 4}, "mixed": {"1": 1, "x": 2}}),
    );
}

// require

#[test]
fn a_pane_requiring_a_module_draws_what_the_module_builds() {
    let mut pane = PaneRuntime::new(
        USES_UI,
        modules(&[("lib/ui.luau", "local M = {}\nfunction M.title(s) return ui.Text(\"Title: \" .. s) end\nreturn M")]),
        "demo/main.luau",
        Limits::default(),
        Host::default(),
    )
    .unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("Title: Hello"));
}

#[test]
fn a_module_runs_once_per_vm_and_modules_share_it() {
    let mut pane = PaneRuntime::new(
        r#"
        local a = require("lib/count")
        local b = require("./lib/count.luau")
        return { view = function() return ui.Text(tostring(a == b) .. " " .. a.n) end }
        "#,
        modules(&[("lib/count.luau", "return { n = 1 }")]),
        "pane",
        Limits::default(),
        Host::default(),
    )
    .unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("true 1"));
}

#[test]
fn a_reload_runs_the_edited_module() {
    let mut pane = PaneRuntime::new(
        USES_UI,
        modules(&[("lib/ui.luau", "return { title = function(s) return ui.Text(\"one \" .. s) end }")]),
        "pane",
        Limits::default(),
        Host::default(),
    )
    .unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("one Hello"));
    pane.reload(USES_UI, modules(&[("lib/ui.luau", "return { title = function(s) return ui.Text(\"two \" .. s) end }")])).unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("two Hello"));
}

#[test]
fn a_failed_reload_keeps_the_modules_it_had() {
    // The second view requires a module the first never did, so it must come from the old map.
    let mut pane = PaneRuntime::new(
        r#"
        local views = 0
        return { view = function()
            views += 1
            return require(if views == 1 then "lib/x" else "lib/z").node()
        end }
        "#,
        modules(&[
            ("lib/x.luau", "return { node = function() return ui.Text(\"kept\") end }"),
            ("lib/z.luau", "return { node = function() return ui.Text(\"old map\") end }"),
        ]),
        "pane",
        Limits::default(),
        Host::default(),
    )
    .unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("kept"));
    err(pane.reload(
        "return { view = function() return ui.Text(require(\"lib/x\")) end, init = function() require(\"lib/y\") end }",
        HashMap::new(),
    ));
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("old map"));
}

#[test]
fn a_path_out_of_the_package_is_an_error() {
    for path in ["../x", "lib/../../x", "/etc/x", "./../x"] {
        let source = format!("require(\"{path}\")\nreturn {{ view = function() return ui.Text(\"\") end }}");
        let e = err(PaneRuntime::new(&source, HashMap::new(), "demo/main.luau", Limits::default(), Host::default()));
        let Failure::Runtime(message) = &e else { panic!("{path}: {e:?}") };
        assert!(message.contains("demo/main.luau:1:") && message.contains("the path leaves the package"), "{path}: {message}");
    }
}

#[test]
fn a_path_that_climbs_back_into_the_package_resolves() {
    let mut pane = PaneRuntime::new(
        r#"
        local a = require("lib/../ui")
        local b = require("./x/../ui.luau")
        return { view = function() return ui.Text(tostring(a == b) .. " " .. a) end }
        "#,
        modules(&[("ui.luau", "return \"root ui\"")]),
        "pane",
        Limits::default(),
        Host::default(),
    )
    .unwrap();
    assert_eq!(text(&pane.view(&null()).unwrap()).as_deref(), Some("true root ui"));
}

#[test]
fn a_missing_module_is_an_error() {
    let e = err(try_new("require(\"lib/none\")\nreturn { view = function() return ui.Text(\"\") end }"));
    let Failure::Runtime(message) = &e else { panic!("{e:?}") };
    assert!(message.contains("the package has no lib/none.luau"), "{message}");
}

#[test]
fn a_require_cycle_is_an_error_and_the_next_entry_is_not_blamed_for_it() {
    let mods = modules(&[("a.luau", "require(\"b\")\nreturn 1"), ("b.luau", "require(\"a\")\nreturn 2")]);
    let e = err(PaneRuntime::new(
        "require(\"a\")\nreturn { view = function() return ui.Text(\"\") end }",
        mods,
        "demo/main.luau",
        Limits::default(),
        Host::default(),
    ));
    let Failure::Runtime(message) = &e else { panic!("{e:?}") };
    assert!(message.contains("demo/b.luau:1:") && message.contains("a.luau is already being required (a require cycle)"), "{message}");
    // A module that failed while loading is not left marked as loading.
    let mut pane = PaneRuntime::new(
        r#"
        local function view()
            local ok, err = pcall(require, "bad")
            local ok2, err2 = pcall(require, "bad")
            return ui.Text(tostring(err2))
        end
        return { view = view }
        "#,
        modules(&[("bad.luau", "error(\"broken\")")]),
        "demo/main.luau",
        Limits::default(),
        Host::default(),
    )
    .unwrap();
    let first = text(&pane.view(&null()).unwrap()).unwrap_or_default();
    assert!(first.contains("broken"), "{first}");
    let second = text(&pane.view(&null()).unwrap()).unwrap_or_default();
    assert!(second.contains("broken") && !second.contains("cycle"), "{second}");
}

#[test]
fn a_syntax_error_in_a_module_names_the_module() {
    let e = err(PaneRuntime::new(
        "require(\"lib/x\")\nreturn { view = function() return ui.Text(\"\") end }",
        modules(&[("lib/x.luau", "local = =")]),
        "demo/main.luau",
        Limits::default(),
        Host::default(),
    ));
    let Failure::Runtime(message) = &e else { panic!("{e:?}") };
    assert!(message.contains("demo/lib/x.luau"), "{message}");
}
