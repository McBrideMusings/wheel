mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::*;
use serde_json::{Value as Json, json};

fn data() -> Json {
    json!({"pane": "p1", "space": "s1", "cwd": "/p"})
}

/// Every Button below `node`: its hover text (the `help` modifier) and its action.
fn buttons_of(node: &PaneNode) -> Vec<(String, Json)> {
    let mut out = vec![];
    if node.component == "Button"
        && let Some(action) = node.props.get("action") {
            out.push((button_mods(&node.props).0, action.clone()));
        }
    for c in &node.children {
        out.extend(buttons_of(c));
    }
    out
}

fn pane(feeds: impl Fn(&str, &Json) -> Json + 'static, actions: Rec) -> PaneRuntime {
    PaneRuntime::new(&read("plugins/runner/panes/runner.luau"), Default::default(), "pane", Limits::default(), host(feeds, actions)).unwrap()
}

/// Four commands: `build` and `deploy` (asks first, has a status check) at the top, `prototypes.terrarium.dev`
/// and `machine.hello` in groups. `deploy` ran once and ended with 65, `build` goes now.
const COMMANDS: &str = r#"
{"dir":"/p","now":1000,"runners":[
  {"name":"build","label":"Build","group":[],"desc":"Builds it","command":"make","confirm":null,"origin":"project","worktree":false,"status":false},
  {"name":"deploy","label":"Deploy","group":[],"desc":null,"command":"make deploy","confirm":"Ends every agent.","origin":"project","worktree":false,"status":true},
  {"name":"prototypes.terrarium.dev","label":"dev","group":["prototypes","terrarium"],"desc":null,"command":"bun run dev","confirm":null,"origin":"project","worktree":false,"status":false},
  {"name":"machine.hello","label":"hello","group":["machine"],"desc":null,"command":"echo hi","confirm":null,"origin":"machine","worktree":false,"status":false}],
 "runs":[
  {"id":"r2","name":"build","state":"running","exit_code":null,"started":940,"ended":null},
  {"id":"r1","name":"deploy","state":"exited","exit_code":65,"started":100,"ended":110}],
 "checks":{"deploy":{"up":true,"text":"v42"}}}
"#;

fn log(lines: &[String], state: &str) -> Json {
    json!({"id": "r2", "name": "build", "state": state, "exit_code": null, "started": 940, "ended": null, "lines": lines, "error": null})
}

fn press(rt: &mut PaneRuntime, tree: &PaneNode, help: &str) -> PaneNode {
    let btns = buttons_of(tree);
    let Some((_, action)) = btns.iter().find(|b| b.0 == help) else {
        panic!("no button \"{help}\" in {:?}", btns.iter().map(|b| &b.0).collect::<Vec<_>>());
    };
    rt.send(action).unwrap();
    rt.view(&data()).unwrap()
}

fn commands_pane(actions: Rec) -> PaneRuntime {
    let commands = parse(COMMANDS);
    pane(move |_, _| commands.clone(), actions)
}

fn has(all: &[String], t: &str) -> bool {
    all.iter().any(|x| x == t)
}

#[test]
fn the_list_draws_commands_groups_the_running_section_and_each_row() {
    let mut rt = commands_pane(rec());
    let tree = rt.view(&data()).unwrap();
    let all = texts_child(&tree);
    assert_eq!(tree.children.len(), 4);
    // The Running section pinned above the list holds the run that goes, with its duration.
    assert_eq!(texts_child(&tree.children[2]), ["Running", "Build", "1:00", "Open", "Stop"]);
    // The tree: commands first, then groups with their counts, the machine's commands included.
    assert!(has(&all, "Build") && has(&all, "Deploy"));
    assert!(has(&all, "prototypes  1") && has(&all, "terrarium  1") && has(&all, "machine  1"));
    assert!(has(&all, "dev") && has(&all, "hello"));
    // Each row's latest run, and a status check's reading.
    assert!(has(&all, "running 1:00"));
    assert!(has(&all, "exit 65 · 00:01:40"));
    assert!(has(&all, "up · v42"));
    assert!(has(&all, "Builds it"));
}

