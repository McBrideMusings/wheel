//! The window's content: a header with the spaces, the active space's split tree drawn as
//! panes and draggable dividers, and the real Ghostty and WKWebView views on the first
//! terminal, web and browser pane. An agent pane draws its transcript, an empty pane offers
//! what it can become; only a kind this build does not know is drawn as unimplemented.
use crate::{daemon::{Endpoint, Event}, layout::*, native::Native, transcript::{Line, Transcript}};
use gpui::{Context, MouseButton, MouseMoveEvent, MouseUpEvent, Window, div, prelude::*, px, rgb};
use serde_json::json;
use std::{collections::{HashMap, HashSet}, rc::Rc, sync::mpsc};

pub const TITLE: f32 = 22.0;
pub const HEADER: f32 = 34.0;

pub struct Client {
    native: Rc<Native>,
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
    attached: HashSet<String>,
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
    pub fn new(native: Rc<Native>, out: mpsc::Sender<String>, endpoint: Endpoint, prefer_space: Option<String>) -> Self {
        Self {
            native, out, endpoint, spaces: vec![], space_id: prefer_space, overrides: HashMap::new(), drag: None,
            native_panes: vec![], opened: HashMap::new(), placed: HashMap::new(), viewport: (1200., 720.), transcripts: HashMap::new(), attached: HashSet::new(), link: Link::Connecting,
        }
    }
    /// Take one thing the connection reported. Returns true when it was a layout.
    pub fn on_event(&mut self, event: Event) -> bool {
        match event {
            Event::Message(message) => match crate::layout::parse_spaces(&message) {
                Some(spaces) => {
                    self.apply(spaces);
                    true
                }
                None => {
                    for (agent, t) in &mut self.transcripts {
                        t.on_message(&message, agent);
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
    pub fn apply(&mut self, spaces: Vec<Space>) {
        if !spaces.iter().any(|s| Some(&s.id) == self.space_id.as_ref()) {
            self.space_id = spaces.first().map(|s| s.id.clone());
        }
        self.spaces = spaces;
        self.overrides.clear();
    }
    fn space(&self) -> Option<&Space> {
        self.spaces.iter().find(|s| Some(&s.id) == self.space_id.as_ref())
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
    fn send(&self, frame: serde_json::Value) {
        if self.link == Link::Up {
            let _ = self.out.send(frame.to_string());
        }
    }
    pub fn request_status(&self, req: u64) {
        self.send(json!({"op":"status","req":req}));
    }
    fn end_drag(&mut self) {
        if let Some((split, _, _)) = self.drag.take() {
            if let Some(ratio) = self.overrides.get(&split) {
                self.send(json!({"op":"split_ratio","split":split,"ratio":ratio}));
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
                    self.send(json!({"op":"agent_attach","agent":agent}));
                }
            }
        }
    }
    /// Turn an empty pane into `kind` (`terminal` or `browser`).
    fn set_kind(&self, pane: &str, kind: &str) {
        self.send(json!({"op":"pane_set_kind","pane":pane,"kind":kind}));
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
                    (None, _) => self.native.open_web(&p.leaf.id, &self.endpoint.artifact(&p.leaf.id)),
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
        self.sync_native(&leaves);
        self.sync_agents(&leaves);
        let mut root = div().relative().size_full().bg(rgb(0x0e1311)).text_color(rgb(0xd9e8e1)).text_sm()
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
        let mut header = div().absolute().left(px(10.)).top(px(6.)).h(px(HEADER - 8.)).flex().gap_3().items_center();
        if self.spaces.is_empty() {
            header = header.child("waiting for wheeld");
        }
        for s in &self.spaces {
            let (id, active) = (s.id.clone(), Some(&s.id) == self.space_id.as_ref());
            header = header.child(
                div().px_2().rounded_sm().cursor_pointer()
                    .bg(rgb(if active { 0x31483e } else { 0x17201c }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.space_id = Some(id.clone());
                        this.overrides.clear();
                        cx.notify();
                    }))
                    .child(s.name.clone()),
            );
        }
        root = root.child(header);
        // Always drawn, so the header's controls stay where they are; empty while connected.
        root = root.child(div().absolute().right(px(10.)).top(px(6.)).h(px(HEADER - 8.)).w(px(420.)).flex().justify_end().items_center()
            .text_color(rgb(0xe0b070)).child(self.link.banner()));
        for p in &leaves {
            let r = p.rect;
            let native = self.native_panes.contains(&p.leaf.id);
            let id = p.leaf.id.clone();
            let body = match p.leaf.kind.as_str() {
                _ if native => div(),
                "agent" => {
                    let t = p.leaf.agent.as_ref().and_then(|a| self.transcripts.get(a));
                    let mut col = div().flex().flex_col().gap_1().overflow_hidden().h(px((r.h - TITLE).max(1.)))
                        .child(div().text_color(rgb(0x8fa89b)).child(format!("{} · {}", p.leaf.label, t.map(|t| t.state.as_str()).unwrap_or("connecting"))));
                    for line in t.map(|t| t.lines.as_slice()).unwrap_or_default() {
                        let (color, text) = match line {
                            Line::User(x) => (0xe0b070, format!("> {x}")),
                            Line::Reply(x) => (0xd9e8e1, x.clone()),
                            Line::Thought(x) => (0x6f8a7c, x.clone()),
                            Line::Tool { title, status, .. } => (0x8fa89b, format!("[{status}] {title}")),
                            Line::Error(x) => (0xe07070, x.clone()),
                        };
                        col = col.child(div().text_color(rgb(color)).child(text));
                    }
                    col
                }
                "empty" => {
                    let button = |label: &'static str, kind: &'static str, cx: &mut Context<Self>| {
                        let id = id.clone();
                        div().px_2().py_1().rounded_sm().cursor_pointer().bg(rgb(0x31483e))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, _| this.set_kind(&id, kind)))
                            .child(label)
                    };
                    div().flex().gap_2().child(button("Terminal", "terminal", cx)).child(button("Browser", "browser", cx))
                }
                other => div().child(format!("unimplemented: {other} pane\n{}", p.leaf.label)),
            };
            root = root.child(
                div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).bg(rgb(0x17201c))
                    .border_1().border_color(rgb(0x2c3d34))
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
