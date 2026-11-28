mod common;

use common::*;
use serde_json::{Value as Json, json};

fn bead(id: &str) -> Bead<'_> {
    bead_spec(id)
}

/// Ten leaves and an epic. By status word: wheel-1.2 in progress; wheel-1.1, -9, -10, -6 ready;
/// wheel-7, -8, -11 blocked (7 and 8 wait on each other); wheel-2 open; wheel-1.3 done.
fn small() -> Json {
    snapshot(vec![
        Bead { kind: "epic", pri: Some(1), epic: true, ..bead("wheel-1") }.json(),
        Bead { pri: Some(1), parent: Some("wheel-1"), labels: vec!["client"], ready: true, ..bead("wheel-1.1") }.json(),
        Bead { status: "in_progress", pri: Some(2), parent: Some("wheel-1"), labels: vec!["daemon"], ..bead("wheel-1.2") }.json(),
        Bead { status: "closed", pri: Some(3), parent: Some("wheel-1"), ..bead("wheel-1.3") }.json(),
        Bead { pri: Some(1), labels: vec!["client"], ready: true, ..bead("wheel-9") }.json(),
        Bead { kind: "bug", pri: Some(1), labels: vec!["client", "daemon"], ready: true, ..bead("wheel-10") }.json(),
        Bead { pri: Some(2), desc: "flaky socket".into(), ..bead("wheel-2") }.json(),
        Bead { pri: Some(2), ready: true, ..bead("wheel-6") }.json(),
        Bead { pri: Some(2), blocked: vec!["wheel-6", "wheel-8"], ..bead("wheel-7") }.json(),
        Bead { pri: Some(2), blocked: vec!["wheel-7"], ..bead("wheel-8") }.json(),
        Bead { pri: Some(2), blocked: vec!["wheel-2"], ..bead("wheel-11") }.json(),
    ])
}

fn list_pane(value: Json, panes: Json, actions: Rec) -> PaneRuntime {
    wh_pane("beads", "beads", value, panes, actions)
}

fn list() -> PaneRuntime {
    list_pane(small(), Json::Null, rec())
}

fn send(rt: &mut PaneRuntime, kind: &str, fields: Json) {
    rt.send(&wh_msg(kind, fields)).unwrap();
}

fn view(rt: &mut PaneRuntime) -> PaneNode {
    rt.view(&wh_data()).unwrap()
}

