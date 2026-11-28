//! A plugin pane's script run by wheel-luau and fed on this window's own wheeld connection
//! (ADR-0056, ADR-0052): the source, its feeds and its actions travel as `plugin_source`,
//! `feed_subscribe`, `feed_update` and `plugin_action`, and `plugin_draw.rs` draws the tree. Every
//! plugin pane runs this way, and so does an agent pane shown as its `agent/view` page.
use super::*;
use serde_json::Value as Json;
use wheel_luau::pane::{Effect, PluginPane};

/// One plugin pane run here: its script and the requests it waits on.
#[derive(Default)]
pub struct PluginHost {
    pub pane: PluginPane,
    /// The `<plugin>/<pane>` it runs and the directory of its pane's space: a pane that comes to name
    /// another, or to sit in another space, runs afresh.
    runs: (String, String),
    /// The `plugin_source` request in flight.
    source_req: Option<u64>,
    /// Each `feed_subscribe` in flight, as (feed, params).
    feed_reqs: HashMap<u64, (String, Json)>,
    /// Each `plugin_action` in flight, by its action's name.
    action_reqs: HashMap<u64, String>,
    /// Each TextField's input, by node id and the node's serial: a new node at the same id gets a
    /// new input, as Swift's draft goes with its node.
    pub(super) fields: HashMap<String, (u64, gpui::Entity<InputState>, String)>,
    /// When a Button's copy action last ran, by node id: its label shows a checkmark for 1.5 s.
    pub(super) copied: HashMap<String, std::time::Instant>,
    /// The List each `scrollTo` last scrolled, by node id, with the target it scrolled to.
    pub(super) scrolled: RefCell<HashMap<String, (gpui::ScrollHandle, Option<String>)>>,
    /// The pane's own scroll when the tree does not fill it.
    pub(super) scroll: gpui::ScrollHandle,
    /// What SwiftUI keeps in a view's own `@State`: the Modal sheets and Popovers open, each Tabs'
    /// tab, each Table's own sort (column key, ascending) and each ViewThatFits' last choice.
    pub(super) open: HashSet<String>,
    pub(super) tabs: HashMap<String, usize>,
    pub(super) sorts: HashMap<String, (String, bool)>,
    /// The row each Table's last plain or ⌘ click selected, where a shift-click's range starts.
    pub(super) anchors: HashMap<String, String>,
    pub(super) fits: Rc<RefCell<HashMap<String, Fit>>>,
    /// Each Table's list scroll, by node id.
    tables: RefCell<HashMap<String, gpui::UniformListScrollHandle>>,
    /// The width a check draws the page at instead of the pane's.
    pub(super) width: Option<f32>,
    /// The width the page was last drawn at.
    pub(super) drawn: std::cell::Cell<f32>,
    /// The pane's height, which a scene Canvas with `minHeight` and `reserve` sizes itself from.
    pub(super) viewport: std::cell::Cell<f32>,
    /// Each Canvas's camera, pointer, replayed commands and reported size, by node id.
    pub(super) canvases: RefCell<HashMap<String, Rc<RefCell<super::plugin_canvas::CanvasView>>>>,
    /// Each Terminal, AgentChat, WebView and Map embed's state, by node id, and the embed holding the keyboard
    /// while the pane has it.
    pub(super) embeds: HashMap<String, super::plugin_embeds::Embed>,
    pub(super) focused_embed: Option<String>,
    /// The embeds' bodies built for this frame's draw (`sync_plugin_embeds`), which every build of the node draws.
    pub(super) embed_els: HashMap<String, super::plugin_embeds::EmbedBody>,
    /// The nodes with a hover highlight the pointer is over, and those the last draw lit (by the
    /// pointer or the script's held hover), as WheelUI's `PaneHoverProbe.lit`.
    pub(super) hovered: HashSet<String>,
    pub(super) lit: RefCell<std::collections::BTreeSet<String>>,
}

/// The child a ViewThatFits drew, at what width, and that child's height there.
#[derive(Clone, Copy, Debug)]
pub struct Fit {
    pub choice: usize,
    pub width: f32,
    pub height: f32,
}

/// What a menu item a plugin pane opened does.
#[derive(Clone, Debug)]
pub enum Pick {
    Press(String),
    Set(String, Json),
    Raise(String, String, Json),
    Separator,
}

/// One item of a menu a plugin pane opens: its title, whether it shows a checkmark, what it does,
/// and the submenu it opens instead.
#[derive(Clone, Debug)]
pub struct Entry {
    pub title: String,
    pub on: bool,
    pub pick: Pick,
    pub submenu: Vec<Entry>,
}

impl Entry {
    pub fn new(title: String, on: bool, pick: Pick) -> Self {
        Self { title, on, pick, submenu: vec![] }
    }
}

/// `entries`' picks depth-first, each answering item's id its place in that order.
fn flatten(entries: &[Entry], out: &mut Vec<Pick>) {
    for e in entries {
        if e.submenu.is_empty() {
            if !matches!(e.pick, Pick::Separator) {
                out.push(e.pick.clone());
            }
        } else {
            flatten(&e.submenu, out);
        }
    }
}

