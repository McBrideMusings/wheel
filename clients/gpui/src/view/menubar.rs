//! The menu bar item (WheelApp.swift's MenuBarExtra, MenuBarView.swift): it lives while the app
//! runs, with or without a window, so it holds its own connection to wheeld for the sessions,
//! spaces and waiting approvals, and reads the app's list of windows for what they show. The
//! bridge owns the NSStatusItem (`native::APP`); a click on it opens the popover, a GPUI pop-up
//! window under the item, with the server line, a card per session and New Window and Quit Wheel;
//! a right-click opens the native menu of StatusItemMenu.swift. Stop, Start and Restart run
//! through the app's one server control (`view::server`).
use super::settings::{BOX, DIM, LINE, PANEL, TEXT};
use super::server;
use super::*;
use crate::server::{Facts, LinkKind, Listener};
use crate::menubar::{self, CARD_HEAD, CARD_PAD, Card, GAP, PAD, ROW, ROW_GAP, WIDTH};
use futures::StreamExt as _;
use gpui::{Entity, Image, ImageFormat, ImageSource, WindowBounds, WindowHandle, WindowKind, WindowOptions, img, point, size};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use std::{cell::Cell, sync::Arc};
use serde_json::{Value, json};
use wheel_proto::PermInfo;

thread_local! {
    /// The app's one menu bar item, from launch until the app quits.
    static MENU_BAR: RefCell<Option<Entity<MenuBar>>> = const { RefCell::new(None) };
}

/// The menu bar item, once `start` made it.
pub fn menu_bar() -> Option<Entity<MenuBar>> {
    MENU_BAR.with(|m| m.borrow().clone())
}

/// The main window that was key last, else the newest (Registry::focused), as its Client: where
/// the popover's pairing row and Settings… act.
pub fn focused_client(_: &App) -> Option<WindowHandle<Client>> {
    crate::windows::with(|r| r.focused().map(|w| w.id.clone())).and_then(|id| crate::window_handle(&id))
}

/// Orange behind a waiting count and green for the shown dot (SpaceRow).
const ORANGE: u32 = 0xff9f0a;
const GREEN: u32 = 0x30d158;
const RED: u32 = 0xff453a;
const ACCENT: u32 = 0x0a84ff;
/// The details' indent: past the app icon and its gap (ServerDetails' `.padding(.leading, 46)`).
const DETAILS_INDENT: f32 = 46.;

pub struct MenuBar {
    /// The bridge's app half: the status item and the Dock tile.
    native: Rc<Native>,
    bridge: Rc<Bridge>,
    endpoint: Endpoint,
    hidden: bool,
    /// The connection's sender; dropping it closes the socket.
    send: mpsc::Sender<String>,
    up: bool,
    /// The link as the server line reads it, and why it is down.
    link: LinkKind,
    down_reason: Option<String>,
    /// wheeld answered at another protocol: it waits for a restart.
    incompatible: bool,
    /// From wheeld's last `status`: its source hash, protocol and start (unix ms).
    running_source: Option<String>,
    protocol: Option<u32>,
    started: Option<u64>,
    /// The remote listener and the paired devices, for the server line's details.
    remote: Option<crate::settings::Remote>,
    paired: Vec<String>,
    /// The right-click menu as the bridge built it last.
    status_menu: Value,
    app_icon: RefCell<Option<Option<(Arc<Image>, f32)>>>,
    sessions: Vec<Session>,
    spaces: Vec<Space>,
    perms: Vec<PermInfo>,
    /// What the item was last told: the badge, the accessibility label, the Dock tile's badge and
    /// whether a new server waits for a restart (the update dot).
    told: Option<(usize, String, Option<String>, bool)>,
    /// The bridge's account of the item after it was last told.
    item: Value,
    popover: Option<WindowHandle<Popover>>,
    symbols: Symbols,
    log: Vec<String>,
}

/// Make the menu bar item: its own connection over `endpoint`, the status item, and the tasks
/// that feed it wheeld's frames, its clicks and every change to the app's windows.
pub fn start(cx: &mut App, bridge: &Rc<Bridge>, endpoint: Endpoint, hidden: bool) -> Entity<MenuBar> {
    let (frames, mut clicks) = futures::channel::mpsc::unbounded::<String>();
    let native = Native::app(bridge, hidden, frames);
    server::ensure(cx, endpoint.service.clone(), hidden);
    let crate::daemon::Connection { send, mut incoming } = crate::daemon::Connection::open(endpoint.clone());
    let mut changes = crate::windows::with(|r| r.watch());
    let menu = cx.new(|_| MenuBar {
        native, bridge: bridge.clone(), endpoint, hidden, send, up: false, link: LinkKind::Connecting, down_reason: None, incompatible: false,
        running_source: None, protocol: None, started: None, remote: None, paired: vec![], status_menu: Value::Null, app_icon: RefCell::default(),
        sessions: vec![], spaces: vec![], perms: vec![], told: None, item: Value::Null, popover: None, symbols: RefCell::default(), log: vec![],
    });
    MENU_BAR.with(|m| *m.borrow_mut() = Some(menu.clone()));
    menu.update(cx, |m, cx| m.refresh(cx));
    let held = menu.clone();
    cx.spawn(async move |cx| {
        while let Some(event) = incoming.next().await {
            held.update(cx, |m, cx| m.on_event(event, cx));
        }
    })
    .detach();
    let held = menu.clone();
    cx.spawn(async move |cx| {
        while let Some(frame) = clicks.next().await {
            let Ok(frame) = serde_json::from_str::<Value>(&frame) else { continue };
            match frame["event"].as_str() {
                Some("status_item") if frame["button"] == "right" => held.update(cx, |m, _| m.open_status_menu()),
                Some("status_item") => {
                    let at = frame["frame"].as_array().filter(|f| f.len() == 4).map(|f| [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(0.) as f32));
                    held.update(cx, |m, cx| m.toggle_popover(at, cx));
                }
                Some("status_menu") => {
                    let item = frame["item"].as_str().unwrap_or_default().to_owned();
                    held.update(cx, |m, cx| m.choose(&item, cx));
                }
                Some("alert") => {
                    let (id, button) = (frame["id"].as_str().unwrap_or_default().to_owned(), frame["button"].as_i64().unwrap_or(-1));
                    held.update(cx, |m, cx| m.alert_answered(&id, button, cx));
                }
                _ => {}
            }
        }
    })
    .detach();
    let held = menu.clone();
    cx.spawn(async move |cx| {
        while changes.next().await.is_some() {
            held.update(cx, |m, cx| m.refresh(cx));
        }
    })
    .detach();
    menu
}

