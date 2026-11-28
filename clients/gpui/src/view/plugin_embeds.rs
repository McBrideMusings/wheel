//! A plugin pane's `ui.Terminal` and `ui.AgentChat` drawn by GPUI (wheel-4d1o.5), as the Swift
//! bridge's `EmbedTerminal` and `EmbeddedAgentChat` draw them: the named pane's live terminal or
//! agent conversation, as wide as it is offered and `height` tall (default 320), or a note saying
//! why it shows nothing (`PaneEmbedNote` in WheelUI's `Embed.swift`). Each embed's terminal pane
//! and agent body go under its own key, `<host pane>#<node id>`, so it is drawn, sized, focused
//! and typed into as a pane of its own.
use super::*;
use super::plugin_draw::{Draw, Env, SECONDARY, greedy};
use std::cell::Cell;
use wheel_luau::tree::ViewNode;

/// An embed's height when its node names none.
const HEIGHT: f32 = 320.;
/// A note's dashed border: `.secondary` at half its opacity.
const NOTE_BORDER: u32 = 0xffffff46;

/// The key an embed's own terminal pane and agent body go under.
pub(super) fn embed_key(host: &str, node: &str) -> String {
    format!("{host}#{node}")
}

/// Whether `component` names an embed of another pane.
pub(super) fn is_pane_embed(component: &str) -> bool {
    matches!(component, "Terminal" | "AgentChat")
}

/// One embed's own state, kept by node id on its host while the node keeps its component.
pub struct Embed {
    pub component: String,
    /// Where the embed drew in the last frame, in window points: x, y, width, height.
    pub bounds: Rc<Cell<[f32; 4]>>,
}

/// What a `component` embed naming pane `pane` shows among `leaves`, as the bridge resolves it from
/// `pane_targets` (a pane holding neither a term nor an agent is not among them): Ok the pane, or Err
/// the note in its place. An AgentChat says nothing until the first listing has come (`known`).
pub(super) fn resolve<'a>(component: &str, pane: &str, mut leaves: impl Iterator<Item = &'a Leaf>, known: bool) -> Result<&'a Leaf, String> {
    if pane.is_empty() {
        return Err(format!("{component} names no pane"));
    }
    let leaf = leaves.find(|l| l.id == pane && (l.term.is_some() || l.agent.is_some()));
    if component == "Terminal" {
        return match leaf {
            Some(l) if l.term.is_some() => Ok(l),
            Some(_) => Err(format!("Pane {pane} has no terminal")),
            None => Err(format!("Pane {pane} is gone")),
        };
    }
    match leaf {
        Some(l) if l.agent.is_some() => Ok(l),
        _ if !known => Err(String::new()),
        Some(_) => Err(format!("Pane {pane} holds no agent")),
        None => Err(format!("Pane {pane} is gone")),
    }
}

/// Why an embed shows nothing, in the place it would have filled: 12 pt secondary text centred in a
/// dashed rounded border. An empty note draws nothing.
pub(super) fn note(text: &str) -> gpui::Div {
    if text.is_empty() {
        return div().size_full();
    }
    div().size_full().p(px(8.)).flex().items_center().justify_center().text_center().text_size(px(12.)).text_color(rgba(SECONDARY))
        .rounded(px(6.)).border_1().border_dashed().border_color(rgba(NOTE_BORDER)).child(text.to_owned())
}

impl Client {
    /// Whether the first listing since the connection came up has arrived, as the bridge's `targetsKnown`.
    fn embeds_known(&self) -> bool {
        !self.awaiting_layout && !self.spaces.is_empty()
    }

    /// What embed `n` resolves to now.
    fn embed_target(&self, n: &ViewNode) -> Result<&Leaf, String> {
        resolve(&n.component, n.string("pane").unwrap_or(""), self.spaces.iter().flat_map(|s| s.all()), self.embeds_known())
    }

