//! Wheel's GPUI client: a window over wheeld, drawn in Rust.
pub use wheel_gpui_core::{cards, chat_links, command_palette, composer, conversations, crash_guard, daemon, debug, earlier, focus, footer, grid_drop, header, layout, markdown, menubar, motion, native, pairing, palette, pane_header, perms, popped, procs, queue, running, server, settings, sidebar, strip, toasts, topic, transcript, tray, windows};
pub mod actions;
pub mod browser;
pub mod edit_menu;
pub mod keymap;
pub mod saved;
pub mod status;
pub mod tooltip;
pub mod view;

use anyhow::Result;
use futures::StreamExt as _;
use gpui::{App, Bounds, KeyDownEvent, KeyBinding, KeystrokeEvent, NoAction, Window, WindowBounds, WindowOptions, point, prelude::*, px, rgb, rgba, size};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::atomic::{AtomicU32, Ordering}};
use wheel_proto::{ServerMsg, WindowCommand};

static WINDOWS: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Every open window by id, for a window command or a debug request that names another window.
    static HANDLES: RefCell<HashMap<String, view::ClientWindow>> = RefCell::default();
}

/// The open window `id`, if it is open.
pub fn window_handle(id: &str) -> Option<view::ClientWindow> {
    HANDLES.with(|h| h.borrow().get(id).copied())
}

/// Every open window by id, main and little.
pub fn window_handles() -> Vec<(String, view::ClientWindow)> {
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
        let handles: Vec<view::ClientWindow> = HANDLES.with(|h| h.borrow().values().copied().collect());
        for handle in handles {
            let _ = handle.update(cx, |_, _, cx| cx.notify());
        }
    });
}

/// Set up before the first window opens: gpui-component's bindings and theme (its text inputs
/// draw from it) in the client's palette, and the hook that gives the composer's keys to the
/// client before the input's own bindings.
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    let colors = &mut gpui_component::Theme::global_mut(cx).colors;
    colors.foreground = rgb(0xd9e8e1).into();
    colors.background = rgb(0x0e1311).into();
    colors.caret = rgb(0x7fb89a).into();
    colors.selection = rgba(0x7fb89a55).into();
    colors.muted_foreground = rgb(0x6f8a7d).into();
    // Root binds Tab to the next focus stop; the client's keys read Tab themselves.
    cx.bind_keys([KeyBinding::new("tab", NoAction, Some("Root")), KeyBinding::new("shift-tab", NoAction, Some("Root"))]);
    cx.intercept_keystrokes(composer_key).detach();
}

/// A keystroke in a window whose composer input is focused, before GPUI resolves it against
/// the input's bindings: the client's own handling of the key (the command list, the queue,
/// Return to send, files pasted as chips, its commands) goes first, and a key it takes goes no
/// further. ⇧Return starts a new line, which the input leaves unbound. Every other key is the
/// input's: caret, selection, clipboard, undo, word and line moves, IME.
fn composer_key(event: &KeystrokeEvent, window: &mut Window, cx: &mut App) {
    let Some(client) = view::ClientWindow::client_of(window, cx) else { return };
    let Some(input) = client.read(cx).focused_input(window, cx) else { return };
    let k = &event.keystroke;
    // GPUI's interceptor event has no repeat flag: the same key again with no key-up between
    // (the root's `on_key_up` clears it) is the platform's auto-repeat, as `isARepeat` is.
    let is_held = client.update(cx, |client, _| client.key_down.replace(k.clone()).as_ref() == Some(k));
    let down = KeyDownEvent { keystroke: k.clone(), is_held, prefer_character_input: false };
    // The keyboard is about to leave the input (the pane changed, an overlay opened; the next
    // frame moves it): the client takes the key and the input gets none.
    let leaving = client.read(cx).composer_agent().is_none();
    if client.update(cx, |client, cx| {
        let handled = client.key(&down, cx);
        if handled {
            cx.notify();
        }
        handled
    }) || leaving {
        cx.stop_propagation();
    } else if k.key == "enter" && k.modifiers.shift && !k.modifiers.platform && !k.modifiers.control {
        input.update(cx, |input, cx| input.replace("\n", window, cx));
        cx.stop_propagation();
    }
}