/// The Dock icon was clicked with no window visible: open one on the first session no window
/// shows, or on the focused window's session when every session is shown
/// (AppDelegate.applicationShouldHandleReopen, AppModel.openWindowForBackground). GPUI calls this
/// only when AppKit's `hasVisibleWindows` is false (gpui_macos platform.rs `should_handle_reopen`),
/// and a minimised window is not visible, so this opens one whether the windows are all closed or
/// all minimised.
pub fn reopen(cx: &mut App) {
    let Some(menu) = menu_bar() else { return };
    menu.update(cx, |m, cx| {
        let session = m.background_sessions().into_iter().next();
        let open = crate::windows::with(|r| r.windows.len());
        m.log(format!("dock icon: no window visible ({open} open); opening one on {}", session.as_deref().unwrap_or("the focused window's session")));
        m.open_window(session, cx);
    });
}

impl MenuBar {
    fn log(&mut self, line: String) {
        eprintln!("wheel-gpui: {line}");
        self.log.push(line);
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        // Only a layout or a change to the waiting requests changes what the item counts; the
        // agents' streams and every other frame pass by.
        let changed = match event {
            Event::Message(message) => {
                let server_line = self.on_server_message(&message);
                let layout = crate::layout::parse_layout(&message).map(|layout| {
                    self.sessions = layout.sessions;
                    self.spaces = layout.spaces;
                });
                crate::perms::follow(&mut self.perms, &message) || layout.is_some() || server_line
            }
            Event::Up(_) => {
                (self.link, self.down_reason, self.incompatible) = (LinkKind::Connected, None, false);
                server::link(true);
                self.ask_remote();
                self.up = true;
                true
            }
            Event::Down(reason) => {
                (self.link, self.down_reason, self.incompatible) = (LinkKind::Unreachable, Some(reason), false);
                server::link(false);
                self.up = false;
                true
            }
            Event::Incompatible { server: protocol, .. } => {
                (self.link, self.down_reason, self.incompatible, self.protocol) = (LinkKind::Connected, None, true, Some(protocol));
                server::link(true);
                self.up = false;
                true
            }
        };
        if changed {
            self.refresh(cx);
        }
    }

