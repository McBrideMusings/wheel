//! The window's content: a header with the sessions and the shown session's spaces (ADR-0014),
//! the shown space's split tree drawn as
//! panes and draggable dividers, and the real Ghostty and WKWebView views on the first
//! terminal, web and browser pane. An agent pane draws its transcript, an empty pane offers
//! what it can become; only a kind this build does not know is drawn as unimplemented.
use crate::{daemon::{Endpoint, Event}, focus::{Focus, Typed, type_into}, header::{self, Hit}, layout::*, native::{Bridge, Native}, transcript::{Kind, ToolRow, Transcript}};
use gpui::{App, Context, FocusHandle, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, ScrollHandle, Window, div, prelude::*, px, rgb};
use wheel_proto::{ClientMsg, NewPaneKind};
use std::{collections::{HashMap, HashSet}, rc::Rc, sync::mpsc};

pub const TITLE: f32 = 22.0;
pub const HEADER: f32 = header::HEIGHT;

pub struct Client {
    native: Rc<Native>,
    bridge: Rc<Bridge>,
    /// The test mode the window was opened in; a window this one opens is opened the same way.
    hidden: bool,
    /// Names this window to wheeld in `view` reports (ADR-0022).
    window_id: String,
    sessions: Vec<Session>,
    /// The session shown: kept across layouts by id.
    session_id: Option<String>,
    /// The last (session, space, pane) reported to wheeld.
    reported: Option<(Option<String>, Option<String>, Option<String>)>,
    out: mpsc::Sender<String>,
    endpoint: Endpoint,
    spaces: Vec<Space>,
    /// The space shown: kept across layouts by id.
    space_id: Option<String>,
    /// Ratios being dragged, until the daemon confirms them in its next layout.
    overrides: HashMap<String, f64>,
    drag: Option<(String, bool, Rect)>,
    /// Panes drawn by the native views in the space shown, in layout order.
    pub native_panes: Vec<String>,
    /// Every pane that has a native view, with its kind, and where it was last placed.
    opened: HashMap<String, String>,
    placed: HashMap<String, ([f64; 4], bool)>,
    viewport: (f32, f32),
    /// The conversation of every agent pane seen, by agent name, and the agents attached to on
    /// this connection.
    pub transcripts: HashMap<String, Transcript>,
    /// The transcript column's scroll position per agent; it follows the stream at the bottom.
    scrolls: HashMap<String, ScrollHandle>,
    attached: HashSet<String>,
    /// The pane with the keyboard, and what has been typed to each agent and not yet sent.
    pub focus: Focus,
    drafts: HashMap<String, String>,
    /// GPUI delivers keys only to a focused element: the root holds this for good, while the
    /// keyboard itself moves between the window's views by AppKit's first responder.
    handle: FocusHandle,
    /// What the connection to wheeld is doing, shown at the right of the header.
    pub link: Link,
}

/// The state of the connection to wheeld.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    Connecting,
    Up,
    Down(String),
    Incompatible { server: u32, client: u32 },
}
impl Link {
    pub fn banner(&self) -> String {
        match self {
            Link::Up => String::new(),
            Link::Connecting => "connecting to wheeld".into(),
            Link::Down(_) => "wheeld is not reachable, reconnecting".into(),
            Link::Incompatible { server, client } => format!("wheeld speaks protocol {server}, this client needs {client}: restart wheeld"),
        }
    }
}