/// What kind of window to open: a main one, or a little one showing a single space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    Main,
    Little,
}

/// Open a client window over `connection_out` and attach the native bridge to it. `hidden` is
/// the test mode: the window is never shown, never key, and the application never activates.
/// The caller feeds the window the connection's events. `frame` is where a restored window goes
/// (saved.rs), in the bridge's top-left screen coordinates; none opens it at its default place.
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
    frame: Option<saved::Frame>,
) -> Result<view::ClientWindow> {
    // The app's one server control, made by the first window or the menu bar.
    view::server::ensure(cx, endpoint.service.clone(), hidden);
    view::ensure_pairing_copy(cx);
    edit_menu::install(cx);
    let native = native::Native::new(bridge);
    let (b, n) = (bridge.clone(), native.clone());
    let window_id = format!("gpui-{}-{}", std::process::id(), WINDOWS.fetch_add(1, Ordering::Relaxed));
    let registered = window_id.clone();
    let little = opening == Opening::Little;
    // A little window opens at its own small size, centred, and shrinks no further than its minimum.
    let (mut bounds, min_size) = match opening {
        Opening::Main => (Bounds { origin: point(px(0.), px(0.)), size: size(px(1200.), px(720.)) }, None),
        Opening::Little => {
            let ((w, h), (mw, mh)) = (view::LITTLE_SIZE, view::LITTLE_MIN_SIZE);
            (Bounds::centered(None, size(px(w), px(h)), cx), Some(size(px(mw), px(mh))))
        }
    };
    // A restored window opens at its saved size near its place; the bridge then puts its content's
    // top left exactly where it was, as WindowManager puts a displaced window back.
    // A frame whose centre is on no screen now (a display that went) is dropped, as AppKit moves a
    // restored frame onto the screens it can see.
    let on_screen = |f: &saved::Frame| cx.displays().iter().any(|d| d.bounds().contains(&point(px(f[0] + f[2] / 2.), px(f[1] + f[3] / 2.))));
    let frame = frame.filter(|f| f[2] > 0. && f[3] > 0. && on_screen(f));
    if let Some([x, y, w, h]) = frame {
        bounds = Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
    }
    let window = view::ClientWindow::open(
        cx,
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
                    view::float::release(client.window_id());
                    let closed = client.window_id().to_owned();
                    client.shutdown();
                    view::pane_window_closed(&closed, cx);
                    refresh_windows(cx);
                })
                .detach();
                // Anything that notifies the client may change what a pane body shows, so every body
                // draws again; a notify that reaches only a body (its working arc, its scroll) does not.
                cx.observe_self(|client: &mut view::Client, cx| client.invalidate_bodies(cx)).detach();
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
        // The focused window is the one that became key last (NSApp.keyWindow in the Swift app).
        let id = registered.clone();
        cx.observe_window_activation(window, move |_, window, _| {
            if window.is_window_active() {
                windows::with(|r| {
                    if r.get(&id).is_some() {
                        r.focused = Some(id.clone());
                    }
                });
                saved::save();
            }
        })
        .detach();
        window.on_window_should_close(cx, red_light);
        let RawWindowHandle::AppKit(handle) = window.window_handle().map_err(|e| anyhow::anyhow!("{e:?}"))?.as_raw() else {
            anyhow::bail!("not an AppKit window")
        };
        native.attach_to(handle.ns_view.as_ptr(), hidden, frames);
        // Every move and resize is saved with the window (saved.rs).
        cx.observe_window_bounds(window, |client: &mut view::Client, _, _| client.measure_frame()).detach();
        if let Some([x, y, ..]) = frame {
            native.call(serde_json::json!({"op": "move_window", "point": [f64::from(x), f64::from(y)]}));
        }
        Ok(())
    })??;
    let _ = window.update(cx, |client, _, _| client.measure_frame());
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

