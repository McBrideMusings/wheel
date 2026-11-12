//! The request card an agent pane draws above its composer (ChatView.swift's `PermissionCard`):
//! who asks, why, the request, and its buttons; Return, Command-Return and Esc answer it from the
//! focused pane's empty composer. The model is `crate::perms`.
use super::*;
use crate::perms::{self, ANSWER_GAP, ANSWER_PAD, Answer, BUTTON, GAP, LINE, MARGIN, PADDING};
use serde_json::{Value, json};
use wheel_proto::PermInfo;

const ORANGE: u32 = 0xe0a050;

impl Client {
    /// Send `choice` for request `id`. The card stays until `perm_resolved` comes back, from this
    /// client or any other.
    pub fn answer_perm(&mut self, id: &str, choice: &str) {
        self.log(format!("answering {id}: {choice}"));
        self.press(format!("perm {id} {choice}"));
        self.send(&ClientMsg::PermAnswer { req: None, id: id.to_owned(), choice: choice.to_owned() });
    }
    /// The oldest waiting request of the agent in `pane`, with its place among them all.
    pub fn perm_for_pane(&self, pane: &str) -> Option<(usize, &PermInfo)> {
        let leaf = self.spaces.iter().flat_map(|s| leaves_of(&s.root)).find(|l| l.id == pane && l.kind == "agent")?;
        perms::waiting(&self.perms, leaf.agent.as_deref()?)
    }
    /// The height `agent`'s card takes in an agent pane body `body` tall (`perms::fitted`), zero
    /// when its agent waits on nothing. The transcript keeps two lines at least.
    pub(super) fn card_height(&self, agent: Option<&str>, body: f32) -> f32 {
        let room = body - LABEL - COMPOSER - FOOTER - 8. - 2. * LINE;
        agent.and_then(|a| perms::waiting(&self.perms, a)).map_or(0., |(_, perm)| perms::fitted(perm, room))
    }
    /// A key in the focused agent pane while its agent waits on a request: Return with an empty
    /// draft allows, Command-Return always allows, Esc denies (Skip, for a question). A key held
    /// down answers nothing, so a repeat that began before the card came up cannot answer it.
    pub(super) fn perm_key(&mut self, e: &KeyDownEvent) -> bool {
        let k = &e.keystroke;
        if e.is_held {
            return false;
        }
        let Some(pane) = self.focus.current.clone() else { return false };
        let Some((_, perm)) = self.perm_for_pane(&pane) else { return false };
        let draft_empty = self.drafts.get(&perm.agent).is_none_or(|d| d.trim().is_empty());
        let Some(answer) = Answer::for_key(&k.key, k.modifiers.platform, k.modifiers.shift || k.modifiers.control || k.modifiers.alt, draft_empty) else { return false };
        let Some(option) = answer.option(perm) else { return false };
        let (id, choice) = (perm.id.clone(), option.id.clone());
        self.answer_perm(&id, &choice);
        true
    }
    /// The splits above `pane` in its space (`Tree::splits_to`).
    pub fn splits_to(&self, pane: &str) -> Vec<(String, bool, f64)> {
        self.spaces.iter().find_map(|s| s.root.splits_to(pane)).unwrap_or_default()
    }
    /// How many card buttons the last frame drew for `agent`.
    pub fn perm_buttons_drawn(&self, agent: &str) -> usize {
        self.controls.borrow().iter().filter(|c| c.agent == agent && c.id.starts_with("perm:")).count()
    }
    /// What `/state` says about the card `pane` shows: the request, its place, and each button
    /// with its tooltip, as the Swift app's debug server does.
    pub fn perm_card_json(&self, pane: &str) -> Value {
        let keys = self.focus.current.as_deref() == Some(pane);
        match self.perm_for_pane(pane) {
            Some((position, perm)) => json!({
                "id": perm.id, "title": perm.title, "question": perms::is_question(perm), "position": position, "count": self.perms.len(),
                "height": perms::height(perm),
                "answers": perms::answers(perm).iter().map(|o| json!({"id": o.id, "name": o.name, "description": o.description})).collect::<Vec<_>>(),
                "buttons": perms::row(perm).iter().map(|b| json!({"id": b.option.id, "kind": b.option.kind, "name": b.option.name, "prominent": b.prominent, "help": perms::help(&b.option, perm, keys)})).collect::<Vec<_>>(),
            }),
            None => Value::Null,
        }
    }
}

/// A tooltip's view: one line of text in a dark box.
struct Tip(String);

