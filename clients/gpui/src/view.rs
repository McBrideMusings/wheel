//! The window's content: a header with the sessions and the shown session's spaces (ADR-0014),
//! the shown space's split tree drawn as
//! panes and draggable dividers, and the real Ghostty and WKWebView views on the first
//! terminal, web and browser pane. An agent pane draws its transcript, an empty pane offers
//! what it can become; only a kind this build does not know is drawn as unimplemented.
mod commands;
mod debug;
mod procs;
mod settings;
mod toasts;
pub use commands::{Confirmation, Pending};
pub use debug::Answer as DebugAnswer;
pub use settings::{SettingsEdit, SettingsReqs};
use crate::{browser::{self, CHROME, Press}, command_palette::CommandPalette, daemon::{Endpoint, Event}, focus::{Focus, Typed, type_into}, footer, header::{self, Hit}, keymap::Keymap, layout::*, markdown::{self, Block, Mark, Run, Text}, native::{Answer, Bridge, BrowserEvent, BrowserState, Frame, Native}, palette::{self, Palette}, status, transcript::{Kind, ToolRow, Transcript}};
use gpui::{Anchor, App, Bounds, ClipboardItem, Context, FocusHandle, FontStyle, FontWeight, HighlightStyle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollHandle, StrikethroughStyle, StyledText, TextLayout, UnderlineStyle, Window, anchored, canvas, deferred, div, prelude::*, px, rgb, rgba};
use wheel_proto::{AgentInfo, ArtifactInfo, ClientMsg, NewPaneKind, PaneStatus, RecentDir, ServerMsg};
use std::{cell::RefCell, collections::{HashMap, HashSet}, ops::Range, rc::Rc, sync::mpsc};

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
    /// Every transcript text drawn in the last frame, in reading order, with its layout: the
    /// selection maps a pointer position to a text and a byte offset through these.
    drawn: Rc<RefCell<Vec<Drawn>>>,
    /// The transcript text selected with the pointer, if any.
    pub selection: Option<Selection>,
    /// Pointer positions the next frame turns into the selection's ends, with the agent pressed in.
    pending_down: Option<(String, Point<Pixels>)>,
    pending_move: Option<Point<Pixels>>,
    pending_up: bool,
    /// Each reply row's blocks, parsed again only when its text changed.
    parsed: Parsed,
    /// Syntax colours by (language, code), kept across frames while a block streams in.
    highlights: Highlights,
    /// The ranks `status.dots` lists: the statuses that draw a dot. Every rank until the daemon says.
    pub status_dots: HashSet<&'static str>,
    /// What the line under an agent's composer shows, in order: `chat.footer.items`, as the
    /// daemon resolved it for each space it was asked about (`None` is the machine-wide list).
    pub footer_items: HashMap<Option<String>, Vec<String>>,
    /// The agents wheeld lists, for the home each harness keeps its login in.
    agents: Vec<AgentInfo>,
    /// The panes whose context meter reads the raw figures rather than the percent.
    raw_context: HashSet<String>,
    /// The config control whose menu is open: (agent, option id).
    menu: Option<(String, String)>,
    /// Where the footer's controls were drawn in the last frame, so a check can press them.
    controls: Rc<RefCell<Vec<Control>>>,
    /// The last presses the window's listeners took, in order, for a failing check to read.
    pub presses: Vec<String>,
    /// The artifact palette open inside a pane, by pane id: at most one per pane, drawn over
    /// that pane's body only.
    palettes: HashMap<String, Palette>,
    /// The next request id for a reply this window tells apart (`artifact_ls`, `pane_browser_open`).
    next_req: u64,
    /// Each browser pane's chrome state, as the bridge last emitted it.
    browsers: HashMap<String, BrowserState>,
    /// The address field being edited: the pane and the draft.
    address_edit: Option<(String, String)>,
    /// `pane_browser_open` requests waiting for wheeld to read typed text: (pane, the text).
    open_reqs: HashMap<u64, (String, String)>,
    /// Which chord runs which action: the built-in keys with the `keybind` setting applied.
    pub keymap: Keymap,
    keybind: String,
    /// The `[runner]` names of the space `runners_space`, as last listed for the palette.
    runners: Vec<String>,
    runners_space: Option<String>,
    /// The command palette over the window (⌘K), if open.
    pub command_palette: Option<CommandPalette>,
    recent_dirs: Vec<RecentDir>,
    palette_artifacts: Vec<ArtifactInfo>,
    palette_reqs: commands::PaletteReqs,
    /// The shortcut sheet (⌘/) is over the window.
    pub shortcuts_open: bool,
    /// A destructive step waiting on a yes.
    pub confirmation: Option<Confirmation>,
    /// The chords the bridge hands the GPUI view the keyboard for, as last sent.
    routed: Vec<String>,
    /// The last actions run or refused, in order, for a failing check to read.
    pub actions_log: Vec<String>,
    /// Close Window ran: the next render removes the window.
    close_window_requested: bool,
    /// The Settings window (⌘,) is over the window, and what it shows.
    pub settings_open: bool,
    pub settings: crate::settings::Model,
    settings_reqs: SettingsReqs,
    /// What the keyboard types into while the Settings window is open.
    pub settings_edit: Option<SettingsEdit>,
    /// What the bundled `wheel install` printed, from the thread that ran it, until the next render reads it.
    command_line_result: std::sync::Arc<std::sync::Mutex<Option<(i32, String)>>>,
    /// The QR modules last drawn for a pairing payload.
    qr_cache: Option<(String, Vec<String>)>,
    /// A file the Settings window opens for editing at the next render, which has the app to open it with.
    pending_open: Option<String>,
    /// When the armed redraw timer fires (unix ms): the Settings window's countdown or running command, the pairing window's times.
    redraw_due: Option<u64>,
    /// The notices posted to this window (`notify` toasts, agents' notices), oldest first.
    pub posted: Vec<crate::toasts::Toast>,
    /// What the notice stack is built from besides the posted ones.
    pub toast_sources: crate::toasts::Sources,
    /// The layout ops this window sent and wheeld has not answered; a refusal of one is the layout-error notice.
    layout_reqs: HashSet<u64>,
    /// The permission requests waiting, from `status` and `perm_requested`.
    pub perms: Vec<wheel_proto::PermInfo>,
    /// The pairing window and the copied code.
    pub pairing: crate::pairing::Pairing,
    /// The spaces and panes the last crash named, held out until opened anyway.
    pub quarantine: crate::crash_guard::Quarantine,
    /// The executable's stamp when this process started, and when it was last compared with the disk.
    launch_stamp: Option<u64>,
    client_checked_at: Option<std::time::Instant>,
    /// What the notices, the pairing window and the client check did, for a check to read.
    pub toast_log: Vec<String>,
    /// The process card and the lists behind it.
    pub procs: crate::procs::Procs,
    /// The protocol the connected wheeld speaks, from its last `status`; None before it answered.
    pub server_protocol: Option<u32>,
    /// How this client stops and starts wheeld.
    pub server_control: crate::server::Control,
    /// A restart is running on its thread; its outcome (None when it worked) waits here for the next render.
    server_restarting: bool,
    server_result: std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>,
    /// Why the last restart did not work.
    pub server_problem: Option<String>,
}

/// One footer control as drawn: the context meter (`context`) or a config option by id.
#[derive(Clone, Debug)]
pub struct Control {
    pub agent: String,
    pub id: String,
    pub bounds: Bounds<Pixels>,
}

/// Reply blocks by (agent, row id), with the text they were parsed from.
type Parsed = RefCell<HashMap<(String, usize), (String, Rc<Vec<Block>>)>>;
/// Syntax colours by (language, code); none for a language `syntect` does not know.
type Highlights = RefCell<HashMap<(String, String), Option<Rc<Vec<Run>>>>>;

/// A place in an agent's transcript: a row (by position), a text within it (by order drawn), and a
/// byte offset into that text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub row: usize,
    pub block: usize,
    pub offset: usize,
}

/// A pointer selection over one agent's transcript; empty while `anchor == head`.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub agent: String,
    pub anchor: Pos,
    pub head: Pos,
    pub dragging: bool,
}
impl Selection {
    pub fn range(&self) -> (Pos, Pos) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
    /// The byte range of the text at (`row`, `block`) with `len` bytes that lies inside the selection.
    pub fn slice(&self, row: usize, block: usize, len: usize) -> Option<Range<usize>> {
        let (a, b) = self.range();
        let key = (row, block);
        if self.is_empty() || key < (a.row, a.block) || key > (b.row, b.block) {
            return None;
        }
        let start = if key == (a.row, a.block) { a.offset.min(len) } else { 0 };
        let end = if key == (b.row, b.block) { b.offset.min(len) } else { len };
        (start < end).then_some(start..end)
    }
}

/// One transcript text as drawn: what it is, and the layout that maps pixels to bytes.
#[derive(Clone)]
pub struct Drawn {
    pub agent: String,
    pub row: usize,
    pub block: usize,
    /// `reply`, `heading`, `list`, `code`, `quote`, `table`, `user`, `thought`, `notice` or `error`.
    pub kind: &'static str,
    pub language: Option<String>,
    pub highlighted: bool,
    pub text: String,
    pub layout: TextLayout,
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
            Link::Incompatible { .. } => crate::server::UPDATED.into(),
        }
    }
}