impl Client {
    #[allow(clippy::too_many_arguments)]
    pub fn new(native: Rc<Native>, bridge: Rc<Bridge>, hidden: bool, window_id: String, out: mpsc::Sender<String>, endpoint: Endpoint, prefer_session: Option<String>, prefer_space: Option<String>, handle: FocusHandle) -> Self {
        Self {
            native, bridge, hidden, window_id, sessions: vec![], session_id: prefer_session, reported: None, out, endpoint, spaces: vec![], space_id: prefer_space, overrides: HashMap::new(), drag: None,
            native_panes: vec![], opened: HashMap::new(), placed: HashMap::new(), viewport: (1200., 720.), transcripts: HashMap::new(), scrolls: HashMap::new(), attached: HashSet::new(), focus: Focus::default(), drafts: HashMap::new(), handle, link: Link::Connecting,
        }
    }
    /// Take one thing the connection reported. Returns true when it was a layout.
    pub fn on_event(&mut self, event: Event) -> bool {
        match event {
            Event::Message(message) => match crate::layout::parse_layout(&message) {
                Some(layout) => {
                    self.apply(layout);
                    true
                }
                None => {
                    for (agent, t) in &mut self.transcripts {
                        // Follow the stream only while the column is at its bottom; a reader who
                        // scrolled up stays where they are.
                        let at_bottom = self.scrolls.get(agent).map(|s| -f32::from(s.offset().y) >= f32::from(s.max_offset().y) - 8.);
                        if t.on_message(&message, agent)
                            && at_bottom == Some(true)
                            && let Some(scroll) = self.scrolls.get(agent)
                        {
                            scroll.scroll_to_bottom();
                        }
                    }
                    false
                }
            },
            Event::Up(endpoint) => {
                // The daemon may have restarted: its terminals and its token are new, so every
                // native view is closed, the bridge's socket dropped, and the views open again
                // against the new endpoint and layout.
                if self.link != Link::Connecting {
                    // The socket goes first: a detach sent down a dead socket never finishes.
                    self.native.disconnect();
                    for id in self.opened.keys().cloned().collect::<Vec<_>>() {
                        self.native.close(&id);
                    }
                    self.opened.clear();
                    self.placed.clear();
                    self.attached.clear();
                    self.transcripts.clear();
                    // The layout we hold is the old daemon's: its terminal ids mean nothing to the
                    // new one. Nothing opens until the layout that follows this event arrives.
                    self.spaces.clear();
                    self.sessions.clear();
                    self.reported = None;
                    self.overrides.clear();
                    self.drag = None;
                }
                self.endpoint = endpoint;
                self.link = Link::Up;
                false
            }
            Event::Down(reason) => {
                self.link = Link::Down(reason);
                false
            }
            Event::Incompatible { server, client } => {
                self.link = Link::Incompatible { server, client };
                false
            }
        }
    }
    /// The daemon's layout replaces ours, dragged ratios included.
    pub fn apply(&mut self, layout: Layout) {
        self.spaces = layout.spaces;
        self.sessions = layout.sessions;
        self.overrides.clear();
        // The session shown stays unless it is gone; a space shown stays while its session
        // holds it. A session that holds the space asked for (a new window's) is the one shown.
        if !self.sessions.iter().any(|s| Some(&s.id) == self.session_id.as_ref()) {
            let holder = self.sessions.iter().find(|s| self.space_id.as_ref().is_some_and(|id| s.spaces.contains(id)));
            self.session_id = holder.or(self.sessions.first()).map(|s| s.id.clone());
        }
        if !self.session_spaces().iter().any(|s| Some(&s.id) == self.space_id.as_ref()) {
            self.space_id = self.session_spaces().first().map(|s| s.id.clone());
        }
        self.sync_view();
    }
    /// The spaces of the session shown, in the session's order; every space when wheeld lists
    /// no sessions.
    pub fn session_spaces(&self) -> Vec<&Space> {
        match self.sessions.iter().find(|s| Some(&s.id) == self.session_id.as_ref()) {
            Some(session) => session.spaces.iter().filter_map(|id| self.spaces.iter().find(|s| &s.id == id)).collect(),
            None => self.spaces.iter().collect(),
        }
    }
    pub fn window_id(&self) -> &str {
        &self.window_id
    }
    pub fn native(&self) -> Rc<Native> {
        self.native.clone()
    }
    /// The header's chips as drawn at the window's current size.
    pub fn header_chips(&self) -> Vec<header::Chip> {
        let pair = |id: &String, name: &String| (id.clone(), name.clone());
        let sessions: Vec<_> = self.sessions.iter().map(|s| pair(&s.id, &s.name)).collect();
        let spaces: Vec<_> = self.session_spaces().iter().map(|s| pair(&s.id, &s.name)).collect();
        header::chips(self.viewport.0, &sessions, &spaces, self.session_id.as_deref(), self.space_id.as_deref())
    }
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
    pub fn space_id(&self) -> Option<&str> {
        self.space_id.as_deref()
    }
    fn space(&self) -> Option<&Space> {
        self.spaces.iter().find(|s| Some(&s.id) == self.space_id.as_ref())
    }
    /// Show `session`: the space it was last showing is not remembered, so its first space.
    pub fn select_session(&mut self, session: &str) {
        if self.session_id.as_deref() == Some(session) || !self.sessions.iter().any(|s| s.id == session) {
            return;
        }
        self.session_id = Some(session.to_owned());
        self.space_id = self.session_spaces().first().map(|s| s.id.clone());
        self.overrides.clear();
        self.sync_view();
    }
    /// Show `space` of the session shown.
    pub fn select_space(&mut self, space: &str) {
        if self.space_id.as_deref() != Some(space) && self.session_spaces().iter().any(|s| s.id == space) {
            self.space_id = Some(space.to_owned());
            self.overrides.clear();
            self.sync_view();
        }
    }
    /// Tell wheeld what this window shows (ADR-0022) when it differs from the last report.
    fn sync_view(&mut self) {
        if self.link != Link::Up {
            return;
        }
        // A pane is reported only while it is one of the space shown.
        let pane = self.focus.current.clone().filter(|p| self.space().is_some_and(|s| leaves_of(&s.root).iter().any(|l| &l.id == p)));
        let now = (self.session_id.clone(), self.space_id.clone(), pane);
        if now.0.is_none() && now.1.is_none() || self.reported.as_ref() == Some(&now) {
            return;
        }
        self.send(&ClientMsg::View { req: None, window: self.window_id.clone(), session: now.0.clone(), space: now.1.clone(), pane: now.2.clone() });
        self.reported = Some(now);
    }
    /// Open another window over its own connection, showing what this one shows.
    pub fn new_window(&self, cx: &mut App) {
        let _ = crate::open_window(cx, &self.bridge, self.endpoint.clone(), self.session_id.clone(), self.space_id.clone(), self.hidden);
    }
    /// The window is closing: drop its native views. Its connection ends with it, and wheeld
    /// drops the window's reported view with the connection.
    pub fn shutdown(&mut self) {
        self.native.detach();
    }
    pub fn scene(&self) -> (Vec<Placed>, Vec<Divider>) {
        let (mut leaves, mut dividers) = (vec![], vec![]);
        if let Some(space) = self.space() {
            let area = Rect { x: 0., y: HEADER, w: self.viewport.0, h: self.viewport.1 - HEADER };
            place(&space.root, area, &self.overrides, &mut leaves, &mut dividers);
        }
        (leaves, dividers)
    }
    pub fn tree_ratio(&self, split: &str) -> Option<f64> {
        fn find(t: &Tree, id: &str) -> Option<f64> {
            match t {
                Tree::Split { id: i, ratio, a, b, .. } => if i == id { Some(*ratio) } else { find(a, id).or_else(|| find(b, id)) },
                _ => None,
            }
        }
        self.space().and_then(|s| find(&s.root, split))
    }
    /// Frames go out only while the link is up; the connection drops any others.
    fn send(&self, message: &ClientMsg) {
        if self.link == Link::Up {
            let _ = self.out.send(crate::daemon::frame(message));
        }
    }
    pub fn request_status(&self, req: u64) {
        self.send(&ClientMsg::Status { req: Some(req) });
    }
    fn end_drag(&mut self) {
        if let Some((split, _, _)) = self.drag.take() {
            if let Some(ratio) = self.overrides.get(&split) {
                self.send(&ClientMsg::SplitRatio { req: None, split: split.clone(), ratio: *ratio });
            }
        }
    }
    fn move_drag(&mut self, x: f32, y: f32) {
        if let Some((split, horizontal, parent)) = &self.drag {
            self.overrides.insert(split.clone(), drag_ratio(*horizontal, *parent, x, y));
        }
    }
    /// Attach to every agent pane's agent once per connection, so its transcript streams in.
    fn sync_agents(&mut self, leaves: &[Placed]) {
        for p in leaves {
            if let Some(agent) = p.leaf.agent.as_ref().filter(|_| p.leaf.kind == "agent") {
                if self.link == Link::Up && self.attached.insert(agent.clone()) {
                    self.transcripts.entry(agent.clone()).or_default();
                    self.send(&ClientMsg::AgentAttach { req: None, agent: agent.clone() });
                }
            }
        }
    }
    /// Give `pane` the keyboard: its native view, or the GPUI view for a pane GPUI draws.
    fn focus_pane(&mut self, pane: &str) {
        self.focus.current = Some(pane.to_owned());
        self.native.focus(if self.native_panes.iter().any(|p| p == pane) { pane } else { "" });
        self.sync_view();
    }
    fn key(&mut self, e: &KeyDownEvent) -> bool {
        let k = &e.keystroke;
        if k.modifiers.platform && matches!(k.key.as_str(), "[" | "]") {
            // A native view may have taken the keyboard by a click GPUI never saw.
            let status = self.native.status();
            let held = self.native_panes.iter().find(|p| status["panes"][p.as_str()]["focused"] == true).cloned()
                .or_else(|| status["held_before_key"].as_str().filter(|h| !h.is_empty()).map(str::to_owned));
            if held.is_some() {
                self.focus.current = held;
            }
            let order: Vec<String> = self.scene().0.into_iter().map(|p| p.leaf.id).collect();
            if let Some(next) = self.focus.step(&order, k.key == "]") {
                self.focus_pane(&next);
            }
            return true;
        }
        let agent = self.scene().0.into_iter()
            .find(|p| Some(&p.leaf.id) == self.focus.current.as_ref() && p.leaf.kind == "agent")
            .and_then(|p| p.leaf.agent);
        let Some(agent) = agent else { return false };
        let draft = self.drafts.entry(agent.clone()).or_default();
        match type_into(draft, &k.key, k.key_char.as_deref(), k.modifiers.platform || k.modifiers.control) {
            Typed::Nothing => false,
            Typed::Edited => true,
            Typed::Send(text) => {
                self.send(&ClientMsg::AgentSend { req: None, agent: agent.clone(), text });
                true
            }
        }
    }
    /// Turn an empty pane into `kind` (`terminal` or `browser`).
    fn set_kind(&self, pane: &str, kind: NewPaneKind) {
        self.send(&ClientMsg::PaneSetKind { req: None, pane: pane.to_owned(), kind, artifact: None, scope: None, url: None });
    }
    /// Give every web pane and every terminal pane (with a daemon terminal) its own native view,
    /// put each on its pane's body, hide the views of panes in other spaces, and close the views
    /// of panes no space has any more.
    fn sync_native(&mut self, leaves: &[Placed]) {
        self.native_panes.clear();
        for p in leaves {
            let kind = match (p.leaf.kind.as_str(), &p.leaf.term) {
                ("web", _) => "web".to_owned(),
                ("browser", _) => format!("browser:{}", p.leaf.url.clone().unwrap_or_default()),
                ("terminal", Some(_)) => "terminal".to_owned(),
                _ => continue,
            };
            if self.opened.get(&p.leaf.id) != Some(&kind) {
                match (&p.leaf.term, p.leaf.kind.as_str()) {
                    (Some(term), _) => self.native.open_terminal(&p.leaf.id, term, &self.endpoint.ws),
                    (None, "browser") => self.native.open_web(&p.leaf.id, p.leaf.url.as_deref().unwrap_or("about:blank")),
                    (None, _) => self.native.open_artifact(&p.leaf.id, &self.endpoint.artifact(&p.leaf.id), &p.leaf.label, p.leaf.scope.as_deref().unwrap_or("project"), self.space_id.as_deref(), &self.endpoint.ws),
                }
                self.opened.insert(p.leaf.id.clone(), kind);
            }
            let rect = [f64::from(p.rect.x), f64::from(p.rect.y + TITLE), f64::from(p.rect.w), f64::from((p.rect.h - TITLE).max(1.))];
            if self.placed.get(&p.leaf.id) != Some(&(rect, false)) {
                self.native.place(&p.leaf.id, rect, false);
                self.placed.insert(p.leaf.id.clone(), (rect, false));
            }
            self.native_panes.push(p.leaf.id.clone());
        }
        let shown: HashSet<&String> = self.native_panes.iter().collect();
        for (id, (rect, hidden)) in self.placed.clone() {
            if !shown.contains(&id) && !hidden {
                self.native.place(&id, rect, true);
                self.placed.insert(id, (rect, true));
            }
        }
        let alive: HashSet<&str> = self.spaces.iter().flat_map(|s| leaves_of(&s.root)).map(|l| l.id.as_str()).collect();
        for id in self.opened.keys().cloned().collect::<Vec<_>>() {
            if !alive.contains(id.as_str()) {
                self.native.close(&id);
                self.opened.remove(&id);
                self.placed.remove(&id);
            }
        }
    }
}

