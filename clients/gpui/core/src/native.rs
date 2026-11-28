//! Rust side of native/Bridge.swift: loads the bridge library and speaks its JSON command ABI.
//! The bridge makes each native pane's content (a WKWebView, a browser, a plugin pane's view) and
//! adds it to the slot view the window's [`Host`] made for the pane; the host places, masks and
//! focuses the slots. All calls run on the main thread. The bridge also speaks first: a
//! browser pane's state, its reports for wheeld and the answers to the daemon requests the view
//! relayed arrive as frames through the callback `attach` is given, each read as a [`Frame`].
use anyhow::{Context as _, Result};
use futures::channel::mpsc as fmpsc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_void},
    path::PathBuf,
    rc::Rc,
    sync::Mutex,
};
use wheel_proto::ConsoleEntry;

/// The bridge's callback: its window id and one JSON object.
type Emit = unsafe extern "C" fn(i32, *const c_char);

/// Where each bridge window's frames go: the channel `attach_to` registered for it.
static EMITS: Mutex<Option<HashMap<i32, fmpsc::UnboundedSender<String>>>> = Mutex::new(None);

unsafe extern "C" fn on_emit(window: i32, json: *const c_char) {
    // The bridge owns the string for the length of the call; it is copied here.
    let text = unsafe { CStr::from_ptr(json) }.to_string_lossy().into_owned();
    if let Some(sender) = EMITS.lock().unwrap().as_ref().and_then(|m| m.get(&window)) {
        let _ = sender.unbounded_send(text);
    }
}

/// The loaded bridge library, shared by every window.
pub struct Bridge {
    _library: libloading::Library,
    attach: unsafe extern "C" fn(*mut c_void, i32, Emit) -> i32,
    command: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(*mut c_void),
}
impl Bridge {
    /// `WHEEL_GPUI_BRIDGE`, else `libWheelGpuiBridge.dylib` beside the executable.
    pub fn load() -> Result<Rc<Self>> {
        let path = match std::env::var_os("WHEEL_GPUI_BRIDGE") {
            Some(path) => PathBuf::from(path),
            None => std::env::current_exe()?.with_file_name("libWheelGpuiBridge.dylib"),
        };
        // The ABI is declared in native/Bridge.swift; the library lives as long as this struct.
        unsafe {
            let library = libloading::Library::new(&path).with_context(|| format!("loading {}", path.display()))?;
            Ok(Rc::new(Self {
                attach: *library.get(b"wheel_native_attach")?,
                command: *library.get(b"wheel_native_command")?,
                free: *library.get(b"wheel_native_free")?,
                _library: library,
            }))
        }
    }
}

// Not `cfg(test)`: the app crate's tests call it, and a gate does not cross crates.
impl Bridge {
    /// A bridge that answers every command with `{}`, for a test that draws a client with no Swift side.
    pub fn stub() -> Rc<Self> {
        unsafe extern "C" fn attach(_: *mut c_void, _: i32, _: Emit) -> i32 {
            1
        }
        unsafe extern "C" fn command(_: *const c_char) -> *mut c_char {
            unsafe { libc::strdup(c"{}".as_ptr()) }
        }
        unsafe extern "C" fn free(p: *mut c_void) {
            unsafe { libc::free(p) }
        }
        Rc::new(Self { _library: libloading::os::unix::Library::this().into(), attach, command, free })
    }
}