impl Client {
    #[allow(clippy::too_many_arguments)]
    pub fn new(native: Rc<Native>, bridge: Rc<Bridge>, hidden: bool, window_id: String, out: mpsc::Sender<String>, endpoint: Endpoint, prefer_session: Option<String>, prefer_space: Option<String>, handle: FocusHandle) -> Self {
        let server_control = crate::server::Control::from_env();
        let toast_sources = crate::toasts::Sources { server_restart_available: server_control.available(), ..Default::default() };
        Self {
            native, bridge, hidden, window_id, sessions: vec![], session_id: prefer_session, reported: None, out, endpoint, spaces: vec![], space_id: prefer_space, overrides: HashMap::new(), drag: None,
            native_panes: vec![], opened: HashMap::new(), placed: HashMap::new(), viewport: (1200., 720.), transcripts: HashMap::new(), scrolls: HashMap::new(), attached: HashSet::new(), focus: Focus::default(), drafts: HashMap::new(), handle, link: Link::Connecting,
            drawn: Rc::default(), selection: None, pending_down: None, pending_move: None, pending_up: false, highlights: RefCell::default(), parsed: RefCell::default(),
            status_dots: status::all_ranks(), footer_items: HashMap::new(), agents: vec![], raw_context: HashSet::new(), menu: None, controls: Rc::default(), presses: vec![], palettes: HashMap::new(), next_req: 100_000,
            browsers: HashMap::new(), address_edit: None, open_reqs: HashMap::new(),
            keymap: Keymap::default(), keybind: String::new(), runners: vec![], runners_space: None, command_palette: None, recent_dirs: vec![], palette_artifacts: vec![], palette_reqs: commands::PaletteReqs::default(),
            shortcuts_open: false, confirmation: None, routed: vec![], actions_log: vec![], close_window_requested: false,
            settings_open: false, settings: crate::settings::Model::default(), settings_reqs: SettingsReqs::default(), settings_edit: None, command_line_result: std::sync::Arc::default(), qr_cache: None, pending_open: None, redraw_due: None,
            posted: vec![], toast_sources, layout_reqs: HashSet::new(), perms: vec![], pairing: crate::pairing::Pairing::default(),
            quarantine: crate::crash_guard::launch_quarantine(), launch_stamp: Self::executable_stamp(), client_checked_at: None, toast_log: vec![], procs: crate::procs::Procs::default(),
            server_protocol: None, server_control, server_restarting: false, server_result: std::sync::Arc::default(), server_problem: None,
        }
    }
    /// Take one thing the connection reported. Returns true when it was a layout.
    pub fn on_event(&mut self, event: Event) -> bool {
        match event {
            Event::Message(message) => match crate::layout::parse_layout(&message) {
                Some(layout) => {
                    if let ServerMsg::Status { agents, perms, protocol, .. } = message.as_ref() {
                        if self.server_protocol != Some(*protocol) {
                            self.log(format!("server protocol {protocol}, this app's {}", crate::daemon::PROTOCOL));
                        }
                        self.server_protocol = Some(*protocol);
                        self.agents = agents.clone();
                        self.perms = perms.clone();
                        self.rebuild_keymap();
                        self.on_settings_status(message.as_ref());
                    }
                    // A `status` reply carries every status; a `spaces` broadcast carries none.
                    self.apply(layout, matches!(message.as_ref(), ServerMsg::Spaces { .. }));
                    true
                }
                None => {
                    if self.on_settings_reply(message.as_ref()) || self.on_palette_reply(message.as_ref()) || self.on_toast_message(message.as_ref()) || self.on_procs_message(message.as_ref()) {
                        return false;
                    }
                    match message.as_ref() {
                        ServerMsg::StatusChanged { space, pane, status, space_status } => {
                            apply_status(&mut self.spaces, space, pane.as_deref(), status.clone(), space_status.clone());
                            return false;
                        }
                        // A file in a web pane's folder changed: its page opens again.
                        ServerMsg::PaneReload { pane } => {
                            if self.opened.contains_key(pane) {
                                self.native.reload(pane);
                            }
                            return false;
                        }
                        ServerMsg::Artifacts { req, artifacts, .. } => {
                            if let Some(palette) = self.palettes.values_mut().find(|p| p.req == Some(*req)) {
                                palette.req = None;
                                palette.listed(artifacts.clone());
                            }
                            return false;
                        }
                        ServerMsg::Error { req: Some(req), message, .. } => {
                            if let Some(palette) = self.palettes.values_mut().find(|p| p.req == Some(*req)) {
                                palette.req = None;
                                palette.failed(message);
                            }
                            // wheeld read typed text as no page: the field shows why.
                            if let Some((pane, text)) = self.open_reqs.remove(req) {
                                self.native.browser(&pane, "refused", serde_json::json!({"text": text, "message": message}));
                            }
                            return false;
                        }
                        // wheeld read typed text as a page: the pane goes there, even when it is already on it.
                        ServerMsg::Opened { req, pane, url } => {
                            if self.open_reqs.remove(req).is_some() {
                                self.native.browser(pane, "opened", serde_json::json!({"url": url}));
                            }
                            return false;
                        }
                        // A browser pane is on a new page: the one field, patched into the tree held
                        // (ADR-0005), and the view sent there unless it reported the page itself.
                        ServerMsg::PaneUrl { pane, url } => {
                            for space in &mut self.spaces {
                                set_url(&mut space.root, pane, url);
                            }
                            if self.opened.contains_key(pane) {
                                self.native.browser(pane, "daemon_says", serde_json::json!({"url": url}));
                            }
                            return false;
                        }
                        // `wheel pane shot`: the bridge draws the pane and answers through a frame.
                        ServerMsg::RenderPane { id, pane, full } => {
                            if self.opened.contains_key(pane) {
                                self.native.render(*id, pane, *full);
                            } else {
                                self.send(&ClientMsg::RenderRefused { id: *id, message: "not shown".into() });
                            }
                            return false;
                        }
                        // `wheel pane eval|dom|shot --selector|record` on a browser pane's page.
                        ServerMsg::InspectPane { id, pane, action, arg } => {
                            if self.opened.get(pane).map(String::as_str) == Some("browser") {
                                self.native.inspect(*id, pane, action, arg.as_deref());
                            } else {
                                self.send(&ClientMsg::RenderRefused { id: *id, message: "not shown".into() });
                            }
                            return false;
                        }
                        ServerMsg::Settings { settings, space, .. } => {
                            let mut home_changed = false;
                            for s in settings {
                                match s.key.as_str() {
                                    // Machine-wide, whichever space the list was resolved for.
                                    "status.dots" => self.status_dots = status::listed(&s.value),
                                    "chat.footer.items" => {
                                        self.footer_items.insert(space.clone(), footer::items(&s.value));
                                    }
                                    // Machine-wide: the keys follow Ghostty's one setting.
                                    "keybind" if space.is_none() => self.set_keybind(&s.value),
                                    key => {
                                        // `agent.<name>.home` changes the home the footer shows; the
                                        // agent list that carries it comes with the next `status`.
                                        if let Some(name) = key.strip_prefix("agent.").and_then(|k| k.strip_suffix(".home")) {
                                            let held = self.agents.iter().find(|a| a.name == name).and_then(|a| a.home.as_deref()).unwrap_or("");
                                            if held != s.value {
                                                home_changed = true;
                                            }
                                        }
                                    }
                                }
                            }
                            if home_changed {
                                self.send(&ClientMsg::Status { req: None });
                            }
                            return false;
                        }
                        _ => {}
                    }
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
                    // The old daemon's notices and requests: the new one sends its own on attach.
                    self.posted.clear();
                    self.perms.clear();
                    self.layout_reqs.clear();
                    // Answers to the last connection's listings never come: a palette open across
                    // a reconnect lists again, against the layout the new daemon sends.
                    for palette in self.palettes.values_mut() {
                        palette.req = None;
                        palette.relisting();
                    }
                }
                // Answers to the last connection's address reads never come.
                for (pane, text) in std::mem::take(&mut self.open_reqs).into_values() {
                    self.native.browser(&pane, "refused", serde_json::json!({"text": text, "message": "The connection to wheeld dropped before it read this address."}));
                }
                self.endpoint = endpoint;
                self.link = Link::Up;
                // A new connection is a new server as far as the notice knows: closing it for one
                // older wheeld does not silence it for the next.
                self.toast_sources.server_updated = None;
                self.toast_sources.server_update_dismissed = None;
                // This window draws its panes as PNGs for `wheel pane shot` and runs `wheel pane
                // eval|dom|record` in its browser panes: wheeld may send it `render_pane` and `inspect_pane`.
                self.send(&ClientMsg::RenderOffer {});
                // The dots and the footer follow two settings; a later change arrives as a broadcast.
                self.send(&ClientMsg::SettingsLs { req: None, space: None });
                self.ask_space_settings();
                self.settings_reconnected();
                false
            }
            Event::Down(reason) => {
                self.link = Link::Down(reason);
                false
            }
            Event::Incompatible { server, client } => {
                // The link is not up: wheeld's frames may lack fields this build reads. What needs
                // the newer server is the whole layout, so the window shows the notice and Restart
                // Server (Swift: serverRestartWaiting) until wheeld answers at this protocol.
                if self.link != (Link::Incompatible { server, client }) {
                    self.log(format!("server protocol {server}, this app's {client}: restart offered"));
                }
                self.link = Link::Incompatible { server, client };
                self.server_protocol = Some(server);
                self.toast_sources.server_updated = Some(server);
                // A window that was up on a newer wheeld shows nothing of it: the older one's
                // layout is not read, so the old one's panes would be stale. The next `Up` applies
                // a layout again.
                self.spaces.clear();
                self.sessions.clear();
                false
            }
        }
    }
    /// The daemon's layout replaces ours, dragged ratios included.
    pub fn apply(&mut self, mut layout: Layout, keep_statuses: bool) {
        if keep_statuses {
            layout.keep_statuses(&self.spaces);
        }
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
        self.ask_space_settings();
        // A palette is for the pane as it was when it opened: a pane that left, or that
        // something else changed, closes it. One still waiting for a listing asks again.
        let kinds: HashMap<String, String> = self.spaces.iter().flat_map(|s| leaves_of(&s.root)).map(|l| (l.id.clone(), l.kind.clone())).collect();
        self.palettes.retain(|pane, palette| kinds.get(pane) == Some(&palette.opened_over));
        for leaf in self.spaces.iter().flat_map(|s| leaves_of(&s.root)).filter(|l| l.kind == "browser") {
            if self.opened.get(&leaf.id).map(String::as_str) == Some("browser")
                && let Some(url) = &leaf.url
            {
                self.native.browser(&leaf.id, "daemon_says", serde_json::json!({"url": url}));
            }
        }
        for pane in self.palettes.iter().filter(|(_, p)| p.req.is_none() && p.is_loading()).map(|(id, _)| id.clone()).collect::<Vec<_>>() {
            self.ask_artifacts(&pane);
        }
        // A pane that closed leaves the keyboard with no pane GPUI knows of; the next key reads
        // where the window's first responder is.
        if self.focus.current.as_ref().is_some_and(|p| !kinds.contains_key(p)) {
            self.focus.current = None;
        }
        self.refresh_command_palette();
        self.sync_view();
    }
    /// Ask for the shown space's settings once, for its `chat.footer.items`.
    fn ask_space_settings(&mut self) {
        if let Some(space) = self.space_id.clone()
            && !self.footer_items.contains_key(&Some(space.clone()))
            && self.link == Link::Up
        {
            self.footer_items.insert(Some(space.clone()), self.footer_items.get(&None).cloned().unwrap_or_else(|| footer::DEFAULT_ITEMS.iter().map(|s| s.to_string()).collect()));
            self.send(&ClientMsg::SettingsLs { req: None, space: Some(space) });
        }
    }
    /// The footer items for `space`: the list the daemon resolved for it, else the machine's,
    /// else the default.
    fn footer_items_for(&self, space: Option<&str>) -> Vec<String> {
        self.footer_items.get(&space.map(str::to_owned)).or_else(|| self.footer_items.get(&None)).cloned()
            .unwrap_or_else(|| footer::DEFAULT_ITEMS.iter().map(|s| s.to_string()).collect())
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
        self.menu = None;
        self.ask_space_settings();
        self.sync_view();
    }
    /// Show `space` of the session shown.
    pub fn select_space(&mut self, space: &str) {
        if self.space_id.as_deref() != Some(space) && self.session_spaces().iter().any(|s| s.id == space) {
            self.space_id = Some(space.to_owned());
            self.overrides.clear();
            self.menu = None;
            self.ask_space_settings();
            if self.command_palette.is_some() {
                self.ask_runners();
            }
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
    /// The place in `agent`'s transcript under `point`, from the last frame's texts: the text whose
    /// box holds the point, else the end of the last text above it, else the start of the first.
    fn resolve(&self, agent: &str, point: Point<Pixels>) -> Option<Pos> {
        let drawn = self.drawn.borrow();
        let texts: Vec<&Drawn> = drawn.iter().filter(|d| d.agent == agent).collect();
        let mut above: Option<&Drawn> = None;
        for d in &texts {
            let b = d.layout.bounds();
            if b.contains(&point) {
                let offset = d.layout.index_for_position(point).unwrap_or_else(|i| i);
                return Some(Pos { row: d.row, block: d.block, offset: offset.min(d.text.len()) });
            }
            if b.bottom() <= point.y || (b.top() <= point.y && point.y <= b.bottom() && b.right() <= point.x) {
                above = Some(d);
            } else if b.top() <= point.y && point.y <= b.bottom() && point.x < b.left() {
                return Some(Pos { row: d.row, block: d.block, offset: 0 });
            }
        }
        match (above, texts.first()) {
            (Some(d), _) => Some(Pos { row: d.row, block: d.block, offset: d.text.len() }),
            (None, Some(d)) => Some(Pos { row: d.row, block: d.block, offset: 0 }),
            (None, None) => None,
        }
    }
    /// The selected text, texts joined by newlines; none while nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection.as_ref().filter(|s| !s.is_empty())?;
        let drawn = self.drawn.borrow();
        let mut parts = vec![];
        for d in drawn.iter().filter(|d| d.agent == sel.agent) {
            if let Some(range) = sel.slice(d.row, d.block, d.text.len()) {
                let (mut a, mut b) = (range.start, range.end);
                while !d.text.is_char_boundary(a) {
                    a -= 1;
                }
                while !d.text.is_char_boundary(b) {
                    b += 1;
                }
                parts.push(d.text[a..b].to_owned());
            }
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }
    /// What the last frame drew for `agent`'s transcript, text by text.
    pub fn drawn_texts(&self, agent: &str) -> Vec<Drawn> {
        self.drawn.borrow().iter().filter(|d| d.agent == agent).cloned().collect()
    }
    /// The window point at the middle of the line holding byte `index` of the text at (`row`, `block`).
    pub fn text_point(&self, agent: &str, row: usize, block: usize, index: usize) -> Option<(f32, f32)> {
        let drawn = self.drawn.borrow();
        let d = drawn.iter().find(|d| d.agent == agent && d.row == row && d.block == block)?;
        let p = d.layout.position_for_index(index)?;
        Some((f32::from(p.x), f32::from(p.y + d.layout.line_height() / 2.)))
    }
    /// The selection machinery's state in words, for a failing check.
    pub fn selection_debug(&self, agent: &str) -> String {
        let drawn = self.drawn.borrow();
        let boxes: Vec<String> = drawn.iter().filter(|d| d.agent == agent).map(|d| {
            let b = d.layout.bounds();
            format!("{}:{}:{} {:?} at ({:.0},{:.0}) {:.0}x{:.0}", d.row, d.block, d.kind, d.text.chars().take(16).collect::<String>(), f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))
        }).collect();
        format!("pending_down={:?} pending_move={:?} selection={:?} drawn={} [{}]", self.pending_down, self.pending_move, self.selection, drawn.len(), boxes.join("; "))
    }
    /// The status the daemon holds for `pane`, whichever space holds it.
    pub fn pane_status(&self, pane: &str) -> Option<PaneStatus> {
        self.spaces.iter().flat_map(|s| leaves_of(&s.root)).find(|l| l.id == pane).and_then(|l| l.status.clone())
    }
    /// The marks the shown space's chip draws, by rank, and how many more it counts.
    pub fn space_marks(&self) -> (Vec<&'static str>, usize) {
        let Some(space) = self.space() else { return (vec![], 0) };
        let (marks, more) = status::agent_statuses(space, &self.status_dots, status::AGENT_DOT_LIMIT);
        (marks.iter().map(status::rank).collect(), more)
    }
    /// The footer's items for `agent` as drawn, (item, text), in order; an item with nothing to
    /// show is left out, as the Swift client leaves it out.
    pub fn footer_texts(&self, agent: &str) -> Vec<(String, String)> {
        let Some(t) = self.transcripts.get(agent) else { return vec![] };
        let (space, leaf) = self.spaces.iter().find_map(|s| leaves_of(&s.root).into_iter().find(|l| l.agent.as_deref() == Some(agent)).map(|l| (s.id.clone(), l.clone()))).unzip();
        let pane = leaf.as_ref().map(|l| l.id.clone()).unwrap_or_default();
        footer_items(&self.footer_items_for(space.as_deref()), t, leaf.as_ref(), &self.agents, self.raw_context.contains(&pane))
    }
    /// The config controls for `agent` as drawn, (option id, text), in order.
    pub fn config_controls(&self, agent: &str) -> Vec<(String, String)> {
        self.transcripts.get(agent).map(|t| t.config.iter().map(|o| (o.id.clone(), footer::control_text(o))).collect()).unwrap_or_default()
    }
    /// Where control `id` of `agent` was drawn in the last frame, as (x, y, w, h).
    pub fn control_bounds(&self, agent: &str, id: &str) -> Option<(f32, f32, f32, f32)> {
        self.controls.borrow().iter().find(|c| c.agent == agent && c.id == id)
            .map(|c| (f32::from(c.bounds.origin.x), f32::from(c.bounds.origin.y), f32::from(c.bounds.size.width), f32::from(c.bounds.size.height)))
    }
    /// Record a press, keeping the last 32.
    fn press(&mut self, what: String) {
        if self.presses.len() >= 32 {
            self.presses.remove(0);
        }
        self.presses.push(what);
    }
    /// The config control whose menu is open, (agent, option id).
    pub fn open_menu(&self) -> Option<(String, String)> {
        self.menu.clone()
    }
    /// Ask the agent to set option `config` to `value`; the new list comes back as `agent_config`.
    pub fn set_config(&mut self, agent: &str, config: &str, value: &str) {
        self.menu = None;
        self.send(&ClientMsg::AgentConfigSet { req: None, agent: agent.to_owned(), config: config.to_owned(), value: value.to_owned() });
    }
    /// A browser pane's chrome state, as the bridge last emitted it.
    pub fn browser_state(&self, pane: &str) -> Option<&BrowserState> {
        self.browsers.get(pane)
    }
    /// The address field being edited: (pane, draft).
    pub fn address_edit(&self) -> Option<(String, String)> {
        self.address_edit.clone()
    }
    /// Whether wheeld runs on another machine, so a file page names a path this Mac does not have.
    fn daemon_is_remote(&self) -> bool {
        let host = self.endpoint.ws.split("://").nth(1).unwrap_or_default().split(['/', '?']).next().unwrap_or_default();
        let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host).trim_matches(['[', ']']);
        !matches!(host, "127.0.0.1" | "localhost" | "::1" | "")
    }
    /// A frame the bridge sent on its own: a browser pane's state is kept for its chrome, its
    /// reports go to wheeld as this connection's requests, and an answer to a relayed daemon
    /// request goes back as the reply wheeld waits for.
    pub fn on_native(&mut self, frame: &str) {
        let frame: Frame = match serde_json::from_str(frame) {
            Ok(frame) => frame,
            Err(error) => {
                eprintln!("wheel-gpui: a frame from the bridge was not understood ({error}): {frame}");
                self.press(format!("bad native frame: {error}"));
                return;
            }
        };
        match frame {
            Frame::Event(BrowserEvent::Browser(state)) => {
                self.browsers.insert(state.pane.clone(), state);
            }
            Frame::Event(BrowserEvent::Navigated { pane, url }) => self.send(&ClientMsg::PaneBrowserUrl { req: None, pane, url }),
            Frame::Event(BrowserEvent::Typed { pane, text }) => {
                if self.link != Link::Up {
                    self.native.browser(&pane, "refused", serde_json::json!({"text": text, "message": "Not connected to wheeld, which says what an address means."}));
                    return;
                }
                let req = self.next_req;
                self.next_req += 1;
                self.open_reqs.insert(req, (pane.clone(), text.clone()));
                self.send(&ClientMsg::PaneBrowserOpen { req: Some(req), pane, text });
            }
            Frame::Event(BrowserEvent::Console { pane, reset, entries }) => self.send(&ClientMsg::PaneConsole { req: None, pane, reset, entries }),
            // Open Folder's picker answered: a space for that folder, as the Swift app makes one.
            Frame::Event(BrowserEvent::FolderChosen { path }) => self.create_space(&path),
            Frame::Answer(Answer::Rendered { id, png }) => self.send(&ClientMsg::Rendered { id, png }),
            Frame::Answer(Answer::Inspected { id, text }) => self.send(&ClientMsg::Inspected { id, text }),
            Frame::Answer(Answer::Recorded { id, mov, frames, millis }) => self.send(&ClientMsg::Recorded { id, mov, frames, millis }),
            Frame::Answer(Answer::Refused { id, message }) => self.send(&ClientMsg::RenderRefused { id, message }),
        }
    }
    /// A press on a browser pane's chrome.
    pub fn browser_press(&mut self, pane: &str, press: Press) {
        self.press(format!("browser {pane} {press:?}"));
        self.focus_pane(pane);
        match press {
            Press::Back => self.native.browser(pane, "back", serde_json::json!({})),
            Press::Forward => self.native.browser(pane, "forward", serde_json::json!({})),
            Press::Reload | Press::Retry => self.native.browser(pane, "reload", serde_json::json!({})),
            Press::Stop => self.native.browser(pane, "stop", serde_json::json!({})),
            // Home: the kind again with no page, which opens the `browser.homepage` setting as
            // wheeld resolves it for the pane's space.
            Press::Home => {
                self.native.browser(pane, "clear_failure", serde_json::json!({}));
                self.send(&ClientMsg::PaneSetKind { req: None, pane: pane.to_owned(), kind: NewPaneKind::Browser, artifact: None, scope: None, url: None });
            }
            // The field takes the keyboard from the web view: the GPUI view holds it while editing.
            Press::Edit => {
                let address = self.browsers.get(pane).map(|s| browser::editable(&s.address)).unwrap_or_default();
                self.address_edit = Some((pane.to_owned(), address));
                self.native.focus("");
            }
        }
    }
    /// A key typed while `pane`'s address field is being edited. Returns false when it is not.
    fn address_key(&mut self, pane: &str, key: &str, text: Option<&str>) -> bool {
        let Some((editing, draft)) = self.address_edit.as_mut().filter(|(p, _)| p == pane) else { return false };
        let editing = editing.clone();
        match key {
            "escape" => {
                self.address_edit = None;
                self.native.focus(&editing);
            }
            "enter" => {
                let typed = std::mem::take(draft);
                self.address_edit = None;
                self.native.focus(&editing);
                self.native.browser(&editing, "open", serde_json::json!({"text": typed}));
            }
            "backspace" => {
                draft.pop();
            }
            _ => {
                if let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
                    draft.push_str(t);
                }
            }
        }
        true
    }
    /// Scroll `agent`'s column so row `row` is in view.
    pub fn reveal(&self, agent: &str, row: usize) {
        if let Some(scroll) = self.scrolls.get(agent) {
            scroll.scroll_to_item(row);
        }
    }
    /// Copy the selection to the clipboard. Returns whether there was one.
    pub fn copy_selection(&self, cx: &mut App) -> bool {
        match self.selected_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }
    /// Turn the pointer positions of the frame's events into the selection's ends, through the
    /// texts the last frame drew.
    fn settle_selection(&mut self) {
        if let Some((agent, point)) = self.pending_down.take() {
            self.selection = self.resolve(&agent, point).map(|pos| Selection { agent, anchor: pos, head: pos, dragging: true });
        }
        if let Some(point) = self.pending_move.take()
            && let Some(sel) = &self.selection
            && sel.dragging
            && let Some(pos) = self.resolve(&sel.agent, point)
        {
            self.selection.as_mut().unwrap().head = pos;
        }
        // A release ends the drag after the frame has taken its last position, so a press and a
        // release inside one frame still make a settled (empty) selection.
        if std::mem::take(&mut self.pending_up)
            && let Some(sel) = &mut self.selection
        {
            sel.dragging = false;
        }
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
        // An address edit ends when the keyboard goes to another pane.
        if self.address_edit.as_ref().is_some_and(|(p, _)| p != pane) {
            self.address_edit = None;
        }
        self.focus.current = Some(pane.to_owned());
        self.native.focus(if self.native_panes.iter().any(|p| p == pane) { pane } else { "" });
        self.sync_view();
    }
    pub fn key(&mut self, e: &KeyDownEvent, cx: &mut App) -> bool {
        let k = &e.keystroke;
        // Esc closes the shortcut sheet wherever the keyboard is; the overlays take their keys.
        if self.command_palette.is_some() {
            if k.modifiers.platform && let Some(chord) = crate::keymap::KeyChord::from_keystroke(k) && self.keymap.action(&chord) == Some("command-palette") {
                self.close_command_palette();
                return true;
            }
            return self.command_palette_key(&k.key, k.key_char.as_deref(), k.modifiers.platform || k.modifiers.control, cx);
        }
        // The Settings window takes every key while it is open, except its own chord, which closes it.
        if self.settings_open {
            if (k.modifiers.platform || k.modifiers.control) && let Some(chord) = crate::keymap::KeyChord::from_keystroke(k) && self.keymap.action(&chord) == Some("settings") {
                self.close_settings();
                return true;
            }
            if self.confirmation.is_none() {
                return self.settings_key(&k.key, k.key_char.as_deref(), k.modifiers.platform || k.modifiers.control, cx);
            }
        }
        // The sheet's own chord closes it, as the Swift app's toggle does.
        if self.shortcuts_open && let Some(chord) = crate::keymap::KeyChord::from_keystroke(k) && self.keymap.action(&chord) == Some("shortcuts") {
            self.close_shortcuts();
            return true;
        }
        if self.pairing.open && k.key == "escape" {
            self.close_pairing_window();
            return true;
        }
        // The process card takes every key while it is open and nothing is over it: its filter, and Esc to close.
        if self.procs.open.is_some() && self.confirmation.is_none() {
            self.procs_key(&k.key, k.key_char.as_deref(), k.modifiers.platform || k.modifiers.control);
            return true;
        }
        if self.overlay_key(&k.key) {
            return true;
        }
        if k.modifiers.platform && k.key == "c" && self.copy_selection(cx) {
            return true;
        }
        if k.key == "escape" && self.menu.take().is_some() {
            return true;
        }
        // A native view may have taken the keyboard by a click GPUI never saw: an action on
        // "the" pane means that one.
        if k.modifiers.platform || k.modifiers.control {
            let status = self.native.status();
            let held = self.native_panes.iter().find(|p| status["panes"][p.as_str()]["focused"] == true).cloned()
                .or_else(|| status["held_before_key"].as_str().filter(|h| !h.is_empty()).map(str::to_owned));
            if held.is_some() {
                self.focus.current = held;
            }
        }
        if self.run_key(k, cx) {
            return true;
        }
        let focused = self.scene().0.into_iter().find(|p| Some(&p.leaf.id) == self.focus.current.as_ref()).map(|p| p.leaf);
        if let Some(leaf) = &focused
            && !k.modifiers.platform
            && !k.modifiers.control
        {
            if self.address_key(&leaf.id, &k.key, k.key_char.as_deref()) || self.palette_key(&leaf.id, &k.key, k.key_char.as_deref()) {
                return true;
            }
            // The empty pane's chooser: `a` opens the artifact palette.
            if leaf.kind == "empty" && k.key == "a" {
                self.open_palette(&leaf.id);
                return true;
            }
        }
        let agent = focused.filter(|l| l.kind == "agent").and_then(|l| l.agent);
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
    fn set_kind(&mut self, pane: &str, kind: NewPaneKind) {
        let pane = pane.to_owned();
        self.send_layout(|req| ClientMsg::PaneSetKind { req: Some(req), pane, kind, artifact: None, scope: None, url: None });
    }
    /// The artifact palette open inside `pane`, if any.
    pub fn palette(&self, pane: &str) -> Option<&Palette> {
        self.palettes.get(pane)
    }
    /// "Artifact…": open the palette inside `pane` and list the artifacts its space offers.
    pub fn open_palette(&mut self, pane: &str) {
        if self.palettes.contains_key(pane) {
            return;
        }
        let Some(kind) = self.spaces.iter().flat_map(|s| leaves_of(&s.root)).find(|l| l.id == pane).map(|l| l.kind.clone()) else { return };
        self.palettes.insert(pane.to_owned(), Palette::new(&kind));
        self.focus_pane(pane);
        self.ask_artifacts(pane);
    }
    /// List again for `pane`'s palette: the `pane = true` artifacts of the space the pane is in.
    fn ask_artifacts(&mut self, pane: &str) {
        let space = self.spaces.iter().find(|s| leaves_of(&s.root).iter().any(|l| l.id == pane)).map(|s| s.id.clone());
        let req = self.next_req;
        self.next_req += 1;
        let up = self.link == Link::Up;
        let Some(palette) = self.palettes.get_mut(pane) else { return };
        palette.relisting();
        let (Some(space), true) = (space, up) else {
            palette.failed("wheeld is not connected.");
            return;
        };
        palette.req = Some(req);
        self.send(&ClientMsg::ArtifactLs { req: Some(req), space: Some(space), pane: true });
    }
    pub fn close_palette(&mut self, pane: &str) {
        self.palettes.remove(pane);
    }
    /// A key typed while `pane`'s palette is open. Returns false when the pane has none.
    fn palette_key(&mut self, pane: &str, key: &str, text: Option<&str>) -> bool {
        let Some(palette) = self.palettes.get_mut(pane) else { return false };
        match key {
            "escape" => {
                if palette.escape() {
                    self.close_palette(pane);
                }
            }
            "up" => palette.move_by(-1),
            "down" => palette.move_by(1),
            // Return: open the selected row, or list again after a failure.
            "enter" => {
                if palette.failure().is_some() {
                    self.ask_artifacts(pane);
                } else if let Some(row) = palette.selected_row() {
                    let info = row.info.clone();
                    self.show_artifact(pane, &info);
                }
            }
            "backspace" => {
                let mut query = palette.query.clone();
                query.pop();
                palette.type_text(&query);
            }
            _ => {
                if palette.searchable()
                    && let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
                {
                    let query = format!("{}{t}", palette.query);
                    palette.type_text(&query);
                }
            }
        }
        true
    }
    /// A row, chosen by Return or a click: the artifact's scope and name go to wheeld, never a path.
    fn show_artifact(&mut self, pane: &str, info: &ArtifactInfo) {
        if self.link != Link::Up {
            if let Some(palette) = self.palettes.get_mut(pane) {
                palette.failed("wheeld is not connected.");
            }
            return;
        }
        self.close_palette(pane);
        self.send(&ClientMsg::PaneSetKind { req: None, pane: pane.to_owned(), kind: NewPaneKind::Web, artifact: Some(info.name.clone()), scope: Some(info.scope), url: None });
    }
    /// Give every web pane and every terminal pane (with a daemon terminal) its own native view,
    /// put each on its pane's body, hide the views of panes in other spaces, and close the views
    /// of panes no space has any more.
    fn sync_native(&mut self, leaves: &[Placed]) {
        self.native_panes.clear();
        let cwd = self.space().map(|s| s.cwd.clone()).unwrap_or_default();
        for p in leaves {
            if self.quarantine.isolates_pane(&p.leaf.id) {
                continue;
            }
            crate::crash_guard::mark_pane(&p.leaf.id);
            let scope = p.leaf.scope.as_deref().unwrap_or("project");
            let kind = match (p.leaf.kind.as_str(), &p.leaf.term) {
                // The view is bound to its artifact's scope, name and space directory (its store and scheme handler).
                ("web", _) => format!("web:{scope}:{}:{cwd}", p.leaf.label),
                ("browser", _) => "browser".to_owned(),
                ("terminal", Some(_)) => "terminal".to_owned(),
                _ => continue,
            };
            if self.opened.get(&p.leaf.id) != Some(&kind) {
                match (&p.leaf.term, p.leaf.kind.as_str()) {
                    (Some(term), _) => self.native.open_terminal(&p.leaf.id, term, &self.endpoint.ws),
                    (None, "browser") => self.native.open_browser(&p.leaf.id, p.leaf.url.as_deref().unwrap_or("about:blank"), self.daemon_is_remote()),
                    (None, _) => self.native.open_artifact(&p.leaf.id, &p.leaf.label, scope, self.space_id.as_deref(), &cwd, &self.endpoint.ws),
                }
                self.opened.insert(p.leaf.id.clone(), kind);
                // A view opened in place of another starts unplaced, so it is placed below.
                self.placed.remove(&p.leaf.id);
            }
            // A browser pane's view sits under the chrome GPUI draws, and gives way to the failure
            // card while a load has failed.
            let (top, failed) = match p.leaf.kind.as_str() {
                "browser" => (TITLE + CHROME, self.browsers.get(&p.leaf.id).is_some_and(|s| s.failure.is_some())),
                _ => (TITLE, false),
            };
            let rect = [f64::from(p.rect.x), f64::from(p.rect.y + top), f64::from(p.rect.w), f64::from((p.rect.h - top).max(1.))];
            if self.placed.get(&p.leaf.id) != Some(&(rect, failed)) {
                self.native.place(&p.leaf.id, rect, failed);
                self.placed.insert(p.leaf.id.clone(), (rect, failed));
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
                self.browsers.remove(&id);
            }
        }
        if self.address_edit.as_ref().is_some_and(|(pane, _)| !alive.contains(pane.as_str())) {
            self.address_edit = None;
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
        if std::mem::take(&mut self.close_window_requested) {
            window.remove_window();
        }
        self.sync_routed();
        // An address edit ends when the keyboard left the GPUI view: a click on the page or on a
        // terminal goes to AppKit's first responder, which GPUI never sees.
        if self.address_edit.is_some() && self.native.status()["root_focused"] != true {
            self.address_edit = None;
        }
        crate::crash_guard::mark_space(self.space_id.as_deref().unwrap_or(""));
        let space_held = self.space_id.as_deref().is_some_and(|s| self.quarantine.isolates_space(s));
        // A quarantined space opens none of its views and draws its card in place of its panes.
        let (leaves, dividers) = if space_held { (vec![], vec![]) } else { (leaves, dividers) };
        self.sync_native(&leaves);
        self.sync_agents(&leaves);
        self.settle_selection();
        self.drawn.borrow_mut().clear();
        self.check_client_update();
        self.tick_pairing(cx);
        self.tick_procs(cx);
        let mut root = div().relative().track_focus(&self.handle)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if this.key(e, cx) {
                    cx.notify();
                }
            })).size_full().bg(rgb(0x0e1311)).text_color(rgb(0xd9e8e1)).text_sm()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    this.move_drag(f32::from(e.position.x), f32::from(e.position.y));
                    cx.notify();
                }
                if this.pending_down.is_some() || this.selection.as_ref().is_some_and(|s| s.dragging) {
                    this.pending_move = Some(e.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, cx| {
                this.end_drag();
                if this.pending_down.is_some() || this.selection.as_ref().is_some_and(|s| s.dragging) {
                    this.pending_move = Some(e.position);
                    this.pending_up = true;
                }
                cx.notify();
            }));
        let chips = self.header_chips();
        if !chips.iter().any(|c| matches!(c.hit, Hit::Space(_))) && !matches!(self.link, Link::Incompatible { .. }) {
            root = root.child(div().absolute().left(px(header::MARGIN)).top(px(header::SPACES_Y)).child("waiting for wheeld"));
        }
        self.controls.borrow_mut().clear();
        for chip in chips {
            let (r, hit) = (chip.rect, chip.hit.clone());
            let dots = match &chip.hit {
                Hit::Space(id) => self.spaces.iter().find(|s| &s.id == id).and_then(|s| status::space_dots(s, &self.status_dots)),
                _ => None,
            };
            root = root.child(
                div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).px_2().rounded_sm().cursor_pointer().overflow_hidden().whitespace_nowrap()
                    .flex().items_center()
                    .bg(rgb(if chip.active { 0x31483e } else { 0x17201c }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        match &hit {
                            Hit::Session(id) => this.select_session(id),
                            Hit::Space(id) => this.select_space(id),
                            Hit::Processes => this.toggle_processes(),
                            Hit::NewWindow => this.new_window(cx),
                        }
                        cx.notify();
                    }))
                    .gap_1()
                    .child(div().overflow_hidden().whitespace_nowrap().flex_1().child(chip.label))
                    .children(dots),
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
            crate::crash_guard::mark_pane(&p.leaf.id);
            let body = if self.quarantine.isolates_pane(&p.leaf.id) { self.quarantine_card(&p.leaf.id, &p.leaf.kind, cx) } else { match p.leaf.kind.as_str() {
                "browser" if native => {
                    let state = self.browsers.get(&p.leaf.id).cloned().unwrap_or_default();
                    let editing = self.address_edit.as_ref().filter(|(pane, _)| pane == &p.leaf.id).map(|(_, d)| d.clone());
                    let mut body = div().flex().flex_col().w(px(r.w)).h(px(r.h - TITLE)).overflow_hidden().child(browser::chrome(&p.leaf.id, &state, r.w, editing.as_deref(), &self.controls, cx));
                    if let Some(failure) = &state.failure {
                        body = body.child(div().flex_1().min_h(px(0.)).w(px(r.w)).overflow_hidden().child(browser::failure_card(&p.leaf.id, failure, &self.controls, cx)));
                    }
                    body
                }
                _ if native => div(),
                "agent" => {
                    // A pane whose agent has not started yet shares no draft or scroll state with another.
                    let agent = p.leaf.agent.as_deref();
                    let t = agent.and_then(|a| self.transcripts.get(a));
                    let draft = agent.and_then(|a| self.drafts.get(a)).map(String::as_str).unwrap_or("");
                    let scroll = agent.map(|a| self.scrolls.entry(a.to_owned()).or_default().clone());
                    let focused = self.focus.current.as_ref() == Some(&p.leaf.id);
                    let selection = self.selection.as_ref().filter(|s| Some(s.agent.as_str()) == agent);
                    let items = footer_items(&self.footer_items_for(self.space_id.as_deref()), t.unwrap_or(&EMPTY), Some(&p.leaf), &self.agents, self.raw_context.contains(&p.leaf.id));
                    let menu = self.menu.clone().filter(|(a, _)| Some(a.as_str()) == agent).map(|(_, o)| o);
                    agent_pane(&p.leaf.label, &p.leaf.id, agent, t, draft, focused, scroll, r.h - TITLE - 2. * PAD, selection, &self.drawn, &self.highlights, &self.parsed, items, menu, &self.controls, cx)
                }
                "empty" => {
                    let button = |label: &'static str, kind: NewPaneKind, cx: &mut Context<Self>| {
                        let id = id.clone();
                        div().px_2().py_1().rounded_sm().cursor_pointer().bg(rgb(0x31483e))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, _| this.set_kind(&id, kind)))
                            .child(label)
                    };
                    let artifact = {
                        let id = id.clone();
                        div().px_2().py_1().rounded_sm().cursor_pointer().bg(rgb(0x31483e))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, _| this.open_palette(&id)))
                            .child("Artifact…")
                    };
                    div().flex().gap_2().child(button("Terminal", NewPaneKind::Terminal, cx)).child(artifact).child(button("Browser", NewPaneKind::Browser, cx))
                }
                other => div().child(format!("unimplemented: {other} pane\n{}", p.leaf.label)),
            } };
            root = root.child(
                div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).bg(rgb(0x17201c))
                    .border_1().border_color(rgb(if self.focus.current.as_ref() == Some(&p.leaf.id) { 0x7fb89a } else { 0x2c3d34 }))
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let id = p.leaf.id.clone();
                        // A press in an agent pane's transcript band starts a pointer selection.
                        let agent = p.leaf.agent.clone().filter(|_| p.leaf.kind == "agent");
                        let band = (r.y + TITLE + PAD + LABEL)..(r.y + r.h - PAD - COMPOSER - FOOTER - 8.);
                        move |this, e: &MouseDownEvent, _, cx| {
                            this.press(format!("pane {id} at {:?}", e.position));
                            this.focus_pane(&id);
                            this.menu = None;
                            // A press outside the address field ends its edit; the field stops this event.
                            if this.address_edit.take().is_some() {
                                this.native.focus(&id);
                            }
                            // A press in the band starts a selection; one anywhere else drops the old one.
                            if let Some(agent) = &agent
                                && band.contains(&f32::from(e.position.y))
                            {
                                this.pending_down = Some((agent.clone(), e.position));
                            } else {
                                this.selection = None;
                            }
                            cx.notify();
                        }
                    }))
                    .child(div().h(px(TITLE)).w(px(r.w)).px_2().bg(rgb(0x22302a)).text_xs().flex().items_center().justify_between().overflow_hidden()
                        .child(match (p.leaf.kind.as_str(), self.browsers.get(&p.leaf.id)) {
                            ("browser", Some(state)) => format!("{} · {}", browser::title(state), p.leaf.id),
                            _ => format!("{} · {}", p.leaf.kind, p.leaf.id),
                        })
                        .child(div().w(px(12.)).h(px(12.)).flex_none().children(p.leaf.status.as_ref().and_then(|s| status::dot(s, &self.status_dots, gpui::ElementId::Name(format!("pane-dot-{}", p.leaf.id).into()))))))
                    .child(if p.leaf.kind == "browser" && native { div().text_color(rgb(0x8fa89b)).child(body) } else { div().p_2().text_color(rgb(0x8fa89b)).child(body) })
                    .children(self.palettes.get(&p.leaf.id).map(|palette| palette_overlay(&p.leaf.id, palette, r.w, r.h - TITLE, cx))),
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
        let (w, h) = self.viewport;
        if space_held && let Some(space) = self.space_id.clone() {
            root = root.child(div().absolute().left_0().top(px(TITLE)).w(px(w)).h(px(h - TITLE)).child(self.quarantine_space_card(&space, cx)));
        }
        if self.procs.open.is_some() {
            root = root.child(self.processes_overlay(w, h, cx));
        }
        if let Some(palette) = &self.command_palette {
            root = root.child(commands::command_palette_overlay(palette, w, h, cx));
        }
        if self.shortcuts_open {
            let groups = self.shortcut_sheet();
            root = root.child(commands::shortcut_sheet_overlay(&groups, w, h, cx));
        }
        if let Some(path) = self.pending_open.take() {
            cx.open_url(&format!("file://{}", file_url_path(&path)));
        }
        self.poll_server(cx);
        if matches!(self.link, Link::Incompatible { .. }) {
            root = root.child(div().absolute().left_0().top(px(HEADER)).w(px(w)).h(px(h - HEADER)).flex().justify_center().items_center()
                .child(div().w(px(420.)).child(self.server_box(cx))));
        }
        if self.settings_open {
            self.poll_settings(cx);
            root = root.child(self.settings_overlay(w, h, cx));
        }
        if let Some(c) = &self.confirmation {
            root = root.child(commands::confirmation_overlay(c, w, h, cx));
        }
        if self.pairing.open {
            root = root.child(self.pairing_overlay(w, h, cx));
        }
        root = root.child(self.toast_stack(w, h, cx));
        crate::crash_guard::clear();
        root
    }
}

