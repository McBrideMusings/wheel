mod common;

use common::*;
use serde_json::{Value as Json, json};

fn data() -> Json {
    json!({"pane": "p5", "space": "s1", "cwd": "/p"})
}

struct B {
    help: String,
    action: Json,
    disabled: bool,
}

/// Every Button below `node`: its hover text, its action and whether it is disabled.
fn buttons(node: &PaneNode) -> Vec<B> {
    let mut out = vec![];
    if node.component == "Button"
        && let Some(action) = node.props.get("action") {
            let (help, disabled) = button_mods(&node.props);
            out.push(B { help, action: action.clone(), disabled });
        }
    for c in &node.children {
        out.extend(buttons(c));
    }
    out
}

fn pane(posts: &str, agents: &str, actions: Rec) -> PaneRuntime {
    let (p, a) = (parse(posts), parse(agents));
    let h = host(move |name, _| if name == "turn-feed.posts" { p.clone() } else { a.clone() }, actions);
    PaneRuntime::new(&read("plugins/turn-feed/panes/feed.luau"), Default::default(), "pane", Limits::default(), h).unwrap()
}

const OPEN: &str = r##"
{"dir":"/p","posts":[
  {"post":"t2","agent_pane":"p1","space":"s1","dir":"/p","at":"2026-11-16T16:40:09.123Z","at_local":"4:40pm","title":"Which one?","body":"First **bold**.\n\nSecond.",
   "body_blocks":[{"kind":"paragraph","text":"First **bold**."},{"kind":"paragraph","text":"Second."}],
   "question":"1. Which?","options":[
     {"id":"1A","action":"do a","case":"c1","cost":"k1","checked":"x1","pick":true},
     {"id":"1B","action":"do b","case":"c2","cost":"k2","checked":"x2"}],"answer":null}]}
"##;
const AGENTS: &str = r##"[{"pane":"p1","space":"s1","kind":"agent","agent":"claude-1","status":"idle"}]"##;
/// The two icon buttons every card's header has, whatever the post asks.
const ITEM_ACTIONS: [&str; 2] = ["Delete this post", "Show Agent p1's pane"];

fn answering(tree: &PaneNode) -> Vec<B> {
    buttons(tree).into_iter().filter(|b| !ITEM_ACTIONS.contains(&b.help.as_str())).collect()
}

fn has(all: &[String], t: &str) -> bool {
    all.iter().any(|x| x == t)
}

