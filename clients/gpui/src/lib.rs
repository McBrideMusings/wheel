//! Wheel's GPUI client: a window over wheeld, drawn in Rust.
pub mod actions;
pub mod browser;
pub mod command_palette;
pub mod composer;
pub mod conversations;
pub mod crash_guard;
pub mod daemon;
pub mod debug;
pub mod focus;
pub mod footer;
pub mod grid_drop;
pub mod header;
pub mod keymap;
pub mod layout;
pub mod markdown;
pub mod menubar;
pub mod motion;
pub mod native;
pub mod pairing;
pub mod perms;
pub mod popped;
pub mod palette;
pub mod pane_header;
pub mod procs;
pub mod server;
pub mod settings;
pub mod sidebar;
pub mod status;
pub mod strip;
pub mod toasts;
pub mod tooltip;
pub mod transcript;
pub mod tray;
pub mod view;
pub mod windows;

use anyhow::Result;
use futures::StreamExt as _;
use gpui::{App, Bounds, WindowBounds, WindowHandle, WindowOptions, point, prelude::*, px, size};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::atomic::{AtomicU32, Ordering}};
use wheel_proto::{ServerMsg, WindowCommand};

static WINDOWS: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Every open window by id, for a window command or a debug request that names another window.
    static HANDLES: RefCell<HashMap<String, WindowHandle<view::Client>>> = RefCell::default();
}

/// The open window `id`, if it is open.
pub fn window_handle(id: &str) -> Option<WindowHandle<view::Client>> {
    HANDLES.with(|h| h.borrow().get(id).copied())
}

/// Every open window by id, main and little.
pub fn window_handles() -> Vec<(String, WindowHandle<view::Client>)> {
    HANDLES.with(|h| h.borrow().iter().map(|(id, w)| (id.clone(), *w)).collect())
}

/// Redraw every window but `except`, and the menu bar's popover: a space drag moved over or off
/// them, and the one under the pointer draws the bar, the outline and the chip.
pub fn refresh_other_windows(cx: &mut App, except: &str) {
    let except = except.to_owned();
    cx.defer(move |cx| {
        for (id, handle) in window_handles() {
            if id != except {
                let _ = handle.update(cx, |_, _, cx| cx.notify());
            }
        }
        if let Some(menu) = view::menu_bar() {
            menu.update(cx, |_, cx| cx.notify());
        }
    });
}

/// A window closed: it leaves the handles and the app's list of windows.
pub fn forget_window(id: &str) {
    HANDLES.with(|h| h.borrow_mut().remove(id));
    windows::with(|r| r.remove(id));
}

/// Redraw every open window after the app's list of windows changed under them: a little window
/// opened or closed, so its owner's tab, row and grid change.
pub fn refresh_windows(cx: &mut App) {
    cx.defer(|cx| {
        let handles: Vec<WindowHandle<view::Client>> = HANDLES.with(|h| h.borrow().values().copied().collect());
        for handle in handles {
            let _ = handle.update(cx, |_, _, cx| cx.notify());
        }
    });
}

