mod common;

use common::*;
use serde_json::{Value as Json, json};

const PACKAGE: &str = "daemon/plugins/agent";

fn texts(node: &PaneNode) -> Vec<String> {
    texts_children(node)
}

const TRANSCRIPT: &str = r#"
{"agent":"claude-1","events":[
  {"kind":"turn_start","turn":1},
  {"kind":"user","turn":1,"text":"fix the bug"},
  {"kind":"text","text":"On it."},
  {"kind":"thought","text":"look at a.rs first"},
  {"kind":"tool_call","id":"c1","title":"Edit a.rs","tool_kind":"edit","status":"pending","content":[]},
  {"kind":"tool_update","id":"c1","status":"completed","content":[
    {"type":"diff","path":"a.rs","old_text":"one\ntwo\nthree","new_text":"one\n2\nthree"},
    {"type":"text","text":"ok"}]},
  {"kind":"plan","entries":[{"content":"write test","status":"pending"},{"content":"fix","status":"completed"}]},
  {"kind":"error","message":"boom"},
  {"kind":"turn_end","turn":1,"stop_reason":"end_turn"}
]}
"#;

fn runtime(host: Host) -> PaneRuntime {
    PaneRuntime::new(&read(&format!("{PACKAGE}/main.luau")), modules_of(PACKAGE), "pane", Limits::default(), host).unwrap()
}

fn transcript_host(events: Json) -> Host {
    host_feed(move |name, _| if name == "wheel.agent.transcript" { events.clone() } else { Json::Null })
}

#[test]
fn the_agent_view_draws_every_event_kind_and_sends_the_draft() {
    let actions = rec();
    let events = parse(TRANSCRIPT);
    let status = parse(r#"{"agent":"claude-1","state":{"phase":"busy","queued":2}}"#);
    let requests = parse(r#"{"agent":"claude-1","requests":[{"id":"r1","agent":"claude-1","title":"Edit a.rs","tool_call":"c1","options":[]}]}"#);
    let h = host(
        move |name, params| {
            same(params, json!({"agent": "p3"}));
            match name {
                "wheel.agent.transcript" => events.clone(),
                "wheel.agent.status" => status.clone(),
                "wheel.agent.requests" => requests.clone(),
                _ => Json::Null,
            }
        },
        actions.clone(),
    );
    let mut rt = runtime(h);
    let data = json!({"pane": "p3", "space": "s1"});
    let mut tree = rt.view(&data).unwrap();
    assert_eq!(tree.children.len(), 4);
    let list = &tree.children[1];
    assert_eq!(list.component, "List");
    // user, text, thought, tool card, plan, error
    assert_eq!(list.children.len(), 6);
    same(&prop(list, "scrollTo"), json!(6));
    let all = texts(list);
    for want in ["fix the bug", "On it.", "Waiting for permission", "- two", "+ 2", "\u{2611} fix", "boom"] {
        assert!(all.iter().any(|t| t == want), "no {want:?} in {all:?}");
    }
    let header = texts(&tree.children[0]);
    assert!(header.contains(&"p3".to_string()) && header.contains(&"busy".to_string()) && header.contains(&"2 queued".to_string()), "{header:?}");

    rt.send(&json!({"type": "draft", "value": "hi"})).unwrap();
    rt.send(&json!({"type": "send"})).unwrap();
    assert_eq!(actions.borrow().len(), 1);
    assert_eq!(actions.borrow()[0].0, "wheel.agent.send");
    same(&actions.borrow()[0].1, json!({"agent": "p3", "text": "hi"}));
    tree = rt.view(&data).unwrap();
    assert_eq!(tree.children.len(), 4);
    // the send's result clears the sending note; an error shows in the fixed line
    rt.send(&json!({"action": "wheel.agent.send", "error": "agent is gone"})).unwrap();
    tree = rt.view(&data).unwrap();
    assert_eq!(texts(&tree.children[3]), ["agent is gone"]);
    // an empty draft sends nothing
    rt.send(&json!({"type": "draft", "value": "   "})).unwrap();
    rt.send(&json!({"type": "send"})).unwrap();
    assert_eq!(actions.borrow().len(), 1);
}

#[test]
fn an_empty_transcript_draws_one_row() {
    let mut rt = runtime(host_feed(|_, _| Json::Null));
    let tree = rt.view(&json!({"pane": "p3"})).unwrap();
    assert_eq!(tree.children[1].children.len(), 1);
    assert_eq!(texts(&tree.children[1]), ["Give this agent a task."]);
    same(&prop(&tree.children[1], "scrollTo"), json!(1));
}

#[test]
fn a_large_diff_is_not_diffed_and_a_long_text_is_capped() {
    let big = (1..=401).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
    let long = (1..=30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let events = json!({"agent": "a", "events": [
        {"kind": "tool_call", "id": "c", "title": "t", "tool_kind": "edit", "status": "completed", "content": [
            {"type": "diff", "path": "big.txt", "old_text": big, "new_text": "x"},
            {"type": "text", "text": long}]}]});
    let mut rt = runtime(transcript_host(events));
    let all = texts(&rt.view(&json!({"pane": "p3"})).unwrap().children[1]);
    assert!(all.contains(&"diff too large: 401 lines".to_string()), "{all:?}");
    assert!(all.iter().any(|t| t.ends_with("\u{2026} 10 more lines") && t.starts_with("line 1\n")), "{all:?}");
}

#[test]
fn a_long_session_of_tool_cards_stays_inside_the_value_limit() {
    // 300 cards of 12 diffs, each changing every other line of 60, so each diff draws about 60 runs: far over the
    // runtime's 100,000 values if every row were drawn.
    let old = (1..=60).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
    let new = (1..=60).map(|i| if i % 2 == 0 { format!("x{i}") } else { i.to_string() }).collect::<Vec<_>>().join("\n");
    let diff = json!({"type": "diff", "path": "f", "old_text": old, "new_text": new});
    let cards: Vec<Json> = (1..=300)
        .map(|i| {
            json!({"kind": "tool_call", "id": format!("c{i}"), "title": format!("t{i}"), "tool_kind": "edit", "status": "completed",
                   "content": vec![diff.clone(); 12]})
        })
        .collect();
    let events = json!({"agent": "a", "events": cards});
    let mut rt = runtime(transcript_host(events));
    let tree = rt.view(&json!({"pane": "p3"})).unwrap();
    let list = &tree.children[1];
    assert!(list.children.len() > 1 && list.children.len() < 300, "{}", list.children.len());
    assert!(texts(list.children.last().unwrap()).contains(&"t300".to_string()));
    assert!(texts(list).contains(&"\u{2026} 4 more items".to_string()));
}
