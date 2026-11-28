mod common;

use std::collections::HashMap;

use base64::Engine;
use common::*;
use serde_json::{Value as Json, json};
use wheel_luau::{CanvasFont, Design};

fn pane_args(draw: &str, canvas_args: &str) -> PaneRuntime {
    new(&format!(
        r#"
    return {{
        view = function()
            return ui.Canvas{{ {canvas_args} draw = function(ctx, w, h)
                {draw}
            end }}
        end,
    }}
    "#
    ))
}

fn pane(draw: &str) -> PaneRuntime {
    pane_args(draw, "")
}

fn commands(node: &PaneNode) -> Vec<Json> {
    match node.props.get("commands") {
        Some(Json::Array(list)) => list.clone(),
        _ => vec![],
    }
}

fn null() -> Json {
    Json::Null
}

fn view_err(rt: &mut PaneRuntime) -> Failure {
    err(rt.view(&null()))
}

#[test]
fn every_call_records_its_command() {
    let node = pane(
        r#"
        ctx:beginPath() ctx:moveTo(1, 2) ctx:lineTo(3, 4) ctx:bezierCurveTo(1, 2, 3, 4, 5, 6)
        ctx:quadraticCurveTo(1, 2, 3, 4) ctx:arc(1, 2, 3, 0, 1.5) ctx:arc(1, 2, 3, 0, 1.5, true)
        ctx:rect(1, 2, 3, 4) ctx:roundRect(1, 2, 3, 4, 5) ctx:closePath() ctx:fill() ctx:stroke()
        ctx:save() ctx:translate(5, 6) ctx:scale(2, 3) ctx:rotate(0.5) ctx:restore()
        ctx:fillText('hi', 7, 8)
        ctx.moveTo(ctx, 9, 9)
        "#,
    )
    .view(&null())
    .unwrap();
    assert_eq!(node.component, "Canvas");
    same(
        &Json::Array(commands(&node)),
        json!([
            ["beginPath"],
            ["moveTo", 1, 2],
            ["lineTo", 3, 4],
            ["bezierCurveTo", 1, 2, 3, 4, 5, 6],
            ["quadraticCurveTo", 1, 2, 3, 4],
            ["arc", 1, 2, 3, 0, 1.5, false],
            ["arc", 1, 2, 3, 0, 1.5, true],
            ["rect", 1, 2, 3, 4],
            ["roundRect", 1, 2, 3, 4, 5],
            ["closePath"],
            ["fill"],
            ["stroke"],
            ["save"],
            ["translate", 5, 6],
            ["scale", 2, 3],
            ["rotate", 0.5],
            ["restore"],
            ["fillText", "hi", 7, 8],
            ["moveTo", 9, 9],
        ]),
    );
    same(&prop(&node, "height"), json!(240));
    same(&prop(&node, "canvasKey"), json!("canvas"));
    assert!(!has_prop(&node, "draw"));
}

#[test]
fn style_fields_record_set_commands_and_restore_reads_back() {
    let mut runtime = new(
        r#"
        return { view = function()
            local seen = {}
            local node = ui.Canvas{ draw = function(ctx, w, h)
                seen.fill0, seen.font0, seen.alpha0 = ctx.fillStyle, ctx.font, ctx.globalAlpha
                ctx.fillStyle = 'red'
                ctx:save()
                ctx.fillStyle = '#00f'
                ctx.font = 'bold 13px sans-serif'
                ctx.lineWidth = 3
                ctx.globalAlpha = 0.5
                ctx.strokeStyle = 'accent'
                seen.fill1 = ctx.fillStyle
                ctx:restore()
                seen.fill2, seen.font2, seen.width2 = ctx.fillStyle, ctx.font, ctx.lineWidth
            end }
            node.seen = seen
            return node
        end }
        "#,
    );
    let node = runtime.view(&null()).unwrap();
    same(
        &prop(&node, "seen"),
        json!({
            "fill0": "fg", "font0": "10px sans-serif", "alpha0": 1,
            "fill1": "#00f", "fill2": "red", "font2": "10px sans-serif",
            "width2": 1,
        }),
    );
    same(
        &Json::Array(commands(&node)),
        json!([
            ["set", "fillStyle", "red"], ["save"],
            ["set", "fillStyle", "#00f"], ["set", "font", "bold 13px sans-serif"],
            ["set", "lineWidth", 3], ["set", "globalAlpha", 0.5], ["set", "strokeStyle", "accent"],
            ["restore"],
        ]),
    );
}

#[test]
fn measure_text_grows_with_text_and_font_size() {
    let mut runtime = new(
        r#"
        return { view = function()
            local out = {}
            ui.Canvas{ draw = function(ctx)
                out.short = ctx:measureText('ab')
                out.long = ctx:measureText('abcdefgh')
                ctx.font = '30px sans-serif'
                out.big = ctx:measureText('ab')
            end }
            return ui.Text{ text = 'x', out = out }
        end }
        "#,
    );
    let node = runtime.view(&null()).unwrap();
    let out = prop(&node, "out");
    let f = |a: &str, b: &str| out[a][b].as_f64().unwrap_or_else(|| panic!("measureText did not return its table: {:?}", node.props));
    let (sw, lw, bw) = (f("short", "width"), f("long", "width"), f("big", "width"));
    let (ascent, descent) = (f("short", "fontBoundingBoxAscent"), f("short", "fontBoundingBoxDescent"));
    assert!(sw > 0.0);
    assert!(lw > sw);
    assert!(bw > sw);
    assert!(ascent > 0.0 && descent > 0.0);
}