/// `entries` as AppKit's menu items, numbered as [`flatten`] numbers their picks.
fn menu_items<'a>(entries: &'a [Entry], ids: &'a [String], next: &mut usize) -> Vec<crate::appkit::MenuItem<'a>> {
    entries.iter().map(|e| {
        if matches!(e.pick, Pick::Separator) && e.submenu.is_empty() {
            return crate::appkit::MenuItem { id: "-", title: "", enabled: false, on: false, submenu: vec![] };
        }
        if e.submenu.is_empty() {
            *next += 1;
            return crate::appkit::MenuItem { id: &ids[*next - 1], title: &e.title, enabled: true, on: e.on, submenu: vec![] };
        }
        crate::appkit::MenuItem { id: "", title: &e.title, enabled: true, on: false, submenu: menu_items(&e.submenu, ids, next) }
    }).collect()
}

/// The menus previewed panes open: the AppKit menus (built lazily, answers through `answers`) and
/// the last one opened, with its pane and its items.
#[derive(Default)]
pub struct Menus {
    dialogs: Option<crate::appkit::Dialogs>,
    open: Option<(String, Vec<Pick>)>,
    built: Vec<Json>,
}

/// A scroll of the pane's page asked from outside the pointer: to an edge, or by a distance in
/// points (positive is down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollTo {
    Top,
    Bottom,
    By(f64),
}

impl ScrollTo {
    /// The bridge's `scroll` arguments: a number `dy`, else `to` `top` or `bottom`.
    pub fn from_args(args: &Json) -> Option<Self> {
        if let Some(dy) = args.get("dy").and_then(Json::as_f64) {
            return Some(Self::By(dy));
        }
        match args.get("to").and_then(Json::as_str) {
            Some("top") => Some(Self::Top),
            Some("bottom") => Some(Self::Bottom),
            _ => None,
        }
    }

    /// The offset from the top this scroll ends at, from `offset` with `max` the most the page
    /// scrolls: never above the top or past the bottom, as SwiftUI's ScrollView holds it.
    pub fn target(self, offset: f64, max: f64) -> f64 {
        let max = max.max(0.);
        match self {
            Self::Top => 0.,
            Self::Bottom => max,
            Self::By(dy) => (offset + dy).clamp(0., max),
        }
    }
}

/// Where the pane's own scroll stands, in points, as SwiftUI's `onScrollGeometryChange` reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollMetrics {
    pub offset: f64,
    pub content: f64,
    pub visible: f64,
}

impl ScrollMetrics {
    /// From GPUI's laid-out scroll: its offset (negative as it goes down), the most it scrolls and
    /// the height it shows. None before the first layout, when it shows nothing.
    pub fn from_layout(offset_y: f32, max_y: f32, visible: f32) -> Option<Self> {
        (visible > 0.).then(|| Self { offset: f64::from(-offset_y).max(0.), content: f64::from(visible + max_y.max(0.)), visible: f64::from(visible) })
    }

    pub fn json(&self) -> Json {
        serde_json::json!({"offset": self.offset, "content": self.content, "visible": self.visible})
    }
}

impl PluginHost {
    /// The pane's own scroll, or None when the view fills the pane (it has none) or has not laid out.
    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        if self.pane.fills_pane() {
            return None;
        }
        ScrollMetrics::from_layout(f32::from(self.scroll.offset().y), f32::from(self.scroll.max_offset().y), f32::from(self.scroll.bounds().size.height))
    }

    /// Moves the pane's own scroll; false when the view fills the pane, which has none.
    pub fn scroll_to(&self, to: ScrollTo) -> bool {
        if self.pane.fills_pane() {
            return false;
        }
        let at = self.scroll_metrics().map_or(0., |m| m.offset);
        let y = to.target(at, f64::from(f32::from(self.scroll.max_offset().y)));
        self.scroll.set_offset(gpui::point(px(0.), px(-(y as f32))));
        true
    }

    /// Drops the view's own state for nodes the tree no longer holds, as SwiftUI's `@State` goes
    /// with its view: a node that comes back starts fresh.
    pub(super) fn prune_view_state(&mut self) {
        let tree = &self.pane.tree;
        self.open.retain(|id| tree.node(id).is_some());
        self.tabs.retain(|id, _| tree.node(id).is_some());
        self.sorts.retain(|id, _| tree.node(id).is_some());
        self.anchors.retain(|id, _| tree.node(id).is_some());
        self.hovered.retain(|id| tree.node(id).is_some());
        self.fits.borrow_mut().retain(|id, _| tree.node(id).is_some());
        self.tables.borrow_mut().retain(|id, _| tree.node(id).is_some());
        self.canvases.borrow_mut().retain(|id, _| tree.node(id).is_some_and(|n| n.component == "Canvas"));
        self.embeds.retain(|id, e| tree.node(id).is_some_and(|n| n.component == e.component));
        if self.focused_embed.as_ref().is_some_and(|id| !self.embeds.contains_key(id)) {
            self.focused_embed = None;
        }
    }

    pub(super) fn table_scroll(&self, id: &str) -> gpui::UniformListScrollHandle {
        self.tables.borrow_mut().entry(id.to_owned()).or_default().clone()
    }
}