/// A window's native pane host (`appkit::pane_host` in the app crate, which links AppKit): one
/// slot view per native pane in the window's content view, where it sits, what is cut out of it,
/// which view holds the keyboard, and the check's synthetic pointer. Rects are window points from
/// the top left.
pub trait Host {
    /// The slot view of `pane` as an address, made and added to the window on first ask.
    fn slot(&self, pane: &str) -> usize;
    fn place(&self, pane: &str, rect: [f64; 4], hidden: bool, holes: &[[f64; 4]], alpha: f64);
    /// Cut `rect` (the palette's, a sheet's) out of every slot, or close the cut with None.
    fn overlay(&self, rect: Option<[f64; 4]>);
    /// Give the keyboard to `pane`'s content view, else to the GPUI view; whether the window took it.
    fn focus(&self, pane: &str) -> bool;
    /// Remove `pane`'s slot and whatever is in it.
    fn close(&self, pane: &str);
    /// Drop every slot and the press monitor: the window is closing.
    fn detach(&self);
    /// A synthesized pointer event sent to the view under `point`, as AppKit routes one; the class of the view it reached.
    fn mouse(&self, mouse: &Mouse) -> Option<String>;
    /// The class of the view under `point` and the pane whose slot holds it.
    fn hit(&self, point: [f64; 2]) -> (Option<String>, Option<String>);
    /// Each slot's facts by pane (`frame_top`, `hidden`, `alpha`, `masked`, `holes`, `mask_rects`) and the overlay's rect.
    fn facts(&self) -> (serde_json::Map<String, Value>, Option<[f64; 4]>);
}

/// One synthesized pointer event (the check's and the debug route's `post_mouse`). A context menu
/// is asked for with `control` on a left press, as a Mac control-click asks for one.
#[derive(Debug, Clone, Deserialize)]
pub struct Mouse {
    pub kind: MouseKind,
    pub point: [f64; 2],
    #[serde(default)]
    pub control: bool,
    #[serde(default)]
    pub command: bool,
    #[serde(default = "one")]
    pub clicks: i64,
}
fn one() -> i64 {
    1
}
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseKind {
    Down,
    Drag,
    Move,
    Up,
}

