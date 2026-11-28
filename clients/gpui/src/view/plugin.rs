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
            self.unsubscribe_plugin(pane, &old);
        }
        self.request_plugin_source(pane);
        true
    }

    /// Ends the preview of `pane`: its feeds are dropped with wheeld.
    pub fn end_plugin_preview(&mut self, pane: &str) {
        if let Some(mut host) = self.plugin_previews.remove(pane) {
            host.pane.close();
            self.unsubscribe_plugin(pane, &host);
        }
    }

    fn unsubscribe_plugin(&self, pane: &str, host: &PluginHost) {
        for (feed, params) in host.pane.feeds() {
            self.send(&ClientMsg::FeedUnsubscribe { req: None, pane: pane.into(), feed, params });
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
    pub(super) fn drain_plugin(&mut self, pane: &str, cx: &mut App) {
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
                    Effect::Copy(text) => {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    }
                }
            }
        }
    }

    /// A `plugin_source`, `feed_update`, `plugin_action` or `error` for a previewed plugin pane.
    /// Returns whether it was one; a `feed_update` for any other pane is left for the queue and
    /// the rest.
    pub fn on_plugin_message(&mut self, message: &ServerMsg, cx: &mut App) -> bool {
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
                let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
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
        self.drain_plugin(&pane, cx);
        true
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
        self.drain_plugin(pane, cx);
        ok
    }

    /// Writes control `id`'s value as the control would.
    pub fn plugin_set(&mut self, pane: &str, id: &str, value: Json, cx: &mut App) -> bool {
        let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
        let ok = host.pane.set(id, value);
        self.drain_plugin(pane, cx);
        ok
    }

    /// Raises the action at `key` of node `id` with the event's fields in `context`.
    pub fn plugin_raise(&mut self, pane: &str, id: &str, key: &str, context: &Json, cx: &mut App) -> bool {
        let Some(host) = self.plugin_previews.get_mut(pane) else { return false };
        let ok = host.pane.raise(id, key, context);
        self.drain_plugin(pane, cx);
        ok
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
            "state": p.state_json(), "error": p.error(), "reloads": p.reloads(), "restarts": p.restarts(),
            "components": p.component_texts(), "feeds": p.feed_json(), "fills": p.fills_pane(),
            "fields": host.fields.iter().map(|(id, (_, _, text))| (id.clone(), Json::String(text.clone()))).collect::<serde_json::Map<_, _>>(),
        })
    }
}