    /// The previewed plugin panes among `leaves`, whose embeds draw this frame.
    pub(super) fn embed_hosts<'a>(&self, leaves: impl IntoIterator<Item = &'a Placed>) -> Vec<String> {
        leaves.into_iter().filter(|p| p.leaf.kind == "plugin" && self.plugin_previews.contains_key(&p.leaf.id)).map(|p| p.leaf.id.clone()).collect()
    }

    /// Every Terminal embed of `hosts` that resolves and has drawn: its key, its body (window points)
    /// and the term it shows.
    pub(super) fn embed_terms(&self, hosts: &[String]) -> Vec<(String, [f32; 4], String)> {
        let mut out = vec![];
        for (h, host) in hosts.iter().filter_map(|h| self.plugin_previews.get(h).map(|host| (h, host))) {
            for n in host.pane.tree.all().filter(|n| n.component == "Terminal") {
                let (Ok(leaf), Some(embed)) = (self.embed_target(n), host.embeds.get(&n.id)) else { continue };
                let body = embed.bounds.get();
                if let Some(term) = leaf.term.clone().filter(|_| body[2] > 0. && body[3] > 0.) {
                    out.push((embed_key(h, &n.id), body, term));
                }
            }
        }
        out
    }

    /// The agents `hosts`' AgentChat embeds show, which are attached as an agent pane's are.
    pub(super) fn embed_agents(&self, hosts: &[String]) -> Vec<String> {
        hosts.iter().filter_map(|h| self.plugin_previews.get(h)).flat_map(|host| host.pane.tree.all().filter(|n| n.component == "AgentChat"))
            .filter_map(|n| self.embed_target(n).ok().and_then(|l| l.agent.clone())).collect()
    }

    /// The pane or embed that holds the keyboard: an embed's key while it does, else the focused pane.
    pub(super) fn keyboard_target(&self) -> Option<String> {
        let pane = self.focus.current.as_ref()?;
        Some(match self.plugin_previews.get(pane).and_then(|h| h.focused_embed.as_ref()) {
            Some(node) => embed_key(pane, node),
            None => pane.clone(),
        })
    }

    /// The pane AgentChat embed `key` shows, while it shows one.
    pub(super) fn embedded_agent_leaf(&self, key: &str) -> Option<&Leaf> {
        let (host, node) = key.split_once('#')?;
        let n = self.plugin_previews.get(host)?.pane.tree.node(node).filter(|n| n.component == "AgentChat")?;
        self.embed_target(n).ok().filter(|l| l.agent.is_some())
    }

    /// Plugin pane `pane`'s embeds before its tree draws: each gets its state, and its body (the
    /// terminal, the agent's body or the note) is built here, where the client can be changed,
    /// for `Draw::embed` to place. `page_w` is the page's width, an AgentChat's until it has drawn.
    pub(super) fn sync_plugin_embeds(&mut self, pane: &str, page_w: f32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.plugin_previews.get(pane) else { return };
        let nodes: Vec<(String, String, f32, Result<Leaf, String>)> = host.pane.tree.all().filter(|n| is_pane_embed(&n.component))
            .map(|n| (n.id.clone(), n.component.clone(), n.number("height").unwrap_or(f64::from(HEIGHT)) as f32, self.embed_target(n).cloned())).collect();
        let host = self.plugin_previews.get_mut(pane).unwrap();
        for (id, component, ..) in &nodes {
            if host.embeds.get(id).is_none_or(|e| &e.component != component) {
                host.embeds.insert(id.clone(), Embed { component: component.clone(), bounds: Rc::new(Cell::new([0.; 4])) });
            }
        }
        let mut els = HashMap::new();
        for (id, component, height, target) in nodes {
            let key = embed_key(pane, &id);
            let bounds = self.plugin_previews[pane].embeds[&id].bounds.get();
            let el = match target {
                Err(text) => note(&text).into_any_element(),
                // The terminal pane is made in `sync_terms` once the embed has a size.
                Ok(_) if component == "Terminal" => match self.terms.panes.contains_key(&key) {
                    true => self.term_element(&key, height, window, cx).into_any_element(),
                    false => div().into_any_element(),
                },
                Ok(leaf) => {
                    let w = if bounds[2] > 0. { bounds[2] } else { page_w };
                    // The rect an agent pane would have for a body `w` × `height`.
                    let rect = Rect { x: bounds[0], y: bounds[1], w: w + 2. + 2. * PAD, h: height + TITLE + 2. * PAD };
                    let view = self.bodies.view(&Placed { leaf: Leaf { id: key.clone(), ..leaf }, rect }, (w, height), cx);
                    div().size_full().overflow_hidden().on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        // The host pane's own press would take the keyboard back from the embed.
                        cx.stop_propagation();
                        this.focus_pane(&key);
                        this.menu = None;
                        this.selection = None;
                        this.invalidate_bodies(cx);
                        cx.notify();
                    })).child(view).into_any_element()
                }
            };
            els.insert(id, el);
        }
        *self.plugin_previews.get_mut(pane).unwrap().embed_els.borrow_mut() = els;
    }

    /// Previewed pane `pane`'s embeds for a check: each one's component, the pane it names, its note,
    /// the term or agent it shows, where it drew and, for a terminal, its grid and the last grid sent.
    pub(super) fn plugin_embeds_json(&self, pane: &str) -> serde_json::Value {
        let Some(host) = self.plugin_previews.get(pane) else { return serde_json::Value::Null };
        host.pane.tree.all().filter(|n| is_pane_embed(&n.component)).map(|n| {
            let target = self.embed_target(n);
            let key = embed_key(pane, &n.id);
            let bounds = host.embeds.get(&n.id).map(|e| e.bounds.get());
            let mut e = serde_json::json!({
                "component": n.component, "pane": n.string("pane").unwrap_or(""), "key": key,
                "note": target.as_ref().err(), "bounds": bounds,
                "focused": self.keyboard_target().as_deref() == Some(key.as_str()),
            });
            match target {
                Ok(leaf) if n.component == "Terminal" => {
                    let p = self.terms.panes.get(&key);
                    e["term"] = leaf.term.clone().into();
                    e["attached"] = p.is_some_and(|p| self.terms.models.contains_key(&p.term)).into();
                    e["grid"] = p.map(|p| serde_json::json!([p.grid.0, p.grid.1])).unwrap_or_default();
                    e["sent"] = p.and_then(|p| self.terms.sent_grids.get(&p.term)).and_then(|g| g.last()).map(|g| serde_json::json!([g.0, g.1])).unwrap_or_default();
                    e["body"] = p.map(|p| serde_json::json!(p.body)).unwrap_or_default();
                    e["viewport"] = p.and_then(|p| self.terms.texts.get(&p.term)).cloned().into();
                }
                Ok(leaf) => {
                    e["agent"] = leaf.agent.clone().into();
                    e["attached"] = leaf.agent.as_ref().is_some_and(|a| self.attached.contains(a)).into();
                    e["draws"] = self.bodies.draws.get(&key).copied().into();
                }
                Err(_) => {}
            }
            (n.id.clone(), e)
        }).collect::<serde_json::Map<_, _>>().into()
    }
}