/// The artifact palette over one pane's body: the body dims, and a search field above a grouped
/// list sits inside the pane's bounds (ArtifactPaletteView.swift). A press on the dim closes it.
/// A file path as the path of a `file://` URL: every byte outside the unreserved set and `/` is
/// percent-encoded, so a folder with a space or a `#` opens.
fn file_url_path(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn palette_overlay(pane: &str, palette: &Palette, width: f32, height: f32, cx: &mut Context<Client>) -> gpui::Div {
    let narrow = width < 420.;
    let compact = height < 340.;
    let inset = if narrow { 8. } else { 10. };
    let card_w = if narrow { width - 2. * inset } else { 560f32.min(width - 2. * inset) };
    let pid = pane.to_owned();
    let dim = cx.listener(move |this, _, _, cx| {
        this.close_palette(&pid);
        cx.stop_propagation();
        cx.notify();
    });
    let field: gpui::Div = if palette.query.is_empty() {
        div().text_color(rgb(0x6f8a7d)).child(palette.field_placeholder())
    } else {
        div().text_color(rgb(0xd9e8e1)).child(palette.query.clone())
    };
    let mut card = div().w(px(card_w)).max_h(px(height - 2. * inset)).flex().flex_col().rounded_md().bg(rgb(0x17201c)).border_1().border_color(rgb(0x3d5247))
        .text_color(rgb(0xd9e8e1)).overflow_hidden()
        .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
        .child(div().h(px(if compact { 34. } else { 40. })).px_3().flex().items_center().gap_2().border_b_1().border_color(rgb(0x2c3d34))
            .child(div().text_color(rgb(0x6f8a7d)).child("⌕")).child(field.flex_1().overflow_hidden().whitespace_nowrap()));
    let note = |title: &str, lines: Vec<String>| {
        let mut n = div().flex_1().min_h_0().p(px(if compact { 12. } else { 18. })).flex().flex_col().gap_1().items_center().text_center()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title.to_owned()));
        for line in lines {
            n = n.child(div().text_xs().text_color(rgb(0x8fa89b)).child(line));
        }
        n
    };
    let content: gpui::AnyElement = match &palette.listing {
        palette::Listing::Loading => div().p_4().text_xs().text_color(rgb(0x8fa89b)).child("Listing artifacts…").into_any_element(),
        palette::Listing::Failed(message) => {
            let pid = pane.to_owned();
            div().p(px(if compact { 10. } else { 16. })).flex().flex_col().gap_1()
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xe08070)).child(palette::FAILED_TITLE))
                .child(div().text_xs().text_color(rgb(0x8fa89b)).child(palette::FAILED_LEAD))
                .child(div().text_xs().font_family("Menlo").text_color(rgb(0xe08070)).child(message.clone()))
                .child(div().mt_1().px_2().py_1().rounded_sm().bg(rgb(0x31483e)).cursor_pointer().text_xs().child(palette::RETRY)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.ask_artifacts(&pid);
                        cx.stop_propagation();
                        cx.notify();
                    }))).into_any_element()
        }
        palette::Listing::Ready(list) if list.is_empty() => note(palette::EMPTY_TITLE, vec![palette::EMPTY_HELP.into(), format!("{} {}", palette::EMPTY_ASK, palette::EMPTY_COMMAND)]).into_any_element(),
        palette::Listing::Ready(_) if palette.rows.is_empty() => note(&format!("No artifact matches “{}”", palette.query), vec![palette.no_match_detail()]).into_any_element(),
        palette::Listing::Ready(_) => {
            let mut list = div().id(gpui::ElementId::Name(format!("palette-{pane}").into())).overflow_y_scroll().flex().flex_col().pb_1();
            let selected = palette.selected_row().map(|r| r.id());
            for scope in [wheel_proto::ArtifactScope::Project, wheel_proto::ArtifactScope::Machine] {
                let rows: Vec<&palette::Row> = palette.rows.iter().filter(|r| r.info.scope == scope).collect();
                if rows.is_empty() {
                    continue;
                }
                list = list.child(div().px_4().pt(px(if compact { 5. } else { 8. })).pb(px(if compact { 2. } else { 3. })).flex().justify_between().text_xs().text_color(rgb(0x6f8a7d))
                    .child(palette::scope_name(scope).to_uppercase()).child(rows.len().to_string()));
                for row in rows {
                    let is_selected = selected.as_deref() == Some(row.id().as_str());
                    let (pid, info) = (pane.to_owned(), row.info.clone());
                    let mut r = div().mx(px(6.)).px(px(10.)).py(px(if compact { 4. } else { 6. })).rounded_md().cursor_pointer().flex().flex_col()
                        .bg(rgb(if is_selected { 0x31483e } else { 0x17201c }))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                            this.show_artifact(&pid, &info);
                            cx.stop_propagation();
                            cx.notify();
                        }));
                    if row.has_title() {
                        r = r.child(div().font_weight(FontWeight::MEDIUM).child(row.info.title.clone()))
                            .child(div().text_xs().font_family("Menlo").text_color(rgb(0x8fa89b)).child(row.info.name.clone()));
                    } else {
                        r = r.child(div().font_weight(FontWeight::MEDIUM).font_family("Menlo").child(row.info.name.clone()));
                    }
                    if let Some(description) = &row.info.description {
                        r = r.child(div().text_xs().text_color(rgb(0x8fa89b)).child(description.clone()));
                    }
                    for problem in &row.info.problems {
                        r = r.child(div().text_xs().text_color(rgb(0xe0b070)).child(format!("⚠ {problem}")));
                    }
                    list = list.child(r);
                }
            }
            list.flex_1().min_h_0().into_any_element()
        }
    };
    card = card.child(content);
    let mut keys = div().flex_none().flex().gap_3().whitespace_nowrap();
    for (i, k) in palette.keys().into_iter().enumerate() {
        // The narrow footer drops the move hint to keep the others on one line.
        if narrow && i == 0 && k.symbol == "↑↓" {
            continue;
        }
        keys = keys.child(div().flex().gap_1().opacity(if k.enabled { 1. } else { 0.45 })
            .child(div().px_1().rounded_sm().border_1().border_color(rgb(0x3d5247)).child(k.symbol)).child(k.word));
    }
    card = card.child(div().px_3().py(px(5.)).flex().justify_between().items_center().text_xs().text_color(rgb(0x6f8a7d)).bg(rgb(0x1d2823)).border_t_1().border_color(rgb(0x2c3d34))
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(palette.footer())).child(keys));
    div().absolute().left_0().top(px(TITLE)).w(px(width)).h(px(height)).bg(rgba(0x0000008c)).flex().justify_center().items_start().pt(px(inset))
        .on_mouse_down(MouseButton::Left, dim)
        .child(card)
}