/// The `<plugin>/<pane>` `leaf` runs: a plugin pane's own, or `agent/view` for an agent pane shown
/// as its view (`agent_view`); None for any other pane.
fn runs_of(leaf: &Leaf, agent_view: bool) -> Option<String> {
    match leaf.kind.as_str() {
        "plugin" => Some(leaf.label.clone()),
        "agent" if agent_view => Some("agent/view".to_owned()),
        _ => None,
    }
}

impl Client {
    /// What pane `pane` runs ([`runs_of`]) and the directory of its space, None when it runs no script.
    fn pane_runs(&self, pane: &str) -> Option<(String, String)> {
        self.spaces.iter().find_map(|s| s.all().find(|l| l.id == pane).map(|l| (s, l))).and_then(|(s, l)| Some((runs_of(l, self.agent_views.contains(pane))?, s.cwd.clone())))
    }

    /// Runs pane `pane`'s script in this client unless it runs it already: it asks wheeld for the
    /// source and draws the tree in the pane's body. False when `pane` runs no script.
    pub(super) fn host_plugin(&mut self, pane: &str) -> bool {
        let Some(runs) = self.pane_runs(pane) else { return false };
        if self.plugins.get(pane).is_some_and(|h| h.runs == runs) {
            return true;
        }
        self.preview_plugin(pane)
    }

    /// Runs pane `pane`'s script afresh, ending the run it had: a check's clean start. False when
    /// `pane` runs no script.
    pub fn preview_plugin(&mut self, pane: &str) -> bool {
        let Some(runs) = self.pane_runs(pane) else { return false };
        let Some(space) = self.spaces.iter().find(|s| s.all().any(|l| l.id == pane)) else { return false };
        let mut host = PluginHost { runs: runs.clone(), ..Default::default() };
        // The context the script's `view` gets as `data`.
        let mut ids = runs.0.splitn(2, '/');
        let (plugin, plugin_pane) = (ids.next().map(str::to_owned), ids.next().map(str::to_owned));
        let cwd = (!space.cwd.is_empty()).then(|| space.cwd.clone());
        for (k, v) in [("pane", Some(pane.to_owned())), ("space", Some(space.id.clone())), ("cwd", cwd), ("plugin", plugin), ("plugin_pane", plugin_pane)] {
            host.pane.context.insert(k.into(), v.map_or(Json::Null, Json::String));
        }
        if let Some(old) = self.plugins.insert(pane.to_owned(), host) {
            self.close_plugin(pane, old);
        }
        self.request_plugin_source(pane);
        true
    }

    /// Whether agent pane `pane` draws its `agent/view` page here: the person chose it and the
    /// script runs (the Agents surface draws the chat).
    pub(super) fn agent_view_shown(&self, pane: &str) -> bool {
        self.mode != "agents" && self.agent_views.contains(pane) && self.plugins.contains_key(pane)
    }

    /// Plugin pane `leaf`'s script error, shown beside its title; None for any other pane.
    pub(super) fn plugin_error(&self, leaf: &Leaf) -> Option<String> {
        (leaf.kind == "plugin").then(|| self.plugins.get(&leaf.id)?.pane.error().map(str::to_owned)).flatten()
    }

    /// Pane `pane`'s page `w` by `h`, with `body` recording where it lies (the check's pictures and `pane shot`).
    pub(super) fn plugin_body(&mut self, pane: &str, w: f32, h: f32, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let el = self.plugin_element(pane, w, h, window, cx);
        div().relative().flex_none().w(px(w)).h(px(h)).overflow_hidden().children(el).child(recorded(pane, "body", &self.controls))
    }

    /// Ends pane `pane`'s script: its feeds are dropped with wheeld.
    pub fn end_plugin(&mut self, pane: &str) {
        if let Some(host) = self.plugins.remove(pane) {
            self.close_plugin(pane, host);
        }
    }

    /// Ends `host`'s script and drops its feeds with wheeld: `close` puts their unsubscribes in
    /// its outbox, which nothing else drains once the host is gone.
    fn close_plugin(&self, pane: &str, mut host: PluginHost) {
        host.pane.close();
        for effect in host.pane.take_effects() {
            if let Effect::Feeds { unsubscribe, .. } = effect {
                for (feed, params) in unsubscribe {
                    self.send(&ClientMsg::FeedUnsubscribe { req: None, pane: pane.into(), feed, params });
                }
            }
        }
    }

    fn request_plugin_source(&mut self, pane: &str) {
        if self.link != Link::Up {
            if let Some(host) = self.plugins.get_mut(pane) {
                host.pane.fail_message("no connection to wheeld");
            }
            return;
        }
        let req = self.req();
        if let Some(host) = self.plugins.get_mut(pane) {
            host.source_req = Some(req);
        }
        self.send(&ClientMsg::PluginSource { req: Some(req), pane: pane.into() });
    }