/// How long the snapshot waits for the bridge to draw every native pane it asked for.
const NATIVE_RENDER_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The native views over the window (terminals, pages, plugin panes), each drawn by the bridge and
/// pasted over `frame` at its place, except where the window cuts it (a card over it, the
/// palette): the window's own drawing has only empty slots there. Answers each pane's outcome.
async fn paste_native_panes(window: view::ClientWindow, frame: &mut image::RgbaImage, cx: &mut gpui::AsyncApp) -> Result<serde_json::Value> {
    let (status, asked, width) = window.update(cx, |client, w, _| {
        let (status, asked) = client.render_native_for_snapshot();
        (status, asked, f32::from(w.viewport_size().width))
    })?;
    let scale = frame.width() as f32 / width.max(1.);
    let rect = |v: &serde_json::Value| -> Option<[f32; 4]> {
        let a = v.as_array()?;
        let n = |i: usize| a.get(i).and_then(|x| x.as_f64()).map(|x| x as f32 * scale);
        Some([n(0)?, n(1)?, n(2)?, n(3)?])
    };
    let overlay = rect(&status["overlay"]);
    let mut outcomes = serde_json::Map::new();
    // Every render was asked at once, so they share one deadline.
    let deadline = std::time::Instant::now() + NATIVE_RENDER_WAIT;
    let mut unanswered = vec![];
    for (pane, render) in asked {
        let Some((id, answer)) = render else {
            outcomes.insert(pane, serde_json::json!("not shown"));
            continue;
        };
        let timer = cx.background_executor().timer(deadline.saturating_duration_since(std::time::Instant::now()));
        let drawn: Result<Vec<u8>, String> = match futures::future::select(answer, timer).await {
            futures::future::Either::Left((Ok(result), _)) => result,
            futures::future::Either::Left((Err(_), _)) => Err("the render was dropped".into()),
            futures::future::Either::Right(_) => {
                unanswered.push(id);
                Err(format!("the bridge did not draw it within {}s", NATIVE_RENDER_WAIT.as_secs()))
            }
        };
        let entry = &status["panes"][pane.as_str()];
        let outcome = drawn.and_then(|png| {
            let at = rect(&entry["frame_top"]).ok_or("the bridge did not report where it is")?;
            let image = image::load_from_memory(&png).map_err(|e| format!("the picture was not a PNG ({e})"))?.to_rgba8();
            let (w, h) = (at[2].round().max(1.) as u32, at[3].round().max(1.) as u32);
            let image = if image.dimensions() == (w, h) { image } else { image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle) };
            let cuts: Vec<[f32; 4]> = entry["holes"].as_array().into_iter().flatten().filter_map(&rect).chain(overlay).collect();
            let alpha = entry["alpha"].as_f64().unwrap_or(1.) as f32;
            let (x0, y0) = (at[0].round() as i64, at[1].round() as i64);
            for (x, y, px) in image.enumerate_pixels() {
                let (fx, fy) = (x0 + i64::from(x), y0 + i64::from(y));
                if fx < 0 || fy < 0 || fx >= i64::from(frame.width()) || fy >= i64::from(frame.height()) {
                    continue;
                }
                let (cx_, cy_) = (fx as f32 + 0.5, fy as f32 + 0.5);
                if cuts.iter().any(|c| cx_ >= c[0] && cx_ < c[0] + c[2] && cy_ >= c[1] && cy_ < c[1] + c[3]) {
                    continue;
                }
                let a = f32::from(px[3]) / 255. * alpha;
                let under = frame.get_pixel_mut(fx as u32, fy as u32);
                for i in 0..3 {
                    under[i] = (f32::from(px[i]) * a + f32::from(under[i]) * (1. - a)).round() as u8;
                }
                under[3] = under[3].max((a * 255.).round() as u8);
            }
            Ok(())
        });
        outcomes.insert(pane, match outcome {
            Ok(()) => serde_json::json!("drawn"),
            Err(why) => serde_json::json!(why),
        });
    }
    if !unanswered.is_empty() {
        window.update(cx, |client, _, _| client.forget_snapshot_renders(&unanswered))?;
    }
    Ok(serde_json::Value::Object(outcomes))
}