/// The composer's height, so the transcript column above it never moves.
const COMPOSER: f32 = 44.;
/// The status line under the composer: the footer's items at the left, the config controls at
/// the right.
const FOOTER: f32 = 22.;
static EMPTY: std::sync::LazyLock<Transcript> = std::sync::LazyLock::new(Transcript::default);
/// The pane body's padding (`p_2`, above and below) and the label line above the transcript
/// column. The composer and the footer each sit 4 px (`mt_1`) under what is above them.
const PAD: f32 = 8.;
const LABEL: f32 = 24.;

/// An agent pane's body: its label and state, the transcript column scrolled to follow the
/// stream, a running line while a turn is in progress, and the composer with the draft. Every
/// text it draws is recorded in `drawn` with its layout, so a pointer selection can find it.
#[allow(clippy::too_many_arguments)]
fn agent_pane(label: &str, pane: &str, agent: Option<&str>, t: Option<&Transcript>, draft: &str, focused: bool, scroll: Option<ScrollHandle>, height: f32,
    selection: Option<&Selection>, drawn: &Rc<RefCell<Vec<Drawn>>>, highlights: &Highlights, parsed: &Parsed, items: Vec<(String, String)>, menu: Option<String>,
    controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> gpui::Div {
    let state = t.map(|t| t.state.as_str()).unwrap_or("connecting");
    let mut col = div().id(gpui::ElementId::Name(format!("transcript-{pane}").into())).flex().flex_col().gap_1().overflow_y_scroll()
        .h(px((height - LABEL - COMPOSER - FOOTER - 8.).max(1.)));
    if let Some(scroll) = &scroll {
        col = col.track_scroll(scroll);
    }
    let mut texts = Texts { agent: agent.unwrap_or("").to_owned(), row: 0, next: 0, selection, drawn };
    for (i, row) in t.map(|t| t.rows.as_slice()).unwrap_or_default().iter().enumerate() {
        texts.row = i;
        texts.next = 0;
        col = col.child(match &row.kind {
            Kind::User { text, queued } => div().flex().gap_2().text_color(rgb(0xe0b070))
                .child(div().child(if *queued { "queued" } else { "you" }).text_color(rgb(0x8fa89b)).text_xs())
                .child(div().child(texts.plain("user", text, 0xe0b070))),
            Kind::Reply(x) => {
                let blocks = {
                    let mut cache = parsed.borrow_mut();
                    let entry = cache.entry((texts.agent.clone(), row.id)).or_insert_with(|| (String::new(), Rc::new(vec![])));
                    if entry.0 != *x {
                        *entry = (x.clone(), Rc::new(markdown::parse(x)));
                    }
                    entry.1.clone()
                };
                let mut col = div().flex().flex_col().gap_1().text_color(rgb(0xd9e8e1));
                for block in blocks.iter() {
                    col = col.child(block_el(block, &mut texts, highlights, cx));
                }
                col
            }
            Kind::Thought(x) => div().text_color(rgb(0x6f8a7c)).child(texts.plain("thought", x, 0x6f8a7c)),
            Kind::Tool { title, kind, status, content, detail, .. } => {
                let info = ToolRow::new(title, kind, status, content, detail.as_deref());
                let mut line = div().flex().gap_2().items_center().text_color(rgb(0x8fa89b))
                    .child(div().w(px(18.)).text_color(rgb(0x6f8a7c)).child(info.glyph))
                    .child(div().child(texts.plain("tool", &info.label, 0x8fa89b)));
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
                    plan = plan.child(texts.plain("plan", &format!("{} {}", match e.status.as_str() { "completed" => "☑", "in_progress" => "◐", _ => "☐" }, e.content), 0x8fa89b));
                }
                plan
            }
            Kind::Mode(m) => div().text_xs().text_color(rgb(0x8fa89b)).child(format!("Mode: {m}")),
            Kind::Notice(x) => div().text_xs().text_color(rgb(0x8fa89b)).child(texts.plain("notice", x, 0x8fa89b)),
            Kind::Error(x) => div().text_color(rgb(0xe07070)).child(texts.plain("error", x, 0xe07070)),
        });
    }
    if t.is_some_and(Transcript::busy) {
        col = col.child(div().text_color(rgb(0xe0b070)).child("working…"));
    }
    div().flex().flex_col().h(px(height.max(1.)))
        .child(div().h(px(LABEL)).text_color(rgb(0x8fa89b)).child(format!("{label} · {state}")))
        .child(col)
        .child(div().h(px(COMPOSER)).mt_1().px_2().py_1().rounded_sm().bg(rgb(0x0e1311))
            .border_1().border_color(rgb(if focused { 0x7fb89a } else { 0x2c3d34 }))
            .text_color(rgb(0xd9e8e1)).overflow_hidden()
            .child(format!("{draft}{}", if focused { "▏" } else { "" })))
        .child(footer_row(pane, agent.unwrap_or(""), t, items, menu, controls, cx))
}