#[test]
fn set_canvas_size_changes_what_draw_gets_and_reports_change() {
    let mut runtime = new(
        r#"
        return { view = function()
            return ui.Canvas{ key = 'chart', height = 100, draw = function(ctx, w, h) ctx:rect(0, 0, w, h) end }
        end }
        "#,
    );
    same(&Json::Array(commands(&runtime.view(&null()).unwrap())), json!([["rect", 0, 0, 0, 100]]));
    assert!(runtime.set_canvas_size("chart", 320.0, 200.0));
    same(&Json::Array(commands(&runtime.view(&null()).unwrap())), json!([["rect", 0, 0, 320, 200]]));
    assert!(!runtime.set_canvas_size("chart", 320.2, 200.0));
    assert!(runtime.set_canvas_size("chart", 321.0, 200.0));
    assert!(runtime.set_canvas_size("other", 5.0, 5.0));
    runtime
        .reload(
            r#"
        return { view = function()
            return ui.Canvas{ key = 'chart', draw = function(ctx, w, h) ctx:rect(0, 0, w, h) end }
        end }
        "#,
            HashMap::new(),
        )
        .unwrap();
    same(&Json::Array(commands(&runtime.view(&null()).unwrap())), json!([["rect", 0, 0, 321, 200]]));
}

#[test]
fn draw_image_sends_the_buffer_as_base64() {
    let node = pane(
        r#"
        local data = buffer.create(8)
        for i = 0, 7 do buffer.writeu8(data, i, i * 10) end
        ctx:drawImage({ width = 2, height = 1, data = data }, 3, 4)
        ctx:drawImage({ width = 2, height = 1, data = data }, 3, 4, 20, 10)
        "#,
    )
    .view(&null())
    .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD.encode([0u8, 10, 20, 30, 40, 50, 60, 70]);
    same(
        &Json::Array(commands(&node)),
        json!([
            ["drawImage", bytes, 2, 1, 3, 4, 2, 1],
            ["drawImage", bytes, 2, 1, 3, 4, 20, 10],
        ]),
    );
}

#[test]
fn a_mismatched_image_buffer_raises_an_error_naming_the_sizes() {
    let mut runtime = pane("ctx:drawImage({ width = 2, height = 2, data = buffer.create(8) }, 0, 0)");
    let message = view_err(&mut runtime).to_string();
    assert!(message.contains("drawImage") && message.contains("2x2") && message.contains("16"), "{message}");
}

#[test]
fn bad_arguments_raise_an_error_naming_the_method() {
    let mut runtime = pane("ctx:lineTo(1, 'x')");
    let message = view_err(&mut runtime).to_string();
    assert!(message.contains("ctx:lineTo expects numbers"), "{message}");
    let mut style = pane("ctx.lineWidth = 'wide'");
    let message = view_err(&mut style).to_string();
    assert!(message.contains("ctx.lineWidth"), "{message}");
}

#[test]
fn a_canvas_without_draw_passes_unchanged() {
    let mut runtime = new(
        r#"
        return { view = function()
            return ui.Canvas{ ops = { { 'line' } }, hits = {}, key = 'k' }
        end }
        "#,
    );
    let node = runtime.view(&null()).unwrap();
    assert_eq!(node.component, "Canvas");
    same(&prop(&node, "ops"), json!([["line"]]));
    assert!(!has_prop(&node, "commands"));
    same(&prop(&node, "key"), json!("k"));
}

#[test]
fn on_pointer_becomes_pointer_action_and_key_becomes_canvas_key() {
    let node = pane_args("", "key = 'c1', height = 90, on_pointer = { type = 'pointer' }, modifiers = { 1 },").view(&null()).unwrap();
    same(&prop(&node, "pointerAction"), json!({"type": "pointer"}));
    same(&prop(&node, "canvasKey"), json!("c1"));
    same(&prop(&node, "height"), json!(90));
    assert!(!has_prop(&node, "on_pointer") && !has_prop(&node, "key"));
    same(&prop(&node, "modifiers"), json!([1]));
    assert_eq!(commands(&node), Vec::<Json>::new());
}