impl Draw<'_> {
    /// A Terminal or AgentChat node: the body `sync_plugin_embeds` built, as wide as it is offered and
    /// `height` tall, recording where it drew so the next frame sizes the terminal or the agent to it.
    pub(super) fn embed(&self, n: &ViewNode, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let height = n.number("height").unwrap_or(f64::from(HEIGHT)) as f32;
        let body = self.host.embed_els.borrow_mut().remove(&n.id).unwrap_or_else(|| div().into_any_element());
        let cell = self.host.embeds.get(&n.id).map(|e| e.bounds.clone());
        let client = cx.entity().downgrade();
        let mark = canvas(move |b, _, cx| {
            let Some(cell) = cell else { return };
            let now = [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)];
            if cell.get() != now {
                cell.set(now);
                cx.defer(move |cx| {
                    client.update(cx, |_, cx| cx.notify()).ok();
                });
            }
        }, |_, _, _, _| ()).absolute().top_0().left_0().size_full();
        greedy(div().relative().flex().flex_col(), outer).h(px(height)).overflow_hidden().child(mark).child(body).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: &str, term: Option<&str>, agent: Option<&str>) -> Leaf {
        Leaf { id: id.into(), kind: String::new(), label: String::new(), term: term.map(Into::into), url: None, scope: None, agent: agent.map(Into::into), harness: None, status: None, name: None, title: None, mirror: None, visited: vec![], quiet: false }
    }

    #[test]
    fn notes_follow_what_the_pane_holds() {
        let leaves = [leaf("t", Some("term-1"), None), leaf("a", None, Some("agent-1")), leaf("e", None, None)];
        let r = |c: &str, p: &str, known: bool| resolve(c, p, leaves.iter(), known).map(|l| l.id.clone());
        assert_eq!(r("Terminal", "", true), Err("Terminal names no pane".into()));
        assert_eq!(r("AgentChat", "", false), Err("AgentChat names no pane".into()));
        assert_eq!(r("Terminal", "t", true), Ok("t".into()));
        assert_eq!(r("Terminal", "a", true), Err("Pane a has no terminal".into()));
        assert_eq!(r("Terminal", "gone", true), Err("Pane gone is gone".into()));
        // A pane holding neither is not among the bridge's targets.
        assert_eq!(r("Terminal", "e", true), Err("Pane e is gone".into()));
        assert_eq!(r("AgentChat", "a", true), Ok("a".into()));
        assert_eq!(r("AgentChat", "t", true), Err("Pane t holds no agent".into()));
        assert_eq!(r("AgentChat", "gone", true), Err("Pane gone is gone".into()));
        // Before the first listing an AgentChat says nothing.
        assert_eq!(r("AgentChat", "gone", false), Err(String::new()));
        assert_eq!(r("AgentChat", "t", false), Err(String::new()));
    }
}
