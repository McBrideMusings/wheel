//! Wheel's GPUI client: a window over wheeld, drawn in Rust.
pub mod actions;
pub mod browser;
pub mod command_palette;
pub mod crash_guard;
pub mod daemon;
pub mod debug;
pub mod focus;
pub mod footer;
pub mod header;
pub mod keymap;
pub mod layout;
pub mod markdown;
pub mod native;
pub mod pairing;
pub mod palette;
pub mod procs;
pub mod server;
pub mod settings;
pub mod status;
pub mod toasts;
pub mod transcript;
pub mod view;

use anyhow::Result;
use futures::StreamExt as _;
use gpui::{App, Bounds, WindowBounds, WindowHandle, WindowOptions, point, prelude::*, px, size};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use std::{rc::Rc, sync::atomic::{AtomicU32, Ordering}};

static WINDOWS: AtomicU32 = AtomicU32::new(0);

/// Open a client window over `connection_out` and attach the native bridge to it. `hidden` is
/// the test mode: the window is never shown, never key, and the application never activates.
/// The caller feeds the window the connection's events.
#[allow(clippy::too_many_arguments)]
pub fn create_window(
    cx: &mut App,
    bridge: &Rc<native::Bridge>,
    connection_out: std::sync::mpsc::Sender<String>,
    endpoint: daemon::Endpoint,
    prefer_session: Option<String>,
    prefer_space: Option<String>,
    hidden: bool,
) -> Result<WindowHandle<view::Client>> {
    let native = native::Native::new(bridge);
    let (b, n) = (bridge.clone(), native.clone());
    let window_id = format!("gpui-{}-{}", std::process::id(), WINDOWS.fetch_add(1, Ordering::Relaxed));
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size: size(px(1200.), px(720.)) })),
            focus: !hidden,
            show: !hidden,
            ..Default::default()
        },
        |_, cx| {
            // The window's content is released when it closes: its native views go with it.
            cx.new(|cx| {
                cx.on_release(|client: &mut view::Client, _| client.shutdown()).detach();
                view::Client::new(n, b, hidden, window_id, connection_out, endpoint, prefer_session, prefer_space, cx.focus_handle())
            })
        },
    )?;
    // The bridge speaks first for a browser pane (its state, its reports) and when it answers a
    // daemon request the view relayed: those frames come through this channel.
    let (frames, mut incoming) = futures::channel::mpsc::unbounded::<String>();
    window.update(cx, |_, window, _| -> Result<()> {
        let RawWindowHandle::AppKit(handle) = window.window_handle().map_err(|e| anyhow::anyhow!("{e:?}"))?.as_raw() else {
            anyhow::bail!("not an AppKit window")
        };
        native.attach_to(handle.ns_view.as_ptr(), hidden, frames);
        Ok(())
    })??;
    cx.spawn(async move |cx| {
        while let Some(frame) = incoming.next().await {
            if window.update(cx, |client, _, cx| {
                client.on_native(&frame);
                cx.notify();
            }).is_err() {
                return;
            }
        }
    })
    .detach();
    Ok(window)
}

/// Answer one debug request against `window`: the view runs the route, then a snapshot is drawn
/// with the view free again. Err when the window is gone.
pub fn answer_debug(window: WindowHandle<view::Client>, request: &debug::Request, cx: &mut gpui::AsyncApp) -> Result<(String, serde_json::Value)> {
    if request.expired() {
        anyhow::bail!("the request's connection stopped waiting");
    }
    match window.update(cx, |client, _, cx| client.debug_request(request, cx))? {
        view::DebugAnswer::Done(status, body) => Ok((status, body)),
        view::DebugAnswer::Snapshot(target) => {
            let any: gpui::AnyWindowHandle = window.into();
            let drawn = any.update(cx, |_, window, cx| -> Result<(u32, u32)> {
                window.draw(cx).clear();
                let image = window.render_to_image()?;
                image.save(&target)?;
                Ok((image.width(), image.height()))
            })?;
            match drawn {
                Ok((w, h)) => {
                    let _ = window.update(cx, |client, _, _| client.log(format!("debug: window snapshot saved to {target}, {w}x{h}")));
                    Ok(("200 OK".to_owned(), serde_json::json!({"path": target, "width": w, "height": h})))
                }
                Err(e) => Ok(("500 Internal Server Error".to_owned(), debug::error(&format!("the window could not be rendered to {target}: {e}")))),
            }
        }
    }
}

/// Answer the debug server's requests against `window` until the window closes or the server stops.
pub fn serve_debug(cx: &mut App, window: WindowHandle<view::Client>, mut server: debug::Server) {
    cx.spawn(async move |cx| {
        while let Some(request) = server.requests.next().await {
            match answer_debug(window, &request, cx) {
                Ok(answer) => request.respond(answer),
                Err(_) if request.expired() => {}
                Err(_) => return,
            }
        }
    })
    .detach();
}

/// A window with a connection of its own, which a task feeds it until the window closes. Every
/// window is a separate client of wheeld (ADR-0022): its own socket, its own reported view.
pub fn open_window(
    cx: &mut App,
    bridge: &Rc<native::Bridge>,
    endpoint: daemon::Endpoint,
    prefer_session: Option<String>,
    prefer_space: Option<String>,
    hidden: bool,
) -> Result<WindowHandle<view::Client>> {
    let mut connection = daemon::Connection::open(endpoint.clone());
    let window = create_window(cx, bridge, connection.send.clone(), endpoint, prefer_session, prefer_space, hidden)?;
    cx.spawn(async move |cx| {
        while let Some(event) = connection.incoming.next().await {
            if window.update(cx, |client, _, cx| {
                client.on_event(event);
                cx.notify();
            }).is_err() {
                return;
            }
        }
    })
    .detach();
    Ok(window)
}
