//! The composer's text input in a real window: keys go through GPUI's own dispatch, so the
//! client's key hook, the input's bindings and the platform text input all take part.
use super::*;
use crate::layout::{Leaf, Space, Tree};
use gpui::{TestAppContext, VisualTestContext};

struct Rig {
    cx: VisualTestContext,
    window: ClientWindow,
    sent: mpsc::Receiver<String>,
}

fn leaf(id: &str, kind: &str, agent: Option<&str>) -> Leaf {
    Leaf { id: id.into(), kind: kind.into(), label: id.into(), term: None, url: None, scope: None, agent: agent.map(str::to_owned), harness: agent.map(|_| "claude".to_owned()), name: None, title: None, status: None, visited: vec![], mirror: None, quiet: false }
}

/// A client window over a space holding one agent pane, focused, as wheeld left it connected.
fn rig(cx: &mut TestAppContext) -> Rig {
    let dir = std::env::temp_dir().join(format!("wheel-composer-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("token"), "t").unwrap();
    let endpoint = Endpoint::new("ws://127.0.0.1:1/ws", &dir.join("token")).unwrap();
    let (out, sent) = crate::daemon::Outbox::detached();
    let window = cx.update(|cx| {
        crate::init(cx);
        let bridge = Bridge::stub();
        let native = Native::new(&bridge);
        ClientWindow::open(cx, gpui::WindowOptions::default(), move |_, cx| {
            cx.new(|cx| {
                let mut client = Client::new(native, bridge, true, "test".into(), out, endpoint, None, None, cx.focus_handle());
                client.spaces = vec![Space { id: "s1".into(), name: "s1".into(), cwd: "/tmp".into(), root: Tree::Pane(leaf("p1", "agent", Some("a1"))), status: None, popped: vec![], backgrounded: vec![], frames: Default::default() }];
                client.space_id = Some("s1".into());
                client.focus.current = Some("p1".into());
                client.link = Link::Up;
                client
            })
        })
        .unwrap()
    });
    let any: gpui::AnyWindowHandle = window.into();
    let mut cx = VisualTestContext::from_window(any, cx);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
    Rig { cx, window, sent }
}

impl Rig {
    fn draft(&mut self) -> String {
        self.window.update(&mut self.cx, |c, _, _| c.drafts.get("a1").cloned().unwrap_or_default()).unwrap()
    }
    fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.cx.update(|window, cx| window.draw(cx).clear());
    }
    fn typed(&mut self, text: &str) {
        self.cx.simulate_input(text);
        self.cx.update(|window, cx| window.draw(cx).clear());
    }
    /// What reached wheeld since the last call, without the attach and layout traffic every frame makes.
    fn messages(&mut self) -> Vec<String> {
        self.sent.try_iter().filter(|m| m.contains("agent_send") || m.contains("agent_unsend")).collect()
    }
}

#[gpui::test]
fn the_composer_input_holds_the_keyboard(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let focused = rig.window.update(&mut rig.cx, |c, window, cx| c.composer_focused(window, cx)).unwrap();
    assert!(focused, "the agent pane's input does not hold the keyboard");
}

#[gpui::test]
fn the_caret_moves_and_typing_goes_where_it_is(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    rig.typed("ab");
    assert_eq!(rig.draft(), "ab");
    rig.keys("left");
    rig.typed("X");
    assert_eq!(rig.draft(), "aXb");
}

#[gpui::test]
fn command_a_selects_everything_and_typing_replaces_it(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    rig.typed("hello");
    rig.keys("cmd-a");
    rig.typed("Z");
    assert_eq!(rig.draft(), "Z");
}

#[gpui::test]
fn shift_return_starts_a_line_and_return_sends(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    rig.typed("one");
    rig.keys("shift-enter");
    rig.typed("two");
    assert_eq!(rig.draft(), "one\ntwo");
    rig.keys("enter");
    assert_eq!(rig.draft(), "");
    let input_value = rig.window.update(&mut rig.cx, |c, _, cx| c.inputs["a1"].read(cx).value().to_string()).unwrap();
    assert_eq!(input_value, "", "the input kept the sent text");
    assert!(rig.messages().iter().any(|m| m.contains("agent_send")), "no message went to wheeld");
}

#[gpui::test]
fn undo_takes_back_the_typing(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    rig.typed("abc");
    rig.keys("cmd-z");
    assert_eq!(rig.draft(), "");
}

fn with_commands(rig: &mut Rig) {
    rig.window.update(&mut rig.cx, |c, _, _| {
        c.transcripts.entry("a1".into()).or_default().commands = Some(vec!["clear".into(), "compact".into(), "review".into()]);
    }).unwrap();
}

fn listed(rig: &mut Rig) -> Vec<String> {
    rig.window.update(&mut rig.cx, |c, _, _| c.command_matches("a1", "claude").0.into_iter().map(|r| r.name).collect()).unwrap()
}

#[gpui::test]
fn a_slash_opens_the_command_list_and_tab_takes_the_first(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    with_commands(&mut rig);
    rig.typed("/co");
    assert_eq!(listed(&mut rig), ["compact"]);
    rig.keys("tab");
    assert_eq!(rig.draft(), "/compact ");
    let input_value = rig.window.update(&mut rig.cx, |c, _, cx| c.inputs["a1"].read(cx).value().to_string()).unwrap();
    assert_eq!(input_value, "/compact ", "the input did not take the picked command");
    // The caret is after it: the next character follows the command.
    rig.typed("x");
    assert_eq!(rig.draft(), "/compact x");
}

#[gpui::test]
fn the_arrows_and_return_pick_a_command_and_escape_shuts_the_list(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    with_commands(&mut rig);
    rig.typed("/");
    assert_eq!(listed(&mut rig), ["clear", "compact", "review"]);
    rig.keys("down down enter");
    assert_eq!(rig.draft(), "/review ");
    rig.keys("backspace backspace backspace backspace backspace backspace backspace");
    assert_eq!(rig.draft(), "/");
    rig.keys("escape");
    assert_eq!(listed(&mut rig), Vec::<String>::new());
    assert_eq!(rig.draft(), "/");
    assert!(rig.messages().is_empty(), "Return on the list sent a message");
}

#[gpui::test]
fn up_in_an_empty_composer_takes_back_the_unanswered_message(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    rig.window.update(&mut rig.cx, |c, _, _| {
        let t = c.transcripts.entry("a1".into()).or_default();
        t.event(&wheel_proto::AgentEvent::User { turn: 7, text: "first words".into() });
        t.event(&wheel_proto::AgentEvent::TurnStart { turn: 7 });
    }).unwrap();
    rig.keys("up");
    let sent = rig.messages();
    assert!(sent.iter().any(|m| m.contains("agent_unsend")), "Up took nothing back: {sent:?}");
    // Up with text in the draft moves the caret and takes nothing back.
    let mut rig2 = rig;
    rig2.typed("x");
    rig2.keys("up");
    assert!(rig2.messages().is_empty());
}

/// The test window has no repeat flag either: a repeat is a second key-down with no key-up
/// between, and a release is a `KeyUpEvent` dispatched to the window as the platform sends it.
#[gpui::test]
fn a_held_up_takes_back_one_message_per_press(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    rig.window.update(&mut rig.cx, |c, _, _| {
        let t = c.transcripts.entry("a1".into()).or_default();
        t.event(&wheel_proto::AgentEvent::User { turn: 7, text: "first".into() });
        t.event(&wheel_proto::AgentEvent::User { turn: 8, text: "second".into() });
        t.event(&wheel_proto::AgentEvent::TurnStart { turn: 7 });
    }).unwrap();
    let unsends = |rig: &mut Rig| rig.messages().iter().filter(|m| m.contains("agent_unsend")).count();
    rig.keys("up up");
    assert_eq!(unsends(&mut rig), 1, "a held Up took back more than one message");
    let up = gpui::Keystroke::parse("up").unwrap();
    rig.cx.simulate_event(gpui::KeyUpEvent { keystroke: up });
    // wheeld's answer to the first take-back is not simulated: the message stays marked as
    // being edited, so nothing else is taken back unless that mark is cleared.
    rig.window.update(&mut rig.cx, |c, _, _| c.editing.clear()).unwrap();
    rig.keys("up");
    assert_eq!(unsends(&mut rig), 1, "Up pressed again after a release took nothing back");
}

#[gpui::test]
fn the_edit_menus_paste_inserts_the_clipboard_text_at_the_caret(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    rig.typed("ad");
    rig.keys("left");
    rig.cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("bc".into())));
    rig.cx.dispatch_action(crate::edit_menu::Paste);
    rig.cx.update(|window, cx| window.draw(cx).clear());
    assert_eq!(rig.draft(), "abcd");
}

#[gpui::test]
fn the_edit_menus_cut_and_copy_act_on_the_selection(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    rig.typed("hello");
    rig.keys("cmd-a");
    rig.cx.dispatch_action(crate::edit_menu::Copy);
    assert_eq!(rig.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text())), Some("hello".to_owned()));
    rig.cx.dispatch_action(crate::edit_menu::Cut);
    rig.cx.update(|window, cx| window.draw(cx).clear());
    assert_eq!(rig.draft(), "");
}