impl Render for Client {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = window.viewport_size();
        self.viewport = (f32::from(v.width), f32::from(v.height));
        let (leaves, dividers) = self.scene();
        if !self.handle.is_focused(window) {
            window.focus(&self.handle, cx);
        }
        self.sync_native(&leaves);
        self.sync_agents(&leaves);
        let mut root = div().relative().track_focus(&self.handle)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if this.key(e) {
                    cx.notify();
                }
            })).size_full().bg(rgb(0x0e1311)).text_color(rgb(0xd9e8e1)).text_sm()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    this.move_drag(f32::from(e.position.x), f32::from(e.position.y));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                this.end_drag();
                cx.notify();
            }));
        let chips = self.header_chips();
        if !chips.iter().any(|c| matches!(c.hit, Hit::Space(_))) {
            root = root.child(div().absolute().left(px(header::MARGIN)).top(px(header::SPACES_Y)).child("waiting for wheeld"));
        }
        for chip in chips {
            let (r, hit) = (chip.rect, chip.hit.clone());
            root = root.child(
                div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).px_2().rounded_sm().cursor_pointer().overflow_hidden().whitespace_nowrap()
                    .flex().items_center()
                    .bg(rgb(if chip.active { 0x31483e } else { 0x17201c }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        match &hit {
                            Hit::Session(id) => this.select_session(id),
                            Hit::Space(id) => this.select_space(id),
                            Hit::NewWindow => this.new_window(cx),
                        }
                        cx.notify();
                    }))
                    .child(chip.label),
            );
        }
        // Always drawn, so the header's controls stay where they are; empty while connected.
        let banner = header::banner_rect(self.viewport.0);
        root = root.child(div().absolute().left(px(banner.x)).top(px(banner.y)).w(px(banner.w)).h(px(banner.h)).flex().justify_end().items_center()
            .text_color(rgb(0xe0b070)).child(self.link.banner()));
        for p in &leaves {
            let r = p.rect;
            let native = self.native_panes.contains(&p.leaf.id);
            let id = p.leaf.id.clone();
            let body = match p.leaf.kind.as_str() {
                _ if native => div(),
                "agent" => {
                    // A pane whose agent has not started yet shares no draft or scroll state with another.
                    let agent = p.leaf.agent.as_deref();
                    let t = agent.and_then(|a| self.transcripts.get(a));
                    let draft = agent.and_then(|a| self.drafts.get(a)).map(String::as_str).unwrap_or("");
                    let scroll = agent.map(|a| self.scrolls.entry(a.to_owned()).or_default().clone());
                    let focused = self.focus.current.as_ref() == Some(&p.leaf.id);
                    agent_pane(&p.leaf.label, &p.leaf.id, t, draft, focused, scroll, r.h - TITLE)
                }
                "empty" => {
                    let button = |label: &'static str, kind: NewPaneKind, cx: &mut Context<Self>| {
                        let id = id.clone();
                        div().px_2().py_1().rounded_sm().cursor_pointer().bg(rgb(0x31483e))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, _| this.set_kind(&id, kind)))
                            .child(label)
                    };
                    div().flex().gap_2().child(button("Terminal", NewPaneKind::Terminal, cx)).child(button("Browser", NewPaneKind::Browser, cx))
                }
                other => div().child(format!("unimplemented: {other} pane\n{}", p.leaf.label)),
            };
            root = root.child(
                div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).bg(rgb(0x17201c))
                    .border_1().border_color(rgb(if self.focus.current.as_ref() == Some(&p.leaf.id) { 0x7fb89a } else { 0x2c3d34 }))
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let id = p.leaf.id.clone();
                        move |this, _, _, cx| {
                            this.focus_pane(&id);
                            cx.notify();
                        }
                    }))
                    .child(div().h(px(TITLE)).px_2().bg(rgb(0x22302a)).text_xs().child(format!("{} · {}", p.leaf.kind, p.leaf.id)))
                    .child(div().p_2().text_color(rgb(0x8fa89b)).child(body)),
            );
        }
        for d in dividers {
            let (split, horizontal, parent, r) = (d.split.clone(), d.horizontal, d.parent, d.rect);
            let bar = div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).bg(rgb(0x3d5247))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.drag = Some((split.clone(), horizontal, parent));
                    cx.notify();
                }));
            root = root.child(if horizontal { bar.cursor_col_resize() } else { bar.cursor_row_resize() });
        }
        root
    }
}