/// The native views of one window. Every command names the window the bridge assigned at
/// `attach_to`; before that, commands answer `detached`.
pub struct Native {
    bridge: Rc<Bridge>,
    window: Cell<i32>,
    /// The window's content view `attach_to` was given (null before): where AppKit places the window.
    ns_view: Cell<usize>,
    host: std::cell::RefCell<Option<Rc<dyn Host>>>,
}
impl Native {
    pub fn new(bridge: &Rc<Bridge>) -> Rc<Self> {
        Rc::new(Self { bridge: bridge.clone(), window: Cell::new(0), ns_view: Cell::new(0), host: std::cell::RefCell::new(None) })
    }
    /// The window's pane host, given once its content view exists.
    pub fn set_host(&self, host: Rc<dyn Host>) {
        *self.host.borrow_mut() = Some(host);
    }
    fn host(&self) -> Option<Rc<dyn Host>> {
        self.host.borrow().clone()
    }
    fn slot(&self, pane: &str) -> usize {
        self.host().map_or(0, |h| h.slot(pane))
    }
    /// Attach to the window's content view. `hidden` is the test mode: no Dock icon, never
    /// active, deaf to the pointer. The bridge's own frames for this window go to `frames`.
    pub fn attach_to(&self, ns_view: *mut c_void, hidden: bool, frames: fmpsc::UnboundedSender<String>) {
        let window = unsafe { (self.bridge.attach)(ns_view, i32::from(hidden), on_emit) };
        self.window.set(window);
        self.ns_view.set(ns_view as usize);
        EMITS.lock().unwrap().get_or_insert_with(HashMap::new).insert(window, frames);
    }
    /// The window's content view, null before `attach_to` and after `detach`.
    pub fn ns_view(&self) -> *mut c_void {
        self.ns_view.get() as *mut c_void
    }
    /// The window is closing: the bridge drops its views, socket and key monitor.
    pub fn detach(&self) {
        self.call(json!({"op":"detach"}));
        if let Some(host) = self.host.borrow_mut().take() {
            host.detach();
        }
        if let Some(map) = EMITS.lock().unwrap().as_mut() {
            map.remove(&self.window.get());
        }
        self.window.set(0);
        self.ns_view.set(0);
    }
    /// One op by its JSON: the host's own (`place`, `overlay`, `focus`, `post_mouse`, `hit`) run
    /// here, every other goes to the bridge. The debug route and the check send ops this way.
    pub fn call(&self, value: Value) -> Value {
        let host = self.host();
        let rect = |v: &Value| serde_json::from_value::<[f64; 4]>(v.clone()).ok();
        match (value["op"].as_str(), &host) {
            (Some("place"), Some(_)) => {
                let holes: Vec<[f64; 4]> = serde_json::from_value(value["holes"].clone()).unwrap_or_default();
                if let Some(r) = rect(&value["rect"]) {
                    self.place(value["pane"].as_str().unwrap_or_default(), r, value["hidden"] == true, &holes, value["alpha"].as_f64().unwrap_or(1.));
                }
                self.status()
            }
            (Some("overlay"), Some(h)) => {
                h.overlay(if value["open"] == true { rect(&value["rect"]) } else { None });
                self.status()
            }
            (Some("focus"), Some(h)) => json!({"accepted": h.focus(value["pane"].as_str().unwrap_or_default())}),
            (Some("post_mouse"), Some(h)) => match serde_json::from_value::<Mouse>(value.clone()) {
                Ok(m) => json!({"target": h.mouse(&m).unwrap_or_else(|| "nil".into())}),
                Err(e) => json!({"error": e.to_string()}),
            },
            (Some("hit"), Some(h)) => {
                let point: [f64; 2] = serde_json::from_value(value["point"].clone()).unwrap_or_default();
                let (class, pane) = h.hit(point);
                json!({"class": class.unwrap_or_else(|| "nil".into()), "pane": pane})
            }
            _ => self.with_facts(self.bridge_call(value)),
        }
    }
    /// The bridge's reply with each pane's slot facts and the overlay folded in, where it lists panes.
    fn with_facts(&self, mut reply: Value) -> Value {
        let Some(host) = self.host() else { return reply };
        if let Some(panes) = reply.get_mut("panes").and_then(Value::as_object_mut) {
            let (facts, overlay) = host.facts();
            for (pane, entry) in panes.iter_mut() {
                if let (Some(into), Some(Value::Object(from))) = (entry.as_object_mut(), facts.get(pane)) {
                    into.extend(from.clone());
                }
            }
            reply["overlay"] = json!(overlay);
        }
        reply
    }
    fn bridge_call(&self, mut value: Value) -> Value {
        value["window"] = json!(self.window.get());
        let input = CString::new(value.to_string()).unwrap();
        unsafe {
            let output = (self.bridge.command)(input.as_ptr());
            let result = serde_json::from_slice(CStr::from_ptr(output).to_bytes()).unwrap();
            (self.bridge.free)(output.cast());
            result
        }
    }
    pub fn status(&self) -> Value {
        self.call(json!({"op":"status"}))
    }
    /// Show an artifact page in a WKWebView for `pane`; opening it again with a new URL navigates.
    pub fn open_web(&self, pane: &str, url: &str) {
        self.call(json!({"op":"open","pane":pane,"kind":"web","url":url,"slot":self.slot(pane)}));
    }
    /// Show an artifact for `pane`: its page in a WKWebView under the client's own `wheel-page` scheme, fetched from the daemon at `ws` and kept in the store of the
    /// artifact in the space at `cwd`.
    pub fn open_artifact(&self, pane: &str, name: &str, scope: &str, space: Option<&str>, cwd: &str, ws: &str) {
        self.call(json!({"op":"open","pane":pane,"kind":"web","artifact":name,"scope":scope,"space":space,"cwd":cwd,"ws":ws,"slot":self.slot(pane)}));
    }
    /// Show plugin pane `pane` (ADR-0041): the bridge draws its Luau script as WheelUI components, asking wheeld at `ws`
    /// for the script and again whenever it changes on disk. `label` is `<plugin>/<pane id>`; the script's `data` gets
    /// the pane, `space`, the space directory `cwd` and those two ids.
    pub fn open_plugin(&self, pane: &str, label: &str, space: Option<&str>, cwd: &str, ws: &str) {
        self.call(json!({"op":"open","pane":pane,"kind":"plugin","label":label,"space":space,"cwd":cwd,"ws":ws,"slot":self.slot(pane)}));
    }
    /// Show agent pane `pane` as the built-in `agent/view` Luau view: a plugin pane the bridge hosts. The client draws
    /// the permission card of the pane's agent above it.
    pub fn open_agent_view(&self, pane: &str, space: Option<&str>, cwd: &str, ws: &str) {
        self.call(json!({"op":"open","pane":pane,"kind":"plugin","label":"agent/view","space":space,"cwd":cwd,"ws":ws,"slot":self.slot(pane)}));
    }
    /// Show a browser pane on `url` (ADR-0005): WheelUI's `BrowserPane`, the model the Swift Mac
    /// app shows under its chrome. `remote` says wheeld runs on another machine, so a file page
    /// names a path this Mac does not have. Opening it again sends the pane to `url` as the daemon
    /// would.
    pub fn open_browser(&self, pane: &str, url: &str, remote: bool) {
        self.call(json!({"op":"open","pane":pane,"kind":"browser","url":url,"remote":remote,"slot":self.slot(pane)}));
    }
    /// One of a browser pane's actions: `back`, `forward`, `reload`, `stop`, `clear_failure`,
    /// `go_home`, `open` (with `text`), `opened` and `home_is` (with `url`), `refused` (with `text` and
    /// `message`).
    pub fn browser(&self, pane: &str, action: &str, fields: Value) {
        let mut value = json!({"op":"browser","pane":pane,"action":action});
        if let (Some(into), Some(from)) = (value.as_object_mut(), fields.as_object()) {
            into.extend(from.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        self.call(value);
    }
    /// wheeld's `render_pane` `id`: the bridge draws `pane` and answers through a frame.
    pub fn render(&self, id: u64, pane: &str, full: bool) {
        self.call(json!({"op":"render","id":id,"pane":pane,"full":full}));
    }
    /// wheeld's `inspect_pane` `id`: the bridge runs `action` on `pane`'s page (or, for `open`, sends the pane's browser to the page `arg`) and answers through a frame.
    pub fn inspect(&self, id: u64, pane: &str, action: &str, arg: Option<&str>) {
        self.call(json!({"op":"inspect","id":id,"pane":pane,"action":action,"arg":arg}));
    }
    /// `pane_reload`: open `pane`'s artifact page again, reload its browser's page, or run its plugin script again from the top.
    pub fn reload(&self, pane: &str) {
        self.call(json!({"op":"reload","pane":pane}));
    }
    /// Every pane and the terminal or agent it holds (`layout::pane_targets`), for the terminals a page embeds.
    pub fn pane_targets(&self, panes: &serde_json::Value) {
        self.call(json!({"op":"pane_targets","panes":panes}));
    }
    /// Put `pane`'s view on `rect` (x, y, w, h in window points, top-left origin), with `holes`
    /// (the same coordinates) cut out of it: the cards drawn over it. `alpha` is how opaque it draws.
    pub fn place(&self, pane: &str, rect: [f64; 4], hidden: bool, holes: &[[f64; 4]], alpha: f64) {
        if let Some(host) = self.host() {
            host.place(pane, rect, hidden, holes, alpha);
        }
    }
    /// Cut `rect` (the palette's, a sheet's; window points from the top left) out of every native
    /// pane and give the keyboard to the GPUI view, or close the cut with None.
    pub fn overlay(&self, rect: Option<[f64; 4]>) {
        if let Some(host) = self.host() {
            host.overlay(rect);
        }
    }
    /// The daemon socket the plugin panes kept across a reconnect ask for their scripts on.
    pub fn plugin_endpoint(&self, ws: &str) {
        self.call(json!({"op":"plugin_endpoint","ws":ws}));
    }
    /// Drop the bridge's daemon socket, so the next terminal opened connects anew.
    pub fn disconnect(&self) {
        self.call(json!({"op":"disconnect"}));
    }
    /// Give `pane`'s native view the keyboard; a pane with none (GPUI draws it) gives it to the GPUI view.
    pub fn focus(&self, pane: &str) {
        if let Some(host) = self.host() {
            host.focus(pane);
        }
    }
    pub fn close(&self, pane: &str) {
        self.call(json!({"op":"close","pane":pane}));
        if let Some(host) = self.host() {
            host.close(pane);
        }
    }
}

/// A browser pane's chrome state, as the bridge last emitted it (WheelUI's `BrowserPane`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct BrowserState {
    pub pane: String,
    /// The page the view is on, or the one the daemon gave it before anything committed.
    pub url: String,
    /// What the address field shows: the failed address while a failure shows, else the page.
    pub address: String,
    pub title: String,
    pub loading: bool,
    pub progress: f64,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub failure: Option<Failure>,
}

/// The last load that failed, shown over the page until the next one starts.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Failure {
    pub address: String,
    pub message: String,
}