/// What kind of window to open: a main one, or a little one showing a single space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    Main,
    Little,
}

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
    opening: Opening,
) -> Result<WindowHandle<view::Client>> {
    // The app's one server control, made by the first window or the menu bar.
    view::server::ensure(cx, endpoint.service.clone(), hidden);
    let native = native::Native::new(bridge);
    let (b, n) = (bridge.clone(), native.clone());
    let window_id = format!("gpui-{}-{}", std::process::id(), WINDOWS.fetch_add(1, Ordering::Relaxed));
    let registered = window_id.clone();
    let little = opening == Opening::Little;
    // A little window opens at its own small size, centred, and shrinks no further than its minimum.
    let (bounds, min_size) = match opening {
        Opening::Main => (Bounds { origin: point(px(0.), px(0.)), size: size(px(1200.), px(720.)) }, None),
        Opening::Little => {
            let ((w, h), (mw, mh)) = (view::LITTLE_SIZE, view::LITTLE_MIN_SIZE);
            (Bounds::centered(None, size(px(w), px(h)), cx), Some(size(px(mw), px(mh))))
        }
    };
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: min_size,
            focus: !hidden,
            show: !hidden,
            ..Default::default()
        },
        |_, cx| {
            // The window's content is released when it closes: its native views go with it, and
            // the other windows redraw (a little window's owner shows its space's grid again).
            cx.new(|cx| {
                cx.on_release(|client: &mut view::Client, cx| {
                    client.shutdown();
                    refresh_windows(cx);
                })
                .detach();
                let mut client = view::Client::new(n, b, hidden, window_id, connection_out, endpoint, prefer_session, prefer_space, cx.focus_handle());
                client.little = little;
                client.publish();
                client
            })
        },
    )?;
    // The bridge speaks first for a browser pane (its state, its reports) and when it answers a
    // daemon request the view relayed: those frames come through this channel.
    let (frames, mut incoming) = futures::channel::mpsc::unbounded::<String>();
    HANDLES.with(|h| h.borrow_mut().insert(registered.clone(), window));
    // The newest main window is the focused one until another is key (AppModel.newWindow brings it
    // front); a little window is never the one CLI commands act on.
    if opening == Opening::Main {
        windows::with(|r| r.focused = Some(registered.clone()));
    }
    window.update(cx, |_, window, cx| -> Result<()> {
        // The red light asks the window first: the last window on a session that holds spaces
        // shows its close question and stays open.
        // The focused window is the one that became key last (NSApp.keyWindow in the Swift app).
        let id = registered.clone();
        cx.observe_window_activation(window, move |_, window, _| {
            if window.is_window_active() {
                windows::with(|r| {
                    if r.get(&id).is_some() {
                        r.focused = Some(id.clone());
                    }
                });
            }
        })
        .detach();
        window.on_window_should_close(cx, |window, cx| {
            let Some(Some(client)) = window.root::<view::Client>() else { return true };
            client.update(cx, |client, cx| {
                cx.notify();
                client.ask_close_window()
            })
        });
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
    // `window=<id>` sends the request to another open window of this app.
    let window = match request.query.get("window") {
        Some(id) => window_handle(id).ok_or_else(|| anyhow::anyhow!("no window {id}"))?,
        None => window,
    };
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
    open_window_as(cx, bridge, endpoint, prefer_session, prefer_space, hidden, Opening::Main)
}

/// `open_window` for a main or a little window.
pub fn open_window_as(
    cx: &mut App,
    bridge: &Rc<native::Bridge>,
    endpoint: daemon::Endpoint,
    prefer_session: Option<String>,
    prefer_space: Option<String>,
    hidden: bool,
    opening: Opening,
) -> Result<WindowHandle<view::Client>> {
    // The window holds the only sender: when it closes, the socket closes with it, and wheeld
    // forgets its view and its offer to run window commands at once.
    let daemon::Connection { send, mut incoming } = daemon::Connection::open(endpoint.clone());
    let window = create_window(cx, bridge, send, endpoint, prefer_session, prefer_space, hidden, opening)?;
    cx.spawn(async move |cx| {
        while let Some(event) = incoming.next().await {
            if cx.update(|cx| deliver(cx, window, event)).is_err() {
                return;
            }
        }
    })
    .detach();
    Ok(window)
}

/// Hand `window` one thing its connection reported. A window command acts on other windows
/// too, so it runs with the app rather than inside the window. Ok(true) for a layout; Err when the
/// window is gone, and then a window command it carried goes unanswered: there is no connection
/// left to answer on, and wheeld gives up on it after its wait.
pub fn deliver(cx: &mut App, window: WindowHandle<view::Client>, event: daemon::Event) -> Result<bool> {
    if let daemon::Event::Message(message) = &event
        && let ServerMsg::WindowRequest { id, command } = message.as_ref()
    {
        window.read(cx)?;
        answer_window_command(cx, window, *id, command.clone());
        return Ok(false);
    }
    // Every window hears a focus request on its own connection; the one it names acts on it.
    if let daemon::Event::Message(message) = &event
        && let ServerMsg::SpaceFocused { space, pane } = message.as_ref()
    {
        let (id, sessions) = window.read(cx).map(|c| (c.window_id().to_owned(), c.layout_lists().0))?;
        // A space in a little window is shown there: that window focuses the pane and comes
        // front (AppModel.focus(space:) goes to the little window).
        if let Some(little) = windows::with(|r| r.little_showing(space).map(|l| l.id.clone())) {
            if little == id {
                window.update(cx, |client, _, cx| {
                    cx.notify();
                    client.show_requested(space, pane.as_deref())
                })?;
                bring_front(cx, window);
            }
            return Ok(false);
        }
        // A space whose session no main window shows opens in a little window that belongs to
        // none (AppModel.showRequested), which focuses the pane once its layout lists it.
        let home = windows::session_holding(&sessions, space).map(|s| s.id.clone());
        let background = home.is_some_and(|home| windows::with(|r| !r.shows_session(&home)));
        if windows::with(|r| view::focus_target(r, &sessions, space)).as_deref() == Some(id.as_str()) && background {
            let opened = window.update(cx, |client, _, cx| {
                client.log(format!("space {space} focused by a request; it runs with no window, so it opens in a little window"));
                cx.notify();
                client.open_little(space, cx)
            })?;
            if let Some(handle) = opened.as_deref().and_then(window_handle) {
                let _ = handle.update(cx, |client, _, cx| {
                    cx.notify();
                    client.show_requested(space, pane.as_deref())
                });
            }
            return Ok(false);
        }
        if windows::with(|r| view::focus_target(r, &sessions, space)).as_deref() == Some(id.as_str()) {
            let shown = window.update(cx, |client, _, cx| {
                cx.notify();
                client.show_requested(space, pane.as_deref())
            })?;
            if shown {
                bring_front(cx, window);
            }
        }
        return Ok(false);
    }
    window.update(cx, |client, _, cx| {
        let layout = client.on_event(event);
        cx.notify();
        layout
    })
}

/// Answer a window command wheeld forwarded to `via` (AppModel.answerWindowCommand): run it on
/// the app's windows and send what it came to back through `via`'s connection.
pub fn answer_window_command(cx: &mut App, via: WindowHandle<view::Client>, id: u64, command: WindowCommand) {
    let result = run_window_command(cx, via, &command);
    let _ = via.update(cx, |client, _, cx| {
        client.window_answer(id, &command, result);
        cx.notify();
    });
}

fn run_window_command(cx: &mut App, via: WindowHandle<view::Client>, command: &WindowCommand) -> Result<serde_json::Value, String> {
    let gone = |_| "the window wheeld asked has closed".to_owned();
    let (sessions, spaces) = via.read(cx).map(|c| c.layout_lists()).map_err(gone)?;
    let target = |text: &str| -> Result<(String, WindowHandle<view::Client>), String> {
        let id = windows::with(|r| windows::window_id(r, text, &spaces))?;
        window_handle(&id).map(|h| (id.clone(), h)).ok_or_else(|| format!("window {id} has closed"))
    };
    match command {
        WindowCommand::Ls => Ok(windows::with(|r| windows::ls(r, &sessions, &spaces))),
        WindowCommand::New { name, space } => {
            let held = space.as_deref().map(|s| windows::space_id(s, &spaces)).transpose()?;
            // The session holding the space, else the one the focused window shows.
            let session = held.as_deref().and_then(|space| windows::session_holding(&sessions, space)).map(|s| s.id.clone());
            let (bridge, endpoint, hidden) = via.read(cx).map(|c| c.opener()).map_err(gone)?;
            let handle = new_window(cx, &bridge, endpoint, hidden, session, held, name.clone()).map_err(|e| format!("{e:#}"))?;
            let new_id = handle.read(cx).map(|c| c.window_id().to_owned()).map_err(gone)?;
            Ok(serde_json::json!({"window": new_id}))
        }
        WindowCommand::Rename { window, name } => {
            let (id, handle) = target(window)?;
            let _ = handle.update(cx, |client, _, cx| {
                client.set_window_name(Some(name.clone()));
                cx.notify();
            });
            Ok(serde_json::json!({"window": id}))
        }
        WindowCommand::Close { window } => {
            let (id, handle) = target(window)?;
            let closes = handle.update(cx, |client, _, cx| {
                cx.notify();
                client.ask_close_window()
            });
            // The window may be the one answering: it goes after its answer is queued.
            match closes {
                // Its connection writes every frame queued before the window's sender drops.
                Ok(true) => cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }),
                // The question shows on that window: bring it front, as the Swift app does.
                Ok(false) => bring_front(cx, handle),
                Err(_) => {}
            }
            Ok(serde_json::json!({"window": id}))
        }
        WindowCommand::SpaceMove { space, to } => {
            let space = windows::space_id(space, &spaces)?;
            if to == "background" {
                return Err(windows::NO_BACKGROUND_MOVE.into());
            }
            let (id, handle) = target(to)?;
            let placed = handle.update(cx, |client, _, cx| {
                cx.notify();
                client.place(&space)
            });
            if placed.unwrap_or(false) {
                windows::with(|r| r.focused = Some(id.clone()));
                bring_front(cx, handle);
                Ok(serde_json::json!({"space": space, "to": id}))
            } else {
                Err(format!("window {id} shows no session to put {space} in"))
            }
        }
    }
}

/// New Window, `wheel window new` and the Dock icon (AppModel.newWindow): a main window on
/// `session`, else the focused window's, showing `space` when one is named, with the focused
/// window's sidebar and `name`, brought front.
pub fn new_window(cx: &mut App, bridge: &Rc<native::Bridge>, endpoint: daemon::Endpoint, hidden: bool, session: Option<String>, space: Option<String>, name: Option<String>) -> Result<WindowHandle<view::Client>> {
    let (focused, sidebar) = windows::with(|r| (r.focused().and_then(|w| w.session.clone()), r.focused().map(|w| w.sidebar).unwrap_or(true)));
    let handle = open_window(cx, bridge, endpoint, session.or(focused), space, hidden)?;
    let _ = handle.update(cx, |client, _, cx| {
        client.set_sidebar(sidebar);
        client.set_window_name(name);
        cx.notify();
    });
    bring_front(cx, handle);
    Ok(handle)
}

/// Bring a window front and make it key, unless the app runs hidden for a check.
pub fn bring_front(cx: &mut App, handle: WindowHandle<view::Client>) {
    if handle.read(cx).is_ok_and(|c| !c.is_hidden()) {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    }
}