#[test]
fn the_canvas_fixture_draws_and_shows_the_pointer_event() {
    let mut runtime = new(&read("tools/fixtures/plugins/canvas/main.luau"));
    runtime.send(&json!({"type": "pointer", "kind": "down", "x": 12.7, "y": 40})).unwrap();
    let _ = runtime.set_canvas_size("main", 300.0, 200.0);
    let tree = runtime.view(&null()).unwrap();
    same(&prop(&tree.children[0], "text"), json!("pointer down 12 40"));
    let canvas = &tree.children[1];
    let names: std::collections::HashSet<String> =
        commands(canvas).iter().filter_map(|c| c.get(0).and_then(|n| n.as_str()).map(String::from)).collect();
    for name in [
        "beginPath", "moveTo", "lineTo", "bezierCurveTo", "quadraticCurveTo", "arc", "rect", "roundRect", "closePath", "fill", "stroke",
        "save", "restore", "translate", "scale", "rotate", "fillText", "drawImage", "set",
    ] {
        assert!(names.contains(name), "{name}");
    }
    same(&prop(canvas, "pointerAction"), json!({"type": "pointer"}));
}

#[test]
fn a_large_draw_renders_and_too_many_commands_raise_the_named_error() {
    let big = pane("for i = 1, 40000 do ctx:lineTo(i, i) end").view(&null()).unwrap();
    assert_eq!(commands(&big).len(), 40_000);
    let mut runtime = pane("for i = 1, 260000 do ctx:lineTo(i, i) end");
    let message = view_err(&mut runtime).to_string();
    assert!(message.contains("250,000"), "{message}");
}

#[test]
fn a_measure_loop_trips_the_step_budget() {
    let mut runtime = new_with(
        r#"
        return { view = function()
            return ui.Canvas{ draw = function(ctx)
                for i = 1, 100000 do ctx:measureText('t' .. i) end
            end }
        end }
        "#,
        Limits { steps: 1_000_000, ..Limits::default() },
    );
    let e = view_err(&mut runtime);
    assert!(matches!(e, Failure::StepBudget(_)), "{e:?}");
}

#[test]
fn measure_text_refuses_more_than_ten_thousand_bytes() {
    let mut runtime = pane("ctx:measureText(string.rep('a', 10001))");
    let message = view_err(&mut runtime).to_string();
    assert!(message.contains("ctx:measureText takes at most 10,000 bytes of text"), "{message}");
    pane("ctx:measureText(string.rep('a', 10000))").view(&null()).unwrap();
}

#[test]
fn draw_image_copies_the_buffer_at_call_time() {
    let node = pane(
        r#"
        local data = buffer.create(4)
        buffer.writeu8(data, 0, 1)
        ctx:drawImage({ width = 1, height = 1, data = data }, 0, 0)
        buffer.writeu8(data, 0, 200)
        ctx:drawImage({ width = 1, height = 1, data = data }, 0, 0)
        "#,
    )
    .view(&null())
    .unwrap();
    let list = commands(&node);
    assert_eq!(list.len(), 2);
    assert_ne!(list[0][1], list[1][1]);
}

#[test]
fn draw_image_refuses_images_over_2048() {
    let mut runtime = pane("ctx:drawImage({ width = 2049, height = 1, data = buffer.create(2049 * 4) }, 0, 0)");
    let message = view_err(&mut runtime).to_string();
    assert!(message.contains("2048"), "{message}");
}

#[test]
fn an_unknown_ctx_member_names_itself() {
    let mut runtime = pane("ctx:fillRect(0, 0, 1, 1)");
    let message = view_err(&mut runtime).to_string();
    assert!(
        message.contains("ctx has no field or method 'fillRect' (see docs/plugin-runtime.md for the Canvas 2D calls ctx has)"),
        "{message}"
    );
}

#[test]
fn a_command_list_the_script_grew_names_itself() {
    // The recorder cannot pass the limit (250,000 commands of at most 8 values); a table appended after ui.Canvas returned can.
    let mut runtime = new_with(
        r#"
        return { view = function()
            local c = ui.Canvas{ draw = function(ctx) ctx:beginPath() end }
            local big = {}
            for i = 1, 300000 do big[i] = i end
            for _ = 1, 8 do table.insert(c.commands, big) end
            return c
        end }
        "#,
        Limits { memory: 512 << 20, steps: 100_000_000 },
    );
    let message = view_err(&mut runtime).to_string();
    assert!(message.contains("a Canvas holds more than 2,000,000 values in its commands"), "{message}");
}

#[test]
fn the_step_budget_stops_an_infinite_loop_in_draw() {
    let mut runtime = new_with(
        r#"
        return { view = function()
            return ui.Canvas{ draw = function(ctx) while true do end end }
        end }
        "#,
        Limits { steps: 100_000, ..Limits::default() },
    );
    let e = view_err(&mut runtime);
    assert!(matches!(e, Failure::StepBudget(_)), "{e:?}");
}

// CanvasFontTests

#[test]
fn parses_the_css_shorthand() {
    let f = |size, weight, italic, design| CanvasFont { size, weight, italic, design };
    assert_eq!(CanvasFont::parse("bold 13px sans-serif"), f(13.0, 700, false, Design::Default));
    assert_eq!(CanvasFont::parse("600 11px monospace"), f(11.0, 600, false, Design::Monospaced));
    assert_eq!(CanvasFont::parse("italic 12px serif"), f(12.0, 400, true, Design::Serif));
    assert_eq!(CanvasFont::parse(""), f(10.0, 400, false, Design::Default));
}