    /// Sends what pane `pane`'s script asked for since the last call: its feed changes, its actions
    /// and the text its buttons copy.
    pub(super) fn drain_plugin(&mut self, pane: &str) {
        loop {
            let Some(effects) = self.plugins.get_mut(pane).map(|h| h.pane.take_effects()) else { return };
            if effects.is_empty() {
                return;
            }
            for effect in effects {
                match effect {
                    Effect::Feeds { subscribe, unsubscribe } => {
                        for (feed, params) in unsubscribe {
                            self.send(&ClientMsg::FeedUnsubscribe { req: None, pane: pane.into(), feed, params });
                        }
                        for (feed, params) in subscribe {
                            // With no connection nothing is sent: the reconnect subscribes every feed the pane holds.
                            if self.link != Link::Up {
                                continue;
                            }
                            let req = self.req();
                            self.plugins.get_mut(pane).unwrap().feed_reqs.insert(req, (feed.clone(), params.clone()));
                            self.send(&ClientMsg::FeedSubscribe { req: Some(req), pane: pane.into(), feed, params });
                        }
                    }
                    Effect::Action { name, args } => {
                        if self.link == Link::Up {
                            let req = self.req();
                            self.plugins.get_mut(pane).unwrap().action_reqs.insert(req, name.clone());
                            self.send(&ClientMsg::PluginAction { req: Some(req), pane: pane.into(), action: name, args });
                        } else {
                            // Never from inside the script's own `update` or `view`: the loop above returned from it.
                            self.plugins.get_mut(pane).unwrap().pane.action_failed(&name, "no connection to wheeld");
                        }
                    }
                    Effect::Copy(text) => self.plugin_copied.push(text),
                }
            }
        }
    }

    /// A `plugin_source`, `feed_update`, `plugin_action` or `error` for a previewed plugin pane.
    /// Returns whether it was one; a `feed_update` for any other pane is left for the queue and
    /// the rest.
    pub fn on_plugin_message(&mut self, message: &ServerMsg) -> bool {
        if self.on_embed_message(message) {
            return true;
        }
        let pane = match message {
            ServerMsg::PluginSource { req, pane, chunk, source, modules, .. } => {
                let Some(host) = self.plugins.get_mut(pane) else { return false };
                if req.is_some() && *req == host.source_req {
                    host.source_req = None;
                }
                host.pane.load(source, modules.clone().into_iter().collect(), chunk);
                pane.clone()
            }
            ServerMsg::FeedUpdate { req, pane, feed, params, value, patch, stale } => {
                // Only a feed the script reads: the same pane may carry another reader's feed (the queue's).
                let Some(host) = self.plugins.get_mut(pane).filter(|h| {
                    req.is_some_and(|r| h.feed_reqs.contains_key(&r)) || h.pane.feeds().iter().any(|(f, p)| f == feed && wheel_luau::tree::json_eq(p, params))
                }) else { return false };
                if let Some(req) = req {
                    host.feed_reqs.remove(req);
                }
                host.pane.feed_update(feed, params, value.as_ref(), patch.as_ref(), *stale);
                pane.clone()
            }
            ServerMsg::PluginAction { req, pane, result, .. } => {
                let Some(host) = self.plugins.get_mut(pane) else { return false };
                let Some(action) = host.action_reqs.remove(req) else { return false };
                host.pane.action_result(&action, result.clone());
                pane.clone()
            }
            ServerMsg::Error { req: Some(req), message, .. } => {
                let Some((pane, host)) = self.plugins.iter_mut().find(|(_, h)| h.source_req == Some(*req) || h.feed_reqs.contains_key(req) || h.action_reqs.contains_key(req)) else { return false };
                if let Some(action) = host.action_reqs.remove(req) {
                    host.pane.action_failed(&action, message);
                } else if let Some((feed, params)) = host.feed_reqs.remove(req) {
                    host.pane.feed_failed(&feed, &params, message);
                } else {
                    host.source_req = None;
                    host.pane.fail_message(message);
                }
                pane.clone()
            }
            _ => return false,
        };
        self.drain_plugin(&pane);
        true
    }

