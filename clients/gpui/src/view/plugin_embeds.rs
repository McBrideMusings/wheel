//! A plugin pane's embeds drawn by GPUI (wheel-4d1o.5), as the Swift bridge draws them. `ui.Terminal`
//! and `ui.AgentChat` (`EmbedTerminal`, `EmbeddedAgentChat`) show the named pane's live terminal or
//! agent conversation; `ui.WebView` (`PaneWebViewView`) a WKWebView on an http or https address or
//! on an artifact's page wheeld keeps for the pane (`ArtifactEmbeds`, ADR-0047); `ui.Map`
//! (`PaneMapView`) an MKMapView; `ui.VideoStream` (`PaneVideoStreamView`, ADR-0053, wheel-4d1o.6)
//! the live stream wheeld names a socket for (`plugin_stream`), played in its slot by
//! `appkit::video_stream`. Each is as wide as it is offered and `height` tall (default 320; a Map
//! at least 160; a VideoStream with none the height the pane leaves), or shows a note saying why it
//! shows nothing (`PaneEmbedNote` in WheelUI's `Embed.swift`). Everything an embed shows goes under
//! its own key, `<host pane>#<node id>`: a terminal pane and agent body GPUI draws, or the native
//! slot a WebView, Map or VideoStream is placed in on the embed's drawn rect, cut to the page's
//! visible body.
use super::*;
use super::plugin_draw::{Draw, Env, SECONDARY, greedy};
use std::cell::Cell;
use std::time::{Duration, Instant};
use wheel_gpui_core::native::Embedded;
use wheel_luau::tree::ViewNode;

/// An embed's height when its node names none.
const HEIGHT: f32 = 320.;
/// A Map's least height (`.frame(minHeight: 160)`).
const MAP_HEIGHT: f32 = 160.;
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

/// Whether `component` shows in a native slot of its own.
fn is_native_embed(component: &str) -> bool {
    matches!(component, "WebView" | "Map" | "VideoStream")
}

/// The gain a `VideoStream`'s audio plays at, as `PaneVideoStreamView.gain`: `volume` held to 0...1
/// (1 when absent), 0 while `muted`.
pub(super) fn stream_gain(volume: Option<f64>, muted: Option<bool>) -> f32 {
    if muted == Some(true) { 0. } else { volume.unwrap_or(1.).clamp(0., 1.) as f32 }
}

/// Why a `VideoStream` naming `stream` shows only a note, when it does: it names none, or the
/// running wheeld (`has`: whether it handles `plugin_stream`, None before its offer came) cannot serve one.
pub(super) fn stream_note(stream: &str, has: Option<bool>) -> Option<String> {
    if stream.is_empty() {
        return Some("VideoStream names no stream".into());
    }
    (has == Some(false)).then(|| format!("This wheeld cannot serve a video stream; {stream} plays once it is updated"))
}

/// One embed's own state, kept by node id on its host while the node keeps its component.
pub struct Embed {
    pub component: String,
    /// Where the embed last drew, in window points: x, y, width, height.
    pub bounds: Rc<Cell<[f32; 4]>>,
    /// The frame (`Client::frames`) whose paint last drew the embed.
    pub drawn: Rc<Cell<Option<u64>>>,
    /// Whether the latest frame drew the embed, as counted when this frame began (`count`): an
    /// embed the page holds but did not draw (an unpicked tab, a closed sheet) shows nothing.
    pub counted: bool,
    /// A Terminal's body, a view of its own so every build of the node draws it.
    pub term: Option<gpui::Entity<EmbedTerm>>,
    /// A `WebView{artifact}`'s page, while the node names that artifact.
    pub artifact: Option<ArtifactEmbed>,
    /// A `VideoStream`'s socket, while the node names that stream.
    pub stream: Option<StreamAsk>,
}

impl Embed {
    fn new(component: &str) -> Self {
        Self { component: component.to_owned(), bounds: Rc::new(Cell::new([0.; 4])), drawn: Rc::default(), counted: false, term: None, artifact: None, stream: None }
    }

    /// Whether frame `frame`'s paint drew the embed.
    fn drawn_in(&self, frame: u64) -> bool {
        self.drawn.get() == Some(frame)
    }

    /// At the start of a frame, `frame` being the one painted last: count the embed as drawn when that paint drew it.
    fn count(&mut self, frame: u64) {
        self.counted = self.drawn_in(frame);
    }

    /// Where the embed drew in the latest frame, or nothing when that frame did not draw it.
    pub fn shown(&self) -> [f32; 4] {
        if self.counted { self.bounds.get() } else { [0.; 4] }
    }
}

/// What an embed's place shows, built before the tree draws and turned into an element by every
/// build of the node (`Draw::embed`): a ViewThatFits builds its children twice.
#[derive(Clone)]
pub(super) enum EmbedBody {
    /// Nothing GPUI draws: a native slot goes over it, or the terminal is not made yet.
    Empty,
    Note(String),
    /// An artifact embed's words while it has no page.
    Opening(String),
    Term(gpui::AnyView),
    /// An AgentChat's agent body, under the embed's key.
    Agent(String, gpui::AnyView),
}

/// A Terminal embed's terminal, drawn by the client under the embed's key.
pub struct EmbedTerm {
    client: gpui::WeakEntity<Client>,
    key: String,
    height: f32,
}

impl Render for EmbedTerm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (key, height) = (self.key.clone(), self.height);
        self.client.update(cx, |c, cx| c.term_element(&key, height, window, cx).into_any_element()).unwrap_or_else(|_| div().into_any_element())
    }
}