/// What the bridge says on its own: a browser pane's state (`browser`), its reports for wheeld
/// (`typed`, `console`), or the answer to a daemon request the view relayed.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Frame {
    Event(BrowserEvent),
    Answer(Answer),
}
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BrowserEvent {
    Browser(BrowserState),
    /// The person submitted `text` in the address field: wheeld says what page it means (`pane_browser_open`).
    Typed { pane: String, text: String },
    /// The page wrote to its console (`pane_console`); `reset` says it just loaded.
    Console { pane: String, reset: bool, entries: Vec<ConsoleEntry> },
    /// A plugin pane drew its view or failed: `error` is the one-line message in the pane's header, or None.
    Plugin { pane: String, error: Option<String> },
    /// A press landed in `pane`'s native view at `point` (the window's top-left coordinates),
    /// where the GPUI view never sees it.
    Pressed { pane: String, point: [f32; 2] },
}
/// The answer to `render_pane` or `inspect_pane` `id`.
#[derive(Debug, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum Answer {
    Rendered { id: u64, png: String },
    Inspected { id: u64, text: String },
    Recorded { id: u64, mov: String, frames: u64, millis: u64 },
    Refused { id: u64, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_read_both_shapes() {
        let state: Frame = serde_json::from_str(r#"{"event":"browser","pane":"p3","url":"about:blank","address":"about:blank","title":"","loading":false,"progress":0,"can_go_back":false,"can_go_forward":false}"#).unwrap();
        assert!(matches!(state, Frame::Event(BrowserEvent::Browser(BrowserState { ref pane, ref failure, .. })) if pane == "p3" && failure.is_none()));
        let typed: Frame = serde_json::from_str(r#"{"event":"typed","pane":"p3","text":"example.com"}"#).unwrap();
        assert!(matches!(typed, Frame::Event(BrowserEvent::Typed { ref text, .. }) if text == "example.com"));
        let console: Frame = serde_json::from_str(r#"{"event":"console","pane":"p3","reset":true,"entries":[{"time":5,"level":"log","text":"hi"}]}"#).unwrap();
        assert!(matches!(console, Frame::Event(BrowserEvent::Console { reset: true, ref entries, .. }) if entries.len() == 1 && entries[0].text == "hi"));
        let plugin: Frame = serde_json::from_str(r#"{"event":"plugin","pane":"p5","error":"sample/main.luau:3: boom"}"#).unwrap();
        assert!(matches!(plugin, Frame::Event(BrowserEvent::Plugin { ref pane, error: Some(ref e) }) if pane == "p5" && e.contains("boom")));
        let healthy: Frame = serde_json::from_str(r#"{"event":"plugin","pane":"p5","error":null}"#).unwrap();
        assert!(matches!(healthy, Frame::Event(BrowserEvent::Plugin { error: None, .. })));
        let pressed: Frame = serde_json::from_str(r#"{"event":"pressed","pane":"p4","point":[12.5,40]}"#).unwrap();
        assert!(matches!(pressed, Frame::Event(BrowserEvent::Pressed { ref pane, point: [12.5, 40.] }) if pane == "p4"));
        let answer: Frame = serde_json::from_str(r#"{"answer":"recorded","id":7,"mov":"AAAA","frames":12,"millis":1001}"#).unwrap();
        assert!(matches!(answer, Frame::Answer(Answer::Recorded { id: 7, frames: 12, .. })));
        let refused: Frame = serde_json::from_str(r#"{"answer":"refused","id":8,"message":"not shown"}"#).unwrap();
        assert!(matches!(refused, Frame::Answer(Answer::Refused { id: 8, .. })));
    }
}