    /// Puts what previewed panes' Buttons copied on the pasteboard, the last copy winning.
    pub(super) fn flush_plugin_copies(&mut self, cx: &mut App) {
        let last = self.plugin_copied.pop();
        self.plugin_copied.clear();
        if let Some(text) = last {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// `pane_reload` for a previewed pane: its script runs from the top with the source read again.
    pub fn reload_plugin(&mut self, pane: &str) -> bool {
        let Some(host) = self.plugins.get_mut(pane) else { return false };
        host.pane.restart();
        self.request_plugin_source(pane);
        true
    }

    /// The connection came up again: wheeld forgot every request and subscription with the old one,
    /// so each previewed pane asks for its script again, keeping its state, and subscribes again.
    pub(super) fn plugin_link_up(&mut self) {
        let panes: Vec<String> = self.plugins.keys().cloned().collect();
        for pane in panes {
            self.request_plugin_source(&pane);
            let feeds = self.plugins[&pane].pane.feeds();
            for (feed, params) in feeds {
                let req = self.req();
                self.plugins.get_mut(&pane).unwrap().feed_reqs.insert(req, (feed.clone(), params.clone()));
                self.send(&ClientMsg::FeedSubscribe { req: Some(req), pane: pane.clone(), feed, params });
            }
        }
    }

    /// The connection dropped: answers to what was asked on it never come.
    pub(super) fn plugin_link_down(&mut self, reason: &str) {
        for host in self.plugins.values_mut() {
            host.source_req = None;
            host.feed_reqs.clear();
            for action in std::mem::take(&mut host.action_reqs).into_values() {
                host.pane.action_failed(&action, "lost the connection to wheeld");
            }
            host.pane.fail_message(&format!("lost the connection to wheeld ({reason}), retrying"));
        }
    }

    /// The scripts of panes that run none any more end: a pane gone, of another kind, or an agent
    /// pane back on its native view.
    pub(super) fn prune_plugins(&mut self) {
        let gone: Vec<String> = self.plugins.keys().filter(|p| self.pane_runs(p).is_none()).cloned().collect();
        for pane in gone {
            self.end_plugin(&pane);
        }
    }

    // MARK: what the person (or a check) does

    /// Presses node `id` of previewed pane `pane` as its button would.
    pub fn plugin_press(&mut self, pane: &str, id: &str, cx: &mut App) -> bool {
        let Some(host) = self.plugins.get_mut(pane) else { return false };
        let copies = host.pane.tree.node(id).is_some_and(|n| n.props.get("action").and_then(|a| a.get("copy")).and_then(|c| c.get("text")).is_some_and(Json::is_string));
        let ok = host.pane.press(id);
        if ok && copies {
            host.copied.insert(id.to_owned(), std::time::Instant::now());
        }
        self.drain_plugin(pane);
        self.flush_plugin_copies(cx);
        ok
    }

    /// Writes control `id`'s value as the control would.
    pub fn plugin_set(&mut self, pane: &str, id: &str, value: Json, cx: &mut App) -> bool {
        let Some(host) = self.plugins.get_mut(pane) else { return false };
        let ok = host.pane.set(id, value);
        self.drain_plugin(pane);
        self.flush_plugin_copies(cx);
        ok
    }

    /// Raises the action at `key` of node `id` with the event's fields in `context`.
    pub fn plugin_raise(&mut self, pane: &str, id: &str, key: &str, context: &Json, cx: &mut App) -> bool {
        let Some(host) = self.plugins.get_mut(pane) else { return false };
        let ok = host.pane.raise(id, key, context);
        self.drain_plugin(pane);
        self.flush_plugin_copies(cx);
        ok
    }

    /// Hands the size command Canvas `key` measured to the script, which draws again when it changed.
    pub fn plugin_canvas_size(&mut self, pane: &str, key: &str, width: f64, height: f64) {
        let Some(host) = self.plugins.get_mut(pane) else { return };
        host.pane.canvas_size(key, width, height);
        self.drain_plugin(pane);
    }

    /// Opens or closes Modal sheet or Popover `id`, which SwiftUI keeps in the view's own state.
    pub fn plugin_toggle_open(&mut self, pane: &str, id: &str) {
        if let Some(h) = self.plugins.get_mut(pane) && !h.open.remove(id) {
            h.open.insert(id.to_owned());
        }
    }

    /// A header click on Table `id`'s column `key`: the page sorts when it has a `sortAction`,
    /// else the view does; the same column again flips the order.
    pub fn plugin_table_sort(&mut self, pane: &str, id: &str, key: &str, cx: &mut App) {
        let Some(host) = self.plugins.get_mut(pane) else { return };
        let Some(n) = host.pane.tree.node(id) else { return };
        if n.action("sortAction").is_empty() {
            let asc = !matches!(host.sorts.get(id), Some((k, true)) if k == key);
            host.sorts.insert(id.to_owned(), (key.to_owned(), asc));
            return;
        }
        let now = n.props.get("sort").and_then(Json::as_object);
        let same = now.and_then(|s| s.get("key")).and_then(Json::as_str) == Some(key);
        let asc = !(same && now.and_then(|s| s.get("asc")).and_then(Json::as_bool).unwrap_or(true));
        self.plugin_raise(pane, id, "sortAction", &serde_json::json!({"key": key, "asc": asc}), cx);
    }

    /// A click on row `row` (its own `id`) of Table `id`: with a `selectAction` it selects the
    /// row (⌘ adds or removes it) and a double click raises `rowAction`; without one a click
    /// raises `rowAction`.
    #[allow(clippy::too_many_arguments)] // mirrors the click's own fields, called from three sites
    pub fn plugin_table_click(&mut self, pane: &str, id: &str, row: &str, clicks: usize, toggle: bool, range: bool, cx: &mut App) {
        let Some(host) = self.plugins.get(pane) else { return };
        let Some(n) = host.pane.tree.node(id) else { return };
        if n.action("selectAction").is_empty() || clicks >= 2 {
            if !row.is_empty() {
                self.plugin_raise(pane, id, n.action("rowAction"), &serde_json::json!({"id": row}), cx);
            }
            return;
        }
        let anchor = host.anchors.get(id).cloned();
        let mut ids: Vec<String> = n.props.get("selected").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_str).map(str::to_owned).collect();
        let order: Vec<String> = super::plugin_controls::table_lines(&host.pane.tree, n, if n.action("sortAction").is_empty() { host.sorts.get(id) } else { None })
            .into_iter().filter_map(|l| match l { super::plugin_controls::Line::Row(r) => host.pane.tree.node(&r).map(|r| r.row_id.clone()), _ => None }).collect();
        if row.is_empty() {
            ids.clear();
        } else if let (true, Some(a), Some(b)) = (range, anchor.as_ref().and_then(|a| order.iter().position(|r| r == a)), order.iter().position(|r| r == row)) {
            ids = order[a.min(b)..=a.max(b)].to_vec();
        } else if toggle {
            match ids.iter().position(|i| i == row) { Some(i) => { ids.remove(i); } None => ids.push(row.to_owned()) }
        } else {
            ids = vec![row.to_owned()];
        }
        if !row.is_empty() && !range && let Some(h) = self.plugins.get_mut(pane) {
            h.anchors.insert(id.to_owned(), row.to_owned());
        }
        let Some(n) = self.plugins.get(pane).and_then(|h| h.pane.tree.node(id)) else { return };
        ids.sort();
        ids.dedup();
        let now: Vec<&str> = n.props.get("selected").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_str).collect();
        let mut was: Vec<&str> = now.clone();
        was.sort();
        if was != ids.iter().map(String::as_str).collect::<Vec<_>>() {
            self.plugin_raise(pane, id, "selectAction", &serde_json::json!({"ids": ids}), cx);
        }
    }

