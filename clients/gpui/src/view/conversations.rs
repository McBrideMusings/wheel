//! The Conversations popover (ConversationPicker.swift) as the client draws it: the pane menu's
//! Conversations… on an agent pane opens it under that pane's header, with a search field, New
//! conversation, then the agent's conversations under their day headings, the current one ticked.
//! A row resumes its conversation, New conversation starts one, and the popover closes when
//! wheeld says the agent is in it. The model is `crate::conversations`.
use super::*;
use super::settings::{BOX, DIM, LINE, PANEL, TEXT, now_ms};
use crate::tooltip::tip;
use crate::conversations::{self as conv, Listing, Popover, PAGE};
use serde_json::Value;

/// The popover's width, and the list's height (`ConversationPicker`'s 340 and 360).
const WIDTH: f32 = 340.;
const LIST_H: f32 = 360.;

impl Client {
    pub(super) fn agent_in(&self, pane: &str) -> Option<String> {
        self.spaces.iter().flat_map(|s| std::iter::once(&s.root).chain(&s.popped).chain(&s.backgrounded)).flat_map(leaves_of)
            .find(|l| l.id == pane && l.kind == "agent").and_then(|l| l.agent.clone())
    }
    /// Open the popover over `pane`'s agent and ask for the newest page.
    pub fn open_conversations(&mut self, pane: &str) -> bool {
        let Some(agent) = self.agent_in(pane) else { return false };
        self.close_pane_menu();
        self.log(format!("conversations opened for {agent} in pane {pane}"));
        self.conversations.open = Some(Popover { pane: pane.to_owned(), agent: agent.clone(), query: String::new(), listing: Listing::Loading, more: false, loading_more: false, asked_at: None });
        self.conversations.errors.remove(&agent);
        self.list_conversations();
        self.sync_overlay();
        true
    }
    pub fn close_conversations(&mut self) {
        if self.conversations.open.take().is_some() {
            self.sync_overlay();
        }
    }
    /// The newest page, replacing what the popover holds.
    fn list_conversations(&mut self) {
        let Some(agent) = self.conversations.open.as_ref().map(|p| p.agent.clone()) else { return };
        if self.link != Link::Up {
            if let Some(p) = self.conversations.open.as_mut() {
                p.listing = Listing::Failed("Not connected to wheeld.".into());
                p.more = false;
            }
            return;
        }
        let req = self.req();
        self.conversations.begin_listing(req, &agent);
        self.log(format!("conversations ask {agent}, {PAGE} at most"));
        self.send(&ClientMsg::AgentConversationLs { req: Some(req), agent, limit: Some(PAGE), before: None, before_id: None });
    }
    /// The page after the last row held, when older ones remain and none is on its way.
    fn more_conversations(&mut self) {
        if self.link != Link::Up {
            return;
        }
        let Some((agent, before, before_id)) = self.conversations.next_page() else { return };
        let req = self.req();
        self.conversations.begin_more(req, &agent);
        self.log(format!("conversations ask {agent} for {PAGE} after {before} {before_id}"));
        self.send(&ClientMsg::AgentConversationLs { req: Some(req), agent, limit: Some(PAGE), before: Some(before), before_id: Some(before_id) });
    }
    /// Put `agent` in a new conversation (`id` None) or the past one `id`.
    pub fn switch_conversation(&mut self, agent: &str, id: Option<&str>) {
        if self.link != Link::Up || self.conversation_block(agent).is_some() {
            return;
        }
        let req = self.req();
        if !self.conversations.begin_switch(req, agent, id) {
            return;
        }
        self.log(format!("conversation switch {agent} to {}", id.unwrap_or("a new one")));
        let agent = agent.to_owned();
        self.send(&match id {
            Some(id) => ClientMsg::AgentConversationResume { req: Some(req), agent, id: id.to_owned() },
            None => ClientMsg::AgentConversationNew { req: Some(req), agent },
        });
    }
    /// Whether `agent`'s harness and wheeld can fork its conversation at a turn (`AppModel`'s
    /// `canFork`: `AgentPanes.canFork` and protocol 28).
    pub fn can_fork(&self, agent: &str) -> bool {
        self.server_protocol.is_some_and(|p| p >= 28)
            && self.spaces.iter().flat_map(|s| s.all()).find(|l| l.agent.as_deref() == Some(agent)).is_some_and(|l| conv::can_fork(l.harness.as_deref()))
    }
    /// Fork `agent`'s conversation through the turn of reply `ix` (`ChatFork.swift`'s `fork`):
    /// wheeld splits a new agent pane right of this one holding it through that reply.
    pub fn fork(&mut self, agent: &str, ix: usize) {
        let can = self.can_fork(agent);
        let Some(turns) = self.transcripts.get(agent).and_then(|t| t.fork_turns(ix, can)) else { return };
        self.press(format!("fork {agent} {ix}"));
        self.log(format!("transcript {agent}: fork at row {ix}, turn {turns}"));
        let req = self.agent_req(agent, false, None);
        self.send(&ClientMsg::AgentFork { req: Some(req), agent: agent.to_owned(), turns });
    }
    /// The branch button under or over reply `ix`, with RowView.swift's tooltip.
    pub(super) fn fork_button(&self, agent: &str, ix: usize, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let a = agent.to_owned();
        let glyph = title::symbol(&self.native, &self.symbols, "arrow.triangle.branch", 11., 0x8fa89b).unwrap_or_else(|| "fork".into_any_element());
        div().id(gpui::ElementId::Name(format!("fork-{agent}-{ix}").into())).relative().cursor_pointer().text_xs().text_color(rgb(0x8fa89b))
            .tooltip(crate::tooltip::tip("Fork the conversation through this turn into a new pane"))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.fork(&a, ix);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(glyph)
            .child(recorded(agent, &format!("fork:{ix}"), &self.controls))
    }
    /// Why the rows cannot be picked now, or None.
    fn conversation_block(&self, agent: &str) -> Option<&'static str> {
        let t = self.transcripts.get(agent);
        conv::block(self.conversations.switching.contains_key(agent), t.is_some_and(Transcript::busy), t.is_some_and(|t| !t.queued_turns().is_empty()))
    }
    /// A `conversations`, a `conversation` or a refusal of one of this popover's requests.
    pub(super) fn on_conversations_message(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::Conversations { req, agent, conversations, more } => {
                let ours = self.conversations.listed(*req, conversations, *more);
                if ours {
                    self.log(format!("agent {agent} listed {} conversations, more: {more}", conversations.len()));
                    // A search looks through every conversation, so each page asks for the next.
                    if self.conversations.open.as_ref().is_some_and(|p| !p.query.is_empty()) {
                        self.more_conversations();
                    }
                }
                ours
            }
            ServerMsg::Conversation { req, agent, id } => {
                let ours = self.conversations.switched(*req);
                if ours {
                    self.log(format!("agent {agent} is in conversation {id}"));
                    self.sync_overlay();
                }
                ours
            }
            ServerMsg::Error { req: Some(req), message, .. } => self.conversations.refused(*req, message),
            _ => false,
        }
    }
    /// A key while the popover is open: it types into the search; Esc closes it. It takes every key.
    pub fn conversations_key(&mut self, key: &str, text: Option<&str>, chord: bool) {
        let Some(p) = self.conversations.open.as_mut() else { return };
        match key {
            "escape" => return self.close_conversations(),
            "backspace" if !chord => {
                p.query.pop();
            }
            _ if !chord => {
                if let Some(t) = text.filter(|t| !t.chars().any(char::is_control)) {
                    p.query.push_str(t);
                }
            }
            _ => return,
        }
        if !p.query.is_empty() {
            self.more_conversations();
        }
    }
    /// The popover for `/state`.
    pub fn conversations_json(&self) -> Value {
        let Some(p) = &self.conversations.open else { return Value::Null };
        let now = (now_ms() / 1000) as i64;
        let (listing, rows) = match &p.listing {
            Listing::Loading => ("loading".to_owned(), vec![]),
            Listing::Failed(m) => (format!("failed: {m}"), vec![]),
            Listing::Loaded(all) => ("loaded".to_owned(), conv::grouped(&conv::matches(all, &p.query), now).into_iter().flat_map(|(day, rows)| rows.into_iter().map(move |c| serde_json::json!({
                "id": c.id, "title": conv::name(c), "day": day.label(), "time": conv::time(conv::updated(c), day), "current": c.current, "reply": c.last_reply,
            }))).collect()),
        };
        serde_json::json!({
            "pane": p.pane, "agent": p.agent, "query": p.query, "listing": listing, "rows": rows, "more": p.more, "loading_more": p.loading_more,
            "switching": self.conversations.switching.get(&p.agent), "block": self.conversation_block(&p.agent),
            "note": self.conversations.note(&p.agent),
        })
    }

    pub(super) fn conversations_overlay(&mut self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let p = self.conversations.open.clone().expect("drawn only while open");
        let agent = p.agent.clone();
        let block = self.conversation_block(&agent);
        let switching = self.conversations.switching.get(&agent).cloned();
        let controls = self.controls.clone();
        let rec = |id: &str| recorded(&agent, id, &controls).into_any_element();
        let field: gpui::Div = if p.query.is_empty() { div().text_color(rgb(DIM)).child("Search conversations") } else { div().child(p.query.clone()) };
        // New conversation: one plain line, disabled (dimmed) while a turn runs or a switch waits.
        let mut new_row = row_base(false, block.is_none(), block.is_none()).id("conversation-new").child(rec("conversation:new"))
            .child(div().w(px(14.)).flex_none().child(if switching.as_deref() == Some("") { "…" } else { "+" }))
            .child(div().child("New conversation"));
        new_row = new_row.tooltip(tip(block.unwrap_or("Start a new conversation")));
        if block.is_none() {
            new_row = new_row.on_mouse_down(MouseButton::Left, pick(cx, agent.clone(), None));
        }
        // The next page is asked for when a scroll brings the list's end into view (the Swift
        // row's onAppear), never from the frame itself, whose scroll extent is the last layout's.
        let mut list = div().id("conversations-list").flex().flex_col().gap(px(6.)).w_full().h(px(LIST_H)).overflow_y_scroll().track_scroll(&self.conversations_scroll).child(rec("conversations-list"))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                let s = &this.conversations_scroll;
                if f32::from(s.max_offset().y) + f32::from(s.offset().y) < 40. {
                    this.more_conversations();
                    cx.notify();
                }
            }));
        match &p.listing {
            Listing::Loading => list = list.child(centred("Loading…")),
            Listing::Failed(m) => list = list.child(centred(m)),
            Listing::Loaded(all) => {
                let shown = conv::matches(all, &p.query);
                if shown.is_empty() && !p.more {
                    list = list.child(centred(if all.is_empty() { "No conversations in this directory yet." } else { "Nothing matches." }));
                }
                let now = (now_ms() / 1000) as i64;
                for (day, rows) in conv::grouped(&shown, now) {
                    list = list.child(div().px(px(8.)).pt(px(4.)).text_xs().text_color(rgb(0x6d8379)).child(day.label()));
                    for c in rows {
                        let enabled = block.is_none() && !c.current;
                        let busy = switching.as_deref() == Some(c.id.as_str());
                        let name = conv::name(c);
                        let mark = if busy { "…" } else if c.current { "✓" } else { "" };
                        let reply = c.last_reply.as_deref().map(conv::reply).unwrap_or_default();
                        let head = div().flex().items_center().gap(px(6.)).w_full().whitespace_nowrap().overflow_hidden()
                            .child(div().w(px(14.)).flex_none().text_color(rgb(0x7fb89a)).child(mark))
                            .child(div().flex_1().overflow_hidden().text_ellipsis().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                            .children(conv::time(conv::updated(c), day).map(|t| div().flex_none().text_xs().text_color(rgb(0x6d8379)).child(t)));
                        // The reply always takes two lines, so every card is the same height.
                        let body = div().h(px(32.)).overflow_hidden().text_xs().text_color(rgb(0x9db3a8)).child(styled_reply(reply));
                        let mut card = row_base(true, enabled, enabled || c.current).id(gpui::ElementId::Name(format!("conversation-{}", c.id).into()))
                            .flex_col().items_start().child(rec(&format!("conversation:{}", c.id))).child(head).child(body)
                            .tooltip(tip(block.unwrap_or(&name)));
                        if enabled {
                            card = card.on_mouse_down(MouseButton::Left, pick(cx, agent.clone(), Some(c.id.clone())));
                        }
                        list = list.child(card);
                    }
                }
                if p.loading_more {
                    list = list.child(div().w_full().flex().justify_center().text_xs().text_color(rgb(DIM)).child("Loading…"));
                }
            }
        }
        let refused = self.conversations.errors.contains_key(&agent);
        let card = div().w(px(WIDTH)).flex().flex_col().gap(px(6.)).p(px(10.)).rounded_md().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT)).text_sm()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().relative().w_full().px_2().py_1().rounded_sm().bg(rgb(BOX)).border_1().border_color(rgb(LINE)).child(field).child(rec("conversations-search")))
            .child(new_row)
            .child(div().h(px(1.)).w_full().bg(rgb(LINE)))
            .child(list)
            // One slot for both, so nothing moves when a refusal replaces the note.
            .child(div().h(px(30.)).overflow_hidden().text_xs().text_color(rgb(if refused { 0xb7cbc1 } else { 0x6d8379 })).child(self.conversations.note(&agent)));
        // Under the pane's header, at its right edge, as the Swift popover hangs from the header.
        let (leaves, _) = self.scene();
        let r = leaves.iter().find(|l| l.leaf.id == p.pane).map(|l| l.rect).unwrap_or(Rect { x: width - WIDTH - 8., y: HEADER, w: WIDTH, h: 0. });
        let left = (r.x + r.w - WIDTH - 8.).clamp(4., (width - WIDTH - 4.).max(4.));
        let top = (r.y + TITLE + 4.).min((height - LIST_H - 140.).max(HEADER));
        div().absolute().left_0().top_0().w(px(width)).h(px(height))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.close_conversations();
                cx.notify();
            }))
            .child(div().absolute().left(px(left)).top(px(top)).child(card))
    }
}