/// Answer one debug request against `window`: the view runs the route, then a snapshot is drawn
/// with the view free again, the native views pasted in. Err when the window is gone.
pub async fn answer_debug(window: view::ClientWindow, request: &debug::Request, cx: &mut gpui::AsyncApp) -> Result<(String, serde_json::Value)> {
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
        view::DebugAnswer::Keys(keys) => {
            let any: gpui::AnyWindowHandle = window.into();
            let taken = any.update(cx, |_, window, cx| keys.into_iter().map(|k| window.dispatch_keystroke(k, cx)).fold(false, |a, b| a | b))?;
            let state = window.update(cx, |client, _, cx| {
                // A dispatched keystroke has no key-up: the next one is a new press, never a repeat.
                client.key_down = None;
                cx.notify();
                client.state_json()
            })?;
            let mut body = state;
            body["taken"] = serde_json::Value::Bool(taken);
            Ok(("200 OK".to_owned(), body))
        }
        view::DebugAnswer::Snapshot(target) => {
            let any: gpui::AnyWindowHandle = window.into();
            let frame = any.update(cx, |_, window, cx| {
                window.draw(cx).clear();
                window.render_to_image()
            })?;
            let drawn: Result<(u32, u32, serde_json::Value)> = async {
                let mut frame = frame?;
                let panes = paste_native_panes(window, &mut frame, cx).await?;
                frame.save(&target)?;
                Ok((frame.width(), frame.height(), panes))
            }
            .await;
            match drawn {
                Ok((w, h, panes)) => {
                    let _ = window.update(cx, |client, _, _| client.log(format!("debug: window snapshot saved to {target}, {w}x{h}, native panes {panes}")));
                    Ok(("200 OK".to_owned(), serde_json::json!({"path": target, "width": w, "height": h, "native_panes": panes})))
                }
                Err(e) => Ok(("500 Internal Server Error".to_owned(), debug::error(&format!("the window could not be rendered to {target}: {e}")))),
            }
        }
    }
}

/// `POST /measure/sweep?seconds=2.5&by=300`: the window width swings `by` points either side of
/// where it starts, twice, for `seconds`, as the Swift app's `POST /windows?action=sweep`
/// does, one step due every 1/60 s. Each step resizes the window and draws one frame (`Window::draw`, the call the hidden
/// check uses, since a window never shown has no display link); the reply is the draw times.
async fn measure_sweep(window: view::ClientWindow, request: &debug::Request, cx: &mut gpui::AsyncApp) -> serde_json::Value {
    let num = |name: &str, default: f64| request.query.get(name).and_then(|v| v.parse::<f64>().ok()).unwrap_or(default);
    let (seconds, by) = (num("seconds", 2.5), num("by", 300.));
    let any: gpui::AnyWindowHandle = window.into();
    let Ok(start) = window.update(cx, |_, w, _| w.viewport_size()) else { return debug::error("no window") };
    let began = std::time::Instant::now();
    let (mut draws, mut gaps) = (vec![], vec![]);
    let mut last = began;
    loop {
        let t = began.elapsed().as_secs_f64();
        let width = if t < seconds { f64::from(f32::from(start.width)) + by * (t / seconds * 4. * std::f64::consts::PI).sin() } else { f64::from(f32::from(start.width)) };
        let ms = any.update(cx, |_, w, cx| {
            w.resize(gpui::size(gpui::px(width as f32), start.height));
            let drawn = std::time::Instant::now();
            w.draw(cx).clear();
            drawn.elapsed().as_secs_f64() * 1000.
        });
        let Ok(ms) = ms else { break };
        let now = std::time::Instant::now();
        gaps.push(now.duration_since(last).as_secs_f64() * 1000.);
        last = now;
        draws.push(ms);
        if t >= seconds {
            break;
        }
        // The next frame is due on a 60 Hz schedule from the start, so a slow frame is not
        // followed by a full interval of its own.
        let due = began + std::time::Duration::from_secs_f64((draws.len() as f64) / 60.);
        cx.background_executor().timer(due.saturating_duration_since(std::time::Instant::now())).await;
    }
    let stats = |v: &[f64]| {
        let mut s = v.to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let at = |p: f64| s.get(((s.len() as f64 - 1.) * p).round() as usize).copied().unwrap_or(0.);
        serde_json::json!({"median": at(0.5), "p95": at(0.95), "max": at(1.), "over_34": s.iter().filter(|m| **m > 34.).count()})
    };
    serde_json::json!({"frames": draws.len(), "wall_s": began.elapsed().as_secs_f64(), "draw_ms": stats(&draws), "frame_gap_ms": stats(&gaps)})
}