#[test]
fn a_post_draws_its_options_and_a_pick_goes_to_the_agent() {
    let calls = rec();
    let mut rt = pane(OPEN, AGENTS, calls.clone());
    let mut tree = rt.view(&data()).unwrap();
    let all = texts_child(&tree);
    assert!(has(&all, "Which one?") && has(&all, "Agent p1") && has(&all, "4:40pm") && has(&all, "1A · my pick"), "{all:?}");
    assert!(has(&all, "First **bold**.") && has(&all, "Second."));
    assert!(nodes_of(&tree, "StructuredText").is_empty());
    assert!(has(&all, "Case: c1") && has(&all, "Cost: k2") && has(&all, "Checked: x1"));
    let btns = buttons(&tree);
    let pick = btns.iter().find(|b| b.help == "Pick 1B").expect("Pick 1B");
    assert!(!pick.disabled);
    rt.send(&pick.action).unwrap();
    assert!(calls.borrow().len() == 1 && calls.borrow()[0].0 == "turn-feed.answer");
    same(&calls.borrow()[0].1, json!({"post": "t2", "pick": "1B"}));
    rt.send(&parse(r##"{"action":"turn-feed.answer","result":{"post":"t2","agent_pane":"p1","text":"Turn feed, \"Which one?\": picked 1B"}}"##)).unwrap();
    assert!(calls.borrow().len() == 2 && calls.borrow()[1].0 == "wheel.agent.send");
    same(&calls.borrow()[1].1, json!({"agent": "p1", "text": "Turn feed, \"Which one?\": picked 1B"}));
    rt.send(&parse(r##"{"action":"wheel.agent.send","error":"no agent p1"}"##)).unwrap();
    tree = rt.view(&data()).unwrap();
    assert!(has(&texts_child(&tree), "Answer saved, but the agent did not get it: no agent p1"));
}

#[test]
fn a_reply_sends_the_draft_and_an_answer_locks_the_controls() {
    let calls = rec();
    let mut rt = pane(OPEN, AGENTS, calls.clone());
    let mut tree = rt.view(&data()).unwrap();
    let btns = buttons(&tree);
    let send = btns.iter().find(|b| b.help == "Send the reply").expect("Send the reply");
    rt.send(&send.action).unwrap();
    assert!(calls.borrow().is_empty());
    rt.send(&parse(r##"{"type":"draft","post":"t2","value":"use C"}"##)).unwrap();
    rt.send(&send.action).unwrap();
    assert!(calls.borrow().len() == 1);
    same(&calls.borrow()[0].1, json!({"post": "t2", "reply": "use C"}));
    rt.send(&parse(r##"{"action":"turn-feed.answer","error":"answer refused: post t2 was already answered: 1A at x"}"##)).unwrap();
    tree = rt.view(&data()).unwrap();
    assert!(has(&texts_child(&tree), "answer refused: post t2 was already answered: 1A at x"));

    let answered = OPEN.replace(r##""answer":null"##, r##""answer":{"pick":"1A","at":"2026-11-16T16:41:00.000Z","at_local":"4:41pm","by_pane":"p5"}"##);
    let mut locked = pane(&answered, AGENTS, rec());
    tree = locked.view(&data()).unwrap();
    assert!(has(&texts_child(&tree), "Picked 1A · 4:41pm"));
    let btns = answering(&tree);
    assert!(btns.len() == 2 && btns.iter().all(|b| b.disabled), "{}", btns.len());
    assert!(buttons(&tree).iter().filter(|b| ITEM_ACTIONS.contains(&b.help.as_str())).all(|b| !b.disabled));
}

#[test]
fn an_undelivered_answer_can_be_sent_again_and_a_delivered_one_cannot() {
    let calls = rec();
    let stored = r##""answer":{"pick":"1A","at":"2026-11-16T16:41:00.000Z","at_local":"4:41pm","by_pane":"p5","text":"Turn feed: picked 1A","delivered":false}"##;
    let undelivered = OPEN.replace(r##""answer":null"##, stored);
    let mut rt = pane(&undelivered, AGENTS, calls.clone());
    let mut tree = rt.view(&data()).unwrap();
    assert!(has(&texts_child(&tree), "Saved, but the agent did not get it"));
    assert!(!buttons(&tree).iter().any(|b| b.help == "Send the reply"));
    let btns = buttons(&tree);
    let again = btns.iter().find(|b| b.help == "Send the saved answer to the agent again").expect("send again");
    assert!(!again.disabled);
    assert!(has(&texts_child(&tree), "Send again"));
    assert_eq!(answering(&tree).iter().filter(|b| !b.disabled).count(), 1);
    rt.send(&again.action).unwrap();
    assert!(calls.borrow().len() == 1 && calls.borrow()[0].0 == "wheel.agent.send");
    same(&calls.borrow()[0].1, json!({"agent": "p1", "text": "Turn feed: picked 1A"}));
    rt.send(&parse(r##"{"action":"wheel.agent.send","result":{}}"##)).unwrap();
    assert!(calls.borrow().len() == 2 && calls.borrow()[1].0 == "turn-feed.delivered");
    same(&calls.borrow()[1].1, json!({"post": "t2"}));

    let gone = pane(&undelivered, "[]", rec()).view(&data()).unwrap();
    assert!(answering(&gone).iter().all(|b| b.disabled));

    let done = OPEN.replace(r##""answer":null"##, &stored.replace("false", "true"));
    tree = pane(&done, AGENTS, rec()).view(&data()).unwrap();
    assert!(has(&texts_child(&tree), "Picked 1A · 4:41pm"));
    assert!(!buttons(&tree).iter().any(|b| b.help == "Send the reply") && answering(&tree).iter().all(|b| b.disabled));
}

#[test]
fn a_markdown_page_is_drawn_from_its_blocks_with_images_sized_by_the_page() {
    let blocks = r##""page":"notes","page_blocks":[{"kind":"heading","level":1,"text":"Hi"},{"kind":"image","url":"file:///p/a.png","alt":"A","width":1594,"height":800},{"kind":"table","align":["none"],"header":["H"],"rows":[["1"]]},{"kind":"code","lang":"","text":"let x"}],"question":"1. Which?""##;
    let paged = OPEN.replace(r##""question":"1. Which?""##, blocks);
    let tree = pane(&paged, AGENTS, rec()).view(&data()).unwrap();
    assert!(nodes_of(&tree, "WebView").is_empty() && nodes_of(&tree, "StructuredText").is_empty());
    let images = nodes_of(&tree, "Image");
    let image = images.first().expect("an Image");
    same(&prop(image, "url"), json!("file:///p/a.png"));
    let mut max_width = Json::Null;
    if let Some(Json::Array(mods)) = image.props.get("modifiers") {
        for m in mods {
            if let Some(Json::Object(f)) = m.get("frame") {
                max_width = f.get("maxWidth").cloned().unwrap_or(Json::Null);
            }
        }
    }
    same(&max_width, json!(1594));
    let texts = texts_child(&tree);
    assert!(nodes_of(&tree, "Grid").len() == 1 && has(&texts, "let x") && has(&texts, "Hi"), "{texts:?}");
}

#[test]
fn a_posts_page_is_the_artifact_embedded() {
    let paged = OPEN.replace(r##""question":"1. Which?""##, r##""page":"mockup","question":"1. Which?""##);
    let tree = pane(&paged, AGENTS, rec()).view(&data()).unwrap();
    let webs = nodes_of(&tree, "WebView");
    let web = webs.first().expect("a WebView");
    same(&prop(web, "artifact"), json!("mockup"));
    same(&prop(web, "height"), json!(360));
}

#[test]
fn a_gone_agent_disables_the_buttons_and_an_empty_feed_says_how_to_post() {
    let mut gone = pane(OPEN, "[]", rec());
    let mut tree = gone.view(&data()).unwrap();
    assert!(has(&texts_child(&tree), "Agent pane p1 is gone"));
    assert!(answering(&tree).iter().all(|b| b.disabled));
    let btns = buttons(&tree);
    let show = btns.iter().find(|b| b.help == "Show Agent p1's pane").expect("show");
    assert!(show.disabled);
    let mut empty = pane(r##"{"dir":"/p","posts":[]}"##, "[]", rec());
    tree = empty.view(&data()).unwrap();
    assert!(texts_child(&tree).first().is_some_and(|t| t.starts_with("No posts yet from this space's agents. Agents post with: wheel plugin action")), "{:?}", texts_child(&tree).first());
}

#[test]
fn the_list_draws_thirty_cards_and_show_older_adds_thirty_more() {
    let posts: Vec<String> = (1..=40)
        .map(|i| format!(r##"{{"post":"t{i}","agent_pane":"p1","space":"s1","dir":"/p","at":"2026-11-16T16:40:09.123Z","at_local":"4:40pm","title":"Post {i}","body":"b","body_blocks":[],"answer":null}}"##))
        .collect();
    let mut rt = pane(&format!(r##"{{"dir":"/p","posts":[{}]}}"##, posts.join(",")), AGENTS, rec());
    let post_titles = |tree: &PaneNode| texts_child(tree).into_iter().filter(|t| t.starts_with("Post ")).collect::<Vec<_>>();
    let mut tree = rt.view(&data()).unwrap();
    let mut titles = post_titles(&tree);
    assert!(titles.len() == 30 && titles.first().map(String::as_str) == Some("Post 1") && titles.last().map(String::as_str) == Some("Post 30"), "{titles:?}");
    let btns = buttons(&tree);
    let older = btns.iter().find(|b| b.help == "Show 10 older of 10 not shown").expect("Show 10 older");
    rt.send(&older.action).unwrap();
    tree = rt.view(&data()).unwrap();
    titles = post_titles(&tree);
    assert!(titles.len() == 40 && !buttons(&tree).iter().any(|b| b.help.ends_with(" not shown")));
    rt.send(&parse(r##"{"type":"query","value":""}"##)).unwrap();
    assert_eq!(post_titles(&rt.view(&data()).unwrap()).len(), 30);
}