fn page_texts(rt: &mut PaneRuntime) -> Vec<String> {
    texts_json(&jsonof(&view(rt)))
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

// list.luau: sort, natural ids, ties

#[test]
fn the_default_list_is_sorted_by_status_with_ties_in_feed_order_and_done_hidden() {
    let mut rt = list();
    assert_eq!(ids(&mut rt), strs(&["wheel-1.2", "wheel-1.1", "wheel-9", "wheel-10", "wheel-6", "wheel-7", "wheel-8", "wheel-11", "wheel-2"]));
}

#[test]
fn a_descending_sort_keeps_tied_rows_in_feed_order() {
    let mut rt = list();
    send(&mut rt, "list.sort", json!({"key": "status", "asc": false}));
    assert_eq!(ids(&mut rt), strs(&["wheel-2", "wheel-7", "wheel-8", "wheel-11", "wheel-1.1", "wheel-9", "wheel-10", "wheel-6", "wheel-1.2"]));
}

#[test]
fn ids_sort_naturally_not_as_text() {
    let mut rt = list();
    send(&mut rt, "list.sort", json!({"key": "id", "asc": true}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1.1", "wheel-1.2", "wheel-2", "wheel-6", "wheel-7", "wheel-8", "wheel-9", "wheel-10", "wheel-11"]));
    send(&mut rt, "list.sort", json!({"key": "id", "asc": false}));
    assert_eq!(ids(&mut rt).first().map(String::as_str), Some("wheel-11"));
}

#[test]
fn a_sort_tie_is_broken_by_status_then_feed_order() {
    let mut rt = list();
    send(&mut rt, "list.sort", json!({"key": "pri", "asc": true}));
    // P1: ready rows in feed order; P2: in progress, ready, blocked in feed order, open last.
    assert_eq!(ids(&mut rt), strs(&["wheel-1.1", "wheel-9", "wheel-10", "wheel-1.2", "wheel-6", "wheel-7", "wheel-8", "wheel-11", "wheel-2"]));
}

#[test]
fn an_unknown_sort_key_changes_nothing() {
    let mut rt = list();
    let before = ids(&mut rt);
    send(&mut rt, "list.sort", json!({"key": "nope", "asc": false}));
    assert_eq!(ids(&mut rt), before);
}

// list.luau: filters, Is not, search, Clear

#[test]
fn a_ticked_filter_keeps_rows_with_one_of_its_values() {
    let mut rt = list();
    send(&mut rt, "list.toggle", json!({"key": "label", "item": "client", "value": true}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1.1", "wheel-9", "wheel-10"]));
    send(&mut rt, "list.toggle", json!({"key": "label", "item": "daemon", "value": true}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1.2", "wheel-1.1", "wheel-9", "wheel-10"]));
    send(&mut rt, "list.toggle", json!({"key": "type", "item": "bug", "value": true}));
    assert_eq!(ids(&mut rt), strs(&["wheel-10"]));
}

#[test]
fn is_not_excludes_the_ticked_values_and_the_last_untick_drops_it() {
    let mut rt = list();
    send(&mut rt, "list.toggle", json!({"key": "label", "item": "client", "value": true}));
    send(&mut rt, "list.not", json!({"key": "label", "value": true}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1.2", "wheel-6", "wheel-7", "wheel-8", "wheel-11", "wheel-2"]));
    // The filter's popover title says it excludes.
    assert!(page_texts(&mut rt).contains(&"Label not: client ⌄".to_string()));
    // Unticking the one value matches every row again and forgets the Is not.
    send(&mut rt, "list.toggle", json!({"key": "label", "item": "client", "value": false}));
    assert_eq!(ids(&mut rt).len(), 9);
    send(&mut rt, "list.toggle", json!({"key": "label", "item": "daemon", "value": true}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1.2", "wheel-10"]));
}

#[test]
fn the_done_status_shows_only_while_its_filter_is_set() {
    let mut rt = list();
    assert!(!ids(&mut rt).contains(&"wheel-1.3".to_string()));
    send(&mut rt, "list.filter", json!({"key": "status", "value": "done"}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1.3"]));
    // A priority filter and a status filter together.
    send(&mut rt, "list.filter", json!({"key": "status", "value": "ready,done"}));
    send(&mut rt, "list.toggle", json!({"key": "pri", "item": "1", "value": true}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1.1", "wheel-9", "wheel-10"]));
}

#[test]
fn search_needs_every_word_in_the_id_title_kind_labels_description_or_owner() {
    let mut rt = list();
    send(&mut rt, "list.query", json!({"value": "daemon bug"}));
    assert_eq!(ids(&mut rt), strs(&["wheel-10"]));
    send(&mut rt, "list.query", json!({"value": "SOCKET"}));
    assert_eq!(ids(&mut rt), strs(&["wheel-2"]));
    send(&mut rt, "list.query", json!({"value": "pierce wheel-9"}));
    assert_eq!(ids(&mut rt), strs(&["wheel-9"]));
    send(&mut rt, "list.query", json!({"value": "zzz"}));
    assert_eq!(ids(&mut rt), Vec::<String>::new());
}

#[test]
fn clear_resets_the_search_filters_and_selection() {
    let mut rt = list();
    send(&mut rt, "list.query", json!({"value": "daemon"}));
    send(&mut rt, "list.toggle", json!({"key": "type", "item": "bug", "value": true}));
    send(&mut rt, "list.not", json!({"key": "type", "value": true}));
    let tree = view(&mut rt);
    let btns = buttons(&tree);
    let clear = btns.iter().find(|b| b.text == "Clear").expect("Clear");
    assert!(!clear.disabled);
    rt.send(&clear.action).unwrap();
    assert_eq!(ids(&mut rt).len(), 9);
    // Clear keeps its slot and is disabled when nothing narrows the list.
    let after = buttons(&view(&mut rt)).into_iter().find(|b| b.text == "Clear").expect("Clear");
    assert!(after.disabled);
}

#[test]
fn the_status_boxes_count_and_set_one_filter_and_tap_again_clears() {
    let mut rt = list();
    let mut tree = view(&mut rt);
    let boxes: Vec<Btn> = buttons(&tree).into_iter().filter(|b| action_type(b).as_deref() == Some("list.filter")).collect();
    assert_eq!(boxes.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["4", "1", "3", "1"]);
    rt.send(&boxes[0].action).unwrap();
    assert_eq!(ids(&mut rt), strs(&["wheel-1.1", "wheel-9", "wheel-10", "wheel-6"]));
    // The box that is on now sends "" to clear.
    tree = view(&mut rt);
    let on = buttons(&tree).into_iter().find(|b| action_type(b).as_deref() == Some("list.filter")).expect("a status box");
    rt.send(&on.action).unwrap();
    assert_eq!(ids(&mut rt).len(), 9);
}

#[test]
fn a_change_to_the_list_clears_the_selection_and_the_actions_count_what_is_selected() {
    let mut rt = list();
    send(&mut rt, "list.select", json!({"ids": ["wheel-9", "wheel-6"]}));
    let mut chips = page_texts(&mut rt);
    assert!(chips.contains(&"2 selected · Actions ⌄".to_string()), "{chips:?}");
    let copy = buttons(&view(&mut rt)).into_iter().find(|b| b.text == "Copy IDs").expect("Copy IDs");
    assert!(copy.help.starts_with("Copy the 2 selected ids"), "{}", copy.help);
    send(&mut rt, "list.query", json!({"value": "bead"}));
    chips = page_texts(&mut rt);
    assert!(chips.contains(&"Actions ⌄".to_string()));
    assert!(!chips.iter().any(|t| t.contains("listed")));
}

// the tree views

#[test]
fn the_epics_view_hangs_children_under_their_epic_and_folds_them() {
    let mut rt = list();
    send(&mut rt, "set_view", json!({"value": ["epics"]}));
    // Children by status rank: in progress, then ready; the closed child stays hidden until the done filter is set.
    let mut r = rows(&view(&mut rt));
    assert_eq!(r.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["wheel-1", "wheel-1.2", "wheel-1.1"]);
    let depths: Vec<Json> = r.iter().map(|r| fl(r.row.get("depth").cloned().unwrap_or(Json::Null))).collect();
    assert_eq!(depths, [json!(0.0), json!(1.0), json!(1.0)]);
    assert_eq!(r[0].row.get("kids"), Some(&json!(true)));
    send(&mut rt, "list.filter", json!({"key": "status", "value": "done,ready,in progress"}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1", "wheel-1.2", "wheel-1.1", "wheel-1.3"]));
    send(&mut rt, "list.clear", json!({}));
    send(&mut rt, "toggle_path", json!({"path": ">wheel-1"}));
    r = rows(&view(&mut rt));
    assert_eq!(r.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["wheel-1"]);
    assert_eq!(r[0].row.get("collapsed"), Some(&json!(true)));
}

#[test]
fn the_depends_on_view_roots_at_a_bead_nothing_kept_waits_on() {
    let value = snapshot(vec![
        bead("wheel-2").json(),
        Bead { blocked: vec!["wheel-2"], ..bead("wheel-11") }.json(),
        Bead { blocked: vec!["wheel-11"], ..bead("wheel-12") }.json(),
    ]);
    let mut rt = list_pane(value, Json::Null, rec());
    send(&mut rt, "set_view", json!({"value": ["deps"]}));
    let r = rows(&view(&mut rt));
    // wheel-12 depends on wheel-11 depends on wheel-2: one chain from the bead that waits on the most.
    assert_eq!(r.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["wheel-12", "wheel-11", "wheel-2"]);
    let depths: Vec<Json> = r.iter().map(|r| fl(r.row.get("depth").cloned().unwrap_or(Json::Null))).collect();
    assert_eq!(depths, [json!(0.0), json!(1.0), json!(2.0)]);
}

#[test]
fn the_unlocks_view_walks_a_dependency_cycle_once() {
    let mut rt = list();
    send(&mut rt, "set_view", json!({"value": ["unlocks"]}));
    let r = rows(&view(&mut rt));
    // wheel-6 unlocks 7, 7 unlocks 8, 8 unlocks 7 again (skipped: it is on the path); wheel-2 unlocks 11.
    assert_eq!(r.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["wheel-6", "wheel-7", "wheel-8", "wheel-2", "wheel-11"]);
    let depths: Vec<Json> = r.iter().map(|r| fl(r.row.get("depth").cloned().unwrap_or(Json::Null))).collect();
    assert_eq!(depths, [json!(0.0), json!(1.0), json!(2.0), json!(0.0), json!(1.0)]);
}

#[test]
fn a_tree_view_keeps_an_ancestor_of_a_match_and_drops_the_rest() {
    let mut rt = list();
    send(&mut rt, "set_view", json!({"value": ["epics"]}));
    send(&mut rt, "list.query", json!({"value": "1.1"}));
    assert_eq!(ids(&mut rt), strs(&["wheel-1", "wheel-1.1"]));
}

#[test]
fn a_tree_view_has_no_sort_action_and_the_list_has_one() {
    let mut rt = list();
    let mut table = all(&view(&mut rt), "Table").into_iter().next().expect("a Table");
    assert!(table.get("sortAction").is_some());
    send(&mut rt, "set_view", json!({"value": ["deps"]}));
    table = all(&view(&mut rt), "Table").into_iter().next().expect("a Table");
    assert!(table.get("sortAction").is_none());
}

// the act decision, against buttons::decide (plugins/wheelhouse/src/beads/buttons.rs)

fn groups() -> Json {
    parse(
        r#"
    {"task":{"start":{"label":"Begin","run":"go {id}"},"resume":{"label":null,"run":"again {id}"},"focus":{"label":"Show","run":null}},
     "epic":{"start":{"label":null,"run":null},"resume":{"label":null,"run":null},"focus":{"label":null,"run":null}},
     "closed":"Shipped"}
    "#,
    )
}

/// The footer button of `id`'s detail card: its label, disabled and help.
fn act(value: Json, panes: Json, id: &str) -> Option<Btn> {
    let mut rt = list_pane(value, panes, rec());
    send(&mut rt, "select_bead", json!({"id": id}));
    buttons(&view(&mut rt)).into_iter().find(|b| action_type(b).as_deref() == Some("press"))
}

fn snap(rows: Vec<Json>, records: Json, pressing: Json) -> Json {
    snapshot_with(rows, groups(), records, pressing)
}

fn triple(a: &Btn) -> (&str, bool, &str) {
    (a.text.as_str(), a.disabled, a.help.as_str())
}

#[test]
fn the_act_follows_the_record_and_the_open_panes_as_decide_does() {
    let none = || (json!({}), json!([]));
    // Fresh: no record -> Start, with the project's label and a run.
    let (r, p) = none();
    let a = act(snap(vec![Bead { ready: true, ..bead("wheel-3") }.json()], r, p), Json::Null, "wheel-3").expect("a button");
    assert_eq!(triple(&a), ("Begin", false, ""));
    // The epic group's verbs have no run: disabled, naming the missing key.
    let (r, p) = none();
    let a = act(snap(vec![Bead { kind: "epic", epic: true, ..bead("wheel-4") }.json()], r, p), Json::Null, "wheel-4").expect("a button");
    assert_eq!(triple(&a), ("Start", true, "[beads.epic] has no start.run"));
    // Saved: a record whose pane is not open -> Resume (the verb's label falls back).
    let rec3 = json!({"wheel-3": {"pane": "p9", "conversation": "c"}});
    let a = act(snap(vec![bead("wheel-3").json()], rec3.clone(), json!([])), json!([{"id": "p1"}]), "wheel-3").expect("a button");
    assert_eq!(triple(&a), ("Resume", false, ""));
    // Saved with no resume.run (the epic group has none): disabled, naming the missing key.
    let erec = json!({"wheel-4": {"pane": "p9", "conversation": "c"}});
    let a = act(snap(vec![Bead { kind: "epic", epic: true, ..bead("wheel-4") }.json()], erec, json!([])), json!([{"id": "p1"}]), "wheel-4").expect("a button");
    assert_eq!(triple(&a), ("Resume", true, "[beads.epic] has no resume.run"));
    // Live: the record's pane is open -> Focus, enabled although it has no run.
    let a = act(snap(vec![bead("wheel-3").json()], rec3.clone(), json!([])), json!([{"id": "p9"}]), "wheel-3").expect("a button");
    assert_eq!(triple(&a), ("Show", false, ""));
    // The panes cannot be listed yet: a bead with a record is disabled and says so.
    let a = act(snap(vec![bead("wheel-3").json()], rec3.clone(), json!([])), Json::Null, "wheel-3").expect("a button");
    assert_eq!((a.disabled, a.help.as_str()), (true, "The open panes are not known yet"));
    // A press in flight disables the button.
    let a = act(snap(vec![bead("wheel-3").json()], json!({}), json!(["wheel-3"])), Json::Null, "wheel-3").expect("a button");
    assert_eq!((a.disabled, a.help.as_str()), (true, "Starting; wait for the pane"));
    // Live with no focus label falls back to "Focus".
    let no_labels = parse(
        r#"
        {"task":{"start":{"label":null,"run":"go"},"resume":{"label":null,"run":"go"},"focus":{"label":null,"run":null}},"epic":null,"closed":null}
        "#,
    );
    let a = act(snapshot_with(vec![bead("wheel-3").json()], no_labels, rec3.clone(), json!([])), json!([{"id": "p9"}]), "wheel-3").expect("a button");
    assert_eq!((a.text.as_str(), a.disabled), ("Focus", false));
    // Closed with no group and no closed label: "Closed", disabled.
    let a = act(snapshot(vec![Bead { status: "closed", ..bead("wheel-3") }.json()]), Json::Null, "wheel-3").expect("a button");
    assert_eq!(triple(&a), ("Closed", true, "This bead is closed"));
    // Closed: the project's closed label, disabled, whatever the record says.
    let a = act(snap(vec![Bead { status: "closed", ..bead("wheel-3") }.json()], rec3, json!([])), json!([{"id": "p9"}]), "wheel-3").expect("a button");
    assert_eq!(triple(&a), ("Shipped", true, "This bead is closed"));
}

#[test]
fn a_project_with_no_group_for_a_kind_draws_no_button() {
    let value = snapshot(vec![Bead { ready: true, ..bead("wheel-3") }.json()]);
    assert!(act(value.clone(), Json::Null, "wheel-3").is_none());
    // And the row's menu offers nothing.
    let mut rt = list_pane(value, Json::Null, rec());
    let r = rows(&view(&mut rt));
    assert_eq!(r[0].row.get("menu"), Some(&json!([])));
}

#[test]
fn a_row_menu_offers_the_action_only_when_it_is_enabled_and_press_sends_it_to_the_backend() {
    let actions = rec();
    let value = snapshot_with(vec![Bead { ready: true, ..bead("wheel-3") }.json()], groups(), json!({}), json!([]));
    let mut rt = list_pane(value, Json::Null, actions.clone());
    let r = rows(&view(&mut rt));
    let Some(Json::Array(menu)) = r[0].row.get("menu") else { panic!("no menu item: {:?}", r[0].row.get("menu")) };
    let Some(Json::Object(item)) = menu.first() else { panic!("no menu item: {menu:?}") };
    assert_eq!(s(item.get("label")).as_deref(), Some("Begin"));
    rt.send(item.get("action").expect("an action")).unwrap();
    assert!(actions.borrow().len() == 1 && actions.borrow()[0].0 == "wheelhouse.press");
    same(&actions.borrow()[0].1, json!({"dir": "/p", "bead": "wheel-3"}));
    // The answer is for the oldest press waiting; the detail card shows it.
    rt.send(&parse(r#"{"action":"wheelhouse.press","result":{"said":"Started wheel-3"}}"#)).unwrap();
    send(&mut rt, "select_bead", json!({"id": "wheel-3"}));
    assert!(page_texts(&mut rt).contains(&"Started wheel-3".to_string()));
}

// detail card

#[test]
fn the_detail_card_lists_facts_and_the_trees_and_back_returns_to_the_last() {
    let mut rt = list();
    send(&mut rt, "select_bead", json!({"id": "wheel-7"}));
    let mut t = page_texts(&mut rt);
    let has = |t: &[String], x: &str| t.iter().any(|s| s == x);
    assert!(has(&t, "Bead wheel-7") && has(&t, "wheel-7") && has(&t, "blocked"), "{t:?}");
    assert!(has(&t, "Depends on"));
    // Its two trees: wheel-7 waits on wheel-6 and wheel-8 and unlocks wheel-8.
    assert!(has(&t, "6  Bead wheel-6") && has(&t, "8  Bead wheel-8"), "{t:?}");
    send(&mut rt, "open_bead", json!({"id": "wheel-6"}));
    t = page_texts(&mut rt);
    assert!(has(&t, "Bead wheel-6"));
    send(&mut rt, "back", json!({}));
    t = page_texts(&mut rt);
    assert!(has(&t, "Bead wheel-7"));
    send(&mut rt, "close_detail", json!({}));
    let modals = all(&view(&mut rt), "Modal");
    let modal = modals.first().expect("a Modal");
    assert_eq!(modal.get("presented"), Some(&json!(false)));
}

// the 2,500-bead fixture

/// 2,500 beads: 50 epics of 49 children each, a blocked chain through every tenth child.
fn large() -> Json {
    let mut rows = vec![];
    for e in 0..50 {
        let epic = format!("wheel-e{e}");
        rows.push(Bead { kind: "epic", pri: Some(1), epic: true, ..bead(&epic) }.json());
        for c in 0..49i64 {
            let id = format!("wheel-{e}x{c}");
            let status = if c % 7 == 0 { "closed" } else if c % 5 == 0 { "in_progress" } else { "open" };
            let label = format!("l{}", c % 4);
            let blocked_id = format!("wheel-{e}x{}", c - 10);
            let blocked = if c % 10 == 3 && c > 3 { vec![blocked_id.as_str()] } else { vec![] };
            rows.push(
                Bead { status, pri: Some(c % 5), parent: Some(&epic), labels: vec![label.as_str()], blocked, ready: c % 2 == 0, desc: "word ".repeat(20), ..bead(&id) }.json(),
            );
        }
    }
    snapshot(rows)
}

#[test]
fn a_large_feed_stays_under_the_value_limit_in_every_view() {
    let mut rt = list_pane(large(), Json::Null, rec());
    // List: 2,450 leaves minus the closed ones, cut to the 1,500 sent, with one row saying how many are not.
    let mut r = rows(&view(&mut rt));
    assert_eq!(r.len(), 1501);
    assert_eq!(r.last().map(|r| r.id.as_str()), Some("\u{2026}more"));
    assert_eq!(r.last().map(|r| r.title.as_str()), Some("600 more not shown; search or filter to narrow"));
    for v in ["epics", "deps", "unlocks"] {
        send(&mut rt, "set_view", json!({"value": [v]}));
        r = rows(&view(&mut rt));
        assert!(!r.is_empty() && r.len() <= 1501, "{v}: {} rows", r.len());
    }
    // Narrowed, the rows fit and no more row ends the table.
    send(&mut rt, "set_view", json!({"value": ["list"]}));
    send(&mut rt, "list.query", json!({"value": "wheel-3x1"}));
    r = rows(&view(&mut rt));
    assert!(!r.is_empty() && r.iter().all(|r| r.id.starts_with("wheel-3x1")));
}

// panes/diff.luau's positional ids

fn diff_value(files: usize) -> Json {
    let fs: Vec<Json> = (0..files)
        .map(|i| {
            json!({"path": format!("src/f{i}.rs"), "status": "M", "add": 2, "del": 1, "hunks": [{"header": "-1,2 +1,3 @@", "lines": [
                {"kind": " ", "old": 1, "new": 1, "text": "keep"}, {"kind": "-", "old": 2, "text": "gone"}, {"kind": "+", "new": 2, "text": "new"}]}]})
        })
        .collect();
    json!({"error": null, "scopes": [{"key": "uncommitted", "label": "Uncommitted", "base": "main", "files": fs}]})
}

/// The node `id` names ("root.2.0.0.1": a child index per dot).
fn node(tree: &PaneNode, id: &str) -> PaneNode {
    let mut n = tree;
    for part in id.split('.').skip(1) {
        let i: usize = part.parse().unwrap_or(usize::MAX);
        n = n.children.get(i).unwrap_or_else(|| panic!("{id}: no child {part} under {}", n.component));
    }
    n.clone()
}

#[test]
fn the_diff_pane_scrolls_to_the_lists_its_constants_name_in_both_layouts() {
    let mut rt = wh_pane("diff", "diff", diff_value(3), Json::Null, rec());
    let mut tree = view(&mut rt);
    // Nothing selected draws the first file; the wide stream and the narrow lines are the Lists the constants name.
    let stream = node(&tree, "root.2.0.0.1");
    assert_eq!(stream.component, "List");
    assert!(stream.children.len() == 3 && stream.children.iter().all(|c| c.component == "Section"));
    same(&prop(&stream, "scrollTo"), json!(1));
    let lines = node(&tree, "root.2.1.2");
    assert_eq!(lines.component, "List");
    same(&prop(&lines, "scrollTo"), json!(1));
    // The stream's side is the file table the split holds before it.
    assert_eq!(all(&node(&tree, "root.2.0.0.0"), "Table").len(), 1);
    // Selecting the third file scrolls the stream to its section, and that section is its header and lines.
    send(&mut rt, "select_file", json!({"id": "src/f2.rs"}));
    tree = view(&mut rt);
    let moved = node(&tree, "root.2.0.0.1");
    same(&prop(&moved, "scrollTo"), json!(3));
    let section = node(&tree, "root.2.0.0.1.2");
    assert_eq!(section.component, "Section");
    assert!(texts_json(&jsonof(&section)).contains(&"src/f2.rs".to_string()));
    // The narrow list draws the open file's lines: its first one is what scrollTo names.
    let narrow = node(&tree, "root.2.1.2");
    same(&prop(&narrow, "scrollTo"), json!(1));
    assert!(texts_json(&jsonof(&narrow)).first().is_some_and(|t| t.starts_with("@@ ")));
}

#[test]
fn the_diff_pane_names_a_file_beyond_the_drawn_limit_and_says_how_many_are_left_out() {
    let mut rt = wh_pane("diff", "diff", diff_value(205), Json::Null, rec());
    let mut tree = view(&mut rt);
    assert_eq!(node(&tree, "root.2.0.0.1").children.len(), 200);
    assert!(texts_json(&jsonof(&tree)).contains(&"5 more files; `wheelhouse diff` lists them all.".to_string()));
    // A selected file beyond the first 200 is drawn after them and the scroll lands on it.
    send(&mut rt, "select_file", json!({"id": "src/f204.rs"}));
    tree = view(&mut rt);
    let stream = node(&tree, "root.2.0.0.1");
    assert_eq!(stream.children.len(), 201);
    same(&prop(&stream, "scrollTo"), json!(201));
}
