//! Orchestrator mode's topic line (design §7 screens 3 and 6, §8.2 M1/M8): the current topic's
//! title under the window's title, and pressing it opens the topic's notes, read-only, drawn by the
//! chat's Markdown renderer. The model is `crate::topic`; the reads mirror the phone's
//! `PhoneModel.readTopic`.
use super::*;
use super::settings::{DIM, LINE, PANEL, TEXT, BOX, click};
use crate::topic::{CURRENT, Read, Topic};
use serde_json::{Value, json};

impl Client {
    /// Read `threads/current`, then that topic's notes: on entering Orchestrator mode, when the
    /// folder is listed, and each time the orchestrator's turn ends or its chat attaches, since
    /// `tools/note` and `tools/switch` run inside its turns.
    pub fn read_topic(&mut self) {
        if self.mode != "orchestrator" || self.link != Link::Up {
            return;
        }
        let Some(dir) = self.orchestrator_dir.clone() else { return };
        let req = self.req();
        self.topic.reqs.insert(req, Read::Current);
        self.send(&ClientMsg::FilePeek { req: Some(req), path: CURRENT.into(), cwd: Some(dir), line: None, context: Some(1) });
    }
    /// wheeld's answer to a topic read. Returns whether the answer was one of these.
    pub(super) fn on_topic_peek(&mut self, req: u64, found: bool, lines: &[String]) -> bool {
        let Some(read) = self.topic.reqs.remove(&req) else { return false };
        match read {
            Read::Current => {
                let id = found.then(|| crate::topic::current_id(lines)).flatten();
                match (id, self.orchestrator_dir.clone()) {
                    (Some(id), Some(dir)) if self.link == Link::Up => {
                        let req = self.req();
                        self.topic.reqs.insert(req, Read::Notes(id.clone()));
                        self.send(&ClientMsg::FilePeek { req: Some(req), path: crate::topic::notes_path(&id), cwd: Some(dir), line: None, context: Some(200) });
                    }
                    _ => {
                        if self.topic.topic.take().is_some() {
                            self.log("topic: none current".to_owned());
                        }
                        self.close_topic();
                    }
                }
            }
            Read::Notes(id) => {
                let topic = Topic::of(&id, if found { lines } else { &[] });
                if self.topic.topic.as_ref() != Some(&topic) {
                    self.log(format!("topic {id}: {}, {}", topic.title, if found { format!("{} lines", lines.len()) } else { "no notes file".into() }));
                    self.topic.topic = Some(topic);
                }
            }
        }
        true
    }
    /// A chat event for the orchestrator's agent that may follow a topic change: its turn ended.
    pub(super) fn topic_after(&mut self, agent: &str, event: &wheel_proto::AgentEvent) {
        if matches!(event, wheel_proto::AgentEvent::TurnEnd { .. }) && self.orchestrator_agent().as_deref() == Some(agent) {
            self.read_topic();
        }
    }
    /// The topic the title row shows: Orchestrator mode only.
    pub fn shown_topic(&self) -> Option<&Topic> {
        self.topic.topic.as_ref().filter(|_| self.mode == "orchestrator")
    }
    pub fn toggle_topic(&mut self) {
        if self.topic.open {
            self.close_topic();
        } else if let Some(id) = self.shown_topic().map(|t| t.id.clone()) {
            self.topic.open = true;
            self.log(format!("topic notes: opened {id}"));
            self.sync_overlay();
        }
    }
    pub fn close_topic(&mut self) {
        if self.topic.open {
            self.topic.open = false;
            self.log("topic notes closed".to_owned());
            self.sync_overlay();
        }
    }
    /// The topic as `/state` `orchestrator.topic` carries it.
    pub fn topic_json(&self) -> Value {
        match self.shown_topic() {
            Some(t) => json!({"id": t.id, "title": t.title, "notes": t.notes, "open": self.topic.open}),
            None => json!({"id": null, "open": self.topic.open}),
        }
    }

    pub(super) fn topic_overlay(&mut self, width: f32, height: f32, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let topic = self.shown_topic()?.clone();
        let anchor = self.header_chips().into_iter().find(|c| c.hit == Hit::Topic)?.rect;
        let mut body = div().flex().flex_col().gap_2().w_full();
        if topic.notes.is_empty() {
            body = body.child(div().text_color(rgb(DIM)).child("No notes yet"));
        } else {
            let dir = self.orchestrator_dir.clone().unwrap_or_default();
            let mut texts = Texts { agent: "topic-notes".into(), row: 0, next: 0, selection: None, drawn: &self.drawn, kept: None, images: &self.images, diagrams: &self.diagrams, diagram: 0, charts: &self.charts, chart: 0, dir };
            for block in crate::markdown::parse(&topic.notes) {
                body = body.child(match &block {
                    // A notes heading labels its section, as M8 draws it: body size, semibold and dimmed, so the
                    // topic's title stays the largest text on the card.
                    crate::markdown::Block::Heading { text, .. } => div().pt_1().font_weight(FontWeight::SEMIBOLD).child(texts.inline("heading", text, DIM)),
                    _ => block_el(&block, &mut texts, &self.highlights, cx),
                });
            }
        }
        let card_w = 420f32.min(width - 32.);
        // Notes longer than the card scroll inside it, as the phone's sheet does.
        let body = div().id("topic-notes").overflow_y_scroll().flex_1().min_h_0().child(body);
        let card = div().w(px(card_w)).max_h(px((height - anchor.y - anchor.h - 40.).max(160.))).overflow_hidden().flex().flex_col().gap_2().p_3().rounded_md().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT)).text_sm()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().w_full().flex().justify_between().items_center()
                .child(div().text_xs().text_color(rgb(DIM)).child("Topic notes"))
                .child(div().px_2().rounded_sm().cursor_pointer().bg(rgb(BOX)).child("×").on_mouse_down(MouseButton::Left, click(cx, |this| this.close_topic()))))
            .child(div().text_lg().font_weight(FontWeight::BOLD).child(topic.title.clone()))
            .child(body);
        // The card hangs under the topic line from its left end; a press on the dim closes it.
        let left = anchor.x.min(width - card_w - 8.).max(8.);
        Some(div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x00000055))
            .on_mouse_down(MouseButton::Left, click(cx, |this| this.close_topic()))
            .child(div().absolute().top(px(anchor.y + anchor.h + 6.)).left(px(left)).child(card)))
    }
}