/// Answer the debug server's requests against `window` until the window closes or the server stops.
pub fn serve_debug(cx: &mut App, window: view::ClientWindow, mut server: debug::Server) {
    cx.spawn(async move |cx| {
        while let Some(request) = server.requests.next().await {
            // Measure mode (wheel-ynjo.19): a resize sweep that times every frame's draw.
            if request.method == "POST" && request.path == "/measure/sweep" {
                let report = measure_sweep(window, &request, cx).await;
                request.respond(("200 OK".to_owned(), report));
                continue;
            }
            match answer_debug(window, &request, cx).await {
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
) -> Result<view::ClientWindow> {
    open_window_as(cx, bridge, endpoint, prefer_session, prefer_space, hidden, Opening::Main, new_window_frame())
}

/// Where a new main window opens: where the last one closed, moved down and right by a title bar
/// while an open window already sits there, so the two never stack exactly.
fn new_window_frame() -> Option<saved::Frame> {
    windows::with(|r| {
        let mut frame = r.last_closed?;
        while r.frames.values().any(|f| f[0] == frame[0] && f[1] == frame[1]) {
            frame[0] += 28.;
            frame[1] += 28.;
        }
        Some(frame)
    })
}

/// `open_window` for a main or a little window, at `frame` when it is restored.
#[allow(clippy::too_many_arguments)]
pub fn open_window_as(
    cx: &mut App,
    bridge: &Rc<native::Bridge>,
    endpoint: daemon::Endpoint,
    prefer_session: Option<String>,
    prefer_space: Option<String>,
    hidden: bool,
    opening: Opening,
    frame: Option<saved::Frame>,
) -> Result<view::ClientWindow> {
    // The window holds the only sender: when it closes, the socket closes with it, and wheeld
    // forgets its view and its offer to run window commands at once.
    let daemon::Connection { send, mut incoming } = daemon::Connection::open(endpoint.clone());
    let window = create_window(cx, bridge, send, endpoint, prefer_session, prefer_space, hidden, opening, frame)?;
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
pub fn deliver(cx: &mut App, window: view::ClientWindow, event: daemon::Event) -> Result<bool> {
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
    let (layout, plugins_due) = window.update(cx, |client, _, cx| {
        let layout = client.on_event(event);
        cx.notify();
        (layout, client.take_plugins_due())
    })?;
    // A `plugin_ls` the throttle held back goes out when its window ends.
    if let Some(wait) = plugins_due {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(wait).await;
            let _ = window.update(cx, |client, _, cx| {
                client.plugins_timer_fired();
                cx.notify();
            });
        })
        .detach();
    }
    Ok(layout)
}

/// Answer a window command wheeld forwarded to `via` (AppModel.answerWindowCommand): run it on
/// the app's windows and send what it came to back through `via`'s connection.
pub fn answer_window_command(cx: &mut App, via: view::ClientWindow, id: u64, command: WindowCommand) {
    let result = run_window_command(cx, via, &command);
    let _ = via.update(cx, |client, _, cx| {
        client.window_answer(id, &command, result);
        cx.notify();
    });
}

fn run_window_command(cx: &mut App, via: view::ClientWindow, command: &WindowCommand) -> Result<serde_json::Value, String> {
    let gone = |_| "the window wheeld asked has closed".to_owned();
    let (sessions, spaces) = via.read(cx).map(|c| c.layout_lists()).map_err(gone)?;
    let target = |text: &str| -> Result<(String, view::ClientWindow), String> {
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
            // The window may be the one answering: it goes after its answer is queued, and its
            // connection writes every frame queued before the window's sender drops.
            if handle.update(cx, |client, _, _| client.log_close()).is_ok() {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                });
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

/// At launch (AppModel.openLaunchWindows): open what `file` saved, per `saved::Saved::launch_plan`,
/// and keep writing the windows to it. `space` (`--space`) shows that space in the first window.
/// The handles in the order they opened; the focused window is the last main one.
pub fn launch(cx: &mut App, bridge: &Rc<native::Bridge>, endpoint: daemon::Endpoint, space: Option<String>, hidden: bool, file: Option<std::path::PathBuf>) -> Result<Vec<view::ClientWindow>> {
    let saved = saved::Saved::load(file.as_ref());
    windows::with(|r| r.last_closed = saved.last_closed);
    saved::start(cx, file, saved.restore);
    let opened = open_saved(cx, bridge, endpoint, space, hidden, &saved);
    if opened.is_err() {
        saved::stop();
    }
    opened
}

/// Open the windows `saved` says launch opens.
pub fn open_saved(cx: &mut App, bridge: &Rc<native::Bridge>, endpoint: daemon::Endpoint, space: Option<String>, hidden: bool, saved: &saved::Saved) -> Result<Vec<view::ClientWindow>> {
    let plan = saved.launch_plan();
    eprintln!("wheel-gpui: launch opens {} window(s), restore windows at launch {}", plan.len(), if saved.restore { "on" } else { "off" });
    let mut handles = vec![];
    let mut space = space;
    for launch in plan {
        let handle = match launch {
            saved::Launch::Main(w) => {
                let handle = open_window_as(cx, bridge, endpoint.clone(), w.session, space.take().or(w.active), hidden, Opening::Main, w.frame)?;
                let _ = handle.update(cx, |client, _, cx| {
                    client.set_sidebar(w.sidebar);
                    client.set_window_name(w.name);
                    client.set_pinned(w.pinned);
                    cx.notify();
                });
                handle
            }
            saved::Launch::Little(l) => {
                let handle = open_window_as(cx, bridge, endpoint.clone(), None, Some(l.space), hidden, Opening::Little, l.frame)?;
                let _ = handle.update(cx, |client, _, cx| {
                    client.set_pinned(l.pinned);
                    cx.notify();
                });
                handle
            }
        };
        handles.push(handle);
    }
    if let Some(last) = handles.iter().rev().find(|h| h.read(cx).is_ok_and(|c| !c.is_little())) {
        bring_front(cx, *last);
    }
    Ok(handles)
}

/// New Window, `wheel window new` and the Dock icon (AppModel.newWindow): a main window on
/// `session`, else the focused window's, showing `space` when one is named, with the focused
/// window's sidebar and `name`, brought front.
pub fn new_window(cx: &mut App, bridge: &Rc<native::Bridge>, endpoint: daemon::Endpoint, hidden: bool, session: Option<String>, space: Option<String>, name: Option<String>) -> Result<view::ClientWindow> {
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

/// The red light: the window closes at once, and a session no other window shows stays in the
/// background.
pub fn red_light(window: &mut gpui::Window, cx: &mut App) -> bool {
    if let Some(client) = view::ClientWindow::client_of(window, cx) {
        client.update(cx, |client, _| client.log_close());
    }
    true
}

/// Bring a window front and make it key, unless the app runs hidden for a check.
pub fn bring_front(cx: &mut App, handle: view::ClientWindow) {
    if handle.read(cx).is_ok_and(|c| !c.is_hidden()) {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    }
}
