//! Rust side of native/Bridge.swift: loads the bridge library and speaks its JSON command ABI.
//! The bridge hosts Wheel's real Ghostty terminal view and a WKWebView as children of the
//! GPUI window's NSView. All calls run on the main thread. The bridge also speaks first: a
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
    app_attach: unsafe extern "C" fn(i32, Emit),
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
                app_attach: *library.get(b"wheel_native_app_attach")?,
                command: *library.get(b"wheel_native_command")?,
                free: *library.get(b"wheel_native_free")?,
                _library: library,
            }))
        }
    }
}

/// The bridge's window id for what the app holds outside every window: the menu bar item.
pub const APP: i32 = -1;

/// The native views of one window. Every command names the window the bridge assigned at
/// `attach_to`; before that, commands answer `detached`.
pub struct Native {
    bridge: Rc<Bridge>,
    window: Cell<i32>,
}
impl Native {
    pub fn new(bridge: &Rc<Bridge>) -> Rc<Self> {
        Rc::new(Self { bridge: bridge.clone(), window: Cell::new(0) })
    }
    /// Attach to the window's content view. `hidden` is the test mode: no Dock icon, never
    /// active, deaf to the pointer. The bridge's own frames for this window go to `frames`.
    pub fn attach_to(&self, ns_view: *mut c_void, hidden: bool, frames: fmpsc::UnboundedSender<String>) {
        let window = unsafe { (self.bridge.attach)(ns_view, i32::from(hidden), on_emit) };
        self.window.set(window);
        EMITS.lock().unwrap().get_or_insert_with(HashMap::new).insert(window, frames);
    }
    /// The app's own half of the bridge (`APP`): the menu bar item and the Dock tile, which live
    /// while the app runs, with or without a window. Its frames (the item's clicks) go to `frames`.
    pub fn app(bridge: &Rc<Bridge>, hidden: bool, frames: fmpsc::UnboundedSender<String>) -> Rc<Self> {
        unsafe { (bridge.app_attach)(i32::from(hidden), on_emit) };
        EMITS.lock().unwrap().get_or_insert_with(HashMap::new).insert(APP, frames);
        Rc::new(Self { bridge: bridge.clone(), window: Cell::new(APP) })
    }
    /// The window is closing: the bridge drops its views, socket and key monitor.
    pub fn detach(&self) {
        self.call(json!({"op":"detach"}));
        if let Some(map) = EMITS.lock().unwrap().as_mut() {
            map.remove(&self.window.get());
        }
        self.window.set(0);
    }
    pub fn call(&self, mut value: Value) -> Value {
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
        self.call(json!({"op":"open","pane":pane,"kind":"web","url":url}));
    }
    /// Show an artifact for `pane`: the bridge draws its A2UI page when the artifact's MCP server
    /// speaks A2UI (ADR-0031, ADR-0038), and otherwise its page in a WKWebView under the client's
    /// own `wheel-page` scheme, fetched from the daemon at `ws` and kept in the store of the
    /// artifact in the space at `cwd`.
    pub fn open_artifact(&self, pane: &str, name: &str, scope: &str, space: Option<&str>, cwd: &str, ws: &str) {
        self.call(json!({"op":"open","pane":pane,"kind":"web","artifact":name,"scope":scope,"space":space,"cwd":cwd,"ws":ws}));
    }
    /// Show a browser pane on `url` (ADR-0005): WheelUI's `BrowserPane`, the model the Swift Mac
    /// app shows under its chrome. `remote` says wheeld runs on another machine, so a file page
    /// names a path this Mac does not have. Opening it again sends the pane to `url` as the daemon
    /// would.
    pub fn open_browser(&self, pane: &str, url: &str, remote: bool) {
        self.call(json!({"op":"open","pane":pane,"kind":"browser","url":url,"remote":remote}));
    }
    /// One of a browser pane's actions: `back`, `forward`, `reload`, `stop`, `clear_failure`,
    /// `open` (with `text`), `opened` and `daemon_says` (with `url`), `refused` (with `text` and
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
    /// wheeld's `inspect_pane` `id`: the bridge runs `action` on `pane`'s page and answers through a frame.
    pub fn inspect(&self, id: u64, pane: &str, action: &str, arg: Option<&str>) {
        self.call(json!({"op":"inspect","id":id,"pane":pane,"action":action,"arg":arg}));
    }
    /// A file in `pane`'s folder changed: open its page again.
    pub fn reload(&self, pane: &str) {
        self.call(json!({"op":"reload","pane":pane}));
    }
    /// Show terminal `term` in a Ghostty view for `pane`, streamed over the daemon socket `ws` and
    /// styled by the Ghostty controller of `space`, the space it opens in.
    pub fn open_terminal(&self, pane: &str, term: &str, space: Option<&str>, ws: &str) {
        self.call(json!({"op":"open","pane":pane,"kind":"terminal","term":term,"space":space,"ws":ws}));
    }
    /// `space`'s `terminal.*` settings as wheeld resolved them: its terminals restyle in place.
    pub fn terminal_look(&self, space: &str, settings: &[wheel_proto::SettingInfo]) {
        let terminal: Vec<&wheel_proto::SettingInfo> = settings.iter().filter(|s| s.key.starts_with("terminal.")).collect();
        self.call(json!({"op":"terminal_look","space":space,"settings":terminal}));
    }
    /// Put `pane`'s view on `rect` (x, y, w, h in window points, top-left origin), with `holes`
    /// (the same coordinates) cut out of it: the cards drawn over it.
    pub fn place(&self, pane: &str, rect: [f64; 4], hidden: bool, holes: &[[f64; 4]]) {
        self.call(json!({"op":"place","pane":pane,"rect":rect,"hidden":hidden,"holes":holes}));
    }
    /// Hold every terminal's resizes while the grid animates; on release each sends the grid it ended at.
    pub fn hold_resizes(&self, held: bool) {
        self.call(json!({"op":"hold_resizes","held":held}));
    }
    /// Drop the bridge's daemon socket, so the next terminal opened connects anew.
    pub fn disconnect(&self) {
        self.call(json!({"op":"disconnect"}));
    }
    /// Give `pane`'s native view the keyboard; a pane with none (GPUI draws it) gives it to the GPUI view.
    pub fn focus(&self, pane: &str) {
        self.call(json!({"op":"focus","pane":pane}));
    }
    pub fn close(&self, pane: &str) {
        self.call(json!({"op":"close","pane":pane}));
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
/// (`navigated`, `typed`, `console`), or the answer to a daemon request the view relayed.
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
    /// The view landed on `url`: wheeld's layout follows (`pane_browser_url`).
    Navigated { pane: String, url: String },
    /// The person submitted `text` in the address field: wheeld says what page it means (`pane_browser_open`).
    Typed { pane: String, text: String },
    /// The page wrote to its console (`pane_console`); `reset` says it just loaded.
    Console { pane: String, reset: bool, entries: Vec<ConsoleEntry> },
    /// The person picked a folder in Open Folder's system picker (`choose_folder`).
    FolderChosen { path: String },
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
        let pressed: Frame = serde_json::from_str(r#"{"event":"pressed","pane":"p4","point":[12.5,40]}"#).unwrap();
        assert!(matches!(pressed, Frame::Event(BrowserEvent::Pressed { ref pane, point: [12.5, 40.] }) if pane == "p4"));
        let answer: Frame = serde_json::from_str(r#"{"answer":"recorded","id":7,"mov":"AAAA","frames":12,"millis":1001}"#).unwrap();
        assert!(matches!(answer, Frame::Answer(Answer::Recorded { id: 7, frames: 12, .. })));
        let refused: Frame = serde_json::from_str(r#"{"answer":"refused","id":8,"message":"not shown"}"#).unwrap();
        assert!(matches!(refused, Frame::Answer(Answer::Refused { id: 8, .. })));
    }
}
