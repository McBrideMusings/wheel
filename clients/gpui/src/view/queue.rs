//! Orchestrator mode's queue tray (design §7 screen 2, §8.1): the title row's tray button, whose
//! badge is the `wheelhouse.queue` feed's count in its urgency's colour, opens a card listing the
//! feed's groups. Pressing an item sends its message to the orchestrator's chat, as the phone's
//! queue sheet does. The model is `crate::queue`.
use super::*;
use super::settings::{BOX, DIM, LINE, PANEL, TEXT, click};
use crate::queue::{FEED, PANE_LABEL};
use serde_json::{Value, json};

impl Client {
    /// The orchestrator space's Queue pane, which wheeld grants the feed to, and the folder it reads.
    fn queue_source(&self) -> Option<(String, String)> {
        let space = self.orchestrator_space()?;
        let space = self.spaces.iter().find(|s| s.id == space)?;
        let pane = space.all().find(|l| l.kind == "plugin" && l.label == PANE_LABEL)?;
        Some((pane.id.clone(), space.cwd.clone()))
    }
    /// The agent in the orchestrator space's chat pane.
    pub(super) fn orchestrator_agent(&self) -> Option<String> {
        let space = self.orchestrator_space()?;
        self.spaces.iter().find(|s| s.id == space)?.all().find(|l| l.kind == "agent" && l.mirror.is_none())?.agent.clone()
    }
    /// Subscribe to the queue while an orchestrator space holds a Queue pane, and drop the
    /// subscription when that pane or space goes; runs on every layout and folder change.
    pub fn sync_queue(&mut self) {
        let want = self.queue_source();
        if want == self.queue.subscribed || self.link != Link::Up {
            return;
        }
        if let Some((pane, dir)) = self.queue.subscribed.take() {
            self.send(&ClientMsg::FeedUnsubscribe { req: None, pane, feed: FEED.into(), params: json!({"dir": dir}) });
        }
        self.queue.value = None;
        if let Some((pane, dir)) = want.clone() {
            self.log(format!("queue: reading {FEED} through pane {pane} for {dir}"));
            let req = self.req();
            self.send(&ClientMsg::FeedSubscribe { req: Some(req), pane, feed: FEED.into(), params: json!({"dir": dir}) });
        } else {
            self.close_queue();
        }
        self.queue.subscribed = want;
    }
    /// A `feed_update` for the queue. Returns whether it was one.
    pub fn on_queue_message(&mut self, message: &ServerMsg) -> bool {
        let ServerMsg::FeedUpdate { pane, feed, value, patch, .. } = message else { return false };
        if feed != FEED || self.queue.subscribed.as_ref().map(|(p, _)| p) != Some(pane) {
            return false;
        }
        let before = self.queue.queue().map(|q| (q.count, q.tone));
        if let Err(why) = self.queue.update(value.as_ref(), patch.as_ref()) {
            // Later updates are patches against a value the client no longer holds: subscribing
            // again replaces the subscription and its reply carries the whole value.
            self.log(format!("queue: a patch did not apply ({why}); subscribing again"));
            if let Some((pane, dir)) = self.queue.subscribed.clone() {
                let req = self.req();
                self.send(&ClientMsg::FeedSubscribe { req: Some(req), pane, feed: FEED.into(), params: json!({"dir": dir}) });
            }
        }
        let after = self.queue.queue().map(|q| (q.count, q.tone));
        if before != after {
            self.log(format!("queue {pane}: {}", after.map(|(c, t)| format!("{c} {t}")).unwrap_or_else(|| "none".into())));
        }
        true
    }
    pub fn toggle_queue(&mut self) {
        if self.queue.open {
            self.close_queue();
        } else {
            self.queue.open = true;
            self.log("queue card opened".to_owned());
            self.sync_overlay();
        }
    }
    pub fn close_queue(&mut self) {
        if self.queue.open {
            self.queue.open = false;
            self.log("queue card closed".to_owned());
            self.sync_overlay();
        }
    }
    /// Pressing an item: its message goes to the orchestrator's chat, as if typed there.
    pub fn ask_about(&mut self, id: &str) {
        let Some(item) = self.queue.queue().and_then(|q| q.groups.into_iter().flat_map(|g| g.items).find(|i| i.id == id)) else { return };
        let Some(agent) = self.orchestrator_agent() else {
            self.log(format!("queue: no orchestrator chat to ask about {id}"));
            self.post(crate::toasts::Toast::posted(format!("queue:{id}"), crate::toasts::Level::Warning, format!("No agent chat in the orchestrator's space to ask about {id}"), None));
            return;
        };
        self.log(format!("queue: asked about {} ({}) in {agent}", item.id, item.kind));
        let req = self.req();
        self.send(&ClientMsg::AgentSend { req: Some(req), agent, text: crate::queue::message(&item) });
        self.close_queue();
    }
    /// The tray as `/state` `orchestrator.queue` carries it.
    pub fn queue_json(&self) -> Value {
        let q = self.queue.queue();
        json!({
            "pane": self.queue.subscribed.as_ref().map(|(p, _)| p),
            "open": self.queue.open,
            "count": q.as_ref().map(|q| q.count),
            "tone": q.as_ref().map(|q| q.tone.clone()),
            "groups": q.as_ref().map(|q| q.groups.iter().map(|g| json!({"id": g.id, "title": g.title, "items": g.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>()})).collect::<Vec<_>>()),
        })
    }

