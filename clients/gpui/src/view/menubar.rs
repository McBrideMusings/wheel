//! The menu bar item (WheelApp.swift's MenuBarExtra, MenuBarView.swift): it lives while the app
//! runs, with or without a window, so it holds its own connection to wheeld for the sessions,
//! spaces and waiting approvals, and reads the app's list of windows for what they show. The
//! bridge owns the NSStatusItem (`native::APP`); a click on it opens the popover, a GPUI pop-up
//! window under the item, with a card per session and New Window and Quit Wheel.
use super::settings::{BOX, DIM, LINE, PANEL, TEXT};
use super::*;
use crate::menubar::{self, CARD_HEAD, CARD_PAD, Card, GAP, PAD, ROW, ROW_GAP, WIDTH};
use futures::StreamExt as _;
use gpui::{Entity, WindowBounds, WindowHandle, WindowKind, WindowOptions, point, size};
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

/// Orange behind a waiting count and green for the shown dot (SpaceRow).
const ORANGE: u32 = 0xff9f0a;
const GREEN: u32 = 0x30d158;

pub struct MenuBar {
    /// The bridge's app half: the status item and the Dock tile.
    native: Rc<Native>,
    bridge: Rc<Bridge>,
    endpoint: Endpoint,
    hidden: bool,
    /// The connection's sender; dropping it closes the socket.
    _send: mpsc::Sender<String>,
    up: bool,
    sessions: Vec<Session>,
    spaces: Vec<Space>,
    perms: Vec<PermInfo>,
    /// What the item was last told: the badge, the accessibility label and the Dock tile's badge.
    told: Option<(usize, String, Option<String>)>,
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
    let crate::daemon::Connection { send, mut incoming } = crate::daemon::Connection::open(endpoint.clone());
    let mut changes = crate::windows::with(|r| r.watch());
    let menu = cx.new(|_| MenuBar {
        native, bridge: bridge.clone(), endpoint, hidden, _send: send, up: false, sessions: vec![], spaces: vec![], perms: vec![],
        told: None, item: Value::Null, popover: None, symbols: RefCell::default(), log: vec![],
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
            if frame["event"] == "status_item" {
                let at = frame["frame"].as_array().filter(|f| f.len() == 4).map(|f| [0, 1, 2, 3].map(|i| f[i].as_f64().unwrap_or(0.) as f32));
                held.update(cx, |m, cx| m.toggle_popover(at, cx));
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

/// The Dock icon was clicked: with no main window open, open one on the first session no window
/// shows (AppDelegate.applicationShouldHandleReopen, AppModel.openWindowForBackground).
pub fn reopen(cx: &mut App) {
    let Some(menu) = menu_bar() else { return };
    if crate::windows::with(|r| r.windows.is_empty()) {
        menu.update(cx, |m, cx| {
            let session = m.background_sessions().into_iter().next();
            m.log(format!("dock icon: no window open; opening one on {}", session.as_deref().unwrap_or("no session")));
            m.open_window(session, cx);
        });
    }
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
                let layout = crate::layout::parse_layout(&message).map(|layout| {
                    self.sessions = layout.sessions;
                    self.spaces = layout.spaces;
                });
                crate::perms::follow(&mut self.perms, &message) || layout.is_some()
            }
            Event::Up(_) => !std::mem::replace(&mut self.up, true),
            Event::Down(_) | Event::Incompatible { .. } => std::mem::replace(&mut self.up, false),
        };
        if changed {
            self.refresh(cx);
        }
    }

    /// The sessions no main window shows, in wheeld's order (WindowLayout.backgroundSessions).
    fn background_sessions(&self) -> Vec<String> {
        crate::windows::with(|r| self.sessions.iter().filter(|s| !r.shows_session(&s.id)).map(|s| s.id.clone()).collect())
    }

    /// Tell the item what it counts now, when that changed, and resize an open popover to its cards.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let waiting = menubar::waiting(&self.perms, &self.spaces);
        let (background, unseen) = crate::windows::with(|r| (menubar::windowless(r, &self.sessions, &self.spaces).len(), menubar::unseen(r, &waiting)));
        let told = (background, menubar::accessibility(background, unseen), (unseen > 0).then(|| unseen.to_string()));
        if self.told.as_ref() != Some(&told) {
            if self.told.as_ref().map(|t| t.0) != Some(background) {
                self.log(format!("menu bar badge: {}", if background == 0 { "none".to_owned() } else { background.to_string() }));
            }
            self.item = self.native.call(json!({"op": "status_item", "badge": told.0, "label": told.1, "dock": told.2}));
            self.told = Some(told);
        }
        if let Some(popover) = self.popover {
            let height = menubar::height(&self.cards());
            // After this update: a resize can draw the popover, which reads this model.
            cx.defer(move |cx| {
                let _ = popover.update(cx, |_, window, _| {
                    if (window.viewport_size().height.as_f32() - height).abs() > 0.5 {
                        window.resize(size(px(WIDTH), px(height)));
                    }
                });
            });
        }
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
                        Popover { menu: menu.clone() }
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

    /// What the check reads: the item, its counts, the cards and the popover.
    pub fn state_json(&self, cx: &mut App) -> Value {
        let cards = self.cards();
        // The content's size: a window that is on no screen (the check's) reports a default frame.
        let popover = self.popover.and_then(|p| p.update(cx, |_, window, _| (window.bounds().origin, window.viewport_size())).ok()).map(|(o, s)| {
            json!({"open": true, "frame": [o.x.as_f32(), o.y.as_f32(), s.width.as_f32(), s.height.as_f32()]})
        });
        json!({
            "up": self.up,
            "item": self.item,
            "told": self.told.as_ref().map(|t| json!({"badge": t.0, "label": t.1, "dock": t.2})),
            "waiting": self.perms.len(),
            "cards": cards.iter().map(|c| json!({"session": c.session, "name": c.name, "windows": c.windows,
                "rows": c.rows.iter().map(|r| json!({"space": r.space, "name": r.name, "waiting": r.waiting, "shown": r.shown, "little": r.ownerless_little})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "height": menubar::height(&cards),
            "popover": popover.unwrap_or(json!({"open": false})),
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

/// The popover's view: it draws the menu bar's model.
pub struct Popover {
    menu: Entity<MenuBar>,
}

impl Render for Popover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = self.menu.read(cx);
        let cards = menu.cards();
        let mut col = div().size_full().flex().flex_col().gap(px(GAP)).p(px(PAD)).bg(rgb(PANEL)).text_sm().text_color(rgb(TEXT));
        for card in &cards {
            col = col.child(menu.card(card));
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
        col.child(div().h(px(1.)).bg(rgb(LINE))).child(div().flex().flex_col().gap(px(ROW_GAP)).child(new_window).child(quit))
    }
}
