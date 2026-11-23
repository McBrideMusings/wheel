//! An agent pane's earlier conversations as the column draws them (WheelUI's
//! `EarlierConversations.swift` and `TranscriptView`): one list item per conversation above the
//! live rows, a divider with its first message and Resume, then its rows at half opacity. A
//! conversation is read the first time its item is drawn. The model is `crate::earlier`.
use super::*;
use crate::earlier::earlier_ids;
use crate::tooltip::tip;
use serde_json::Value;

/// The op that reads a past conversation; without it no earlier conversation is drawn.
const OP: &str = "agent_conversation_history";
/// The height the empty transcript's prompt and Recent shortcut take under earlier conversations.
const EMPTY_H: f32 = 260.;

impl Client {
    /// Give each agent the conversations its pane showed before the current one; runs on every
    /// layout. An agent no pane shows drops its store.
    pub(super) fn sync_earlier(&mut self) {
        let reads = self.server_has(OP);
        let shown: Vec<(String, Vec<String>)> = self.spaces.iter().flat_map(|s| s.all())
            .filter(|l| l.kind == "agent" && l.mirror.is_none())
            .filter_map(|l| Some((l.agent.clone()?, earlier_ids(&l.visited, reads).to_vec()))).collect();
        self.earlier.retain(|agent, _| shown.iter().any(|(a, _)| a == agent));
        for (agent, ids) in shown {
            if self.earlier.entry(agent.clone()).or_default().show(&ids) {
                self.log(format!("agent {agent} earlier conversations: {}", ids.join(",")));
            }
        }
    }
    /// How many earlier conversations `agent`'s column draws above its rows.
    pub(super) fn earlier_len(&self, agent: &str) -> usize {
        self.earlier.get(agent).map_or(0, |e| e.list.len())
    }
    /// What the earlier items were laid out at: their count and the store's generation.
    pub(super) fn earlier_shape(&self, agent: &str) -> (usize, u64) {
        self.earlier.get(agent).map_or((0, 0), |e| (e.list.len(), e.generation))
    }
    /// Item `ix` of `agent`'s column: an earlier conversation, the live transcript's row, or,
    /// under earlier conversations while the live one is empty, the prompt an empty one shows.
    pub(super) fn column_item(&mut self, agent: &str, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let earlier = self.earlier_len(agent);
        if ix < earlier {
            return self.earlier_item(agent, ix, cx).into_any_element();
        }
        if earlier > 0 && self.transcripts.get(agent).is_some_and(|t| t.rows.is_empty() && !t.busy()) {
            // The prompt and the Recent shortcut an empty transcript shows, under the dividers.
            let pane = self.spaces.iter().flat_map(|s| s.all()).find(|l| l.kind == "agent" && l.mirror.is_none() && l.agent.as_deref() == Some(agent)).map(|l| l.id.clone()).unwrap_or_default();
            return div().w_full().h(px(EMPTY_H)).child(self.empty_conversation(&pane, agent, cx)).into_any_element();
        }
        self.row_el(agent, ix - earlier, cx).into_any_element()
    }
    /// Earlier conversation `i` of `agent`: its divider, then its rows dimmed once read. Drawing
    /// it the first time asks wheeld for its events.
    fn earlier_item(&mut self, agent: &str, i: usize, cx: &mut Context<Self>) -> gpui::Div {
        let Some(id) = self.earlier.get(agent).and_then(|e| e.list.get(i)).map(|p| p.id.clone()) else { return div() };
        if self.link == Link::Up && self.earlier.get_mut(agent).is_some_and(|e| e.ask(&id)) {
            let req = self.req();
            self.history_reqs.insert(req, (agent.to_owned(), id.clone()));
            self.log(format!("agent {agent} reading earlier {id}"));
            self.send(&ClientMsg::AgentConversationHistory { req: Some(req), agent: agent.to_owned(), id: id.clone() });
        }
        let Some(past) = self.earlier.get(agent).and_then(|e| e.list.get(i)) else { return div() };
        let block = self.conversation_block(agent).or(past.missing.then_some("The agent's store no longer holds this conversation"));
        let mut resume = div().id(gpui::ElementId::Name(format!("resume-{agent}-{id}").into())).relative().flex_none()
            .px(px(8.)).py(px(2.)).rounded(px(5.)).border_1().border_color(rgb(0x3a5046)).text_xs()
            .text_color(rgb(if block.is_some() { 0x4f6a5c } else { 0xd9e8e1 }))
            .tooltip(tip(block.unwrap_or("Continue this conversation")))
            .child("Resume")
            .child(recorded(agent, &format!("resume:{id}"), &self.controls));
        if block.is_none() {
            let (a, c) = (agent.to_owned(), id.clone());
            resume = resume.cursor_pointer().hover(|s| s.bg(rgb(0x2c3d34)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.press(format!("resume {a} {c}"));
                    this.switch_conversation(&a, Some(&c));
                    cx.notify();
                }));
        }
        let divider = div().relative().w_full().flex().flex_col().gap(px(6.)).py(px(6.))
            .child(div().flex().items_center().gap(px(10.))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_xs().text_color(rgb(0x8fa89b)).child(past.title()))
                .child(resume))
            .child(div().h(px(1.)).w_full().bg(rgb(0x2c3d34)))
            .child(recorded(agent, &format!("earlier:{id}"), &self.controls));
        let mut rows = div().w_full().flex().flex_col().opacity(0.5);
        if let Some(chat) = &past.chat {
            for ix in 0..chat.rows.len() {
                rows = rows.child(self.row_of(agent, Some(&id), chat, ix, cx));
            }
        }
        div().w_full().flex().flex_col().pt(px(if i == 0 { 0. } else { 8. })).child(divider).child(rows)
    }
    /// A `conversation_history`, or wheeld refusing one. Returns whether it answered one of ours.
    pub(super) fn on_earlier_message(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::ConversationHistory { req, agent, id, found, events } => {
                if self.history_reqs.remove(req).is_none() {
                    return false;
                }
                let Some(store) = self.earlier.get_mut(agent) else { return true };
                if *found {
                    store.loaded(id, events);
                    self.log(format!("agent {agent} earlier {id}: {} events", events.len()));
                } else {
                    store.gone(id);
                    self.log(format!("agent {agent} earlier {id}: not in the store"));
                }
                true
            }
            // A refusal (a harness wheeld cannot read without loading) leaves the divider
            // titled generically and Resume usable.
            ServerMsg::Error { req: Some(req), message, .. } => match self.history_reqs.remove(req) {
                Some((agent, id)) => {
                    self.log(format!("agent {agent} earlier {id} refused: {message}"));
                    true
                }
                None => false,
            },
            _ => false,
        }
    }
    /// The connection went: reads in flight are lost, and the next layout makes the stores again.
    pub(super) fn drop_earlier(&mut self) {
        self.history_reqs.clear();
        self.earlier.clear();
    }
    /// `agent`'s earlier conversations for `/state` and the check.
    pub(super) fn earlier_json(&self, agent: &str) -> Value {
        let block = self.conversation_block(agent);
        let list = self.earlier.get(agent).map(|e| e.list.as_slice()).unwrap_or_default();
        Value::Array(list.iter().map(|p| serde_json::json!({
            "id": p.id, "title": p.title(), "missing": p.missing, "read": p.chat.is_some(),
            "rows": p.chat.as_ref().map_or(0, |c| c.rows.len()),
            "resume": block.is_none() && !p.missing,
        })).collect())
    }
}