/// A zero-size element that records where its parent was drawn, so a check can press it.
pub fn recorded(agent: &str, id: &str, controls: &Rc<RefCell<Vec<Control>>>) -> impl IntoElement {
    let (agent, id, controls) = (agent.to_owned(), id.to_owned(), controls.clone());
    canvas(move |bounds, _, _| controls.borrow_mut().push(Control { agent, id, bounds }), |_, _, _, _| ()).absolute().top_0().left_0().size_full()
}

/// The status line under the composer (the Swift client's `ConfigRow`): the footer's items in the
/// order `chat.footer.items` says, then the agent's config controls at the right, each a menu
/// of its choices. The context meter is a 28 by 4 bar with the percent beside it; a click on it
/// reads the raw figures instead.
fn footer_row(pane: &str, agent: &str, t: Option<&Transcript>, items: Vec<(String, String)>, menu: Option<String>, controls: &Rc<RefCell<Vec<Control>>>, cx: &mut Context<Client>) -> gpui::Div {
    let mut left = div().flex().items_center().gap(px(10.)).overflow_hidden().whitespace_nowrap().flex_1().text_xs().text_color(rgb(0x8fa89b));
    for (item, text) in &items {
        if item == "context" {
            let context = t.and_then(|t| t.context);
            let fraction = context.as_ref().map(footer::fraction).unwrap_or(0.);
            let tint = context.as_ref().map(footer::tint).unwrap_or(0x7fb89a);
            let pane = pane.to_owned();
            left = left.child(div().relative().flex().items_center().gap(px(6.)).flex_none().cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.press("context".into());
                    if !this.raw_context.remove(&pane) {
                        this.raw_context.insert(pane.clone());
                    }
                    cx.notify();
                }))
                .child(div().w(px(28.)).h(px(4.)).rounded_full().bg(rgba(0xffffff26)).overflow_hidden()
                    .child(div().w(px(28. * fraction)).h(px(4.)).rounded_full().bg(rgb(tint))))
                .child(div().text_color(rgb(if context.is_some() { 0x8fa89b } else { 0x4f6a5c })).child(text.clone()))
                .child(recorded(agent, "context", controls)));
        } else {
            left = left.child(div().flex_none().child(text.clone()));
        }
    }
    let mut right = div().flex().items_center().gap(px(6.)).flex_none().text_xs();
    for option in t.map(|t| t.config.as_slice()).unwrap_or_default() {
        let choices = footer::control_choices(option);
        let settable = !choices.is_empty();
        let (agent_id, option_id) = (agent.to_owned(), option.id.clone());
        let mut control = div().relative().flex().items_center().px_1().rounded_sm().bg(rgb(0x22302a)).text_color(rgb(if settable { 0xd9e8e1 } else { 0x6f8a7c }))
            .child(footer::control_text(option))
            .child(recorded(agent, &option.id, controls));
        if settable {
            control = control.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                // The pane under the control closes any menu on a press; this press is the control's.
                cx.stop_propagation();
                this.press(format!("control {option_id}"));
                let key = (agent_id.clone(), option_id.clone());
                this.menu = if this.menu.as_ref() == Some(&key) { None } else { Some(key) };
                cx.notify();
            }));
        }
        if menu.as_deref() == Some(option.id.as_str()) {
            let mut list = div().flex().flex_col().min_w(px(120.)).rounded_sm().bg(rgb(0x22302a)).border_1().border_color(rgb(0x3d5247)).py_1().text_xs().text_color(rgb(0xd9e8e1))
                .child(div().px_2().py_1().text_color(rgb(0x8fa89b)).child(option.name.clone()));
            let current = match &option.kind {
                wheel_proto::ConfigKind::Select { current, .. } => current.clone(),
                wheel_proto::ConfigKind::Boolean { current } => current.to_string(),
                wheel_proto::ConfigKind::Unknown => String::new(),
            };
            for (value, name) in choices {
                let (agent_id, option_id, value_id) = (agent.to_owned(), option.id.clone(), value.clone());
                let unchanged = value == current;
                list = list.child(div().relative().px_2().py_1().cursor_pointer().flex().gap_2()
                    .child(div().w(px(12.)).child(if unchanged { "✓" } else { "" }))
                    .child(name)
                    .child(recorded(agent, &format!("{}:{value}", option.id), controls))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        // The value in force needs no request; the menu just closes.
                        if unchanged {
                            this.menu = None;
                        } else {
                            this.set_config(&agent_id, &option_id, &value_id);
                        }
                        cx.notify();
                    })));
            }
            control = control.child(deferred(anchored().anchor(Anchor::BottomLeft).snap_to_window().child(list)));
        }
        right = right.child(control);
    }
    div().h(px(FOOTER)).mt_1().flex().items_center().gap(px(10.)).child(left).child(right)
}

