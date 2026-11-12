//! The message menu (`TranscriptSelection.swift`'s `messageMenu`, WheelUI's `MessageMenu.swift`):
//! a right-click or a control-click on the person's message or the agent's reply offers Copy
//! message, Copy message as Markdown and, after a separator, Attach message as context.
use super::*;
use crate::transcript::MessageAction;

pub const MENU_W: f32 = 240.0;
const ROW_H: f32 = 22.0;
const SEPARATOR_H: f32 = 7.0;

/// The open message menu: the row it is for, by its id (rows above it can come and go while the
/// menu is open, so an index would name another row), and where the press that opened it was.
#[derive(Clone, Debug)]
pub struct MessageMenu {
    pub agent: String,
    pub row_id: usize,
    pub x: f32,
    pub y: f32,
}

impl Client {
    /// Open the menu for row `row` of `agent`'s transcript at the press. False, and nothing
    /// opens, when the row is not a message.
    pub fn open_message_menu(&mut self, agent: &str, row: usize, x: f32, y: f32) -> bool {
        let Some(row_id) = self.transcripts.get(agent).filter(|t| t.message_markdown(row).is_some()).map(|t| t.rows[row].id) else {
            return false;
        };
        self.close_space_menu();
        self.close_pane_menu();
        self.close_title_menu();
        let h = Self::message_menu_height();
        let x = x.min(self.viewport.0 - MENU_W - 4.).max(0.);
        let y = y.min(self.viewport.1 - h - 4.).max(0.);
        self.log(format!("transcript {agent}: message menu on row {row}: {}", MessageAction::lines().join(" | ")));
        self.message_menu = Some(MessageMenu { agent: agent.to_owned(), row_id, x, y });
        self.sync_sidebar_overlay();
        true
    }
    pub fn close_message_menu(&mut self) {
        if self.message_menu.take().is_some() {
            self.sync_sidebar_overlay();
        }
    }
    /// Where the row with id `row_id` stands in `agent`'s transcript now.
    fn row_index(&self, agent: &str, row_id: usize) -> Option<usize> {
        self.transcripts.get(agent)?.rows.iter().position(|r| r.id == row_id)
    }
    /// The menu's height as drawn: its rows, a line for the separator, the padding.
    pub(super) fn message_menu_height() -> f32 {
        let separators = MessageAction::ALL.iter().filter(|a| a.separated_above()).count() as f32;
        // The rows, each separator, the padding and the border.
        MessageAction::ALL.len() as f32 * ROW_H + separators * SEPARATOR_H + 8. + 2.
    }
    /// Row `row`'s text as the person reads it: the person's message as typed, a reply's texts as
    /// the transcript draws them, joined by newlines, as a selection of the whole row copies.
    fn message_plain(&self, agent: &str, row: usize, cx: &mut Context<Self>) -> Option<String> {
        let t = self.transcripts.get(agent)?;
        match &t.rows.get(row)?.kind {
            Kind::User { text, .. } => Some(text.clone()),
            Kind::Reply(_) => {
                if !self.texts_kept(agent, row) {
                    drop(self.row_el(agent, row, cx));
                }
                self.row_texts.borrow().get(agent).and_then(|rows| rows.get(&row)).map(|texts| texts.join("\n"))
            }
            _ => None,
        }
    }
    /// Run one menu item on the row with id `row_id` in `agent`'s transcript (`AgentChat.perform`).
    /// False when the row is gone or is not a message.
    pub fn run_message_action(&mut self, action: MessageAction, agent: &str, row_id: usize, cx: &mut Context<Self>) -> bool {
        self.close_message_menu();
        let Some(row) = self.row_index(agent, row_id) else { return false };
        let Some(markdown) = self.transcripts.get(agent).and_then(|t| t.message_markdown(row)).map(str::to_owned) else { return false };
        self.press(format!("message menu {agent} {row} {}", action.id()));
        self.log(format!("transcript {agent}: message menu {} on row {row}", action.id()));
        match action {
            MessageAction::CopyPlain => {
                let Some(plain) = self.message_plain(agent, row, cx) else { return false };
                cx.write_to_clipboard(ClipboardItem::new_string(plain));
            }
            MessageAction::CopyMarkdown => cx.write_to_clipboard(ClipboardItem::new_string(markdown)),
            MessageAction::Attach => {
                let draft = self.drafts.entry(agent.to_owned()).or_default();
                *draft = crate::composer::attach_as_context(draft, &markdown);
                self.draft_edited(agent);
                // The composer takes the keyboard, so the next keys type under the quote.
                let pane = self.spaces.iter().flat_map(|s| s.all()).find(|l| l.agent.as_deref() == Some(agent)).map(|l| l.id.clone());
                if let Some(pane) = pane {
                    self.focus_pane(&pane);
                }
            }
        }
        true
    }
    /// The open menu, anchored at the press that opened it.
    pub fn message_menu_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let MessageMenu { agent, row_id, x, y } = self.message_menu.clone()?;
        // It occludes the rows under it, so a right press on the menu never opens another.
        let mut list = div().occlude().absolute().left(px(x)).top(px(y)).w(px(MENU_W)).py(px(4.)).rounded_md().bg(rgb(0x22302a)).border_1().border_color(rgb(0x3d5247)).flex().flex_col().text_xs()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for action in MessageAction::ALL {
            if action.separated_above() {
                list = list.child(div().h(px(1.)).my(px(3.)).bg(rgb(0x3d5247)));
            }
            let agent_id = agent.clone();
            list = list.child(div().relative().h(px(ROW_H)).px(px(10.)).flex().items_center().whitespace_nowrap().overflow_hidden().text_color(rgb(0xd9e8e1))
                .cursor_pointer().hover(|s| s.bg(rgb(0x31483e))).child(action.title())
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.run_message_action(action, &agent_id, row_id, cx);
                    cx.notify();
                }))
                .child(recorded(&agent, &format!("message-menu:{}", action.id()), &self.controls)));
        }
        Some(list)
    }
    /// The message menu for `/state`.
    pub fn message_menu_json(&self) -> serde_json::Value {
        serde_json::json!({
            "open": self.message_menu.as_ref().map(|m| serde_json::json!({"agent": m.agent, "row": self.row_index(&m.agent, m.row_id), "row_id": m.row_id, "frame": [m.x, m.y, MENU_W, Self::message_menu_height()]})),
            "items": MessageAction::lines(),
        })
    }
}

/// A message row's right-click and control-click open the menu; a control-click stops before
/// the pane's own press, so it starts no selection.
pub(super) fn message_presses(el: gpui::Div, agent: &str, row: usize, cx: &mut Context<Client>) -> gpui::Div {
    let (a, b) = (agent.to_owned(), agent.to_owned());
    el.on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
        if this.open_message_menu(&a, row, f32::from(e.position.x), f32::from(e.position.y)) {
            cx.stop_propagation();
            cx.notify();
        }
    }))
    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
        if e.modifiers.control && this.open_message_menu(&b, row, f32::from(e.position.x), f32::from(e.position.y)) {
            cx.stop_propagation();
            cx.notify();
        }
    }))
}