    pub(super) fn queue_overlay(&mut self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let queue = self.queue.queue();
        let mut list = div().flex().flex_col().gap_2().w_full().max_h(px((height - 120.).max(160.))).overflow_hidden();
        let note = |text: String| div().w_full().min_h(px(60.)).flex().items_center().justify_center().text_color(rgb(DIM)).child(text);
        match &queue {
            None => list = list.child(note("Reading the queue…".into())),
            Some(q) if !q.has_queue => list = list.child(note("The orchestrator's folder keeps no queue (no .beads directory).".into())),
            Some(q) if q.error.is_some() => list = list.child(note(format!("The queue could not be read: {}", q.error.clone().unwrap_or_default()))),
            Some(q) if q.groups.iter().all(|g| g.items.is_empty()) => list = list.child(note("Nothing waits on you.".into())),
            Some(q) => {
                for g in q.groups.iter().filter(|g| !g.items.is_empty()) {
                    let mut group = div().flex().flex_col().gap(px(2.)).w_full()
                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(DIM)).child(format!("{} · {}", g.title, g.items.len())));
                    for item in &g.items {
                        let id = item.id.clone();
                        let meta = [item.kind.clone(), item.from.clone(), q.age(&item.created)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                        group = group.child(div().flex().flex_col().w_full().px_2().py(px(4.)).rounded_sm().cursor_pointer().hover(|s| s.bg(rgb(BOX)))
                            .on_mouse_down(MouseButton::Left, click(cx, move |this| this.ask_about(&id)))
                            .child(div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(item.title.clone()))
                            .child(div().text_xs().text_color(rgb(DIM)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(meta)));
                    }
                    list = list.child(group);
                }
            }
        }
        let card = div().w(px(360f32.min(width - 32.))).flex().flex_col().gap_2().p_3().rounded_md().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT)).text_sm()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().w_full().flex().justify_between().items_center()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Queue"))
                .child(div().px_2().rounded_sm().cursor_pointer().bg(rgb(BOX)).child("×").on_mouse_down(MouseButton::Left, click(cx, |this| this.close_queue()))))
            .child(list);
        // The card hangs under its button, as the Processes card does; a press on the dim closes it.
        let anchor = self.header_chips().into_iter().find(|c| c.hit == Hit::Queue).map(|c| c.rect)
            .unwrap_or(Rect { x: width - header::TRAILING - header::BUTTON_W, y: header::BUTTON_Y, w: header::BUTTON_W, h: header::BUTTON_H });
        div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x00000055))
            .on_mouse_down(MouseButton::Left, click(cx, |this| this.close_queue()))
            .child(div().absolute().top(px(anchor.y + anchor.h + 6.)).right(px(width - anchor.x - anchor.w)).child(card))
    }
}
