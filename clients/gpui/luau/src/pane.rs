//! A plugin pane's state machine (ADR-0041, ADR-0052): the Luau script's `view` reconciled into
//! a [`ViewTree`] after every send or reload. Every table the script puts where a component
//! raises one is sent to the script's `update` when the component raises it, with the
//! component's own fields for the event added where the table has no such key. A control with an
//! `on_change` table sends it with `value` set to what the control wrote. `view` gets `context`
//! (the pane's ids and directory) as its `data`.
//!
//! A failure keeps the last good view and sets `error`; a good load clears it. A view with a
//! duplicate `key` among siblings is drawn with the first of them and also sets `error`.
//!
//! What the pane asks of its host (feed subscriptions, `wheel.action` calls, pasteboard text)
//! collects in an outbox the host drains with [`PluginPane::take_effects`] after each call.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use serde_json::{Map, Value as Json};

use crate::tree::{Raised, ViewNode, ViewTree, canonical, text};
use crate::{FeedRead, Failure, Host, Limits, PaneRuntime};

/// What the pane asks its host to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Feeds to subscribe and to unsubscribe with wheeld, each as (feed, params).
    Feeds { subscribe: Vec<(String, Json)>, unsubscribe: Vec<(String, Json)> },
    /// The script called `wheel.action(name, args)`: the host sends it to wheeld and answers
    /// with [`PluginPane::action_result`] or [`PluginPane::action_failed`].
    Action { name: String, args: Json },
    /// Text a Button's `{copy = {text}}` puts on the pasteboard.
    Copy(String),
}

/// What the last render cost, for a check.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Perf {
    pub update_ms: f64,
    pub view_ms: f64,
    pub reconcile_ms: f64,
    pub nodes: usize,
    pub renders: usize,
}

/// A feed read by name and params; params are compared as canonical JSON text.
type FeedKey = (String, String);

#[derive(Default)]
struct Feeds {
    /// The feeds subscribed with wheeld, each with its params as the script gave them.
    subscribed: HashMap<FeedKey, Json>,
    /// The value held for each subscribed feed.
    values: HashMap<FeedKey, Json>,
    /// The subscribed feeds whose backend is down: their held value is the last one it sent.
    stale: HashSet<FeedKey>,
    /// The feeds the render in progress has read.
    asked: HashMap<FeedKey, Json>,
}

fn key(name: &str, params: &Json) -> FeedKey {
    (name.to_owned(), canonical(params))
}

/// Params are an object; a Luau `{}` reaches here as an empty array, and none as null.
fn params_of(given: &Json) -> Json {
    match given {
        Json::Array(a) if a.is_empty() => Json::Object(Map::new()),
        _ => given.clone(),
    }
}

pub struct PluginPane {
    pub tree: ViewTree,
    /// The last failure as one readable line.
    error: Option<String>,
    /// Reloads that took the new source, not counting the first load.
    reloads: usize,
    /// Times `restart` ended the script.
    restarts: usize,
    /// The script's file name as wheeld sent it, in error messages.
    chunk: String,
    pub perf: Perf,
    runtime: Option<PaneRuntime>,
    /// Whether `error` came from loading the script: the file is still broken, so a message the
    /// script handles does not clear it.
    load_failed: bool,
    /// The fields `view` gets in `data` besides `feed`: `pane`, `space`, `cwd`, `plugin`,
    /// `plugin_pane`.
    pub context: Map<String, Json>,
    pub limits: Limits,
    feeds: Rc<RefCell<Feeds>>,
    outbox: Rc<RefCell<Vec<Effect>>>,
}

