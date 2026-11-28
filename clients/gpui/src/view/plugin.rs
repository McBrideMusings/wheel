//! A plugin pane's script run by wheel-luau and fed on this window's own wheeld connection
//! (ADR-0056, ADR-0052): the source, its feeds and its actions travel as `plugin_source`,
//! `feed_subscribe`, `feed_update` and `plugin_action`, and `plugin_draw.rs` draws the tree. Until
//! the cutover (wheel-4d1o.7) only the hidden check previews a pane this way (`preview_plugin`);
//! the app's plugin panes keep drawing in the Swift bridge.
use super::*;
use serde_json::Value as Json;
use wheel_luau::pane::{Effect, PluginPane};

/// One previewed plugin pane: its script and the requests it waits on.
#[derive(Default)]
pub struct PluginHost {
    pub pane: PluginPane,
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
    /// Each Terminal and AgentChat embed's state, by node id, and the embed holding the keyboard
    /// while the pane has it.
    pub(super) embeds: HashMap<String, super::plugin_embeds::Embed>,
    pub(super) focused_embed: Option<String>,
    /// The embeds' bodies built for this frame's draw (`sync_plugin_embeds`), taken as it places them.
    pub(super) embed_els: RefCell<HashMap<String, gpui::AnyElement>>,
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

impl PluginHost {
    /// Drops the view's own state for nodes the tree no longer holds, as SwiftUI's `@State` goes
    /// with its view: a node that comes back starts fresh.
    pub(super) fn prune_view_state(&mut self) {
        let tree = &self.pane.tree;
        self.open.retain(|id| tree.node(id).is_some());
        self.tabs.retain(|id, _| tree.node(id).is_some());
        self.sorts.retain(|id, _| tree.node(id).is_some());
        self.anchors.retain(|id, _| tree.node(id).is_some());
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

impl Client {
    /// Runs plugin pane `pane`'s script in this client: it asks wheeld for the source and draws the
    /// tree in the pane's slot under the Swift view. Only the hidden check calls this before the
    /// cutover. False when no space holds a plugin pane `pane`.
    pub fn preview_plugin(&mut self, pane: &str) -> bool {
        let Some((space, leaf)) = self.spaces.iter().find_map(|s| s.all().find(|l| l.id == pane && l.kind == "plugin").map(|l| (s, l))) else { return false };
        let mut host = PluginHost::default();
        // The context Bridge.swift's open gives the script's `view` as `data`.
        let mut ids = leaf.label.splitn(2, '/');
        let (plugin, plugin_pane) = (ids.next().map(str::to_owned), ids.next().map(str::to_owned));
        let cwd = (!space.cwd.is_empty()).then(|| space.cwd.clone());
        for (k, v) in [("pane", Some(pane.to_owned())), ("space", Some(space.id.clone())), ("cwd", cwd), ("plugin", plugin), ("plugin_pane", plugin_pane)] {
            host.pane.context.insert(k.into(), v.map_or(Json::Null, Json::String));
        }
        if let Some(old) = self.plugin_previews.insert(pane.to_owned(), host) {
            self.close_plugin(pane, old);
        }
        self.request_plugin_source(pane);
        true
    }

    /// Ends the preview of `pane`: its feeds are dropped with wheeld.
    pub fn end_plugin_preview(&mut self, pane: &str) {
        if let Some(host) = self.plugin_previews.remove(pane) {
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
            if let Some(host) = self.plugin_previews.get_mut(pane) {
                host.pane.fail_message("no connection to wheeld");
            }
            return;
        }
        let req = self.req();
        if let Some(host) = self.plugin_previews.get_mut(pane) {
            host.source_req = Some(req);
        }
        self.send(&ClientMsg::PluginSource { req: Some(req), pane: pane.into() });
    }

    /// Sends what pane `pane`'s script asked for since the last call: its feed changes, its actions
    /// and the text its buttons copy.
    pub(super) fn drain_plugin(&mut self, pane: &str) {
        loop {
            let Some(effects) = self.plugin_previews.get_mut(pane).map(|h| h.pane.take_effects()) else { return };
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
                            self.plugin_previews.get_mut(pane).unwrap().feed_reqs.insert(req, (feed.clone(), params.clone()));
                            self.send(&ClientMsg::FeedSubscribe { req: Some(req), pane: pane.into(), feed, params });
                        }
                    }
                    Effect::Action { name, args } => {
                        if self.link == Link::Up {
                            let req = self.req();
                            self.plugin_previews.get_mut(pane).unwrap().action_reqs.insert(req, name.clone());
                            self.send(&ClientMsg::PluginAction { req: Some(req), pane: pane.into(), action: name, args });
                        } else {
                            // Never from inside the script's own `update` or `view`: the loop above returned from it.
                            self.plugin_previews.get_mut(pane).unwrap().pane.action_failed(&name, "no connection to wheeld");
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
        let pane = match message {
            ServerMsg::PluginSource { req, pane, chunk, source, modules, .. } => {
                let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
                if req.is_some() && *req == host.source_req {
                    host.source_req = None;
                }
                host.pane.load(source, modules.clone().into_iter().collect(), chunk);
                pane.clone()
            }
            ServerMsg::FeedUpdate { req, pane, feed, params, value, patch, stale } => {
                // Only a feed the script reads: the same pane may carry another reader's feed (the queue's).
                let Some(host) = self.plugin_previews.get_mut(pane).filter(|h| {
                    req.is_some_and(|r| h.feed_reqs.contains_key(&r)) || h.pane.feeds().iter().any(|(f, p)| f == feed && wheel_luau::tree::json_eq(p, params))
                }) else { return false };
                if let Some(req) = req {
                    host.feed_reqs.remove(req);
                }
                host.pane.feed_update(feed, params, value.as_ref(), patch.as_ref(), *stale);
                pane.clone()
            }
            ServerMsg::PluginAction { req, pane, result, .. } => {
                let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
                let Some(action) = host.action_reqs.remove(req) else { return false };
                host.pane.action_result(&action, result.clone());
                pane.clone()
            }
            ServerMsg::Error { req: Some(req), message, .. } => {
                let Some((pane, host)) = self.plugin_previews.iter_mut().find(|(_, h)| h.source_req == Some(*req) || h.feed_reqs.contains_key(req) || h.action_reqs.contains_key(req)) else { return false };
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
    pub fn reload_plugin_preview(&mut self, pane: &str) -> bool {
        let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
        host.pane.restart();
        self.request_plugin_source(pane);
        true
    }

    /// The connection came up again: wheeld forgot every request and subscription with the old one,
    /// so each previewed pane asks for its script again, keeping its state, and subscribes again.
    pub(super) fn plugin_link_up(&mut self) {
        let panes: Vec<String> = self.plugin_previews.keys().cloned().collect();
        for pane in panes {
            self.request_plugin_source(&pane);
            let feeds = self.plugin_previews[&pane].pane.feeds();
            for (feed, params) in feeds {
                let req = self.req();
                self.plugin_previews.get_mut(&pane).unwrap().feed_reqs.insert(req, (feed.clone(), params.clone()));
                self.send(&ClientMsg::FeedSubscribe { req: Some(req), pane: pane.clone(), feed, params });
            }
        }
    }

    /// The connection dropped: answers to what was asked on it never come.
    pub(super) fn plugin_link_down(&mut self, reason: &str) {
        for host in self.plugin_previews.values_mut() {
            host.source_req = None;
            host.feed_reqs.clear();
            for action in std::mem::take(&mut host.action_reqs).into_values() {
                host.pane.action_failed(&action, "lost the connection to wheeld");
            }
            host.pane.fail_message(&format!("lost the connection to wheeld ({reason}), retrying"));
        }
    }

    /// Previews of panes no space holds as plugin panes any more end.
    pub(super) fn prune_plugin_previews(&mut self) {
        let gone: Vec<String> = self.plugin_previews.keys().filter(|p| !self.spaces.iter().any(|s| s.all().any(|l| &l.id == *p && l.kind == "plugin"))).cloned().collect();
        for pane in gone {
            self.end_plugin_preview(&pane);
        }
    }

    // MARK: what the person (or a check) does

    /// Presses node `id` of previewed pane `pane` as its button would.
    pub fn plugin_press(&mut self, pane: &str, id: &str, cx: &mut App) -> bool {
        let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
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
        let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
        let ok = host.pane.set(id, value);
        self.drain_plugin(pane);
        self.flush_plugin_copies(cx);
        ok
    }

    /// Raises the action at `key` of node `id` with the event's fields in `context`.
    pub fn plugin_raise(&mut self, pane: &str, id: &str, key: &str, context: &Json, cx: &mut App) -> bool {
        let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
        let ok = host.pane.raise(id, key, context);
        self.drain_plugin(pane);
        self.flush_plugin_copies(cx);
        ok
    }

    /// Hands the size command Canvas `key` measured to the script, which draws again when it changed.
    pub fn plugin_canvas_size(&mut self, pane: &str, key: &str, width: f64, height: f64) {
        let Some(host) = self.plugin_previews.get_mut(pane) else { return };
        host.pane.canvas_size(key, width, height);
        self.drain_plugin(pane);
    }

    /// Opens or closes Modal sheet or Popover `id`, which SwiftUI keeps in the view's own state.
    pub fn plugin_toggle_open(&mut self, pane: &str, id: &str) {
        if let Some(h) = self.plugin_previews.get_mut(pane) && !h.open.remove(id) {
            h.open.insert(id.to_owned());
        }
    }

    /// A header click on Table `id`'s column `key`: the page sorts when it has a `sortAction`,
    /// else the view does; the same column again flips the order.
    pub fn plugin_table_sort(&mut self, pane: &str, id: &str, key: &str, cx: &mut App) {
        let Some(host) = self.plugin_previews.get_mut(pane) else { return };
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
    pub fn plugin_table_click(&mut self, pane: &str, id: &str, row: &str, clicks: usize, toggle: bool, range: bool, cx: &mut App) {
        let Some(host) = self.plugin_previews.get(pane) else { return };
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
        if !row.is_empty() && !range && let Some(h) = self.plugin_previews.get_mut(pane) {
            h.anchors.insert(id.to_owned(), row.to_owned());
        }
        let Some(n) = self.plugin_previews.get(pane).and_then(|h| h.pane.tree.node(id)) else { return };
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
        let Some(host) = self.plugin_previews.get(pane) else { return vec![] };
        let Some(n) = host.pane.tree.node(id) else { return vec![] };
        let width = host.drawn.get();
        let d = super::plugin_draw::Draw { pane, tree: &host.pane.tree, host, controls: &self.controls, symbols: &self.symbols, width };
        d.table_rows(n, range, cx)
    }

    /// Row `row`'s context menu in Table `table`: each `menu` item raises its own `action` on the row.
    pub(super) fn plugin_row_menu(&mut self, pane: &str, _table: &str, row: &str, cx: &mut Context<Self>) {
        let Some(r) = self.plugin_previews.get(pane).and_then(|h| h.pane.tree.node(row)) else { return };
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
        let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
        host.width = width;
        true
    }

    /// Scrolls previewed pane `pane`'s page so `y` points of it lie above the top.
    pub fn plugin_preview_scroll(&self, pane: &str, y: f32) -> bool {
        let Some(host) = self.plugin_previews.get(pane) else { return false };
        host.scroll.set_offset(gpui::point(px(0.), px(-y)));
        true
    }

    /// Previewed pane `pane` for a check: the fields the bridge's `plugin` `state` answers with.
    pub fn plugin_preview_json(&self, pane: &str) -> Json {
        let Some(host) = self.plugin_previews.get(pane) else { return Json::Null };
        let p = &host.pane;
        serde_json::json!({
            "state": p.state_json(), "error": p.error(), "running": p.running(), "reloads": p.reloads(), "restarts": p.restarts(),
            "components": p.component_texts(), "feeds": p.feed_json(), "fills": p.fills_pane(),
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