/// The footer's items for a transcript as text, (item, text), in the order the setting says; an
/// item with nothing to show is left out (`ChatView.swift`'s `footerItem`).
fn footer_items(order: &[String], t: &Transcript, leaf: Option<&Leaf>, agents: &[AgentInfo], raw: bool) -> Vec<(String, String)> {
    let mut out = vec![];
    for item in order {
        let text = match item.as_str() {
            "context" => Some(t.context.as_ref().map(|c| if raw { footer::raw_text(c) } else { footer::short_text(c) }).unwrap_or_else(|| "0%".into())),
            "background" => t.footer.as_ref().and_then(footer::background_text),
            "directory" => t.footer.as_ref().map(footer::shown_directory),
            "home" => leaf.map(|l| {
                let harness = l.harness.as_deref().unwrap_or("claude");
                let listed = agents.iter().find(|a| a.name == harness).and_then(|a| a.home.as_deref());
                footer::shown_home(l.home.as_deref(), harness, listed)
            }),
            "branch" => t.footer.as_ref().and_then(|f| f.branch.as_ref()).map(|b| format!("\u{2387} {b}")),
            "changes" => t.footer.as_ref().and_then(footer::changes_text),
            _ => None,
        };
        if let Some(text) = text {
            out.push((item.clone(), text));
        }
    }
    out
}