/// The page wheeld keeps for a `WebView{artifact}` (`plugin_embed`): the request in flight, then
/// the page id and where the name resolved, or wheeld's refusal.
#[derive(Debug, Default)]
pub struct ArtifactEmbed {
    pub name: String,
    pub req: Option<u64>,
    pub id: Option<String>,
    pub scope: Option<wheel_proto::ArtifactScope>,
    pub error: Option<String>,
}

/// The socket wheeld names for a `VideoStream`'s stream (`plugin_stream`): the request in flight,
/// the socket, or wheeld's refusal or why the last connection ended. A refusal is final, as the
/// bridge's; an ended connection is asked for again after `wait`, which doubles to 30 s and starts
/// over at 1 s once a connection has shown a picture.
#[derive(Debug)]
pub struct StreamAsk {
    pub name: String,
    pub req: Option<u64>,
    pub socket: Option<String>,
    pub error: Option<String>,
    /// When an ended connection is asked for again.
    pub retry_at: Option<Instant>,
    pub wait: Duration,
    /// Whether a timer will draw a frame at `retry_at`.
    pub timer: bool,
}

impl StreamAsk {
    fn new(name: &str) -> Self {
        Self { name: name.to_owned(), req: None, socket: None, error: None, retry_at: None, wait: Duration::from_secs(1), timer: false }
    }

    /// Whether wheeld should be asked for the socket now.
    fn due(&self, now: Instant) -> bool {
        self.socket.is_none() && self.req.is_none() && match self.retry_at {
            Some(at) => now >= at,
            None => self.error.is_none(),
        }
    }

    /// The connection to `socket` ended for `why`, `shown` whether it showed a picture: wait, then ask again.
    fn ended(&mut self, socket: &str, why: &str, shown: bool, now: Instant) -> bool {
        if self.socket.as_deref() != Some(socket) {
            return false;
        }
        if shown {
            self.wait = Duration::from_secs(1);
        }
        self.socket = None;
        self.error = Some(why.to_owned());
        self.retry_at = Some(now + self.wait);
        self.wait = (self.wait * 2).min(Duration::from_secs(30));
        true
    }
}

/// What a `WebView` shows, as `PaneWebViewView` chooses from its `url` and `artifact`.
#[derive(Debug, PartialEq)]
pub(super) enum WebChoice {
    Url(String),
    Artifact(String),
    Note(String),
}

pub(super) fn web_choice(url: &str, artifact: &str) -> WebChoice {
    if !artifact.is_empty() && !url.is_empty() {
        return WebChoice::Note("WebView names both a url and an artifact".into());
    }
    if !artifact.is_empty() {
        return WebChoice::Artifact(artifact.to_owned());
    }
    // `URL(string:)`'s scheme: letters first, then letters, digits, `+`, `-` and `.`, up to the first colon.
    let scheme = url.split_once(':').map(|(s, _)| s).filter(|s| s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)));
    if scheme.is_some_and(|s| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https")) {
        return WebChoice::Url(url.to_owned());
    }
    WebChoice::Note(if url.is_empty() { "WebView names no url".into() } else { format!("WebView shows only http and https addresses, not {url}") })
}

/// Where embed slot `bounds` goes on a page whose visible body is `body` (both window points), with
/// `holes` (the cards and bars over the host pane) cut out of it: the parts the page's scroll has
/// carried past the body's edges are cut too, and an embed with no part inside the body, or none
/// drawn yet, hides. No body (the host has no slot) hides it.
pub(super) fn place_embed(bounds: [f32; 4], body: Option<[f64; 4]>, holes: &[[f64; 4]], hidden: bool, alpha: f64) -> Placement {
    let b = bounds.map(f64::from);
    let Some(body) = body else { return (b, true, vec![], alpha) };
    let (x1, y1, bx1, by1) = (b[0] + b[2], b[1] + b[3], body[0] + body[2], body[1] + body[3]);
    let inside = b[2] > 0. && b[3] > 0. && b[0] < bx1 && x1 > body[0] && b[1] < by1 && y1 > body[1];
    let mut cut = holes.to_vec();
    if b[1] < body[1] {
        cut.push([b[0], b[1], b[2], body[1] - b[1]]);
    }
    if y1 > by1 {
        cut.push([b[0], by1, b[2], y1 - by1]);
    }
    if b[0] < body[0] {
        cut.push([b[0], b[1], body[0] - b[0], b[3]]);
    }
    if x1 > bx1 {
        cut.push([bx1, b[1], x1 - bx1, b[3]]);
    }
    (b, hidden || !inside, cut, alpha)
}

/// `ui.Map`'s markers: (title, latitude, longitude), a missing field empty or 0 as the bridge reads it.
fn markers(n: &ViewNode) -> Vec<(String, f64, f64)> {
    n.props.get("markers").and_then(serde_json::Value::as_array).into_iter().flatten().filter_map(serde_json::Value::as_object)
        .map(|m| {
            let num = |k: &str| m.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.);
            (m.get("title").and_then(serde_json::Value::as_str).unwrap_or("").to_owned(), num("latitude"), num("longitude"))
        })
        .collect()
}