/// The composer's height, so the transcript column above it never moves.
const COMPOSER: f32 = 44.;

/// An agent pane's body: its label and state, the transcript column scrolled to follow the
/// stream, a running line while a turn is in progress, and the composer with the draft.
fn agent_pane(label: &str, pane: &str, t: Option<&Transcript>, draft: &str, focused: bool, scroll: Option<ScrollHandle>, height: f32) -> gpui::Div {
    let state = t.map(|t| t.state.as_str()).unwrap_or("connecting");
    let mut col = div().id(gpui::ElementId::Name(format!("transcript-{pane}").into())).flex().flex_col().gap_1().overflow_y_scroll()
        .h(px((height - COMPOSER - 24.).max(1.)));
    if let Some(scroll) = &scroll {
        col = col.track_scroll(scroll);
    }
    for row in t.map(|t| t.rows.as_slice()).unwrap_or_default() {
        col = col.child(match &row.kind {
            Kind::User { text, queued } => div().flex().gap_2().text_color(rgb(0xe0b070))
                .child(div().child(if *queued { "queued" } else { "you" }).text_color(rgb(0x8fa89b)).text_xs())
                .child(div().child(text.clone())),
            Kind::Reply(x) => div().text_color(rgb(0xd9e8e1)).child(x.clone()),
            Kind::Thought(x) => div().text_color(rgb(0x6f8a7c)).child(x.clone()),
            Kind::Tool { title, kind, status, content, detail, .. } => {
                let info = ToolRow::new(title, kind, status, content, detail.as_deref());
                let mut line = div().flex().gap_2().items_center().text_color(rgb(0x8fa89b))
                    .child(div().w(px(18.)).text_color(rgb(0x6f8a7c)).child(info.glyph))
                    .child(div().child(info.label));
                if let (Some(a), Some(r)) = (info.added, info.removed) {
                    line = line.child(div().text_xs().text_color(rgb(0x7fb89a)).child(format!("+{a} −{r}")));
                }
                line = line.child(div().text_xs().text_color(rgb(match status.as_str() { "failed" => 0xe07070, "completed" => 0x4f6a5c, _ => 0xe0b070 })).child(status.clone()));
                if let Some(reason) = info.reason {
                    line = line.child(div().text_xs().text_color(rgb(0xe07070)).child(reason));
                }
                line
            }
            Kind::Plan(entries) => {
                let mut plan = div().flex().flex_col().text_color(rgb(0x8fa89b));
                for e in entries {
                    plan = plan.child(format!("{} {}", match e.status.as_str() { "completed" => "☑", "in_progress" => "◐", _ => "☐" }, e.content));
                }
                plan
            }
            Kind::Mode(m) => div().text_xs().text_color(rgb(0x8fa89b)).child(format!("Mode: {m}")),
            Kind::Notice(x) => div().text_xs().text_color(rgb(0x8fa89b)).child(x.clone()),
            Kind::Error(x) => div().text_color(rgb(0xe07070)).child(x.clone()),
        });
    }
    if t.is_some_and(Transcript::busy) {
        col = col.child(div().text_color(rgb(0xe0b070)).child("working…"));
    }
    div().flex().flex_col().h(px(height.max(1.)))
        .child(div().h(px(24.)).text_color(rgb(0x8fa89b)).child(format!("{label} · {state}")))
        .child(col)
        .child(div().h(px(COMPOSER)).mt_1().px_2().py_1().rounded_sm().bg(rgb(0x0e1311))
            .border_1().border_color(rgb(if focused { 0x7fb89a } else { 0x2c3d34 }))
            .text_color(rgb(0xd9e8e1)).overflow_hidden()
            .child(format!("{draft}{}", if focused { "▏" } else { "" })))
}