impl Default for PluginPane {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginPane {
    pub fn new() -> Self {
        Self {
            tree: ViewTree::new(),
            error: None,
            reloads: 0,
            restarts: 0,
            chunk: "pane".into(),
            perf: Perf::default(),
            runtime: None,
            load_failed: false,
            context: Map::new(),
            limits: Limits::default(),
            feeds: Rc::default(),
            outbox: Rc::default(),
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn reloads(&self) -> usize {
        self.reloads
    }

    pub fn restarts(&self) -> usize {
        self.restarts
    }

    pub fn chunk(&self) -> &str {
        &self.chunk
    }

    pub fn running(&self) -> bool {
        self.runtime.is_some()
    }

    pub fn fills_pane(&self) -> bool {
        self.tree.fills_pane()
    }

    /// What the pane asked of its host since the last call, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut *self.outbox.borrow_mut())
    }

    /// Takes the script's source and the package's other `.luau` files for its `require`: the
    /// first call starts the runtime, a later one reloads it with its state kept.
    pub fn load(&mut self, source: &str, modules: HashMap<String, String>, chunk: &str) {
        self.chunk = chunk.to_owned();
        let started = match self.runtime.as_mut() {
            Some(runtime) => runtime.reload(source, modules).map(|()| {
                self.reloads += 1;
            }),
            None => PaneRuntime::new(source, modules, chunk, self.limits, self.host()).map(|r| {
                self.runtime = Some(r);
            }),
        };
        match started.and_then(|()| self.render()) {
            Ok(()) => {
                self.error = None;
                self.load_failed = false;
            }
            Err(e) => {
                self.fail(&e);
                self.load_failed = true;
            }
        }
    }

    fn host(&self) -> Host {
        let feeds = self.feeds.clone();
        let outbox = self.outbox.clone();
        Host {
            feed: Box::new(move |name, given| {
                let params = params_of(given);
                let k = key(name, &params);
                let mut f = feeds.borrow_mut();
                let read = FeedRead { value: f.values.get(&k).cloned().unwrap_or(Json::Null), stale: f.stale.contains(&k) };
                f.asked.insert(k, params);
                read
            }),
            action: Box::new(move |name, args| outbox.borrow_mut().push(Effect::Action { name: name.to_owned(), args: args.clone() })),
            ..Host::default()
        }
    }

    /// Ends the running script and its state (`pane_reload`): the next `load` runs the script
    /// from the top. The last view stays drawn until then, and the feeds stay subscribed.
    pub fn restart(&mut self) {
        self.runtime = None;
        self.load_failed = false;
        self.tree.forget_given();
        self.restarts += 1;
    }

    /// Reports a failure that is not the script's (wheeld could not give the source).
    pub fn fail_message(&mut self, message: &str) {
        self.error = Some(one_line(message));
    }

    /// Takes a change to a feed the pane reads: the whole `value`, or an RFC 6902 `patch`
    /// applied to the value held. `stale` says the feed's backend is down. A patch that does not
    /// apply sets `error`, drops the held value and subscribes the feed again.
    pub fn feed_update(&mut self, feed: &str, params: &Json, value: Option<&Json>, patch: Option<&Json>, stale: bool) {
        let k = key(feed, &params_of(params));
        let applied = {
            let mut f = self.feeds.borrow_mut();
            if !f.subscribed.contains_key(&k) {
                return;
            }
            if stale {
                f.stale.insert(k.clone());
            } else {
                f.stale.remove(&k);
            }
            if let Some(patch) = patch {
                let mut doc = f.values.remove(&k).unwrap_or(Json::Null);
                let result = serde_json::from_value::<json_patch::Patch>(patch.clone()).map_err(|e| e.to_string()).and_then(|p| json_patch::patch(&mut doc, &p).map_err(|e| e.to_string()));
                if result.is_ok() {
                    f.values.insert(k.clone(), doc);
                }
                result
            } else {
                f.values.insert(k.clone(), value.cloned().unwrap_or(Json::Null));
                Ok(())
            }
        };
        if let Err(why) = applied {
            let held = self.feeds.borrow().subscribed.get(&k).cloned();
            if let Some(held) = held {
                self.outbox.borrow_mut().push(Effect::Feeds { subscribe: vec![(feed.to_owned(), held.clone())], unsubscribe: vec![(feed.to_owned(), held)] });
            }
            self.fail_message(&format!("feed {feed}: {why}"));
            return;
        }
        if self.runtime.is_none() {
            return;
        }
        match self.render() {
            Ok(()) if !self.load_failed => self.error = None,
            Ok(()) => {}
            Err(e) => self.fail(&e),
        }
    }

    /// wheeld refused a `feed_subscribe`: the feed is not held, so the next render subscribes it
    /// again, and `message` is shown.
    pub fn feed_failed(&mut self, feed: &str, params: &Json, message: &str) {
        let k = key(feed, &params_of(params));
        {
            let mut f = self.feeds.borrow_mut();
            f.subscribed.remove(&k);
            f.values.remove(&k);
            f.stale.remove(&k);
        }
        self.fail_message(message);
    }

    /// wheeld answered an action the script called: `update` gets `{action, result}`.
    pub fn action_result(&mut self, action: &str, result: Json) {
        let mut msg = Map::new();
        msg.insert("action".into(), Json::String(action.to_owned()));
        msg.insert("result".into(), result);
        self.deliver(&Json::Object(msg));
    }

    /// wheeld refused an action or its backend failed: `update` gets `{action, error}`.
    pub fn action_failed(&mut self, action: &str, message: &str) {
        let mut msg = Map::new();
        msg.insert("action".into(), Json::String(action.to_owned()));
        msg.insert("error".into(), Json::String(message.to_owned()));
        self.deliver(&Json::Object(msg));
    }

    /// The pane is going away: unsubscribes every feed it holds.
    pub fn close(&mut self) {
        let unsubscribe: Vec<(String, Json)> = {
            let mut f = self.feeds.borrow_mut();
            let out = f.subscribed.drain().map(|((name, _), p)| (name, p)).collect();
            f.values.clear();
            f.stale.clear();
            out
        };
        if !unsubscribe.is_empty() {
            self.outbox.borrow_mut().push(Effect::Feeds { subscribe: Vec::new(), unsubscribe });
        }
    }

    /// The feeds the pane reads now, as (feed, params), for a host that must subscribe them
    /// again.
    pub fn feeds(&self) -> Vec<(String, Json)> {
        let mut out: Vec<(String, Json)> = self.feeds.borrow().subscribed.iter().map(|((n, _), p)| (n.clone(), p.clone())).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The value held for the pane's subscription to `feed` (the first by params when it reads
    /// several), None until wheeld sends one.
    pub fn value_of(&self, feed: &str) -> Option<Json> {
        let f = self.feeds.borrow();
        f.subscribed.keys().filter(|k| k.0 == feed).min_by(|a, b| a.1.cmp(&b.1)).and_then(|k| f.values.get(k).cloned())
    }

    /// The held value of each subscribed feed as canonical JSON text, keyed `feed params`.
    pub fn feed_json(&self) -> BTreeMap<String, String> {
        let f = self.feeds.borrow();
        f.subscribed.keys().map(|k| (format!("{} {}", k.0, k.1), canonical(f.values.get(k).unwrap_or(&Json::Null)))).collect()
    }

    /// After a good render: subscribes the feeds it read that are new and unsubscribes those it
    /// no longer reads.
    fn sync_feeds(&mut self) {
        let (mut subscribe, mut unsubscribe) = {
            let mut f = self.feeds.borrow_mut();
            let subscribe: Vec<(String, Json)> = f.asked.iter().filter(|(k, _)| !f.subscribed.contains_key(*k)).map(|(k, p)| (k.0.clone(), p.clone())).collect();
            let gone: Vec<FeedKey> = f.subscribed.keys().filter(|k| !f.asked.contains_key(*k)).cloned().collect();
            if subscribe.is_empty() && gone.is_empty() {
                return;
            }
            let mut unsubscribe = Vec::new();
            for k in gone {
                f.values.remove(&k);
                f.stale.remove(&k);
                if let Some(p) = f.subscribed.get(&k) {
                    unsubscribe.push((k.0.clone(), p.clone()));
                }
            }
            f.subscribed = f.asked.clone();
            (subscribe, unsubscribe)
        };
        subscribe.sort_by(|a, b| a.0.cmp(&b.0));
        unsubscribe.sort_by(|a, b| a.0.cmp(&b.0));
        self.outbox.borrow_mut().push(Effect::Feeds { subscribe, unsubscribe });
    }

    fn deliver(&mut self, msg: &Json) {
        let Some(runtime) = self.runtime.as_mut() else { return };
        let start = Instant::now();
        let sent = runtime.send(msg);
        self.perf.update_ms = ms(start);
        match sent.and_then(|()| self.render()) {
            Ok(()) if !self.load_failed => self.error = None,
            Ok(()) => {}
            Err(e) => self.fail(&e),
        }
    }

    fn fail(&mut self, failure: &Failure) {
        let text = failure.to_string();
        let line = if text.contains(&self.chunk) { text } else { format!("{}: {text}", self.chunk) };
        self.error = Some(one_line(&line));
    }

    fn render(&mut self) -> Result<(), Failure> {
        let Some(runtime) = self.runtime.as_mut() else { return Ok(()) };
        self.feeds.borrow_mut().asked.clear();
        let start = Instant::now();
        let view = runtime.view(&Json::Object(self.context.clone()))?;
        self.perf.view_ms = ms(start);
        self.sync_feeds();
        let start = Instant::now();
        self.tree.reconcile(&view);
        self.perf.reconcile_ms = ms(start);
        self.perf.nodes = self.tree.written();
        self.perf.renders += 1;
        match self.tree.duplicate() {
            Some(at) => Err(Failure::View(format!("view: duplicate key {at}"))),
            None => Ok(()),
        }
    }

    // MARK: what the person (or a check) does

    /// Holds a pointer on node `id`, or takes it away with None. False when the view has no
    /// node `id`.
    pub fn hover(&mut self, id: Option<&str>) -> bool {
        if let Some(id) = id {
            if self.tree.node(id).is_none() {
                return false;
            }
        }
        self.tree.hover_held = id.map(str::to_owned);
        true
    }

    /// Presses the component as its button would; false when the view has no node `id`.
    pub fn press(&mut self, id: &str) -> bool {
        if self.tree.node(id).is_none() {
            return false;
        }
        match self.tree.activate(id) {
            Some(Raised::Copy(text)) => self.outbox.borrow_mut().push(Effect::Copy(text)),
            Some(Raised::Message(msg)) => self.deliver(&msg),
            None => {}
        }
        true
    }

    /// Writes a control's value as the control would; false when the view has no node `id`.
    pub fn set(&mut self, id: &str, value: Json) -> bool {
        if self.tree.node(id).is_none() {
            return false;
        }
        if let Some(msg) = self.tree.change(id, value) {
            self.deliver(&msg);
        }
        true
    }

    /// Raises the action the script put at `key` of node `id`, with the event's own fields in
    /// `context`; false when the last render put no action there.
    pub fn raise(&mut self, id: &str, key: &str, context: &Json) -> bool {
        let fields = context.as_object().cloned().unwrap_or_default();
        match self.tree.raise(id, key, &fields) {
            Some(msg) => {
                self.deliver(&msg);
                true
            }
            None => false,
        }
    }

    /// A command Canvas measured itself: the runtime keeps the size for `draw`, and the view is
    /// drawn again only when it changed.
    pub fn canvas_size(&mut self, key: &str, width: f64, height: f64) {
        let Some(runtime) = self.runtime.as_ref() else { return };
        if !runtime.set_canvas_size(key, width, height) {
            return;
        }
        match self.render() {
            Ok(()) if !self.load_failed => self.error = None,
            Ok(()) => {}
            Err(e) => self.fail(&e),
        }
    }

    // MARK: for a check

    /// The script's state as canonical JSON text.
    pub fn state_json(&self) -> String {
        self.runtime.as_ref().and_then(|r| r.state().ok()).map_or_else(|| "null".into(), |s| canonical(&s))
    }

    /// The drawn components in order, each as its id, type and text (a Text's text, a
    /// TextField's label and value). A Modal's `actions` follow its content, a Button carries
    /// `disabled` when a modifier disables it, and a Table adds one `TableRow` entry per row.
    pub fn component_texts(&self) -> Vec<BTreeMap<String, String>> {
        let mut out = Vec::new();
        if let Some(root) = self.tree.root() {
            self.walk(root, 0, &mut out);
        }
        out
    }

    fn walk(&self, n: &ViewNode, depth: usize, out: &mut Vec<BTreeMap<String, String>>) {
        if depth >= 64 {
            return;
        }
        let mut row = BTreeMap::from([("id".to_owned(), n.id.clone()), ("type".to_owned(), n.component.clone())]);
        if let Some(t) = n.string("text") {
            row.insert("text".into(), t.to_owned());
        }
        if let Some(l) = n.string("label") {
            row.insert("label".into(), l.to_owned());
        }
        if n.component == "TextField" {
            row.insert("value".into(), n.value().map(text).unwrap_or_default());
        }
        if n.component == "Button" && n.modifiers().any(|m| m.get("disabled").and_then(Json::as_bool) == Some(true)) {
            row.insert("disabled".into(), "true".into());
        }
        out.push(row);
        if n.component == "Table" {
            out.extend(self.table_rows(n));
        }
        for id in n.parts() {
            if let Some(c) = self.tree.node(id) {
                self.walk(c, depth + 1, out);
            }
        }
        let actions = n.props.get("actions").and_then(Json::as_array).map_or(0, Vec::len);
        for i in 0..actions {
            if let Some(a) = n.embedded.get(&format!("actions.{i}")).and_then(|r| self.tree.node(&r.id)) {
                self.walk(a, depth + 1, out);
            }
        }
    }

    fn table_rows(&self, table: &ViewNode) -> Vec<BTreeMap<String, String>> {
        let keys: Vec<&str> = table.props.get("columns").and_then(Json::as_array).into_iter().flatten().filter_map(|c| c.get("key").and_then(Json::as_str)).collect();
        let mut out = Vec::new();
        for sec in table.props.get("sections").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_object) {
            for r in sec.get("rows").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_str).filter_map(|id| self.tree.node(id)) {
                let cells: Vec<&Map<String, Json>> = r.props.get("cells").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_object).collect();
                let texts: Vec<String> = keys.iter().map(|k| cells.iter().find(|c| c.get("key").and_then(Json::as_str) == Some(k)).and_then(|c| c.get("text")).map(text).unwrap_or_default()).collect();
                let mut row = BTreeMap::from([
                    ("id".to_owned(), r.id[table.id.len() + 1..].to_owned()),
                    ("node".to_owned(), r.id.clone()),
                    ("type".to_owned(), "TableRow".to_owned()),
                    ("table".to_owned(), table.id.clone()),
                    ("cells".to_owned(), texts.join(" | ")),
                ]);
                if let Some(t) = sec.get("title").and_then(Json::as_str).filter(|t| !t.is_empty()) {
                    row.insert("section".into(), t.to_owned());
                }
                out.push(row);
            }
        }
        out
    }
}

fn one_line(text: &str) -> String {
    text.lines().find(|l| !l.is_empty()).unwrap_or(text).to_owned()
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