#[test]
fn run_starts_a_command_through_the_backend() {
    let actions = rec();
    let mut rt = commands_pane(actions.clone());
    let mut tree = rt.view(&data()).unwrap();
    press(&mut rt, &tree, "Run Build");
    assert!(actions.borrow().len() == 1 && actions.borrow()[0].0 == "runner.run");
    same(&actions.borrow()[0].1, json!({"dir": "/p", "name": "build"}));
    // The answer clears nothing to show; a guard that failed shows its message in the fixed line.
    rt.send(&parse(r#"{"action":"runner.run","result":{"started":false,"guard":"the tree is dirty"}}"#)).unwrap();
    tree = rt.view(&data()).unwrap();
    assert!(has(&texts_child(&tree), "build did not start: the tree is dirty"));
    // Open reads a run's log.
    tree = press(&mut rt, &tree, "Open the log of Build");
    assert!(has(&texts_child(&tree), "Back"));
}

#[test]
fn a_command_with_a_question_asks_in_its_row_and_cancel_starts_nothing() {
    let actions = rec();
    let mut rt = commands_pane(actions.clone());
    let mut tree = rt.view(&data()).unwrap();
    tree = press(&mut rt, &tree, "Run Deploy");
    assert!(actions.borrow().is_empty(), "pressing Run on a command with a question starts nothing");
    assert!(has(&texts_child(&tree), "Ends every agent."));
    let helps: Vec<String> = buttons_of(&tree).into_iter().map(|b| b.0).collect();
    assert!(has(&helps, "Run Deploy: Ends every agent.") && has(&helps, "Do not run Deploy"), "{helps:?}");
    // Cancel: the question goes, nothing started.
    tree = press(&mut rt, &tree, "Do not run Deploy");
    assert!(actions.borrow().is_empty());
    assert!(!has(&texts_child(&tree), "Ends every agent."));
    // Ask again and answer: the run starts with the command text it was asked about.
    tree = press(&mut rt, &tree, "Run Deploy");
    press(&mut rt, &tree, "Run Deploy: Ends every agent.");
    assert!(actions.borrow().len() == 1 && actions.borrow()[0].0 == "runner.run");
    same(&actions.borrow()[0].1, json!({"dir": "/p", "name": "deploy", "confirmed": "make deploy"}));
    rt.send(&parse(r#"{"action":"runner.run","result":{"started":true,"id":"r3"}}"#)).unwrap();
    tree = rt.view(&data()).unwrap();
    assert!(!has(&texts_child(&tree), "Ends every agent."));
}

#[test]
fn search_narrows_the_list_and_folding_a_group_hides_its_rows() {
    let mut rt = commands_pane(rec());
    let mut tree = rt.view(&data()).unwrap();
    // Folding `prototypes` hides terrarium and dev; the other rows stay.
    tree = press(&mut rt, &tree, "Fold prototypes");
    let mut all = texts_child(&tree);
    assert!(has(&all, "prototypes  1") && !has(&all, "terrarium  1") && !has(&all, "dev"), "{all:?}");
    assert!(has(&all, "hello") && has(&all, "Build"));
    // Opening it again shows them.
    tree = press(&mut rt, &tree, "Open prototypes");
    assert!(has(&texts_child(&tree), "dev"));
    // Search narrows to what holds the text, groups with nothing left go, and a fold never hides a match.
    press(&mut rt, &tree, "Fold machine");
    rt.send(&parse(r#"{"type":"query","value":"HELLO"}"#)).unwrap();
    tree = rt.view(&data()).unwrap();
    // The Running section stays above the list whatever the search says; the list is the fourth child.
    all = texts_child(&tree.children[3]);
    assert!(has(&all, "hello") && has(&all, "machine  1"), "{all:?}");
    assert!(!has(&all, "Build") && !has(&all, "Deploy") && !has(&all, "dev") && !has(&all, "prototypes  1"), "{all:?}");
    rt.send(&parse(r#"{"type":"query","value":"zzz"}"#)).unwrap();
    tree = rt.view(&data()).unwrap();
    assert_eq!(texts_child(&tree.children[3]), ["Nothing matches the search."]);
}

#[test]
fn the_log_fills_the_pane_follows_new_lines_and_stops() {
    let commands = parse(COMMANDS);
    let lines = Rc::new(RefCell::new(vec!["12:00:01  one".to_string(), "12:00:02  two".to_string()]));
    let actions = rec();
    let shared = lines.clone();
    let mut rt = pane(
        move |name, params| {
            if name == "runner.log" {
                same(params, json!({"dir": "/p", "id": "r2"}));
                return log(&shared.borrow(), "running");
            }
            commands.clone()
        },
        actions.clone(),
    );
    let mut tree = rt.view(&data()).unwrap();
    tree = press(&mut rt, &tree, "Open the log of Build");
    assert!(tree.children.len() == 3 && tree.children[2].component == "List");
    assert_eq!(texts_child(&tree.children[2]), *lines.borrow());
    same(&prop(&tree.children[2], "scrollTo"), json!(2));
    let head = texts_child(&tree.children[0]);
    assert!(has(&head, "Back") && has(&head, "Build") && has(&head, "running 1:00") && has(&head, "Stop"), "{head:?}");
    // The log grows: the view follows its last line.
    lines.borrow_mut().push("12:00:03  three".into());
    tree = rt.view(&data()).unwrap();
    assert_eq!(texts_child(&tree.children[2]).last().map(String::as_str), Some("12:00:03  three"));
    same(&prop(&tree.children[2], "scrollTo"), json!(3));
    // Stop asks the backend to end that run.
    tree = press(&mut rt, &tree, "Stop Build");
    assert!(actions.borrow().len() == 1 && actions.borrow()[0].0 == "runner.stop");
    same(&actions.borrow()[0].1, json!({"dir": "/p", "id": "r2"}));
    // Back returns to the list.
    tree = press(&mut rt, &tree, "Back to the commands");
    assert_eq!(tree.children.len(), 4);
}

#[test]
fn the_log_of_a_stopped_run_shows_its_state_and_no_stop() {
    let commands = parse(COMMANDS);
    let ended = parse(r#"{"id":"r2","name":"build","state":"stopped","exit_code":143,"started":940,"ended":950,"lines":["12:00:01  one"],"error":null}"#);
    let mut rt = pane(move |name, _| if name == "runner.log" { ended.clone() } else { commands.clone() }, rec());
    let mut tree = rt.view(&data()).unwrap();
    tree = press(&mut rt, &tree, "Open the log of Build");
    let head = texts_child(&tree.children[0]);
    assert!(has(&head, "stopped") && !has(&head, "Stop"), "{head:?}");
}

#[test]
fn a_log_that_cannot_be_read_says_why() {
    let commands = parse(COMMANDS);
    let broken = parse(r#"{"id":"r2","name":null,"state":null,"exit_code":null,"started":null,"ended":null,"lines":[],"error":"no such run"}"#);
    let mut rt = pane(move |name, _| if name == "runner.log" { broken.clone() } else { commands.clone() }, rec());
    let mut tree = rt.view(&data()).unwrap();
    tree = press(&mut rt, &tree, "Open the log of Build");
    assert_eq!(texts_child(&tree.children[2]), ["no such run"]);
}

#[test]
fn a_backend_error_shows_in_the_fixed_line_and_nothing_is_read_yet_draws_a_placeholder() {
    let mut rt = commands_pane(rec());
    let mut tree = rt.view(&data()).unwrap();
    tree = press(&mut rt, &tree, "Run Build");
    let _ = tree;
    rt.send(&parse(r#"{"action":"runner.run","error":"run failed: sh: no such file"}"#)).unwrap();
    tree = rt.view(&data()).unwrap();
    assert_eq!(texts_child(&tree.children[1]), ["run failed: sh: no such file"]);
    let mut empty = pane(|_, _| Json::Null, rec());
    assert_eq!(texts_child(&empty.view(&data()).unwrap()), ["Reading the commands…"]);
}