/// A tooltip reading `text`.
pub(super) fn tip(text: &str) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let text = text.to_owned();
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().rounded_sm().bg(rgb(0x0e1311)).border_1().border_color(rgb(0x2c3d34)).text_xs().text_color(rgb(0xd9e8e1)).child(self.0.clone())
    }
}

/// One line of the card, exactly `LINE` tall, cut with `…` when it is too wide.
fn line(text: &str, color: u32) -> gpui::Div {
    div().h(px(LINE)).line_height(px(LINE)).truncate().text_color(rgb(color)).child(text.to_owned())
}

/// The card, `height` tall (`Client::card_height`): who asks and which of how many, then why, the
/// request and a question's answers, scrolling when the pane is too short for them, then the
/// buttons. Every press on a button sends `perm_answer`; each button records its bounds as control
/// `perm:<option id>`.
#[allow(clippy::too_many_arguments)]
pub(super) fn perm_card(perm: &PermInfo, position: usize, count: usize, keys: bool, height: f32, controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> gpui::Div {
    let agent = perm.agent.as_str();
    let button = |option: &wheel_proto::PermOption, prominent: bool, tall: bool, cx: &mut Context<Client>| {
        let (id, choice) = (perm.id.clone(), option.id.clone());
        let help = perms::help(option, perm, keys);
        let mut b = div().id(gpui::ElementId::Name(format!("perm-{}-{}", perm.id, option.id).into())).relative().rounded_sm().cursor_pointer().px_2()
            .bg(rgb(if prominent { 0x3f7a5c } else { 0x31483e })).text_color(rgb(0xd9e8e1))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.answer_perm(&id, &choice);
                cx.stop_propagation();
            }))
            .tooltip(tip(&help))
            .child(recorded(agent, &format!("perm:{}", option.id), controls));
        if tall {
            b = b.w_full().py(px(ANSWER_PAD)).flex().flex_col().child(line(&option.name, 0xd9e8e1));
            if let Some(description) = &option.description {
                b = b.child(line(description, 0x8fa89b).text_xs());
            }
            b
        } else {
            b.h(px(BUTTON)).flex().items_center().text_xs().child(option.name.clone())
        }
    };
    let mut header = div().h(px(LINE)).flex_none().flex().items_center().gap_2()
        .child(div().text_color(rgb(ORANGE)).child("✋"))
        .child(line(&format!("Agent {} is asking", perm.agent), 0xd9e8e1).flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD));
    if count > 1 {
        header = header.child(div().text_xs().text_color(rgb(0x8fa89b)).child(format!("{position} of {count}")));
    }
    // Never shrunk by the pane's column: the transcript above gives up the room instead.
    let mut card = div().relative().flex_none().h(px(height - MARGIN)).overflow_hidden().flex().flex_col().gap(px(GAP)).p(px(PADDING)).mt(px(MARGIN)).rounded_md().bg(rgb(0x0e1311))
        .border_1().border_color(rgb(ORANGE)).text_sm().text_color(rgb(0xd9e8e1)).child(recorded(agent, "perm-card", controls)).child(header);
    let mut middle = div().id(gpui::ElementId::Name(format!("perm-lines-{}", perm.id).into())).flex_1().min_h(px(0.)).overflow_y_scroll().flex().flex_col().gap(px(GAP));
    let why = perms::why_lines(perm);
    if !why.is_empty() {
        let mut col = div().flex().flex_col().flex_none();
        for (i, text) in why.iter().enumerate() {
            col = col.child(line(text, if i == 0 && perm.why.reason.is_some() { 0xd9e8e1 } else { 0x8fa89b }));
        }
        middle = middle.child(col);
    }
    let question = perms::is_question(perm);
    let mut title = div().flex().flex_col().flex_none();
    for text in perms::title_lines(perm) {
        title = title.child(if question { line(&text, 0xd9e8e1) } else { line(&text, 0xe0b070).font_family("Menlo") });
    }
    middle = middle.child(title);
    if question {
        let mut col = div().flex().flex_col().flex_none().gap(px(ANSWER_GAP));
        for option in perms::answers(perm) {
            col = col.child(button(option, false, true, cx));
        }
        middle = middle.child(col);
    }
    card = card.child(middle);
    let row = perms::row(perm);
    if !row.is_empty() {
        let mut r = div().h(px(BUTTON)).flex_none().flex().justify_end().items_center().gap_2();
        for b in &row {
            r = r.child(button(&b.option, b.prominent, false, cx));
        }
        card = card.child(r);
    }
    card
}