    /// Lines `range` of Table `id` in pane `pane`, for its list.
    pub(super) fn plugin_table_rows(&mut self, pane: &str, id: &str, range: Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let Some(host) = self.plugins.get(pane) else { return vec![] };
        let Some(n) = host.pane.tree.node(id) else { return vec![] };
        let width = host.drawn.get();
        let d = super::plugin_draw::Draw { pane, tree: &host.pane.tree, host, controls: &self.controls, symbols: &self.symbols, width };
        d.table_rows(n, range, cx)
    }

    /// Row `row`'s context menu in Table `table`: each `menu` item raises its own `action` on the row.
    pub(super) fn plugin_row_menu(&mut self, pane: &str, _table: &str, row: &str, cx: &mut Context<Self>) {
        let Some(r) = self.plugins.get(pane).and_then(|h| h.pane.tree.node(row)) else { return };
        let items: Vec<Entry> = r.props.get("menu").and_then(Json::as_array).into_iter().flatten().enumerate()
            .map(|(i, m)| Entry::new(m.get("label").map(wheel_luau::tree::text).unwrap_or_default(), false, Pick::Raise(row.to_owned(), format!("menu.{i}.action"), m.get("context").cloned().unwrap_or(Json::Object(Default::default()))))).collect();
        if !items.is_empty() {
            self.plugin_open_menu(pane, items, cx);
        }
    }