/// A press that switches `agent` to a new conversation (`id` None) or the past one `id`.
fn pick(cx: &mut Context<Client>, agent: String, id: Option<String>) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
        cx.stop_propagation();
        this.switch_conversation(&agent, id.as_deref());
        cx.notify();
    })
}

/// A list line: a card has a soft fill that deepens under the pointer; a plain line fills only
/// then. Only a pressable line reacts to the pointer; a dimmed one is drawn at 40%.
fn row_base(card: bool, enabled: bool, bright: bool) -> gpui::Div {
    let mut d = div().relative().w_full().flex().items_center().gap(px(6.)).px(px(8.)).py(px(if card { 7. } else { 4. })).rounded(px(6.));
    if card {
        d = d.bg(rgba(0xffffff0a));
    }
    if enabled {
        d = d.cursor_pointer().hover(|s| s.bg(rgba(0xffffff17)));
    }
    if !bright {
        d = d.opacity(0.4);
    }
    d
}

fn centred(text: &str) -> gpui::Div {
    div().w_full().h(px(LIST_H - 20.)).flex().items_center().justify_center().text_xs().text_color(rgb(DIM)).child(text.to_owned())
}

/// A reply's inline Markdown: bold, italic and code drawn, links underlined.
fn styled_reply(t: Text) -> StyledText {
    let highlights: Vec<(Range<usize>, HighlightStyle)> = t.runs.iter().map(|r| (r.range.clone(), HighlightStyle {
        font_weight: r.mark.bold.then_some(FontWeight::BOLD),
        font_style: r.mark.italic.then_some(FontStyle::Italic),
        underline: r.mark.link.as_ref().map(|_| UnderlineStyle { thickness: px(1.), ..Default::default() }),
        color: r.mark.code.then(|| rgb(0xd9e8e1).into()),
        ..Default::default()
    })).collect();
    StyledText::new(t.text).with_highlights(highlights)
}