/// The background of selected text.
const SELECTION: u32 = 0x3d5a4c;
/// The code block's ground and its header's.
const CODE_BG: u32 = 0x0b100e;
const CODE_HEADER: u32 = 0x1a2621;

/// Hands out the texts of one row in drawing order, recording each in `drawn` and painting the
/// selection's part of it.
struct Texts<'a> {
    agent: String,
    row: usize,
    next: usize,
    selection: Option<&'a Selection>,
    drawn: &'a Rc<RefCell<Vec<Drawn>>>,
}
impl Texts<'_> {
    /// A text with `runs` already flat; `base` is its colour where no run says otherwise.
    fn styled(&mut self, kind: &'static str, text: &str, runs: &[Run], base: u32, language: Option<&str>, highlighted: bool) -> StyledText {
        let block = self.next;
        self.next += 1;
        let selected = self.selection.and_then(|s| s.slice(self.row, block, text.len()));
        let styled = StyledText::new(text.to_owned()).with_highlights(highlight_runs(text, runs, selected, base));
        self.drawn.borrow_mut().push(Drawn { agent: self.agent.clone(), row: self.row, block, kind, language: language.map(str::to_owned), highlighted, text: text.to_owned(), layout: styled.layout().clone() });
        styled
    }
    fn plain(&mut self, kind: &'static str, text: &str, base: u32) -> StyledText {
        self.styled(kind, text, &[], base, None, false)
    }
    fn inline(&mut self, kind: &'static str, text: &Text, base: u32) -> StyledText {
        self.styled(kind, &text.text, &text.runs, base, None, false)
    }
}