    /// What the server line reads from wheeld's frames: its `status`, the remote listener and the
    /// paired devices; a `stopping` frame goes to the app's watch. True when the line changed.
    fn on_server_message(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::Status { protocol, uptime_secs, host, .. } => {
                self.protocol = Some(*protocol);
                self.started = Some(super::settings::now_ms().saturating_sub(uptime_secs * 1000));
                self.running_source = Some(host.source.clone());
                true
            }
            ServerMsg::Remote { listen, addresses, port, fingerprint, problem, .. } => {
                self.remote = Some(crate::settings::Remote { listen: listen.clone(), addresses: addresses.clone(), port: *port, fingerprint: fingerprint.clone(), problem: problem.clone() });
                true
            }
            ServerMsg::Devices { devices, .. } | ServerMsg::DevicesChanged { devices } => {
                self.paired = devices.iter().map(|d| d.name.clone()).collect();
                true
            }
            ServerMsg::Stopping => {
                server::stopping();
                false
            }
            _ => false,
        }
    }

    /// Ask wheeld for its remote listener and paired devices: on each connection and each popover open.
    fn ask_remote(&self) {
        let _ = self.send.send(crate::daemon::frame(&ClientMsg::RemoteInfo { req: Some(1) }));
        let _ = self.send.send(crate::daemon::frame(&ClientMsg::DeviceList { req: Some(2) }));
    }

    /// A newer server is installed and waits for a restart (AppModel.serverRestartWaiting): the
    /// update dot on the item, the orange word and Restart Server in the details.
    fn restart_waiting(&self) -> bool {
        let bundled = server::with(|s| s.control.service().and_then(|s| s.bundled_source.clone())).flatten();
        self.incompatible || crate::server::restart_waiting(self.up, self.running_source.as_deref(), bundled.as_deref(), self.protocol, crate::daemon::PROTOCOL)
    }

    fn stopped_by_menu(&self) -> bool {
        server::with(|s| s.stopped_by_menu).unwrap_or(false)
    }

    /// Whether the server runs, so the right-click menu's first item stops it (StatusItemMenu.running).
    fn running(&self) -> bool {
        self.up && !self.stopped_by_menu()
    }

    /// The server line's word and dot (AppModel.serverStatus).
    pub fn server_status(&self) -> crate::server::Status {
        let remote = self.remote.as_ref().map(|r| crate::server::Remote { problem: r.problem.is_some(), listening: r.listen.is_some() });
        crate::server::status(self.link, self.restart_waiting(), remote, self.stopped_by_menu())
    }

    /// The lines under the server line while it is open (AppModel.serverDetails).
    pub fn server_details(&self) -> Vec<String> {
        let listener = self.remote.as_ref().map(|r| match (&r.problem, &r.listen, r.default_address()) {
            (Some(problem), _, _) => Listener::Failed(problem.clone()),
            (None, Some(_), Some(address)) => Listener::Listening { address: address.to_owned(), port: r.port },
            _ => Listener::Off,
        });
        crate::server::details(&Facts {
            stopped: self.stopped_by_menu(),
            down: match self.link {
                LinkKind::Connected => None,
                LinkKind::Connecting => Some(None),
                LinkKind::Unreachable => Some(Some(self.down_reason.clone().unwrap_or_default())),
            },
            waiting: self.restart_waiting(),
            uptime_secs: self.started.filter(|_| self.link == LinkKind::Connected).map(|t| super::settings::now_ms().saturating_sub(t) / 1000),
            spaces: self.spaces.len(),
            listener,
            paired: self.paired.clone(),
        })
    }

    /// A right-click on the item: the bridge builds the native menu from these items and opens
    /// it under the item; a pick comes back as a `status_menu` event.
    fn open_status_menu(&mut self) {
        let items: Vec<Value> = crate::server::menu(self.running(), self.stopped_by_menu()).iter().map(|i| json!({"id": i.id, "title": i.title, "key": i.key})).collect();
        self.status_menu = self.native.call(json!({"op": "status_menu", "items": items}));
        self.log(format!("status item menu: {}", self.status_menu["menu"]));
    }

    /// A pick from the right-click menu (StatusItemMenu's actions). Stop Server… on a running
    /// server asks first in an alert; one that is not running has nothing to lose and stops at once.
    pub fn choose(&mut self, item: &str, cx: &mut Context<Self>) {
        self.log(format!("status item menu: {item} chosen"));
        let spaces = self.spaces.len();
        match item {
            "stop" if self.running() => {
                let (message, informative) = crate::server::stop_question(spaces);
                self.native.call(json!({"op": "alert", "id": "stop", "message": message, "informative": informative, "buttons": ["Stop Server", "Cancel"], "activate": !self.hidden}));
            }
            "stop" => server::stop(spaces),
            "start" => server::start(),
            "restart" => server::restart(spaces),
            "quit" => cx.quit(),
            _ => {}
        }
        cx.notify();
    }

    /// The stop question was answered: its first button stops the server.
    fn alert_answered(&mut self, id: &str, button: i64, cx: &mut Context<Self>) {
        self.log(format!("alert {id} answered with button {button}"));
        if id == "stop" && button == 0 {
            server::stop(self.spaces.len());
        }
        cx.notify();
    }

    /// Whether the pairing row's link is up: connected, and not stopped from the menu.
    fn pairing_link(&self) -> bool {
        self.up && !self.stopped_by_menu()
    }

    /// The pairing row's facts: this connection's listener, with the address the focused window picked.
    fn pairing_devices(&self, cx: &App) -> crate::settings::Devices {
        let picked = focused_client(cx).and_then(|h| h.read(cx).ok()).and_then(|c| c.settings.devices.picked.clone());
        crate::settings::Devices { info: self.remote.clone(), picked, ..Default::default() }
    }

    /// Run `act` in the focused window's Client, or in one opened New Window's way when no window
    /// is open, after this update; `front` brings that window front.
    fn in_window(&mut self, front: bool, act: impl FnOnce(&mut Client) + 'static, cx: &mut Context<Self>) {
        let (bridge, endpoint, hidden) = (self.bridge.clone(), self.endpoint.clone(), self.hidden);
        cx.defer(move |cx| {
            let handle = match focused_client(cx) {
                Some(handle) => handle,
                None => match crate::new_window(cx, &bridge, endpoint, hidden, None, None, None) {
                    Ok(handle) => handle,
                    Err(error) => return eprintln!("wheel-gpui: the popover opened no window: {error:#}"),
                },
            };
            if let Err(error) = handle.update(cx, |client, _, cx| {
                act(client);
                cx.notify();
            }) {
                eprintln!("wheel-gpui: the popover's window is gone: {error:#}");
            }
            if front {
                if !hidden {
                    cx.activate(true);
                }
                crate::bring_front(cx, handle);
            }
        });
    }

    /// The pairing row's buttons (AppModel.copyPairingCode, showPairingWindow).
    fn pair(&mut self, ask: crate::pairing::Ask, cx: &mut Context<Self>) {
        self.log(format!("popover: pairing {} asked", if ask == crate::pairing::Ask::Copy { "copy" } else { "window" }));
        self.in_window(ask == crate::pairing::Ask::Show, move |c| c.pairing_from_menu(ask), cx);
    }

    /// Settings… (`openSettings`): the focused window's Settings, brought front.
    fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.in_window(true, |c| {
            if !c.settings_open {
                c.open_settings();
            }
        }, cx);
    }

    /// The bridge's app half, for the check's presses and picks.
    pub fn bridge_call(&self, value: Value) -> Value {
        self.native.call(value)
    }

    /// The app's icon as the bridge drew it, `size` points square, cached.
    fn app_icon(&self, size: f32) -> Option<gpui::AnyElement> {
        let mut cache = self.app_icon.borrow_mut();
        if cache.is_none() {
            let answer = self.native.call(json!({"op": "app_icon", "size": size}));
            let Some(hex) = answer["png_hex"].as_str() else { return None };
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect();
            *cache = Some(Some((Arc::new(Image::from_bytes(ImageFormat::Png, bytes)), size)));
        }
        cache.as_ref().and_then(|c| c.as_ref()).map(|(image, s)| img(ImageSource::Image(image.clone())).w(px(*s)).h(px(*s)).flex_none().into_any_element())
    }

    /// The sessions no main window shows, in wheeld's order (WindowLayout.backgroundSessions).
    fn background_sessions(&self) -> Vec<String> {
        crate::windows::with(|r| self.sessions.iter().filter(|s| !r.shows_session(&s.id)).map(|s| s.id.clone()).collect())
    }

    /// Tell the item what it counts now, when that changed, and resize an open popover to its cards.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let waiting = menubar::waiting(&self.perms, &self.spaces);
        let (background, unseen) = crate::windows::with(|r| (menubar::windowless(r, &self.sessions, &self.spaces).len(), menubar::unseen(r, &waiting)));
        let update = self.restart_waiting();
        let told = (background, menubar::accessibility(background, unseen, update), (unseen > 0).then(|| unseen.to_string()), update);
        if self.told.as_ref() != Some(&told) {
            if self.told.as_ref().map(|t| t.0) != Some(background) {
                self.log(format!("menu bar badge: {}", if background == 0 { "none".to_owned() } else { background.to_string() }));
            }
            if self.told.as_ref().map(|t| t.3) != Some(update) {
                self.log(format!("menu bar update dot: {}", if update { "shown" } else { "none" }));
            }
            self.item = self.native.call(json!({"op": "status_item", "badge": told.0, "label": told.1, "dock": told.2, "update": told.3}));
            self.told = Some(told);
        }
        // An open popover redraws from this model and fits the height its content measures.
        cx.notify();
    }

    pub fn cards(&self) -> Vec<Card> {
        let waiting = menubar::waiting(&self.perms, &self.spaces);
        crate::windows::with(|r| menubar::cards(r, &self.sessions, &self.spaces, &waiting))
    }

    /// A click on the item: open the popover under `at` (the item's frame on the main screen), or
    /// close it when it is open. The check opens it with no frame, at the screen's corner.
    pub fn toggle_popover(&mut self, at: Option<[f32; 4]>, cx: &mut Context<Self>) {
        if let Some(open) = self.popover.take() {
            let _ = open.update(cx, |_, window, _| window.remove_window());
            self.log("popover closed".into());
            return;
        }
        let height = menubar::height(&self.cards());
        let origin = at.map(|[x, y, w, h]| ((x + w / 2. - WIDTH / 2.).max(8.), y + h + 4.)).unwrap_or((0., 0.));
        let bounds = Bounds { origin: point(px(origin.0), px(origin.1)), size: size(px(WIDTH), px(height)) };
        let hidden = self.hidden;
        if !hidden {
            cx.activate(true);
        }
        self.ask_remote();
        let menu = cx.entity();
        // Opened after this update: a new window draws at once, and the popover reads this model.
        cx.defer(move |cx| {
            let opened = cx.open_window(
                WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), kind: WindowKind::PopUp, titlebar: None, focus: !hidden, show: !hidden, is_movable: false, ..Default::default() },
                |window, cx| {
                    // The popover closes when the person clicks anywhere else (MenuBarExtra's window style).
                    cx.new(|cx| {
                        cx.observe(&menu, |_, _, cx| cx.notify()).detach();
                        let pressed = menu.clone();
                        cx.observe_window_activation(window, move |_: &mut Popover, window, cx| {
                            if !window.is_window_active() && !pressed.read(cx).pointer_on_item() {
                                window.remove_window();
                            }
                        })
                        .detach();
                        let held = menu.clone();
                        cx.on_release(move |_, cx| {
                            held.update(cx, |m, _| m.popover = None);
                        })
                        .detach();
                        let focus = cx.focus_handle();
                        window.focus(&focus, cx);
                        Popover { menu: menu.clone(), focus, content: Rc::default(), controls: Rc::default(), cards: Rc::default(), view: Rc::default() }
                    })
                },
            );
            menu.update(cx, |m, _| match opened {
                Ok(handle) => {
                    m.popover = Some(handle);
                    m.log(format!("popover opened at {:.0},{:.0}, {height:.0} tall", origin.0, origin.1));
                }
                Err(error) => m.log(format!("popover did not open: {error:#}")),
            });
        });
    }

    /// Whether the pointer is on the status item: a press there toggles the popover itself.
    fn pointer_on_item(&self) -> bool {
        self.native.call(json!({"op": "pointer_on_item"}))["pointer_on_item"] == true
    }

    /// Press the status item as a click does (the check's item is never in the menu bar): the
    /// bridge sends its click back, which opens the popover. The bridge's account of the item.
    pub fn press_item(&self) -> Value {
        self.native.call(json!({"op": "press_status_item"}))
    }

    pub fn popover(&self) -> Option<WindowHandle<Popover>> {
        self.popover
    }

    /// New Window (AppModel.newWindow): a main window on the focused window's session, or on
    /// `session`, brought front; the popover closes.
    fn open_window(&mut self, session: Option<String>, cx: &mut Context<Self>) {
        let (bridge, endpoint, hidden) = (self.bridge.clone(), self.endpoint.clone(), self.hidden);
        cx.defer(move |cx| {
            if let Err(error) = crate::new_window(cx, &bridge, endpoint, hidden, session, None, None) {
                eprintln!("wheel-gpui: New Window failed: {error:#}");
            }
        });
    }

    /// The popover's cards as a space drag meets them, in screen points from the main screen's
    /// top left: none while the popover is closed or not yet drawn.
    pub fn drop_popover(&self, cx: &App) -> Option<crate::sidebar::DropPopover> {
        use crate::layout::Rect;
        let popover = self.popover?.read(cx).ok()?;
        let view = popover.view.get();
        if view == 0 {
            return None;
        }
        let at = self.native.call(json!({"op": "view_frame", "view": view}))["frame"].as_array()?.iter().map(|v| v.as_f64().unwrap_or(0.) as f32).collect::<Vec<_>>();
        let [x, y, w, h] = at[..] else { return None };
        let rows: HashMap<String, Vec<String>> = self.cards().into_iter().map(|c| (c.session, c.rows.into_iter().map(|r| r.space).collect())).collect();
        let cards = popover.cards.borrow().iter().map(|(session, [cx_, cy, cw, ch])| {
            let frame = Rect { x: x + cx_, y: y + cy, w: *cw, h: *ch };
            let top = frame.y + 1. + CARD_PAD + CARD_HEAD;
            let spaces = rows.get(session).cloned().unwrap_or_default();
            let rows = (0..spaces.len()).map(|i| Rect { x: frame.x + CARD_PAD, y: top + i as f32 * ROW, w: frame.w - 2. * CARD_PAD, h: ROW }).collect();
            crate::sidebar::DropCard { session: session.clone(), frame, rows, spaces }
        }).collect();
        Some(crate::sidebar::DropPopover { frame: Rect { x, y, w, h }, cards })
    }

    /// What the check reads: the item, its counts, the cards and the popover.
    pub fn state_json(&self, cx: &mut App) -> Value {
        let cards = self.cards();
        // The content's size: a window that is on no screen (the check's) reports a default frame.
        let popover = self.popover.and_then(|p| p.update(cx, |pop, window, _| (window.bounds().origin, window.viewport_size(), pop.content.get(), pop.controls.borrow().clone())).ok()).map(|(o, s, content, controls)| {
            json!({"open": true, "frame": [o.x.as_f32(), o.y.as_f32(), s.width.as_f32(), s.height.as_f32()], "content_height": content, "controls": controls})
        });
        let devices = self.pairing_devices(cx);
        let pairing = json!({"disabled": crate::pairing::Pairing::disabled_reason(self.pairing_link(), &devices), "row": crate::pairing::Pairing::row_detail(self.pairing_link(), &devices)});
        let status = self.server_status();
        let server = server::with(|s| json!({
            "word": status.word, "colour": format!("{:06x}", status.colour), "expanded": s.expanded, "confirming": s.confirming,
            "stopped_by_menu": s.stopped_by_menu, "running": self.running(), "waiting": self.restart_waiting(), "details": self.server_details(),
            "problem": s.problem, "busy": s.busy.map(|op| format!("{op:?}")), "available": s.control.available(), "log": s.log,
        })).unwrap_or(Value::Null);
        json!({
            "up": self.up,
            "item": self.item,
            "told": self.told.as_ref().map(|t| json!({"badge": t.0, "label": t.1, "dock": t.2, "update": t.3})),
            "server": server,
            "status_menu": self.status_menu,
            "waiting": self.perms.len(),
            "spaces": self.spaces.len(),
            "cards": cards.iter().map(|c| json!({"session": c.session, "name": c.name, "windows": c.windows,
                "rows": c.rows.iter().map(|r| json!({"space": r.space, "name": r.name, "waiting": r.waiting, "shown": r.shown, "little": r.ownerless_little})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "height": menubar::height(&cards),
            "popover": popover.unwrap_or(json!({"open": false})),
            "pairing": pairing,
            "log": self.log,
        })
    }

    fn symbol(&self, name: &str, size: f32, colour: u32) -> Option<gpui::AnyElement> {
        symbol(&self.native, &self.symbols, name, size, colour)
    }

    /// One line of the popover (PopoverRow): an icon, a title, trailing text.
    fn row(&self, icon: &str, title: &str, trailing: Option<&str>) -> gpui::Div {
        let mut row = div().h(px(ROW)).flex().items_center().gap_2().px_1().rounded_sm().cursor_pointer().hover(|s| s.bg(rgb(BOX)))
            .child(div().w(px(18.)).flex().justify_center().children(self.symbol(icon, 13., DIM)))
            .child(div().flex_1().text_color(rgb(TEXT)).child(title.to_owned()));
        if let Some(trailing) = trailing {
            row = row.child(div().text_color(rgb(DIM)).child(trailing.to_owned()));
        }
        row
    }

    /// A window's numbered mark (PlaceGlyph `.window`): a frame with a title bar and the number.
    fn window_glyph(number: usize) -> gpui::Div {
        div().w(px(15.)).h(px(12.)).rounded(px(2.)).border_1().border_color(rgb(DIM)).border_t_2().flex().items_center().justify_center()
            .text_color(rgb(DIM)).text_size(px(7.)).font_weight(gpui::FontWeight::BOLD).child(number.to_string())
    }

    fn card(&self, card: &Card) -> gpui::Div {
        let mut head = div().h(px(CARD_HEAD)).pb(px(4.)).flex().items_center().gap_2()
            .child(div().w(px(16.)).flex().justify_center().children(self.symbol("rectangle.stack", 12., DIM)))
            .child(div().text_color(rgb(TEXT)).font_weight(gpui::FontWeight::SEMIBOLD).overflow_hidden().whitespace_nowrap().child(card.name.clone()));
        if card.windows.is_empty() {
            head = head.child(div().text_xs().text_color(rgb(DIM)).child("No window"));
        } else {
            head = head.children(card.windows.iter().map(|n| Self::window_glyph(*n)));
        }
        let count = card.rows.len();
        head = head.child(div().flex_1()).child(div().text_xs().text_color(rgb(DIM)).child(format!("{count} {}", if count == 1 { "space" } else { "spaces" })));
        let mut body = div().flex().flex_col().p(px(CARD_PAD)).rounded(px(8.)).bg(rgb(BOX)).border_1().border_color(rgb(LINE)).child(head);
        if card.rows.is_empty() {
            body = body.child(div().h(px(ROW)).flex().items_center().text_xs().text_color(rgb(DIM)).child("No spaces. Drop one here."));
        }
        for row in &card.rows {
            let mut line = div().h(px(ROW)).flex().items_center().gap_2()
                .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_color(rgb(TEXT)).child(row.name.clone()));
            if row.ownerless_little {
                // PlaceGlyph `.little`: a frame with the small window in its lower right.
                line = line.child(div().relative().w(px(13.)).h(px(10.)).rounded(px(2.)).border_1().border_color(rgb(DIM))
                    .child(div().absolute().right(px(1.)).bottom(px(1.)).w(px(4.)).h(px(3.)).rounded(px(1.)).bg(rgb(DIM))));
            }
            if row.waiting > 0 {
                line = line.child(div().min_w(px(16.)).h(px(16.)).px(px(4.)).rounded_full().bg(rgb(ORANGE)).flex().items_center().justify_center()
                    .text_size(px(10.)).font_weight(gpui::FontWeight::BOLD).text_color(rgb(0xffffff)).child(row.waiting.to_string()));
            } else if row.shown {
                line = line.child(div().w(px(7.)).h(px(7.)).rounded_full().bg(rgb(GREEN)));
            }
            body = body.child(line);
        }
        body
    }
}

/// The popover's view: it draws the menu bar's model, and fits its window to the height its
/// content measures (MenuBarExtra sizes its window to the SwiftUI content).
pub struct Popover {
    menu: Entity<MenuBar>,
    focus: gpui::FocusHandle,
    /// The content's height as last laid out.
    content: Rc<Cell<f32>>,
    /// Each named control's frame in the popover, as last laid out, for the check's presses.
    controls: Rc<RefCell<HashMap<&'static str, [f32; 4]>>>,
    /// Each session's card frame in the popover, as last laid out: a space drag's targets.
    cards: Rc<RefCell<Vec<(String, [f32; 4])>>>,
    /// The popover's AppKit view, once drawn, for the bridge to measure and size its window.
    view: Rc<Cell<u64>>,
}

/// How a popover button looks: prominent (`.borderedProminent`), red prominent (the confirm's
/// Stop Server), destructive (`role: .destructive`) or plain.
#[derive(Clone, Copy)]
enum Look {
    Prominent,
    Red,
    Destructive,
    Plain,
}

impl Popover {
    /// Record `el`'s frame under `name` each time it is laid out.
    fn recorded(&self, el: gpui::Div, name: &'static str) -> gpui::Div {
        let controls = self.controls.clone();
        el.relative().child(canvas(move |b, _, _| {
            controls.borrow_mut().insert(name, [b.origin.x.as_f32(), b.origin.y.as_f32(), b.size.width.as_f32(), b.size.height.as_f32()]);
        }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
    }

    fn button(&self, label: &str, look: Look, name: &'static str, act: impl Fn(&mut Window, &mut App) + 'static) -> gpui::Div {
        let (bg, fg, border) = match look {
            Look::Prominent => (ACCENT, 0xffffff, ACCENT),
            Look::Red => (RED, 0xffffff, RED),
            Look::Destructive => (BOX, RED, LINE),
            Look::Plain => (BOX, TEXT, LINE),
        };
        let el = div().px(px(10.)).py(px(3.)).rounded(px(5.)).bg(rgb(bg)).border_1().border_color(rgb(border)).text_color(rgb(fg)).cursor_pointer()
            .child(label.to_owned())
            .on_mouse_up(MouseButton::Left, move |_, window, cx| act(window, cx));
        self.recorded(el, name)
    }

    /// The pairing row (PairingRow: copy the code, show it as a QR code) over Settings….
    fn pairing_rows(&self, menu: &MenuBar, devices: &crate::settings::Devices) -> gpui::Div {
        use crate::pairing::{Ask, Pairing};
        let reason = Pairing::disabled_reason(menu.pairing_link(), devices);
        let detail = Pairing::row_detail(menu.pairing_link(), devices);
        let icon_button = |symbol: &str, name: &'static str, ask: Ask, help: &str| {
            let tip = reason.unwrap_or(help);
            let mut button = div().id(name).w(px(22.)).h(px(22.)).flex().items_center().justify_center().rounded_sm()
                .tooltip(crate::tooltip::tip(tip))
                .children(menu.symbol(symbol, 13., if reason.is_some() { DIM } else { ACCENT }));
            if reason.is_some() {
                button = button.opacity(0.4);
            } else {
                let held = self.menu.clone();
                button = button.cursor_pointer().hover(|s| s.bg(rgb(BOX))).on_mouse_up(MouseButton::Left, move |_, window, cx| {
                    // The pairing window takes the person's eye; a copy leaves the popover open.
                    if ask == Ask::Show {
                        window.remove_window();
                    }
                    held.update(cx, |m, cx| {
                        if ask == Ask::Show {
                            m.popover = None;
                            m.log("popover closed by Show QR code".into());
                        }
                        m.pair(ask, cx);
                    });
                });
            }
            self.recorded(div().child(button), name)
        };
        let row = div().h(px(menubar::PAIRING_ROW)).flex().items_center().gap(px(4.)).px_1()
            .child(div().flex_1().min_w_0().flex().items_center().gap_2()
                .child(div().w(px(18.)).flex().justify_center().children(menu.symbol("iphone", 13., DIM)))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                    .child(div().text_color(rgb(TEXT)).child("Pairing code"))
                    .child(div().text_xs().text_color(rgb(DIM)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(detail))))
            .child(icon_button("doc.on.doc", "pairing-copy", Ask::Copy, "Copy the pairing code; the clipboard clears after a minute"))
            .child(icon_button("qrcode", "pairing-qr", Ask::Show, "Show the pairing code as a QR code"));
        let held = self.menu.clone();
        let settings = self.recorded(menu.row("gearshape", "Settings…", Some("⌘,")), "settings").on_mouse_up(MouseButton::Left, move |_, window, cx| {
            window.remove_window();
            held.update(cx, |m, cx| {
                m.popover = None;
                m.log("popover closed by Settings…".into());
                m.open_settings(cx);
            });
        });
        div().flex().flex_col().gap(px(ROW_GAP)).child(self.recorded(row, "pairing")).child(settings)
    }

    /// The server's line (ServerHeader): the app icon, its name, a dot with a word, and the
    /// chevron; pressing it opens the details.
    fn server_line(&self, menu: &MenuBar) -> gpui::Div {
        let status = menu.server_status();
        let expanded = server::with(|s| s.expanded).unwrap_or(false);
        let icon = menu.app_icon(menubar::SERVER_LINE).unwrap_or_else(|| div().w(px(menubar::SERVER_LINE)).h(px(menubar::SERVER_LINE)).into_any_element());
        let line = div().h(px(menubar::SERVER_LINE)).flex().items_center().gap(px(10.)).cursor_pointer()
            .child(icon)
            .child(div().flex().flex_col().gap(px(2.))
                .child(div().text_color(rgb(TEXT)).font_weight(gpui::FontWeight::SEMIBOLD).child("Wheel Server"))
                .child(div().flex().items_center().gap(px(5.))
                    .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(rgb(status.colour)))
                    .child(div().text_xs().text_color(rgb(DIM)).child(status.word))))
            .child(div().flex_1())
            .children(menu.symbol(if expanded { "chevron.up" } else { "chevron.down" }, 11., DIM))
            .on_mouse_up(MouseButton::Left, |_, window, _| {
                server::toggle_expanded();
                window.refresh();
            });
        let mut col = div().flex().flex_col().gap(px(8.)).child(self.recorded(line, "server"));
        if expanded {
            col = col.child(self.server_details(menu));
        }
        col
    }

    /// The details (ServerDetails): the lines, why the last command failed, and Restart, Start
    /// or Stop Server with its question.
    fn server_details(&self, menu: &MenuBar) -> gpui::Div {
        let (stopped, confirming, problem, unavailable) = server::with(|s| (s.stopped_by_menu, s.confirming, s.problem.clone(), s.control.unavailable())).unwrap_or((false, false, None, Some(crate::server::UNAVAILABLE)));
        let spaces = menu.spaces.len();
        let mut col = div().pl(px(DETAILS_INDENT)).flex().flex_col().gap(px(6.));
        for line in menu.server_details() {
            col = col.child(div().text_sm().text_color(rgb(DIM)).child(line));
        }
        if let Some(problem) = problem {
            col = col.child(div().flex().items_center().gap_1().text_xs().text_color(rgb(crate::server::ORANGE))
                .children(menu.symbol("exclamationmark.triangle.fill", 10., crate::server::ORANGE)).child(problem));
        }
        let row = |el: gpui::Div| div().flex().child(el);
        if menu.restart_waiting() && unavailable.is_none() {
            col = col.child(row(self.button("Restart Server", Look::Prominent, "restart", move |window, _| {
                server::restart(spaces);
                window.refresh();
            })));
        }
        if let Some(why) = unavailable {
            col = col.child(div().text_xs().text_color(rgb(DIM)).child(why));
        } else if stopped || !menu.up {
            col = col.child(row(self.button("Start Server", Look::Prominent, "start", |window, _| {
                server::start();
                window.refresh();
            })));
        } else if confirming {
            let (question, informative) = crate::server::stop_question(spaces);
            col = col.child(div().flex().flex_col().gap(px(6.)).p(px(10.)).rounded(px(8.)).bg(rgb(BOX)).border_1().border_color(rgb(LINE))
                .child(div().text_color(rgb(TEXT)).font_weight(gpui::FontWeight::SEMIBOLD).child(question))
                .child(div().text_sm().text_color(rgb(DIM)).child(informative))
                .child(div().flex().gap_2().child(div().flex_1())
                    .child(self.button("Cancel", Look::Plain, "cancel", |window, _| {
                        server::cancel_stop();
                        window.refresh();
                    }))
                    .child(self.button("Stop Server", Look::Red, "confirm-stop", move |window, _| {
                        server::stop(spaces);
                        window.refresh();
                    }))));
        } else {
            col = col.child(row(self.button("Stop Server", Look::Destructive, "stop", |window, _| {
                server::ask_stop();
                window.refresh();
            })));
        }
        col
    }
}

impl Render for Popover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = self.menu.read(cx);
        let cards = menu.cards();
        let devices = menu.pairing_devices(cx);
        let mut col = div().w_full().flex().flex_col().gap(px(GAP)).p(px(PAD)).text_sm().text_color(rgb(TEXT))
            .child(self.server_line(menu))
            .child(div().h(px(1.)).bg(rgb(LINE)))
            .child(self.pairing_rows(menu, &devices));
        self.cards.borrow_mut().clear();
        // A space dragged over a card outlines it (SessionCard's drop highlight).
        let over = super::sidebar::space_hover().and_then(|h| match h.target {
            Some(crate::sidebar::SpaceDropTarget::Card { session, index }) => Some((session, index, h.space)),
            _ => None,
        });
        for card in &cards {
            let (frames, session) = (self.cards.clone(), card.session.clone());
            let aimed = over.as_ref().filter(|(s, _, _)| *s == card.session);
            // The bar between the card's rows (SpaceDrag's card insertion), none beside the dragged row.
            let held: Vec<String> = card.rows.iter().map(|r| r.space.clone()).collect();
            let bar = aimed.and_then(|(_, index, space)| crate::sidebar::bar_index(*index, &held, space)).map(|i| {
                div().absolute().left(px(CARD_PAD)).right(px(CARD_PAD)).top(px(1. + CARD_PAD + CARD_HEAD + i as f32 * ROW - 1.5)).h(px(3.)).rounded_full().bg(rgb(ACCENT))
            });
            col = col.child(menu.card(card).when(aimed.is_some(), |d| d.border_color(rgb(ACCENT))).relative().children(bar).child(canvas(move |b, _, _| {
                frames.borrow_mut().push((session.clone(), [b.origin.x.as_f32(), b.origin.y.as_f32(), b.size.width.as_f32(), b.size.height.as_f32()]));
            }, |_, _, _, _| {}).absolute().top_0().left_0().size_full()));
        }
        let held = self.menu.clone();
        // The popover closes through the window the row is in: GPUI holds that window while it
        // dispatches the press, so its handle cannot reach it then.
        let new_window = menu.row("macwindow", "New Window", Some("⌘N")).on_mouse_up(MouseButton::Left, move |_, window, cx| {
            window.remove_window();
            held.update(cx, |m, cx| {
                m.popover = None;
                m.log("popover closed by New Window".into());
                m.open_window(None, cx);
            });
        });
        let quit = menu.row("power", "Quit Wheel", Some("⌘Q")).on_mouse_up(MouseButton::Left, |_, _, cx| cx.quit());
        col = col.child(div().h(px(1.)).bg(rgb(LINE))).child(div().flex().flex_col().gap(px(ROW_GAP)).child(new_window).child(quit));
        // The window follows the content's height, keeping its top edge under the item.
        let (content, native, ns_view) = (self.content.clone(), menu.native.clone(), self.view.clone());
        let fit = canvas(move |b, window, cx| {
            let height = b.size.height.as_f32();
            content.set(height);
            if let Ok(handle) = window.window_handle()
                && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
            {
                ns_view.set(appkit.ns_view.as_ptr() as usize as u64);
            }
            if (window.viewport_size().height.as_f32() - height).abs() > 0.5 {
                let view = ns_view.get();
                cx.defer(move |cx| {
                    // The view is the popover's: a popover closed since this draw is not touched.
                    if menu_bar().is_some_and(|m| m.read(cx).popover.is_some()) {
                        native.call(json!({"op": "fit_window", "view": view, "width": WIDTH, "height": height}));
                    }
                });
            }
        }, |_, _, _, _| {}).absolute().top_0().left_0().size_full();
        // The confirm's Cancel answers Escape (`.keyboardShortcut(.cancelAction)`).
        let keys = div().track_focus(&self.focus).on_key_down(|event: &KeyDownEvent, window, _| {
            if event.keystroke.key == "escape" && server::with(|s| s.confirming).unwrap_or(false) {
                server::cancel_stop();
                window.refresh();
            }
        });
        keys.size_full().bg(rgb(PANEL)).child(col.relative().child(fit))
    }
}