    /// Opens an AppKit menu of `items` at the pointer for pane `pane`; a pick runs its [`Pick`].
    pub(super) fn plugin_open_menu(&mut self, pane: &str, items: Vec<Entry>, cx: &mut Context<Self>) {
        if self.plugin_menu.dialogs.is_none() {
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            self.plugin_menu.dialogs = Some(crate::appkit::Dialogs::new(self.hidden, tx));
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                while let Some(answer) = rx.next().await {
                    if let crate::appkit::Answer::Menu(item) = answer && this.update(cx, |c, cx| { c.plugin_menu_chosen(&item, cx); cx.notify(); }).is_err() {
                        break;
                    }
                }
            }).detach();
        }
        let mut picks = vec![];
        flatten(&items, &mut picks);
        let ids: Vec<String> = (0..picks.len()).map(|i| i.to_string()).collect();
        let entries = menu_items(&items, &ids, &mut 0);
        self.plugin_menu.built = self.plugin_menu.dialogs.as_mut().unwrap().context_menu(&entries);
        self.plugin_menu.open = Some((pane.to_owned(), picks));
    }

    /// Item `item` (its index) of the open menu was picked.
    pub fn plugin_menu_chosen(&mut self, item: &str, cx: &mut App) {
        let Some((pane, items)) = self.plugin_menu.open.take() else { return };
        let Some(pick) = item.parse::<usize>().ok().and_then(|i| items.into_iter().nth(i)) else { return };
        match pick {
            Pick::Press(id) => { self.plugin_press(&pane, &id, cx); }
            Pick::Set(id, v) => { self.plugin_set(&pane, &id, v, cx); }
            Pick::Raise(id, key, fields) => { self.plugin_raise(&pane, &id, &key, &fields, cx); }
            Pick::Separator => {}
        }
    }

    /// Draws previewed pane `pane` at `width` instead of the pane's own, for a check; None goes back.
    pub fn plugin_preview_width(&mut self, pane: &str, width: Option<f32>) -> bool {
        let Some(host) = self.plugins.get_mut(pane) else { return false };
        host.width = width;
        true
    }

    /// Scrolls previewed pane `pane`'s page so `y` points of it lie above the top.
    pub fn plugin_preview_scroll(&self, pane: &str, y: f32) -> bool {
        let Some(host) = self.plugins.get(pane) else { return false };
        host.scroll.set_offset(gpui::point(px(0.), px(-y)));
        true
    }

    /// The `plugin` op on plugin pane `pane` (the debug server's `/plugin`, the check): `args.action`
    /// is `state`, `press`, `hover`, `set`, `raise`, `scroll`, `agent_send`, `stream_key` or
    /// `stream_click`, with `component`, `value`, `key`, `context`, `to` and `dy`. The reply is `ok`
    /// and the pane's state ([`Client::plugin_state`]), or `ok` false and a `message` when no plugin
    /// pane `pane` runs here.
    pub fn plugin_op(&mut self, pane: &str, args: &Json, cx: &mut Context<Self>) -> Json {
        if !self.plugins.contains_key(pane) {
            return serde_json::json!({"ok": false, "message": "no plugin pane"});
        }
        let text = |k: &str| args.get(k).and_then(Json::as_str).map(str::to_owned);
        let (component, value) = (text("component"), text("value"));
        let ok = match args.get("action").and_then(Json::as_str).unwrap_or("") {
            "press" => component.is_some_and(|c| self.plugin_press(pane, &c, cx)),
            "hover" => self.plugins.get_mut(pane).is_some_and(|h| h.pane.hover(component.as_deref())),
            "set" => match (component, args.get("value")) {
                (Some(c), Some(v)) => self.plugin_set(pane, &c, v.clone(), cx),
                _ => false,
            },
            "raise" => match (component, text("key")) {
                (Some(c), Some(k)) => self.plugin_raise(pane, &c, &k, args.get("context").unwrap_or(&Json::Null), cx),
                _ => false,
            },
            "scroll" => ScrollTo::from_args(args).is_some_and(|to| self.plugins[pane].scroll_to(to)),
            "agent_send" => match (component, value) {
                (Some(c), Some(v)) => self.plugin_agent_send(pane, &c, &v),
                _ => false,
            },
            "stream_key" => match (component, value) {
                (Some(c), Some(v)) => self.plugin_stream_touch(pane, &c, &wheel_gpui_core::native::StreamTouch::Type(v)),
                _ => false,
            },
            "stream_click" => component.is_some_and(|c| self.plugin_stream_touch(pane, &c, &wheel_gpui_core::native::StreamTouch::Click)),
            _ => true,
        };
        cx.notify();
        let mut reply = self.plugin_state(pane);
        reply["ok"] = ok.into();
        reply
    }

    /// Plugin pane `pane` as the `plugin` op answers: the script's state, error, reloads,
    /// restarts, components and feeds, whether it fills the pane, the hover held and the nodes the
    /// last draw lit, the last render's cost, the pane's own scroll (null when it fills the pane or
    /// has not laid out) and its embeds.
    pub fn plugin_state(&self, pane: &str) -> Json {
        self.plugins.get(pane).map_or(Json::Null, |host| host_state(host, self.plugin_embed_states(pane)))
    }
}

/// [`Client::plugin_state`] for `host`, with its `embeds` as the client lists them.
fn host_state(host: &PluginHost, embeds: Vec<Json>) -> Json {
    let p = &host.pane;
    let load = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
    serde_json::json!({
        "state": p.state_json(), "error": p.error(), "reloads": p.reloads(), "restarts": p.restarts(),
        "components": p.component_texts(), "feeds": p.feed_json(), "fills": p.fills_pane(),
        "hover": {"held": p.tree.hover_held, "lit": host.lit.borrow().iter().collect::<Vec<_>>()},
        "perf": {"update_ms": p.perf.update_ms, "view_ms": p.perf.view_ms, "reconcile_ms": p.perf.reconcile_ms, "nodes": p.perf.nodes, "renders": p.perf.renders,
            "bodies": load(&super::plugin_draw::BODIES), "cells": load(&super::plugin_draw::CELLS)},
        "scroll": host.scroll_metrics().map(|m| m.json()),
        "embeds": embeds,
    })
}