/// GPUI highlight runs for `text`: the marks' styles, with the selection's background laid over
/// `selected`. Ranges come out sorted and disjoint, as `StyledText` needs.
fn highlight_runs(text: &str, runs: &[Run], selected: Option<Range<usize>>, base: u32) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut all: Vec<Run> = runs.to_vec();
    if let Some(sel) = selected {
        let (mut a, mut b) = (sel.start, sel.end);
        while !text.is_char_boundary(a) {
            a -= 1;
        }
        while !text.is_char_boundary(b) {
            b += 1;
        }
        all.push(Run { range: a..b, mark: Mark { selected: true, ..Mark::default() } });
    }
    markdown::flatten(&all).into_iter().map(|r| (r.range, style_of(&r.mark, base))).collect()
}

fn style_of(mark: &Mark, base: u32) -> HighlightStyle {
    let mut style = HighlightStyle { color: Some(rgb(mark.color.unwrap_or(if mark.code { 0xe0b070 } else { base })).into()), ..HighlightStyle::default() };
    if mark.bold {
        style.font_weight = Some(FontWeight::BOLD);
    }
    if mark.italic {
        style.font_style = Some(FontStyle::Italic);
    }
    if mark.code {
        style.background_color = Some(rgb(0x22302a).into());
    }
    if mark.link.is_some() {
        style.color = Some(rgb(0x7fb89a).into());
        style.underline = Some(UnderlineStyle { thickness: px(1.), color: None, wavy: false });
    }
    if mark.strike {
        style.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None });
    }
    if mark.selected {
        style.background_color = Some(rgb(SELECTION).into());
    }
    style
}

/// One block of a reply, drawn as WheelUI's `ChatRich` does.
fn block_el(block: &Block, texts: &mut Texts, highlights: &Highlights, cx: &mut Context<Client>) -> gpui::Div {
    match block {
        Block::Paragraph(t) => div().child(texts.inline("reply", t, 0xd9e8e1)),
        Block::Heading { level, text: t } => {
            let h = div().font_weight(FontWeight::BOLD).child(texts.inline("heading", t, 0xd9e8e1));
            match level { 1 => h.text_lg(), 2 => h.text_base(), _ => h.text_sm() }
        }
        Block::List(items) => {
            let mut list = div().flex().flex_col();
            for item in items {
                let line = div().flex().gap_1().pl(px(16. * item.depth as f32))
                    .child(div().w(px(20.)).flex_none().text_color(rgb(0x8fa89b)).child(item.marker.clone()));
                list = list.child(match &item.block {
                    Some(b) => line.child(div().flex_1().child(block_el(b, texts, highlights, cx))),
                    None => line.child(div().child(texts.inline("list", &item.text, 0xd9e8e1))),
                });
            }
            list
        }
        Block::Code { language, code } => {
            let key = (language.clone().unwrap_or_default(), code.clone());
            let runs = {
                let mut cache = highlights.borrow_mut();
                if cache.len() > 256 {
                    cache.clear();
                }
                cache.entry(key).or_insert_with(|| markdown::highlight(language.as_deref(), code).map(Rc::new)).clone()
            };
            let highlighted = runs.as_ref().is_some_and(|r| !r.is_empty());
            let body = texts.styled("code", code, runs.as_deref().map(Vec::as_slice).unwrap_or(&[]), 0xd9e8e1, language.as_deref(), highlighted);
            let copied = code.clone();
            div().rounded_sm().bg(rgb(CODE_BG)).border_1().border_color(rgb(0x2c3d34)).flex().flex_col().overflow_hidden()
                .child(div().h(px(20.)).px_2().flex().items_center().justify_between().bg(rgb(CODE_HEADER)).text_xs().text_color(rgb(0x8fa89b))
                    .child(language.clone().unwrap_or_else(|| "code".to_owned()))
                    .child(div().id(gpui::ElementId::Name(format!("copy-{}-{}", texts.row, texts.next).into())).px_1().rounded_sm().cursor_pointer().bg(rgb(0x22302a)).child("Copy")
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                            cx.stop_propagation();
                        }))))
                .child(div().p_2().font_family("Menlo").text_xs().child(body))
        }
        Block::Quote(t) => div().pl_2().border_l_2().border_color(rgb(0x3d5247)).text_color(rgb(0x8fa89b)).child(texts.inline("quote", t, 0x8fa89b)),
        Block::Table { header, rows } => {
            let cell = |texts: &mut Texts, t: &Text, bold: bool| {
                let c = div().flex_1().px_1().py_0p5().border_1().border_color(rgb(0x2c3d34)).child(texts.inline("table", t, 0xd9e8e1));
                if bold { c.font_weight(FontWeight::BOLD).bg(rgb(0x1a2621)) } else { c }
            };
            let mut table = div().flex().flex_col();
            let mut head = div().flex();
            for h in header {
                head = head.child(cell(texts, h, true));
            }
            table = table.child(head);
            for r in rows {
                let mut line = div().flex();
                for c in r {
                    line = line.child(cell(texts, c, false));
                }
                table = table.child(line);
            }
            table
        }
        Block::Rule => div().h(px(1.)).bg(rgb(0x2c3d34)),
    }
}
