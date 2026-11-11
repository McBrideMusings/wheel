//! Wheel's GPUI client: a window over wheeld, drawn in Rust.
pub mod daemon;
pub mod focus;
pub mod layout;
pub mod native;
pub mod transcript;
pub mod view;

use anyhow::Result;
use gpui::{AsyncApp, Bounds, WindowBounds, WindowHandle, WindowOptions, point, prelude::*, px, size};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use std::rc::Rc;

/// Open the client window and attach the native bridge to it. `hidden` is the test mode: the
/// window is never shown, never key, and the application never activates.
pub fn open_window(
    cx: &mut AsyncApp,
    native: &Rc<native::Native>,
    connection_out: std::sync::mpsc::Sender<String>,
    endpoint: daemon::Endpoint,
    prefer_space: Option<String>,
    hidden: bool,
) -> Result<WindowHandle<view::Client>> {
    let n = native.clone();
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size: size(px(1200.), px(720.)) })),
                focus: !hidden,
                show: !hidden,
                ..Default::default()
            },
            |_, cx| cx.new(|cx| view::Client::new(n, connection_out, endpoint, prefer_space, cx.focus_handle())),
        )
    })?;
    window.update(cx, |_, window, _| -> Result<()> {
        let RawWindowHandle::AppKit(handle) = window.window_handle().map_err(|e| anyhow::anyhow!("{e:?}"))?.as_raw() else {
            anyhow::bail!("not an AppKit window")
        };
        native.attach_to(handle.ns_view.as_ptr(), hidden);
        Ok(())
    })??;
    Ok(window)
}