impl Client {
    /// Plugin pane `pane` for a check: the `<plugin>/<pane>` it runs and the fields the `plugin` op's `state` answers with.
    pub fn plugin_preview_json(&self, pane: &str) -> Json {
        let Some(host) = self.plugins.get(pane) else { return Json::Null };
        let p = &host.pane;
        serde_json::json!({
            "runs": host.runs.0, "state": p.state_json(), "error": p.error(), "running": p.running(), "reloads": p.reloads(), "restarts": p.restarts(),
            "components": p.component_texts(), "feeds": p.feed_json(), "fills": p.fills_pane(), "cwd": p.context.get("cwd"),
            "fields": host.fields.iter().map(|(id, (_, _, text))| (id.clone(), Json::String(text.clone()))).collect::<serde_json::Map<_, _>>(),
            "open": host.open.iter().collect::<Vec<_>>(),
            "fits": host.fits.borrow().iter().map(|(id, f)| (id.clone(), serde_json::json!({"choice": f.choice, "width": f.width, "height": f.height}))).collect::<serde_json::Map<_, _>>(),
            "sorts": host.sorts.iter().map(|(id, (k, asc))| (id.clone(), serde_json::json!({"key": k, "asc": asc}))).collect::<serde_json::Map<_, _>>(),
            "tables": p.tree.all().filter(|n| n.component == "Table").map(|n| {
                let sort = if n.action("sortAction").is_empty() { host.sorts.get(&n.id) } else { None };
                let lines: Vec<Json> = super::plugin_controls::table_lines(&p.tree, n, sort).into_iter().map(|l| match l {
                    super::plugin_controls::Line::Title(t) => serde_json::json!({"title": t}),
                    super::plugin_controls::Line::Row(r) => serde_json::json!({"row": p.tree.node(&r).map(|r| r.row_id.clone())}),
                }).collect();
                (n.id.clone(), Json::Array(lines))
            }).collect::<serde_json::Map<_, _>>(),
            "pending_feeds": host.feed_reqs.values().map(|(f, p)| format!("{f} {p}")).collect::<Vec<_>>(),
            "canvases": host.canvases.borrow().iter().map(|(id, c)| (id.clone(), c.borrow().json())).collect::<serde_json::Map<_, _>>(),
            "embeds": self.plugin_embeds_json(pane),
            "menu": self.plugin_menu.open.as_ref().filter(|(p, _)| p == pane).map(|_| self.plugin_menu.built.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scroll_reads_dy_before_to() {
        assert_eq!(ScrollTo::from_args(&serde_json::json!({"dy": 100.0, "to": "top"})), Some(ScrollTo::By(100.)));
        assert_eq!(ScrollTo::from_args(&serde_json::json!({"dy": -3})), Some(ScrollTo::By(-3.)));
        assert_eq!(ScrollTo::from_args(&serde_json::json!({"to": "top"})), Some(ScrollTo::Top));
        assert_eq!(ScrollTo::from_args(&serde_json::json!({"to": "bottom"})), Some(ScrollTo::Bottom));
        assert_eq!(ScrollTo::from_args(&serde_json::json!({"to": "middle"})), None);
        assert_eq!(ScrollTo::from_args(&serde_json::json!({"dy": "100"})), None);
        assert_eq!(ScrollTo::from_args(&serde_json::json!({})), None);
    }

    #[test]
    fn a_scroll_stays_between_the_top_and_the_bottom() {
        assert_eq!(ScrollTo::Top.target(250., 800.), 0.);
        assert_eq!(ScrollTo::Bottom.target(0., 800.), 800.);
        assert_eq!(ScrollTo::By(100.).target(0., 800.), 100.);
        assert_eq!(ScrollTo::By(100.).target(750., 800.), 800.);
        assert_eq!(ScrollTo::By(-100.).target(40., 800.), 0.);
        // A page shorter than the pane does not scroll at all.
        assert_eq!(ScrollTo::Bottom.target(0., -5.), 0.);
        assert_eq!(ScrollTo::By(30.).target(0., 0.), 0.);
    }

    #[test]
    fn scroll_metrics_come_from_the_laid_out_scroll() {
        assert_eq!(ScrollMetrics::from_layout(-120., 900., 600.), Some(ScrollMetrics { offset: 120., content: 1500., visible: 600. }));
        assert_eq!(ScrollMetrics::from_layout(0., 0., 600.), Some(ScrollMetrics { offset: 0., content: 600., visible: 600. }));
        // Not laid out yet: SwiftUI has reported nothing either.
        assert_eq!(ScrollMetrics::from_layout(0., 0., 0.), None);
        let j = ScrollMetrics { offset: 1., content: 2., visible: 3. }.json();
        assert_eq!((j["offset"].as_f64(), j["content"].as_f64(), j["visible"].as_f64()), (Some(1.), Some(2.), Some(3.)));
    }

    #[test]
    fn the_state_reply_has_the_bridges_fields() {
        let mut host = PluginHost::default();
        host.pane.load(r#"return { view = function() return ui.Column{ ui.Button{ child = ui.Text("Go"), variant = "borderless" } } end }"#, Default::default(), "t.luau");
        host.lit.borrow_mut().insert("b".into());
        host.lit.borrow_mut().insert("a".into());
        assert!(host.pane.error().is_none(), "{:?}", host.pane.error());
        let reply = host_state(&host, vec![]);
        let keys: std::collections::BTreeSet<&str> = reply.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["components", "embeds", "error", "feeds", "fills", "hover", "perf", "reloads", "restarts", "scroll", "state"].into_iter().collect());
        assert_eq!(reply["hover"], serde_json::json!({"held": null, "lit": ["a", "b"]}));
        assert!(reply["scroll"].is_null(), "an unlaid page reports no scroll: {}", reply["scroll"]);
        for k in ["update_ms", "view_ms", "reconcile_ms", "nodes", "renders", "bodies", "cells"] {
            assert!(reply["perf"][k].is_number(), "perf.{k}: {}", reply["perf"]);
        }
        assert!(reply["state"].is_string() && reply["components"].is_array() && reply["feeds"].is_object(), "{reply}");
    }
}