/// An artifact embed's words while it has no page: "Opening <name>", or wheeld's refusal, in the
/// footnote style `ArtifactEmbedView` draws them in.
fn opening(text: String) -> gpui::Div {
    div().size_full().p(px(8.)).flex().items_center().justify_center().text_center().text_size(px(10.)).text_color(rgba(SECONDARY)).child(text)
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
        leaves.into_iter().filter(|p| p.leaf.kind == "plugin" && self.plugins.contains_key(&p.leaf.id)).map(|p| p.leaf.id.clone()).collect()
    }

    /// Every Terminal embed of `hosts` that resolves and has drawn: its key, its body (window points)
    /// and the term it shows.
    pub(super) fn embed_terms(&self, hosts: &[String]) -> Vec<(String, [f32; 4], String)> {
        let mut out = vec![];
        for (h, host) in hosts.iter().filter_map(|h| self.plugins.get(h).map(|host| (h, host))) {
            for n in host.pane.tree.all().filter(|n| n.component == "Terminal") {
                let (Ok(leaf), Some(embed)) = (self.embed_target(n), host.embeds.get(&n.id)) else { continue };
                let body = embed.shown();
                if let Some(term) = leaf.term.clone().filter(|_| body[2] > 0. && body[3] > 0.) {
                    out.push((embed_key(h, &n.id), body, term));
                }
            }
        }
        out
    }

    /// The agents `hosts`' drawn AgentChat embeds show, which are attached as an agent pane's are.
    pub(super) fn embed_agents(&self, hosts: &[String]) -> Vec<String> {
        hosts.iter().filter_map(|h| self.plugins.get(h))
            .flat_map(|host| host.pane.tree.all().filter(|n| n.component == "AgentChat" && host.embeds.get(&n.id).is_some_and(|e| e.counted)))
            .filter_map(|n| self.embed_target(n).ok().and_then(|l| l.agent.clone())).collect()
    }

    /// At the start of a frame: each embed counts as drawn only when the frame painted last drew it.
    pub(super) fn count_drawn_embeds(&mut self) {
        let frame = self.frames;
        for e in self.plugins.values_mut().flat_map(|h| h.embeds.values_mut()) {
            e.count(frame);
        }
    }

    /// After this frame paints: when it drew an embed the count left out, or left out one it
    /// counted (a tab picked away from it, a sheet closed over it), another frame counts again,
    /// so that embed's terminal detaches or its native view hides with nothing else to redraw.
    pub(super) fn recount_embeds_after_paint(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.plugins.values().all(|h| h.embeds.is_empty()) {
            return;
        }
        let client = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            client.update(cx, |c, cx| {
                let frame = c.frames;
                if c.plugins.values().flat_map(|h| h.embeds.values()).any(|e| e.counted != e.drawn_in(frame)) {
                    cx.notify();
                }
            }).ok();
        });
    }

    /// The pane or embed that holds the keyboard: an embed's key while it does, else the focused pane.
    pub(super) fn keyboard_target(&self) -> Option<String> {
        let pane = self.focus.current.as_ref()?;
        Some(match self.plugins.get(pane).and_then(|h| h.focused_embed.as_ref()) {
            Some(node) => embed_key(pane, node),
            None => pane.clone(),
        })
    }

    /// The pane AgentChat embed `key` shows, while it shows one.
    pub(super) fn embedded_agent_leaf(&self, key: &str) -> Option<&Leaf> {
        let (host, node) = key.split_once('#')?;
        let n = self.plugins.get(host)?.pane.tree.node(node).filter(|n| n.component == "AgentChat")?;
        self.embed_target(n).ok().filter(|l| l.agent.is_some())
    }

    /// Plugin pane `pane`'s embeds before its tree draws: each gets its state, and its body (the
    /// terminal, the agent's body or the note) is built here, where the client can be changed,
    /// for `Draw::embed` to place. `page_w` is the page's width, an AgentChat's until it has drawn.
    pub(super) fn sync_plugin_embeds(&mut self, pane: &str, page_w: f32, cx: &mut Context<Self>) {
        let Some(host) = self.plugins.get(pane) else { return };
        let nodes: Vec<(String, String, f32, Result<Leaf, String>)> = host.pane.tree.all().filter(|n| is_pane_embed(&n.component))
            .map(|n| (n.id.clone(), n.component.clone(), n.number("height").unwrap_or(f64::from(HEIGHT)) as f32, self.embed_target(n).cloned())).collect();
        let natives: Vec<(String, String, Option<WebChoice>, Option<String>)> = host.pane.tree.all().filter(|n| is_native_embed(&n.component))
            .map(|n| (n.id.clone(), n.component.clone(), (n.component == "WebView").then(|| web_choice(n.string("url").unwrap_or(""), n.string("artifact").unwrap_or(""))),
                (n.component == "VideoStream").then(|| n.string("stream").unwrap_or("").to_owned()))).collect();
        let host = self.plugins.get_mut(pane).unwrap();
        for (id, component) in nodes.iter().map(|(i, c, ..)| (i, c)).chain(natives.iter().map(|(i, c, ..)| (i, c))) {
            if host.embeds.get(id).is_none_or(|e| &e.component != component) {
                host.embeds.insert(id.clone(), Embed::new(component));
            }
        }
        let mut els = HashMap::new();
        for (id, _, choice, stream) in natives {
            if let Some(stream) = stream {
                els.insert(id.clone(), self.ask_stream(pane, &id, &stream, cx).map_or(EmbedBody::Empty, EmbedBody::Note));
                continue;
            }
            let el = match choice {
                Some(WebChoice::Note(text)) => EmbedBody::Note(text),
                Some(WebChoice::Artifact(name)) => match self.ask_artifact_embed(pane, &id, &name) {
                    Some(text) => EmbedBody::Opening(text),
                    None => EmbedBody::Empty,
                },
                // The native slot is placed over this rect (`sync_embed_slots`).
                _ => EmbedBody::Empty,
            };
            els.insert(id, el);
        }
        let client = cx.entity().downgrade();
        for (id, component, height, target) in nodes {
            let key = embed_key(pane, &id);
            let embed = self.plugins.get_mut(pane).unwrap().embeds.get_mut(&id).unwrap();
            let bounds = embed.shown();
            if component != "Terminal" || target.is_err() || !self.terms.panes.contains_key(&key) {
                embed.term = None;
            }
            let el = match target {
                Err(text) => EmbedBody::Note(text),
                // The terminal pane is made in `sync_terms` once the embed has drawn.
                Ok(_) if component == "Terminal" => match self.terms.panes.contains_key(&key) {
                    true => {
                        let view = embed.term.get_or_insert_with(|| cx.new(|_| EmbedTerm { client: client.clone(), key: key.clone(), height })).clone();
                        view.update(cx, |v, _| v.height = height);
                        EmbedBody::Term(view.into())
                    }
                    false => EmbedBody::Empty,
                },
                Ok(leaf) => {
                    let w = if bounds[2] > 0. { bounds[2] } else { page_w };
                    // The rect an agent pane would have for a body `w` × `height`.
                    let rect = Rect { x: bounds[0], y: bounds[1], w: w + 2. + 2. * PAD, h: height + TITLE + 2. * PAD };
                    EmbedBody::Agent(key.clone(), self.bodies.view(&Placed { leaf: Leaf { id: key, ..leaf }, rect }, (w, height), cx))
                }
            };
            els.insert(id, el);
        }
        self.plugins.get_mut(pane).unwrap().embed_els = els;
    }

    /// Previewed pane `pane`'s embeds for a check: each one's component, the pane it names, its note,
    /// the term or agent it shows, where it drew and, for a terminal, its grid and the last grid sent.
    pub(super) fn plugin_embeds_json(&self, pane: &str) -> serde_json::Value {
        let Some(host) = self.plugins.get(pane) else { return serde_json::Value::Null };
        let natives = host.pane.tree.all().filter(|n| is_native_embed(&n.component)).map(|n| {
            let key = embed_key(pane, &n.id);
            let embed = host.embeds.get(&n.id);
            let mut e = serde_json::json!({
                "component": n.component, "key": key, "bounds": embed.map(Embed::shown), "drawn": embed.is_some_and(|e| e.counted),
                "slot": self.embed_slots.get(&key).map(|_| self.native.embed_facts(&key)),
                "placed": self.embed_slots.get(&key).and_then(|(_, p)| p.as_ref()).map(|p| serde_json::json!({"rect": p.0, "hidden": p.1, "holes": p.2})),
            });
            if n.component == "VideoStream" {
                let name = n.string("stream").unwrap_or("");
                e["stream"] = name.into();
                e["input"] = n.bool("input").unwrap_or(true).into();
                e["fit"] = n.string("fit").into();
                e["gain"] = stream_gain(n.number("volume"), n.bool("muted")).into();
                e["note"] = stream_note(name, self.server_offer.as_ref().map(|o| o.has("plugin_stream"))).into();
                if let Some(a) = embed.and_then(|e| e.stream.as_ref()) {
                    e["socket"] = a.socket.clone().into();
                    e["error"] = a.error.clone().into();
                    e["waiting"] = a.req.is_some().into();
                }
                return (n.id.clone(), e);
            }
            if n.component == "Map" {
                e["asked"] = serde_json::json!({"latitude": n.number("latitude").unwrap_or(0.), "longitude": n.number("longitude").unwrap_or(0.), "span": n.number("span").unwrap_or(10.), "markers": markers(n).len()});
                return (n.id.clone(), e);
            }
            match web_choice(n.string("url").unwrap_or(""), n.string("artifact").unwrap_or("")) {
                WebChoice::Note(text) => e["note"] = text.into(),
                WebChoice::Url(url) => e["url"] = url.into(),
                WebChoice::Artifact(name) => {
                    e["artifact"] = name.into();
                    if let Some(a) = embed.and_then(|e| e.artifact.as_ref()) {
                        e["id"] = a.id.clone().into();
                        e["scope"] = a.scope.map(|s| s.name()).into();
                        e["error"] = a.error.clone().into();
                        e["waiting"] = a.req.is_some().into();
                    }
                }
            }
            (n.id.clone(), e)
        }).collect::<Vec<_>>();
        host.pane.tree.all().filter(|n| is_pane_embed(&n.component)).map(|n| {
            let target = self.embed_target(n);
            let key = embed_key(pane, &n.id);
            let bounds = host.embeds.get(&n.id).map(Embed::shown);
            let mut e = serde_json::json!({
                "component": n.component, "pane": n.string("pane").unwrap_or(""), "key": key,
                "note": target.as_ref().err(), "bounds": bounds, "drawn": host.embeds.get(&n.id).is_some_and(|e| e.counted),
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
        }).chain(natives).collect::<serde_json::Map<_, _>>().into()
    }

    /// Pane `pane`'s embeds as the bridge's `plugin` op lists them: each Terminal (`pane`, `term`,
    /// `bytes`), each AgentChat (`pane`, `agent`, `note`, `state`, `items`, `draft`, `error`), each
    /// artifact page (`artifact`, `loads`, `id`, `error`, `status`), then each stream with a slot
    /// (`stream`, `frames`, `audio`, `inputs`, `bytes`, `socket`, `error`, `video`).
    pub(super) fn plugin_embed_states(&self, pane: &str) -> Vec<serde_json::Value> {
        use serde_json::{Value, json};
        let Some(host) = self.plugins.get(pane) else { return vec![] };
        let nodes = |c: &'static str| host.pane.tree.all().filter(move |n| n.component == c);
        let mut out: Vec<Value> = nodes("Terminal").map(|n| {
            let term = self.embed_target(n).ok().and_then(|l| l.term.clone());
            let bytes = term.as_ref().and_then(|t| self.terms.models.get(t)).map_or(0, |m| m.received);
            json!({"pane": n.string("pane").unwrap_or(""), "term": term, "bytes": bytes})
        }).collect();
        out.extend(nodes("AgentChat").map(|n| {
            let target = self.embed_target(n);
            let agent = target.as_ref().ok().and_then(|l| l.agent.clone());
            let chat = agent.as_ref().and_then(|a| self.transcripts.get(a));
            json!({
                "pane": n.string("pane").unwrap_or(""), "agent": agent, "note": target.as_ref().err().cloned().unwrap_or_default(),
                "state": chat.map(|t| t.state.clone()), "items": chat.map_or(0, |t| t.rows.len()),
                "draft": agent.as_ref().and_then(|a| self.drafts.get(a)).cloned().unwrap_or_default(),
                "error": agent.as_ref().and_then(|a| self.composer_errors.get(a)),
            })
        }));
        let facts = |id: &str| self.native.embed_facts(&embed_key(pane, id));
        for n in nodes("WebView") {
            let Some(a) = host.embeds.get(&n.id).and_then(|e| e.artifact.as_ref()) else { continue };
            let f = facts(&n.id);
            let mut o = json!({"artifact": a.name, "loads": f.get("loads").cloned().unwrap_or(json!(0))});
            if let Some(id) = &a.id { o["id"] = json!(id); }
            if let Some(error) = &a.error { o["error"] = json!(error); }
            if let Some(status) = f.get("status").filter(|s| !s.is_null()) { o["status"] = status.clone(); }
            out.push(o);
        }
        for n in nodes("VideoStream") {
            let Some(Value::Object(f)) = facts(&n.id).get("stream").cloned() else { continue };
            out.push(f.into_iter().filter(|(k, _)| matches!(k.as_str(), "stream" | "frames" | "audio" | "inputs" | "bytes" | "socket" | "error" | "video")).collect());
        }
        out
    }

    /// The bridge's `agent_send`: the AgentChat embed of pane `pane` showing pane `named` sends
    /// `text` as its composer's Send does. False when no such embed shows an agent.
    pub(super) fn plugin_agent_send(&mut self, pane: &str, named: &str, text: &str) -> bool {
        let Some(host) = self.plugins.get(pane) else { return false };
        let agent = host.pane.tree.all().filter(|n| n.component == "AgentChat" && n.string("pane") == Some(named))
            .find_map(|n| self.embed_target(n).ok().and_then(|l| l.agent.clone()));
        let Some(agent) = agent else { return false };
        self.set_draft(&agent, text.to_owned());
        self.send_draft(&agent, false);
        true
    }

    /// The bridge's `stream_key` and `stream_click`: `touch` on the first VideoStream of pane `pane`
    /// playing `stream` that takes input and has a slot.
    pub(super) fn plugin_stream_touch(&self, pane: &str, stream: &str, touch: &wheel_gpui_core::native::StreamTouch) -> bool {
        let Some(host) = self.plugins.get(pane) else { return false };
        host.pane.tree.all().filter(|n| n.component == "VideoStream" && n.string("stream") == Some(stream) && n.bool("input").unwrap_or(true))
            .any(|n| self.native.stream_touch(&embed_key(pane, &n.id), touch))
    }

    /// The socket `VideoStream` node `node` of plugin pane `pane` names (`name`), asked of wheeld
    /// while the node is drawn and no socket, request or refusal stands (again after a connection
    /// ended, once its wait is over): Some the note shown in its place instead of the stream.
    fn ask_stream(&mut self, pane: &str, node: &str, name: &str, cx: &mut Context<Self>) -> Option<String> {
        let has = self.server_offer.as_ref().map(|o| o.has("plugin_stream"));
        if let Some(text) = stream_note(name, has) {
            return Some(text);
        }
        let up = self.link == Link::Up;
        let embed = self.plugins.get_mut(pane)?.embeds.get_mut(node)?;
        // Another stream is another session, as `VideoStreamHostView` is identified by its stream.
        if embed.stream.as_ref().is_none_or(|a| a.name != name) {
            embed.stream = Some(StreamAsk::new(name));
        }
        let counted = embed.counted;
        let a = embed.stream.as_mut()?;
        let now = Instant::now();
        if let Some(at) = a.retry_at.filter(|at| *at > now && !a.timer) {
            a.timer = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(at - Instant::now().min(at)).await;
                this.update(cx, |_, cx| cx.notify()).ok();
            }).detach();
        }
        // wheeld's offer comes with its first status; the socket is asked for then.
        if counted && up && has == Some(true) && a.due(now) {
            a.retry_at = None;
            a.timer = false;
            let req = self.req();
            let a = self.plugins.get_mut(pane)?.embeds.get_mut(node)?.stream.as_mut()?;
            a.req = Some(req);
            self.send(&ClientMsg::PluginStream { req: Some(req), pane: pane.to_owned(), stream: name.to_owned() });
        }
        None
    }

    /// The connection of stream embed `key` to `socket` ended for `why` (`shown`: after it showed a
    /// picture): its socket is asked for again after a wait.
    pub(super) fn stream_ended(&mut self, key: &str, socket: &str, why: &str, shown: bool) {
        let Some((host, node)) = key.split_once('#') else { return };
        if let Some(a) = self.plugins.get_mut(host).and_then(|h| h.embeds.get_mut(node)).and_then(|e| e.stream.as_mut())
            && a.ended(socket, why, shown, Instant::now())
        {
            self.log(format!("video stream {key}: {why}"));
        }
    }

    /// The artifact page `WebView` node `node` of plugin pane `pane` names (`name`), asked of wheeld
    /// once (again after a refusal only when the name changes): None once it has a page id to show,
    /// else the words shown in its place.
    fn ask_artifact_embed(&mut self, pane: &str, node: &str, name: &str) -> Option<String> {
        let lacking = self.server_offer.as_ref().map(|o| !o.has("plugin_embed"));
        let up = self.link == Link::Up;
        let embed = self.plugins.get_mut(pane)?.embeds.get_mut(node)?;
        // Another artifact is another page, as `ArtifactEmbedView` is identified by its name.
        if embed.artifact.as_ref().is_none_or(|a| a.name != name) {
            embed.artifact = Some(ArtifactEmbed { name: name.to_owned(), ..Default::default() });
        }
        let a = embed.artifact.as_mut().unwrap();
        if a.id.is_none() && a.req.is_none() && a.error.is_none() && up {
            match lacking {
                Some(true) => a.error = Some("This wheeld cannot show an artifact inside a page".into()),
                Some(false) => {
                    let req = self.next_req;
                    self.next_req += 1;
                    self.plugins.get_mut(pane)?.embeds.get_mut(node)?.artifact.as_mut()?.req = Some(req);
                    self.send(&ClientMsg::PluginEmbed { req: Some(req), pane: pane.to_owned(), artifact: name.to_owned() });
                }
                // wheeld's offer comes with its first status; the page is asked for then.
                None => {}
            }
        }
        let a = self.plugins.get(pane)?.embeds.get(node)?.artifact.as_ref()?;
        match (&a.error, &a.id) {
            (Some(error), _) => Some(error.clone()),
            (None, Some(_)) => None,
            (None, None) => Some(format!("Opening {name}")),
        }
    }

    /// wheeld's answer to a `plugin_embed`: the page to show, or why there is none. Returns whether it was one.
    pub(super) fn on_embed_message(&mut self, message: &ServerMsg) -> bool {
        let stream_req = match message {
            ServerMsg::PluginStream { req, .. } => Some(*req),
            ServerMsg::Error { req: Some(req), .. } => Some(*req),
            _ => None,
        };
        if let Some(req) = stream_req
            && let Some(a) = self.plugins.values_mut().flat_map(|h| h.embeds.values_mut()).filter_map(|e| e.stream.as_mut()).find(|a| a.req == Some(req))
        {
            a.req = None;
            match message {
                ServerMsg::PluginStream { socket, .. } => {
                    a.socket = Some(socket.clone());
                    a.error = None;
                }
                ServerMsg::Error { message, .. } => {
                    a.socket = None;
                    a.retry_at = None;
                    a.error = Some(message.clone());
                }
                _ => {}
            }
            return true;
        }
        let (req, answer) = match message {
            ServerMsg::PluginEmbed { req, scope, id, .. } => (*req, Ok((id.clone(), *scope))),
            ServerMsg::Error { req: Some(req), message, .. } => (*req, Err(message.clone())),
            _ => return false,
        };
        let Some(a) = self.plugins.values_mut().flat_map(|h| h.embeds.values_mut()).filter_map(|e| e.artifact.as_mut()).find(|a| a.req == Some(req)) else { return false };
        a.req = None;
        match answer {
            Ok((id, scope)) => {
                a.id = Some(id);
                a.scope = Some(scope);
                a.error = None;
            }
            Err(message) => {
                a.id = None;
                a.error = Some(message);
            }
        }
        true
    }

    /// A new connection: wheeld may have restarted and forgotten every embed's page, so each is asked for again.
    pub(super) fn embeds_link_up(&mut self) {
        // wheeld may have restarted its backends: every stream asks for its socket again on the next frame.
        for a in self.plugins.values_mut().flat_map(|h| h.embeds.values_mut()).filter_map(|e| e.stream.as_mut()) {
            a.req = None;
            a.socket = None;
            a.error = None;
            a.retry_at = None;
        }
        let asks: Vec<(String, String, String)> = self.plugins.iter()
            .flat_map(|(h, host)| host.embeds.iter().filter_map(move |(n, e)| e.artifact.as_ref().map(|a| (h.clone(), n.clone(), a.name.clone()))))
            .collect();
        for (pane, node, name) in asks {
            let req = self.req();
            if let Some(a) = self.plugins.get_mut(&pane).and_then(|h| h.embeds.get_mut(&node)).and_then(|e| e.artifact.as_mut()) {
                a.req = Some(req);
                a.error = None;
            }
            self.send(&ClientMsg::PluginEmbed { req: Some(req), pane, artifact: name });
        }
    }

    /// The connection dropped: no answer to a `plugin_embed` asked on it comes.
    pub(super) fn embeds_link_down(&mut self) {
        for a in self.plugins.values_mut().flat_map(|h| h.embeds.values_mut()).filter_map(|e| e.artifact.as_mut()) {
            a.req = None;
        }
        for a in self.plugins.values_mut().flat_map(|h| h.embeds.values_mut()).filter_map(|e| e.stream.as_mut()) {
            a.req = None;
        }
    }

    /// `pane_reload` of page `id`: every embed showing that artifact page opens it again.
    pub(super) fn reload_embedded_page(&self, id: &str) {
        for (key, _) in self.embed_slots.iter().filter(|(_, (what, _))| matches!(what, Embedded::Artifact { id: i, .. } if i == id)) {
            self.native.reload_embed(key);
        }
    }

    /// Each previewed page's WebView, Map and VideoStream in its native slot: opened with what it
    /// shows, placed on the rect it drew at in the last frame within its page's body (hidden while
    /// that frame did not draw it, while its host is not drawn, is hidden, or has a sheet or alert
    /// over its page), and closed once its node, its page or its host is gone. A VideoStream's slot
    /// is closed too while the frame did not draw it, as its SwiftUI view disappears, so its sound stops. `hosts` are the previewed panes drawn this frame.
    pub(super) fn sync_embed_slots(&mut self, hosts: &[String]) {
        let mut wanted: HashMap<String, (Embedded, Placement)> = HashMap::new();
        for (h, host) in &self.plugins {
            let cwd = host.pane.context.get("cwd").and_then(serde_json::Value::as_str).unwrap_or("");
            let page = self.placed.get(h);
            let hidden = !hosts.contains(h) || page.is_none_or(|p| p.1) || super::plugin_controls::page_overlaid(&host.pane.tree, host);
            for n in host.pane.tree.all() {
                let what = match n.component.as_str() {
                    "Map" => Embedded::Map {
                        latitude: n.number("latitude").unwrap_or(0.),
                        longitude: n.number("longitude").unwrap_or(0.),
                        span: n.number("span").unwrap_or(10.),
                        markers: markers(n),
                    },
                    "VideoStream" => {
                        let embed = host.embeds.get(&n.id);
                        let Some(a) = embed.filter(|e| e.counted).and_then(|e| e.stream.as_ref()) else { continue };
                        if stream_note(&a.name, self.server_offer.as_ref().map(|o| o.has("plugin_stream"))).is_some() {
                            continue;
                        }
                        Embedded::Stream {
                            stream: a.name.clone(),
                            socket: a.socket.clone(),
                            error: a.error.clone(),
                            input: n.bool("input").unwrap_or(true),
                            fills: n.string("fit") == Some("cover"),
                            gain: stream_gain(n.number("volume"), n.bool("muted")),
                        }
                    }
                    "WebView" => match web_choice(n.string("url").unwrap_or(""), n.string("artifact").unwrap_or("")) {
                        WebChoice::Url(url) => Embedded::Web { url },
                        WebChoice::Artifact(name) => match host.embeds.get(&n.id).and_then(|e| e.artifact.as_ref()).filter(|a| a.name == name && a.error.is_none()) {
                            Some(ArtifactEmbed { id: Some(id), scope: Some(scope), .. }) => {
                                Embedded::Artifact { id: id.clone(), name, scope: scope.name().into(), cwd: cwd.into(), ws: self.endpoint.ws.clone() }
                            }
                            _ => continue,
                        },
                        WebChoice::Note(_) => continue,
                    },
                    _ => continue,
                };
                let bounds = host.embeds.get(&n.id).map_or([0.; 4], Embed::shown);
                // The page's body as drawn (below the title row and inside the padding), not the pane's slot rect.
                let body = self.control_bounds(h, "body").map(|(x, y, w, b)| [f64::from(x), f64::from(y), f64::from(w), f64::from(b)]).or(page.map(|p| p.0));
                let placement = place_embed(bounds, body, page.map_or(&[][..], |p| &p.2), hidden, page.map_or(1., |p| p.3));
                wanted.insert(embed_key(h, &n.id), (what, placement));
            }
        }
        let gone: Vec<String> = self.embed_slots.keys().filter(|k| !wanted.contains_key(*k)).cloned().collect();
        for key in gone {
            self.native.close_embed(&key);
            self.embed_slots.remove(&key);
        }
        for (key, (what, placement)) in wanted {
            let slot = self.embed_slots.entry(key.clone()).or_insert_with(|| (what.clone(), None));
            if slot.1.is_none() || slot.0 != what {
                self.native.open_embed(&key, &what);
                slot.0 = what;
            }
            if slot.1.as_ref() != Some(&placement) {
                self.native.place(&key, placement.0, placement.1, &placement.2, placement.3);
                slot.1 = Some(placement);
            }
        }
    }
}

