//! Rust side of native/Bridge.swift: loads the bridge library and speaks its JSON command ABI.
//! The bridge hosts Wheel's real Ghostty terminal view and a WKWebView as children of the
//! GPUI window's NSView. All calls run on the main thread.
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::PathBuf,
    rc::Rc,
};

pub struct Native {
    _library: libloading::Library,
    pub attach: unsafe extern "C" fn(*mut c_void, i32),
    pub command: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    pub free: unsafe extern "C" fn(*mut c_void),
}
impl Native {
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
    /// Attach to the window's content view. `hidden` is the test mode: no Dock icon, never
    /// active, deaf to the pointer.
    pub fn attach_to(&self, ns_view: *mut c_void, hidden: bool) {
        unsafe { (self.attach)(ns_view, i32::from(hidden)) }
    }
    pub fn call(&self, value: Value) -> Value {
        let input = CString::new(value.to_string()).unwrap();
        unsafe {
            let output = (self.command)(input.as_ptr());
            let result = serde_json::from_slice(CStr::from_ptr(output).to_bytes()).unwrap();
            (self.free)(output.cast());
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
    /// Show terminal `term` in a Ghostty view for `pane`, streamed over the daemon socket `ws`.
    pub fn open_terminal(&self, pane: &str, term: &str, ws: &str) {
        self.call(json!({"op":"open","pane":pane,"kind":"terminal","term":term,"ws":ws}));
    }
    /// Put `pane`'s view on `rect` (x, y, w, h in window points, top-left origin).
    pub fn place(&self, pane: &str, rect: [f64; 4], hidden: bool) {
        self.call(json!({"op":"place","pane":pane,"rect":rect,"hidden":hidden}));
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