impl Draw<'_> {
    /// A Terminal, AgentChat, WebView, Map or VideoStream node: the body `sync_plugin_embeds` built,
    /// as wide as it is offered and `height` tall (a Map at least 160, a VideoStream with no
    /// `height` the height the pane leaves), recording where it drew so the next frame
    /// sizes the terminal or the agent to it, or places the native view on it.
    pub(super) fn embed(&self, n: &ViewNode, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let height = n.number("height").unwrap_or(f64::from(HEIGHT)) as f32;
        let body = match self.host.embed_els.get(&n.id).cloned().unwrap_or(EmbedBody::Empty) {
            EmbedBody::Empty => div().into_any_element(),
            EmbedBody::Note(text) => note(&text).into_any_element(),
            EmbedBody::Opening(text) => opening(text).into_any_element(),
            EmbedBody::Term(view) => view.into_any_element(),
            EmbedBody::Agent(key, view) => div().size_full().overflow_hidden().on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                // The host pane's own press would take the keyboard back from the embed.
                cx.stop_propagation();
                this.focus_pane(&key);
                this.menu = None;
                this.selection = None;
                this.invalidate_bodies(cx);
                cx.notify();
            })).child(view).into_any_element(),
        };
        let cells = self.host.embeds.get(&n.id).map(|e| (e.bounds.clone(), e.drawn.clone()));
        let client = cx.entity().downgrade();
        // Only the copy a frame paints gets here: a ViewThatFits' measured copies are laid out, never painted.
        let mark = canvas(move |b, _, cx| {
            let Some((cell, drawn)) = cells else { return };
            if let Some(c) = client.upgrade() {
                drawn.set(Some(c.read(cx).frames));
            }
            let now = [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)];
            if cell.get() != now {
                cell.set(now);
                cx.defer(move |cx| {
                    client.update(cx, |_, cx| cx.notify()).ok();
                });
            }
        }, |_, _, _, _| ()).absolute().top_0().left_0().size_full();
        let frame = greedy(div().relative().flex().flex_col(), outer);
        let frame = match (n.component.as_str(), n.number("height")) {
            ("Map", _) => frame.min_h(px(MAP_HEIGHT)),
            ("VideoStream", None) => frame.flex_1().min_h_0(),
            _ => frame.h(px(height)),
        };
        frame.overflow_hidden().child(mark).child(body).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: &str, term: Option<&str>, agent: Option<&str>) -> Leaf {
        Leaf { id: id.into(), kind: String::new(), label: String::new(), term: term.map(Into::into), url: None, scope: None, agent: agent.map(Into::into), harness: None, status: None, name: None, title: None, mirror: None, visited: vec![], quiet: false }
    }

    #[test]
    fn a_web_view_shows_a_web_address_an_artifact_or_why_not() {
        use WebChoice::*;
        assert_eq!(web_choice("https://example.com/a", ""), Url("https://example.com/a".into()));
        assert_eq!(web_choice("HTTP://127.0.0.1:8080/", ""), Url("HTTP://127.0.0.1:8080/".into()));
        assert_eq!(web_choice("", "notes"), Artifact("notes".into()));
        assert_eq!(web_choice("https://example.com", "notes"), Note("WebView names both a url and an artifact".into()));
        assert_eq!(web_choice("", ""), Note("WebView names no url".into()));
        assert_eq!(web_choice("file:///etc/hosts", ""), Note("WebView shows only http and https addresses, not file:///etc/hosts".into()));
        assert_eq!(web_choice("example.com", ""), Note("WebView shows only http and https addresses, not example.com".into()));
        assert_eq!(web_choice("httpx://a", ""), Note("WebView shows only http and https addresses, not httpx://a".into()));
    }

    #[test]
    fn a_streams_volume_is_its_gain_held_to_one_and_muted_is_silence() {
        assert_eq!(stream_gain(None, None), 1.);
        assert_eq!(stream_gain(Some(0.25), Some(false)), 0.25);
        assert_eq!(stream_gain(Some(0.25), Some(true)), 0.);
        assert_eq!(stream_gain(Some(3.), None), 1.);
        assert_eq!(stream_gain(Some(-1.), None), 0.);
    }

    #[test]
    fn a_stream_names_why_it_shows_only_a_note() {
        assert_eq!(stream_note("", Some(true)).as_deref(), Some("VideoStream names no stream"));
        assert_eq!(stream_note("gallery.demo", Some(true)), None);
        // Before wheeld's offer it waits, saying nothing.
        assert_eq!(stream_note("gallery.demo", None), None);
        assert!(stream_note("gallery.demo", Some(false)).is_some_and(|t| t.contains("gallery.demo")));
    }

    #[test]
    fn an_ended_stream_asks_again_after_a_wait_that_doubles() {
        let t0 = Instant::now();
        let mut a = StreamAsk::new("gallery.demo");
        assert!(a.due(t0));
        a.socket = Some("/s/demo.sock".into());
        assert!(!a.due(t0));
        // Another socket's end is an old connection's: nothing changes.
        assert!(!a.ended("/s/old.sock", "The stream ended", false, t0));
        assert!(a.ended("/s/demo.sock", "The stream ended", false, t0));
        assert!(!a.due(t0) && a.due(t0 + Duration::from_secs(1)));
        a.socket = Some("/s/demo.sock".into());
        a.ended("/s/demo.sock", "The stream ended", false, t0);
        assert_eq!(a.retry_at, Some(t0 + Duration::from_secs(2)));
        // A connection that showed a picture starts the wait over.
        a.socket = Some("/s/demo.sock".into());
        a.ended("/s/demo.sock", "The stream ended", true, t0);
        assert_eq!(a.retry_at, Some(t0 + Duration::from_secs(1)));
        // A refusal is final.
        let mut refused = StreamAsk::new("gallery.demo");
        refused.error = Some("plugin gallery serves no stream nope".into());
        assert!(!refused.due(t0 + Duration::from_secs(60)));
    }

    #[test]
    fn a_slot_is_cut_to_the_visible_page() {
        let body = Some([0., 100., 400., 300.]);
        // Inside the body: placed on its rect with the host's holes only.
        assert_eq!(place_embed([10., 150., 200., 100.], body, &[[1., 1., 1., 1.]], false, 1.), ([10., 150., 200., 100.], false, vec![[1., 1., 1., 1.]], 1.));
        // Scrolled half above the body: the part above is cut.
        assert_eq!(place_embed([10., 50., 200., 100.], body, &[], false, 1.), ([10., 50., 200., 100.], false, vec![[10., 50., 200., 50.]], 1.));
        // Below the body, not drawn yet, or the host hidden: hidden.
        assert!(place_embed([10., 450., 200., 100.], body, &[], false, 1.).1);
        assert!(place_embed([0., 0., 0., 0.], body, &[], false, 1.).1);
        assert!(place_embed([10., 150., 200., 100.], body, &[], true, 1.).1);
        assert!(place_embed([10., 150., 200., 100.], None, &[], false, 1.).1);
    }

    #[test]
    fn an_embed_shows_only_where_the_latest_frame_drew_it() {
        let mut e = Embed::new("WebView");
        e.bounds.set([10., 20., 300., 200.]);
        // Never painted: nothing, so its slot hides and its terminal does not attach.
        e.count(1);
        assert_eq!(e.shown(), [0.; 4]);
        // Painted in frame 2: its rect, once frame 2's paint is counted.
        e.drawn.set(Some(2));
        assert!(e.counted != e.drawn_in(2), "a newly drawn embed asks for another frame");
        e.count(2);
        assert_eq!(e.shown(), [10., 20., 300., 200.]);
        // Frame 3 did not draw it (another tab picked, its sheet closed): it keeps its old rect but shows nothing.
        assert!(e.counted != e.drawn_in(3), "an embed left undrawn asks for another frame");
        e.count(3);
        assert_eq!(e.shown(), [0.; 4]);
        assert_eq!(e.counted, e.drawn_in(4), "once counted again, nothing more is asked");
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
