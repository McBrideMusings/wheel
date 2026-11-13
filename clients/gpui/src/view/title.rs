//! The title row and the bottom row as drawn and driven (TitleRow.swift, SessionMenu.swift,
//! SpacesView.swift's SpaceTabs, ContentView.swift's ConnectionBadge, UsageViews.swift):
//! icon buttons in fixed slots with the `[[title.button]]` entries wheeld merged for the space
//! (ADR-0037), the session and mode menus under their buttons, the space tabs while the sidebar
//! is hidden; along the bottom edge the connection badge and the usage strip (ADR-0034), whose
//! gauge opens the Usage window over the grid.
use super::*;
use super::settings::{ACCENT, BOX, DIM, LINE, PANEL, TEXT, button, click};
use crate::strip::{self, Item};
use gpui::{Image, ImageFormat, ImageSource, TextRun, img, relative};
use serde_json::{Value, json};
use std::sync::Arc;
use wheel_proto::{TitleSide, UsageAccount, UsageAgent, UsageMeterLevel};

/// The bottom row's height: 6 pt over the 22 pt row and 8 pt under it.
pub const BOTTOM: f32 = 36.0;
const CARD_BG: u32 = 0x17201c;
const ORANGE: u32 = 0xff9f0a;
const BADGE_RED: u32 = 0xff453a;
const MENU_W: f32 = 240.0;
const MENU_ROW_H: f32 = 24.0;
const USAGE_W: f32 = 540.0;
const USAGE_H: f32 = 520.0;

/// A menu open under a title-row button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleMenu {
    Session,
    Mode,
    /// The title's own menu: Rename Space… and Rename Window….
    Title,
}

/// One row of a title menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRow {
    pub id: String,
    pub title: String,
    pub enabled: bool,
    pub checked: bool,
    /// A separator above this row.
    pub divided: bool,
}

/// The named colours a `state` command's `tint` may print; anything else draws in the primary colour.
pub fn tint(name: Option<&str>) -> Option<u32> {
    Some(match name? {
        "red" => 0xff453a,
        "orange" => 0xff9f0a,
        "yellow" => 0xffd60a,
        "green" => 0x30d158,
        "blue" => 0x0a84ff,
        "purple" => 0xbf5af2,
        "pink" => 0xff375f,
        "gray" | "grey" => 0x8e8e93,
        _ => return None,
    })
}

/// The SF Symbols the bridge drew, by name, size (tenths of a point) and colour.
pub type Symbols = RefCell<HashMap<(String, u32, u32), Option<(Arc<Image>, f32, f32)>>>;

/// An SF Symbol as an image `native` drew, cached in `cache`; None for a name the system lacks.
/// Every window and the menu bar's popover draw their symbols through this.
pub fn symbol(native: &Native, cache: &Symbols, name: &str, size: f32, color: u32) -> Option<gpui::AnyElement> {
    let key = (name.to_owned(), (size * 10.) as u32, color);
    let mut cache = cache.borrow_mut();
    if !cache.contains_key(&key) {
        let channel = |shift: u32| f64::from((color >> shift) & 255) / 255.;
        let answer = native.call(serde_json::json!({"op": "symbol", "name": name, "size": size, "color": [channel(16), channel(8), channel(0), 1.0]}));
        match (answer["png_hex"].as_str(), answer["width"].as_f64(), answer["height"].as_f64()) {
            (Some(hex), Some(w), Some(h)) => {
                let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect();
                cache.insert(key.clone(), Some((Arc::new(Image::from_bytes(ImageFormat::Png, bytes)), w as f32, h as f32)));
            }
            // A name the system lacks stays missing; a bridge not attached yet (the first frame) is asked again next frame.
            _ if answer["missing"] == true => {
                cache.insert(key.clone(), None);
            }
            _ => return None,
        }
    }
    cache.get(&key).and_then(|e| e.as_ref()).map(|(image, w, h)| img(ImageSource::Image(image.clone())).w(px(*w)).h(px(*h)).flex_none().into_any_element())
}

impl Client {
    // MARK: The title row

    /// The row as laid out for the window's current size and state.
    pub fn title_row(&self) -> header::Row {
        let spaces: Vec<(String, String)> = self.session_spaces().iter().map(|s| (s.id.clone(), s.name.clone())).collect();
        let plugins = self.space_id.as_ref().and_then(|s| self.title_buttons.get(s)).cloned().unwrap_or_default();
        let name = match self.space() {
            Some(space) => space.name.clone(),
            None if matches!(self.link, Link::Incompatible { .. }) => "Wheel".into(),
            None => "waiting for wheeld".into(),
        };
        // The title shows while the sidebar is open, and while its name field is up.
        header::row(self.viewport.0, self.sidebar_open || self.renaming_window(), &plugins, &spaces, self.space_id.as_deref(), &name, self.pinned, &self.mode)
    }
    /// The row's chips: every button, tab and the title, with the rectangle a check presses.
    pub fn header_chips(&self) -> Vec<header::Chip> {
        self.title_row().chips
    }

    /// Ask wheeld for the `[[title.button]]` entries of every space (`fresh`: a `status` reply,
    /// so every space is asked again) or of the spaces not asked yet, and forget spaces gone.
    pub fn ask_title_buttons(&mut self, fresh: bool) {
        let ids: Vec<String> = self.spaces.iter().map(|s| s.id.clone()).collect();
        self.title_buttons.retain(|id, _| ids.contains(id));
        self.title_asked.retain(|id| ids.contains(id));
        if fresh {
            self.title_asked.clear();
        }
        for id in ids {
            if self.title_asked.insert(id.clone()) {
                // wheeld answers only a request with an id.
                let req = self.req();
                self.send(&ClientMsg::TitleButtonLs { req: Some(req), space: id });
            }
        }
    }
    /// The title row's replies: the entries for a space, and the session this window made.
    pub fn on_title_message(&mut self, message: &ServerMsg) -> bool {
        match message {
            ServerMsg::TitleButtons { space, buttons, .. } | ServerMsg::TitleButtonsChanged { space, buttons } => {
                if self.title_buttons.get(space) != Some(buttons) {
                    let listed: Vec<String> = buttons.iter().map(|b| if b.hidden { format!("{} hidden", b.id) } else { format!("{}:{}", b.id, if b.side == TitleSide::Left { "left" } else { "right" }) }).collect();
                    self.log(format!("title buttons for {space}: {}", if listed.is_empty() { "none".to_owned() } else { listed.join(", ") }));
                    self.title_buttons.insert(space.clone(), buttons.clone());
                }
                true
            }
            ServerMsg::Usage { board, .. } => {
                self.take_usage_board(board);
                true
            }
            ServerMsg::Created { req, id, .. } if self.session_show_req == Some(*req) => {
                self.session_show_req = None;
                self.session_to_show = Some(id.clone());
                // The layout may already list it; else the next layout shows it.
                self.show_made_session();
                false
            }
            _ => false,
        }
    }
    /// A press on a chip of the row.
    pub fn press_title(&mut self, hit: &Hit) {
        self.press(format!("title {}", header::frame_id(hit).unwrap_or_else(|| match hit { Hit::Space(id) => format!("tab {id}"), _ => "title".into() })));
        // A press on any other control dismisses an open menu, as a press outside a menu does.
        if !matches!(hit, Hit::SessionMenu | Hit::Mode) {
            self.close_title_menu();
        }
        match hit {
            Hit::Sidebar => self.toggle_sidebar(),
            Hit::SessionMenu => self.toggle_title_menu(TitleMenu::Session),
            Hit::Mode => self.toggle_title_menu(TitleMenu::Mode),
            Hit::Plugin(id) => self.press_plugin(&id.clone()),
            Hit::Space(id) => self.select_space(&id.clone()),
            Hit::Title => {}
            Hit::Processes => self.toggle_processes(),
            Hit::Palette => self.open_command_palette(crate::command_palette::Mode::Root),
            Hit::Pin => self.set_pinned(!self.pinned),
            Hit::NewSpace => self.open_new_space_palette(),
        }
    }
    /// A press on a `[[title.button]]` entry: wheeld runs its command in the space's directory.
    /// An indicator, or a button its state disabled, takes no press.
    pub fn press_plugin(&mut self, id: &str) {
        let Some(space) = self.space_id.clone() else { return };
        let Some(info) = self.title_buttons.get(&space).and_then(|l| l.iter().find(|b| b.id == id)).cloned() else {
            self.log(format!("title button {id}: not in space {space}"));
            return;
        };
        if !info.has_command {
            self.log(format!("title button {id}: an indicator with no command; press ignored"));
            return;
        }
        if !info.enabled {
            self.log(format!("title button {id}: disabled by its state; press refused"));
            return;
        }
        self.log(format!("title button {id} pressed in space {space}"));
        self.send(&ClientMsg::TitleButtonPress { req: None, space, id: id.to_owned() });
    }
    /// What the window's body draws: `spaces` (the grid) or `desk` (`WindowMode`).
    pub fn set_mode(&mut self, mode: &str) {
        self.close_title_menu();
        if !header::MODES.iter().any(|m| m.0 == mode) {
            self.log(format!("window mode {mode}: no such mode"));
            return;
        }
        if self.mode != mode {
            self.mode = mode.to_owned();
            self.log(format!("window {}: mode {mode}", &self.window_id[..self.window_id.len().min(12)]));
        }
    }
    /// The pin: the window floats over every other while it is on.
    pub fn set_pinned(&mut self, on: bool) {
        if self.pinned == on {
            return;
        }
        self.pinned = on;
        self.publish();
        self.native.call(serde_json::json!({"op": "pin", "on": on}));
        self.log(format!("window {} {}", &self.window_id[..self.window_id.len().min(12)], if on { "pinned" } else { "unpinned" }));
    }

    // MARK: The session and mode menus

    pub fn toggle_title_menu(&mut self, menu: TitleMenu) {
        if self.title_menu == Some(menu) {
            self.close_title_menu();
        } else {
            self.title_menu = Some(menu);
            self.menu = None;
            self.close_space_menu();
            self.sync_overlay();
        }
    }
    pub fn close_title_menu(&mut self) {
        if self.title_menu.take().is_some() {
            self.sync_overlay();
        }
    }
    /// The rows of `menu`: the sessions with the shown one checked, New Session, Rename Session…
    /// and Close Session… (SessionMenu.swift); or the modes with the current one checked.
    pub fn title_menu_rows(&self, menu: TitleMenu) -> Vec<MenuRow> {
        match menu {
            TitleMenu::Session => {
                let current = self.session_id.as_deref();
                let mut rows: Vec<MenuRow> = self.sessions.iter().map(|s| MenuRow { id: format!("session:{}", s.id), title: s.name.clone(), enabled: true, checked: Some(s.id.as_str()) == current, divided: false }).collect();
                rows.push(MenuRow { id: "new-session".into(), title: "New Session".into(), enabled: self.link == Link::Up, checked: false, divided: true });
                rows.push(MenuRow { id: "rename-session".into(), title: "Rename Session…".into(), enabled: current.is_some() && self.link == Link::Up, checked: false, divided: false });
                rows.push(MenuRow { id: "close-session".into(), title: "Close Session…".into(), enabled: current.is_some(), checked: false, divided: false });
                rows
            }
            TitleMenu::Title => vec![
                MenuRow { id: "rename-space".into(), title: "Rename Space…".into(), enabled: self.space_id.is_some(), checked: false, divided: false },
                MenuRow { id: "rename-window".into(), title: "Rename Window…".into(), enabled: true, checked: false, divided: false },
            ],
            TitleMenu::Mode => header::MODES.iter().map(|(id, title, _)| MenuRow { id: format!("mode:{id}"), title: (*title).to_owned(), enabled: true, checked: self.mode == *id, divided: false }).collect(),
        }
    }
    /// Run a row of the open menu, which closes.
    pub fn run_title_menu(&mut self, id: &str) {
        let Some(menu) = self.title_menu else { return };
        let Some(row) = self.title_menu_rows(menu).into_iter().find(|r| r.id == id) else {
            self.log(format!("title menu: no row {id}"));
            return;
        };
        self.close_title_menu();
        if !row.enabled {
            self.log(format!("title menu: {id} not available"));
            return;
        }
        self.log(format!("title menu: {id}"));
        if let Some(session) = id.strip_prefix("session:") {
            self.select_session(session);
        } else if let Some(mode) = id.strip_prefix("mode:") {
            self.set_mode(mode);
        } else if id == "new-session" {
            // The Swift app shows the new session in the focused window once it exists.
            let taken: Vec<&str> = self.sessions.iter().map(|s| s.name.as_str()).collect();
            let name = header::session_name(&taken);
            let req = self.req();
            self.layout_reqs.insert(req);
            self.session_show_req = Some(req);
            self.send(&ClientMsg::SessionNew { req: Some(req), name });
        } else if id == "rename-session" && let Some(session) = self.session_id.clone() {
            self.begin_session_rename(&session);
        } else if id == "rename-window" {
            self.begin_window_rename();
        } else if id == "rename-space" && let Some(space) = self.space_id.clone() {
            self.begin_rename(&space);
        } else if id == "close-session"
            && let Some(session) = self.sessions.iter().find(|s| Some(&s.id) == self.session_id.as_ref()).cloned()
        {
            let message = self.close_warning(&session.id);
            self.confirm(&format!("Close the “{}” session?", session.name), message, "Close Session".into(), commands::Pending::CloseSession(session.id));
        }
    }
    /// What closing `session` ends, as a sentence (AppModel.closeWarning).
    pub fn close_warning(&self, session: &str) -> String {
        let held: Vec<&Space> = self.sessions.iter().find(|s| s.id == session).map(|s| s.spaces.iter().filter_map(|id| self.spaces.iter().find(|sp| &sp.id == id)).collect()).unwrap_or_default();
        if held.is_empty() {
            return "It holds no spaces.".into();
        }
        let panes: Vec<&Leaf> = held.iter().flat_map(|s| leaves_of(&s.root)).collect();
        let count = |n: usize, noun: &str| format!("{n} {noun}{}", if n == 1 { "" } else { "s" });
        let shells = panes.iter().filter(|p| p.kind == "terminal").count();
        let agents = panes.iter().filter(|p| p.kind == "agent").count();
        let mut text = format!("It closes {} and {}", count(held.len(), "space"), count(panes.len(), "pane"));
        let mut running = vec![];
        if shells > 0 {
            running.push(count(shells, "terminal"));
        }
        if agents > 0 {
            running.push(count(agents, "agent"));
        }
        if !running.is_empty() {
            text += &format!(", and ends {}", running.join(" and "));
        }
        text + ". Every window showing it closes."
    }
    /// A session this window asked for is listed now: show it.
    pub fn show_made_session(&mut self) {
        if let Some(id) = self.session_to_show.clone()
            && self.sessions.iter().any(|s| s.id == id)
        {
            self.session_to_show = None;
            self.select_session(&id);
        }
    }

    // MARK: Drawing

    /// An SF Symbol as an image the bridge drew, cached by name, size and colour; None for a
    /// name the system lacks.
    pub(super) fn symbol(&self, name: &str, size: f32, color: u32) -> Option<gpui::AnyElement> {
        symbol(&self.native, &self.symbols, name, size, color)
    }

    /// The row: its buttons, the title or the tabs, and the open menu.
    pub(super) fn title_element(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let row = self.title_row();
        let (w, _) = self.viewport;
        let mut bar = div().absolute().left_0().top_0().w(px(w)).h(px(header::HEIGHT)).bg(rgb(0x0e1311));
        if let Some(track) = row.track {
            bar = bar.child(div().absolute().left(px(track.x)).top(px(track.y)).w(px(track.w)).h(px(track.h)).rounded(px(9.)).bg(rgb(0x1a2420)));
        }
        let dragged = self.space_drag.as_ref().map(|d| d.space.clone());
        let tab_bar = self.space_drag_tab_bar().and_then(|i| {
            // Left of the tab at the drop, or right of the last; kept inside the track.
            let x = match row.tabs.get(i) { Some(t) => t.x, None => row.tabs.last().map(|t| t.x + t.w)? };
            let track = row.track?;
            let x = x.clamp(track.x + 1.5, track.x + track.w - 1.5);
            row.tabs.first().map(|t| Rect { x: x - 1.5, y: t.y, w: 3., h: t.h })
        });
        for chip in row.chips {
            let (r, hit) = (chip.rect, chip.hit.clone());
            let mut el = div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).flex().items_center().justify_center().overflow_hidden().whitespace_nowrap();
            match &chip.hit {
                Hit::Space(id) => {
                    let dots = self.spaces.iter().find(|s| &s.id == id).and_then(|s| status::space_dots(s, &self.status_dots));
                    // A space in a little window draws its tab dimmed (AppModel.isDimmed).
                    let away = self.little_showing(id).is_some();
                    // The tab a space drag holds draws dimmed, as its sidebar row does.
                    let held = dragged.as_deref() == Some(id.as_str());
                    el = el.when(held, |d| d.opacity(0.4)).when(away && !held, |d| d.opacity(super::little::AWAY_OPACITY)).rounded(px(7.)).px_2().gap_1().cursor_pointer().bg(rgb(if chip.active { ACCENT } else { 0x1a2420 }))
                        .text_color(rgb(if chip.active { TEXT } else { DIM }))
                        .child(div().overflow_hidden().whitespace_nowrap().child(chip.label.clone())).children(dots);
                }
                Hit::Title => {
                    // A double click names the window; a right press opens the title's menu.
                    el = el.justify_start().child(self.window_title_element(&chip.label))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            this.press_title(&Hit::Title);
                            if e.click_count >= 2 && !this.renaming_window() {
                                this.begin_window_rename();
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                            this.toggle_title_menu(TitleMenu::Title);
                            cx.stop_propagation();
                            cx.notify();
                        }));
                }
                _ => {
                    let plugin = matches!(chip.hit, Hit::Plugin(_));
                    let colour = if !chip.enabled { 0x5a7266 } else if plugin { tint(chip.tint.as_deref()).unwrap_or(TEXT) } else { DIM };
                    el = el.rounded_md().hover(|s| s.bg(rgb(0x1f2b26)));
                    if chip.pressable {
                        el = el.cursor_pointer();
                    }
                    let face: gpui::AnyElement = if chip.hit == Hit::Palette {
                        div().text_xs().font_weight(FontWeight::MEDIUM).text_color(rgb(colour)).child("⌘K").into_any_element()
                    } else {
                        self.symbol(&chip.icon, 13., colour).unwrap_or_else(|| div().text_xs().text_color(rgb(colour)).child(chip.icon.clone()).into_any_element())
                    };
                    el = el.child(div().relative().w(px(16.)).h(px(16.)).flex().items_center().justify_center().child(face).children(chip.badge.clone().map(|badge| {
                        div().absolute().top(px(-6.)).right(px(-8.)).px(px(3.)).rounded_full().bg(rgb(BADGE_RED)).text_color(rgb(0xffffff)).font_weight(FontWeight::BOLD)
                            .text_size(px(8.)).line_height(px(11.)).child(badge)
                    })));
                }
            }
            if chip.pressable {
                el = el.on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.press_title(&hit);
                    // A press on a tab that then moves drags its space, as a sidebar row's does.
                    if let Hit::Space(id) = &hit
                        && this.space_drag.is_none()
                    {
                        this.space_press = Some((id.clone(), f32::from(e.position.x), f32::from(e.position.y)));
                    }
                    cx.stop_propagation();
                    cx.notify();
                }));
            }
            bar = bar.child(el);
        }
        if let Some(r) = tab_bar {
            // The sidebar's bar colour, which stands out against the active tab's fill.
            bar = bar.child(div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).rounded_full().bg(rgb(super::sidebar::ACCENT)));
        }
        bar.children(self.title_menu_element(cx))
    }

    /// The open menu's card and its rows, each at its rectangle under the menu's button; what
    /// the element draws and a check presses.
    pub fn title_menu_placed(&self) -> Option<(Rect, Vec<(MenuRow, Rect)>)> {
        let menu = self.title_menu?;
        let anchor = self.header_chips().into_iter().find(|c| c.hit == match menu { TitleMenu::Session => Hit::SessionMenu, TitleMenu::Mode => Hit::Mode, TitleMenu::Title => Hit::Title })?.rect;
        let top = anchor.y + anchor.h + 2.;
        let mut y = top + 4.;
        let mut placed = vec![];
        for row in self.title_menu_rows(menu) {
            if row.divided {
                y += 7.;
            }
            placed.push((row, Rect { x: anchor.x + 4., y, w: MENU_W - 8., h: MENU_ROW_H }));
            y += MENU_ROW_H;
        }
        Some((Rect { x: anchor.x, y: top, w: MENU_W, h: y + 4. - top }, placed))
    }
    /// The open menu under its button.
    fn title_menu_element(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let (card_rect, rows) = self.title_menu_placed()?;
        let mut card = div().absolute().left(px(card_rect.x)).top(px(card_rect.y)).w(px(card_rect.w)).h(px(card_rect.h)).rounded_md().bg(rgb(CARD_BG)).border_1().border_color(rgb(LINE))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()));
        for (row, r) in rows {
            if row.divided {
                card = card.child(div().absolute().left(px(8.)).top(px(r.y - card_rect.y - 4.)).w(px(card_rect.w - 16.)).h(px(1.)).bg(rgb(LINE)));
            }
            let id = row.id.clone();
            let mut el = div().absolute().left(px(r.x - card_rect.x)).top(px(r.y - card_rect.y)).w(px(r.w)).h(px(r.h)).px_2().rounded_sm().flex().items_center().gap_2().text_color(rgb(if row.enabled { TEXT } else { 0x5a7266 }))
                .child(div().w(px(14.)).flex_none().text_xs().child(if row.checked { "✓" } else { "" })).child(row.title.clone());
            if row.enabled {
                el = el.cursor_pointer().hover(|s| s.bg(rgb(ACCENT)));
            }
            card = card.child(el.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.run_title_menu(&id);
                cx.stop_propagation();
                cx.notify();
            })));
        }
        Some(card)
    }

    /// The Desk mode's body: the Swift placeholder until the agent cards are drawn (DeskView.swift).
    pub(super) fn desk_element(&self) -> gpui::Div {
        let r = self.grid_area();
        div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).flex().flex_col().items_center().justify_center().gap(px(6.))
            .children(self.symbol("rectangle.3.group", 28., 0x5a7266))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(rgb(TEXT)).child("Desk"))
            .child(div().text_color(rgb(DIM)).child("Every agent will be a card here."))
    }

    // MARK: The bottom row

    /// The connection badge's words and symbol (ConnectionBadge).
    pub fn connection_badge(&self) -> (&'static str, &'static str) {
        match self.link {
            Link::Up => ("checkmark.circle", "Connected to wheeld"),
            Link::Connecting => ("arrow.triangle.2.circlepath", "Connecting to wheeld…"),
            Link::Down(_) | Link::Incompatible { .. } => ("exclamationmark.triangle", "Not connected to wheeld"),
        }
    }
    /// The entries of the strip in order, how many fit the row beside the badge and the gauge,
    /// and each entry's measured width.
    pub fn strip_fit(&self, window: &Window) -> (Vec<Item>, usize, Vec<f32>) {
        let board = &self.settings.usage;
        let items = strip::items(board);
        let measure = |text: &str, size: f32| text_width(window, text, size);
        let widths: Vec<f32> = items.iter().map(|item| match item {
            Item::Note { agent, note } => 16. + 6. + measure(&format!("{}: {note}", agent.name), 11.),
            Item::Account { agent, account } => {
                let mut w = 16.;
                for (label, text) in strip::labels(board, agent, account) {
                    w += 8. + measure(&text, if label == "billing" { 11. } else { 13. }) + if label == "billing" { 12. } else { 0. };
                }
                if let Some(note) = strip::account_note(account) {
                    w += 8. + measure(note.split(':').next().unwrap_or(&note), 13.);
                } else {
                    let now = now_unix();
                    for (win, level) in strip::rows(board, agent, account) {
                        w += 8. + strip::BAR_W + 6. + strip::PCT_W + 6. + measure(&win.id, 13.);
                        if level == UsageMeterLevel::Full && let Some(clock) = strip::reset_clock(win.resets_at.as_deref(), now) {
                            w += 6. + measure(&clock, 13.);
                        }
                    }
                    if strip::is_stale(account) {
                        w += 8. + measure("stale", 11.);
                    }
                }
                w
            }
        }).collect();
        let badge_text = self.connection_badge().1;
        let badge_w = 16. + 4. + measure(badge_text, 11.);
        let more_w = measure(&format!("+{}", items.len()), 13.);
        let room = self.viewport.0 - 2. * header::MARGIN - badge_w - 12. - 8.;
        let shown = strip::fitting(&widths, more_w, 16., room);
        (items, shown, widths)
    }
    /// The row along the bottom edge: the badge, the entries that fit, `+N` and the gauge.
    pub(super) fn bottom_row(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let (w, h) = self.viewport;
        let (items, shown, _) = self.strip_fit(window);
        let (badge_icon, badge_text) = self.connection_badge();
        let board = self.settings.usage.clone();
        let now = now_unix();
        let mut row = div().absolute().left(px(header::MARGIN)).top(px(h - BOTTOM + 6.)).w(px(w - 2. * header::MARGIN)).h(px(22.)).flex().items_center().gap(px(12.)).overflow_hidden().whitespace_nowrap().text_sm();
        row = row.child(div().flex().items_center().gap_1().flex_none().text_xs().text_color(rgb(DIM)).children(self.symbol(badge_icon, 11., DIM)).child(badge_text));
        let mut strip_el = div().flex().items_center().gap(px(strip::SPACING)).flex_none().px(px(4.));
        for item in items.iter().take(shown) {
            strip_el = strip_el.child(match item {
                Item::Note { agent, note } => div().flex().items_center().gap(px(6.)).children(self.symbol("cpu", 13., DIM)).child(div().text_xs().text_color(rgb(DIM)).child(format!("{}: {note}", agent.name))),
                Item::Account { agent, account } => self.strip_account(&board, agent, account, now),
            });
        }
        if shown < items.len() {
            strip_el = strip_el.child(div().text_color(rgb(DIM)).cursor_pointer().child(format!("+{}", items.len() - shown)).on_mouse_down(MouseButton::Left, click(cx, |this| this.open_usage())));
        }
        strip_el = strip_el.child(div().w(px(16.)).h(px(16.)).flex().items_center().justify_center().cursor_pointer().children(self.symbol("gauge.with.dots.needle.50percent", 13., DIM)).on_mouse_down(MouseButton::Left, click(cx, |this| this.open_usage())));
        row.child(strip_el)
    }
    /// One account in the strip: mark, the labels `usage.labels` names, its metered bars, and
    /// why when it has none or they are stale (StripAccount).
    fn strip_account(&self, board: &wheel_proto::UsageBoard, agent: &UsageAgent, account: &UsageAccount, now: i64) -> gpui::Div {
        let mut el = div().flex().items_center().gap(px(8.)).children(self.symbol("cpu", 13., DIM));
        for (label, text) in strip::labels(board, agent, account) {
            el = el.child(if label == "billing" {
                div().text_xs().px(px(6.)).py(px(1.)).rounded_full().bg(rgb(LINE)).text_color(rgb(TEXT)).child(text)
            } else {
                div().text_color(rgb(DIM)).child(text)
            });
        }
        if let Some(note) = strip::account_note(account) {
            // The reason's first clause ("signed out", "no login in the Keychain"); the window has the rest.
            el = el.child(div().text_color(rgb(DIM)).child(note.split(':').next().unwrap_or(&note).to_owned()));
        } else {
            let stale = strip::is_stale(account);
            for (win, level) in strip::rows(board, agent, account) {
                let full = level == UsageMeterLevel::Full;
                let pace = full.then(|| strip::pace(win.resets_at.as_deref(), win.length_secs, now)).flatten();
                let mut bar = div().flex().items_center().gap(px(6.))
                    .child(usage_bar(win.pct, stale, pace, strip::BAR_W))
                    .child(div().w(px(strip::PCT_W)).flex_none().text_right().child(strip::percent(win.pct)))
                    .child(div().text_color(rgb(DIM)).child(win.id.clone()));
                if full && let Some(clock) = strip::reset_clock(win.resets_at.as_deref(), now) {
                    bar = bar.child(div().text_color(rgb(DIM)).child(clock));
                }
                el = el.child(bar);
            }
            if stale {
                el = el.child(div().text_xs().text_color(rgb(ORANGE)).child("stale"));
            }
        }
        el
    }

    // MARK: The Usage window

    pub fn open_usage(&mut self) {
        if !self.usage_open {
            self.usage_open = true;
            self.log("usage window opened".into());
            self.sync_overlay();
        }
    }
    pub fn close_usage(&mut self) {
        if self.usage_open {
            self.usage_open = false;
            self.log("usage window closed".into());
            self.sync_overlay();
        }
    }
    pub fn toggle_usage(&mut self) {
        if self.usage_open { self.close_usage() } else { self.open_usage() }
    }
    /// Read every account's usage now (`usage` with `due_only` off), as Refresh All does.
    pub fn refresh_usage(&mut self, due_only: bool) {
        if self.link != Link::Up {
            return;
        }
        // A timer's read never clears a Refresh All still on its way.
        if !due_only {
            self.usage_refreshing = true;
        }
        // wheeld reads and answers only a request with an id.
        let req = self.req();
        self.send(&ClientMsg::Usage { req: Some(req), due_only });
    }
    /// A board arrived (a `usage` reply or a `usage_changed` push): the strip, the window and
    /// Settings › Usage draw it; a Refresh All is over.
    pub fn take_usage_board(&mut self, board: &wheel_proto::UsageBoard) {
        self.settings.usage = board.clone();
        self.usage_refreshing = false;
        self.log(format!("usage board agents={} meters={} labels={}",
            board.agents.iter().map(|a| format!("{}{}", a.id, if a.shown { "" } else { "(hidden)" })).collect::<Vec<_>>().join(","),
            board.meters.iter().map(|m| format!("{}:{}:{}", m.agent, m.window, strip::level_name(m.level))).collect::<Vec<_>>().join(","), board.labels.join(",")));
    }
    /// Give `window` of every `agent` login the strip level `level` (None: off): write
    /// `usage.meters` machine-wide (ADR-0034).
    pub fn set_meter(&mut self, agent: &str, window: &str, level: Option<UsageMeterLevel>) {
        let value = strip::meters_setting(&self.settings.usage, agent, window, level);
        self.log(format!("usage meter {agent}:{window} {} -> usage.meters = {value}", level.map(strip::level_name).unwrap_or("off")));
        self.write_machine("usage.meters", value);
    }
    /// Turn a strip label on or off (`usage.labels`).
    pub fn toggle_usage_label(&mut self, label: &str) {
        let value = crate::settings::labels_setting(&self.settings.usage, label);
        self.log(format!("usage label {label} -> usage.labels = {value}"));
        self.write_machine("usage.labels", value);
    }
    /// List or unlist an agent (`usage.agents`).
    pub fn toggle_usage_agent(&mut self, agent: &str) {
        let value = crate::settings::agents_setting(&self.settings.usage, agent);
        self.log(format!("usage agent {agent} -> usage.agents = {value}"));
        self.write_machine("usage.agents", value);
    }
    /// The Usage window over the grid: a card per account of each coding agent shown, with
    /// Refresh All, Settings and Close in its title (UsageWindow).
    pub(super) fn usage_overlay(&mut self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let board = self.settings.usage.clone();
        let loaded = !board.agents.is_empty();
        let now = now_unix();
        let title = div().w_full().flex().items_center().justify_between().px_3().py_2().border_b_1().border_color(rgb(LINE))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Usage"))
            .child(div().flex().gap_2()
                .child(button("Refresh All", click(cx, |this| this.refresh_usage(false))))
                .child(button("Settings", click(cx, |this| {
                    this.close_usage();
                    this.open_settings();
                    this.settings_tab(crate::settings::Tab::Usage);
                })))
                .child(button("Close", click(cx, |this| this.close_usage()))));
        let mut body = div().id("usage-cards").flex_1().min_h(px(0.)).overflow_y_scroll().p(px(16.)).flex().flex_col().gap_3();
        let shown = strip::shown(&board);
        if shown.is_empty() {
            body = body.child(div().text_color(rgb(DIM)).child(if loaded { "No coding agent is set to show usage. Turn one on in Settings." } else { "Asking wheeld for usage…" }));
        }
        for agent in shown {
            if let Some(note) = strip::agent_note(agent) {
                body = body.child(div().flex().items_center().gap_2().text_color(rgb(DIM)).children(self.symbol("exclamationmark.triangle", 12., DIM)).child(format!("{}: {note}", agent.name)));
            }
            for account in &agent.accounts {
                body = body.child(self.usage_card(&board, agent, account, now, cx));
            }
        }
        let card = div().w(px(USAGE_W.min(width - 40.))).h(px(USAGE_H.min(height - 40.))).rounded_lg().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).flex().flex_col().overflow_hidden().text_color(rgb(TEXT))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(title).child(body);
        div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x00000066)).flex().items_center().justify_center()
            .on_mouse_down(MouseButton::Left, click(cx, |this| this.close_usage()))
            .child(card)
    }
    /// One account's card: its name, agent, billing chip and refresh; its note or its windows,
    /// each with a bar, the percentage, the reset and the Off / Meter / Full picker that sets the
    /// strip's meter for this window of every login of the agent (UsageCard).
    fn usage_card(&self, board: &wheel_proto::UsageBoard, agent: &UsageAgent, account: &UsageAccount, now: i64, cx: &mut Context<Self>) -> gpui::Div {
        let mut head = div().flex().items_center().gap_2()
            .children(self.symbol("cpu", 14., DIM))
            .child(div().flex().flex_col().child(div().font_weight(FontWeight::SEMIBOLD).child(account.name.clone())).child(div().text_xs().text_color(rgb(DIM)).child(agent.name.clone())));
        if let Some(billing) = account.usage.reading.as_ref().and_then(|r| r.billing.clone()) {
            head = head.child(div().text_xs().px(px(6.)).py(px(1.)).rounded_full().bg(rgb(LINE)).child(billing));
        }
        head = head.child(div().flex_1()).child(if self.usage_refreshing {
            div().text_xs().text_color(rgb(DIM)).child("reading…")
        } else {
            div().cursor_pointer().children(self.symbol("arrow.clockwise", 12., DIM)).on_mouse_down(MouseButton::Left, click(cx, |this| this.refresh_usage(false)))
        });
        let mut card = div().w_full().p_3().rounded_lg().bg(rgb(BOX)).border_1().border_color(rgb(LINE)).flex().flex_col().gap(px(10.)).child(head);
        if let Some(note) = strip::account_note(account) {
            return card.child(div().flex().items_center().gap_2().text_color(rgb(ORANGE)).children(self.symbol("exclamationmark.triangle", 12., ORANGE)).child(note));
        }
        let stale = strip::is_stale(account);
        if stale {
            card = card.child(div().text_xs().text_color(rgb(ORANGE)).child(format!("Stale, read {} ago: {}", strip::age(account.usage.age_secs.unwrap_or(0)), account.usage.why.clone().unwrap_or_default())));
        }
        let reading = account.usage.reading.as_ref();
        let windows = reading.map(|r| r.windows.clone()).unwrap_or_default();
        for win in &windows {
            let current = strip::level(board, &agent.id, &win.id);
            let pace = strip::pace(win.resets_at.as_deref(), win.length_secs, now);
            let mut picker = div().flex().w(px(140.)).flex_none().rounded_sm().border_1().border_color(rgb(LINE)).overflow_hidden().text_xs();
            for (name, level) in [("Off", None), ("Meter", Some(UsageMeterLevel::Meter)), ("Full", Some(UsageMeterLevel::Full))] {
                let on = current == level;
                let (a, w) = (agent.id.clone(), win.id.clone());
                picker = picker.child(div().flex_1().py(px(2.)).text_center().cursor_pointer().bg(rgb(if on { ACCENT } else { PANEL })).text_color(rgb(if on { TEXT } else { DIM })).child(name)
                    .on_mouse_down(MouseButton::Left, click(cx, move |this| this.set_meter(&a, &w, level))));
            }
            card = card.child(div().flex().items_center().gap_2()
                .child(div().w(px(96.)).flex_none().overflow_hidden().whitespace_nowrap().child(win.label.clone()))
                .child(div().flex_1().min_w(px(0.)).child(usage_bar_flex(win.pct, stale, pace)))
                .child(div().w(px(40.)).flex_none().text_right().child(strip::percent(win.pct)))
                .child(div().w(px(70.)).flex_none().text_xs().text_color(rgb(DIM)).child(strip::resets(win.resets_at.as_deref(), now).unwrap_or_default()))
                .child(picker));
        }
        if windows.is_empty() {
            card = card.child(div().text_color(rgb(DIM)).child(format!("No rate-limit window: {}.", reading.and_then(|r| r.billing.clone()).unwrap_or_else(|| "no plan limits".into()))));
        }
        card = card.child(div().h(px(1.)).bg(rgb(LINE)));
        if let Some(r) = reading {
            card = card.child(div().flex().justify_between().child(div().text_color(rgb(DIM)).child("Extra usage")).child(div().child(match r.extra_usage { Some(true) => "On", Some(false) => "Off", None => "Unknown" })));
            if let Some(email) = &r.email {
                card = card.child(div().flex().justify_between().child(div().text_color(rgb(DIM)).child("Account")).child(div().child(email.clone())));
            }
        }
        card
    }

    // MARK: State for checks

    /// The usage strip and window as `/state` `usage` carries them in the Swift app.
    pub fn usage_json(&self) -> Value {
        let board = &self.settings.usage;
        let now = now_unix();
        let strip_rows: Vec<Value> = strip::shown(board).into_iter().flat_map(|agent| match strip::agent_note(agent) {
            Some(note) => vec![json!({"agent": agent.id, "account": Value::Null, "note": note, "stale": false, "age_secs": Value::Null, "labels": [], "rows": []})],
            None => agent.accounts.iter().map(|account| json!({
                "agent": agent.id, "account": account.name, "note": strip::account_note(account), "stale": strip::is_stale(account), "age_secs": account.usage.age_secs,
                "labels": strip::labels(board, agent, account).into_iter().map(|(_, t)| t).collect::<Vec<_>>(),
                "rows": strip::rows(board, agent, account).into_iter().map(|(w, level)| {
                    let full = level == UsageMeterLevel::Full;
                    json!({"id": w.id, "label": w.label, "pct": w.pct, "level": strip::level_name(level),
                        "pace": full.then(|| strip::pace(w.resets_at.as_deref(), w.length_secs, now)).flatten(),
                        "resets": full.then(|| strip::reset_clock(w.resets_at.as_deref(), now)).flatten()})
                }).collect::<Vec<_>>(),
            })).collect(),
        }).collect();
        let items = strip::items(board);
        json!({
            "window_open": self.usage_open,
            "loaded": !board.agents.is_empty(),
            "refreshing": self.usage_refreshing,
            "meters": board.meters.iter().map(|m| format!("{}:{}:{}", m.agent, m.window, strip::level_name(m.level))).collect::<Vec<_>>(),
            "labels": board.labels,
            "entries": items.iter().map(Item::id).collect::<Vec<_>>(),
            "strip": strip_rows,
            "agents": board.agents.iter().map(|agent| json!({
                "id": agent.id, "name": agent.name, "shown": agent.shown, "readable": agent.readable, "why": agent.why,
                "accounts": agent.accounts.iter().map(|account| json!({
                    "id": account.id, "name": account.name, "kind": format!("{:?}", account.usage.kind).to_lowercase(), "why": account.usage.why,
                    "billing": account.usage.reading.as_ref().and_then(|r| r.billing.clone()), "email": account.usage.reading.as_ref().and_then(|r| r.email.clone()),
                    "source": account.usage.source,
                    "rows": account.usage.reading.as_ref().map(|r| r.windows.iter().map(|w| json!({"id": w.id, "label": w.label, "pct": w.pct, "length_secs": w.length_secs,
                        "resets": strip::resets(w.resets_at.as_deref(), now), "level": strip::level(board, &agent.id, &w.id).map(strip::level_name).unwrap_or("off")})).collect::<Vec<_>>()).unwrap_or_default(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
    /// The title row as `/state` `title` carries it: the entries per space, the open menu and its
    /// rows, the mode and the pin.
    pub fn title_json(&self) -> Value {
        let row = self.title_row();
        json!({
            "buttons": self.title_buttons.iter().map(|(space, list)| (space.clone(), json!(list))).collect::<serde_json::Map<_, _>>(),
            "row": row.chips.iter().map(|c| json!({"id": header::frame_id(&c.hit).unwrap_or_else(|| match &c.hit { Hit::Space(id) => format!("tab:{id}"), _ => "title".into() }), "label": c.label, "icon": c.icon, "badge": c.badge, "tint": c.tint, "enabled": c.enabled, "pressable": c.pressable, "active": c.active, "frame": [c.rect.x, c.rect.y, c.rect.w, c.rect.h]})).collect::<Vec<_>>(),
            "tabs": row.track.map(|t| json!({"track": [t.x, t.y, t.w, t.h], "tab_width": row.tab_width, "scrolls": row.scrolls})),
            "menu": self.title_menu.map(|m| json!({"kind": match m { TitleMenu::Session => "session", TitleMenu::Mode => "mode", TitleMenu::Title => "title" },
                "rows": self.title_menu_placed().map(|(_, rows)| rows.iter().map(|(r, rect)| json!({"id": r.id, "title": r.title, "enabled": r.enabled, "checked": r.checked, "frame": [rect.x, rect.y, rect.w, rect.h]})).collect::<Vec<_>>()).unwrap_or_default()})),
            "mode": self.mode,
            "pinned": self.pinned,
            "connection_badge": self.connection_badge().1,
        })
    }
    /// The frames `GET /geometry` `title_buttons` lists, by the Swift ids.
    pub fn title_button_frames(&self) -> serde_json::Map<String, Value> {
        self.header_chips().iter().filter_map(|c| header::frame_id(&c.hit).map(|id| (id, json!([c.rect.x, c.rect.y, c.rect.w, c.rect.h])))).collect()
    }
}

/// The width of `text` at `size` in the window's font.
pub(super) fn text_width(window: &Window, text: &str, size: f32) -> f32 {
    let style = window.text_style();
    let run = TextRun { len: text.len(), font: style.font(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().layout_line(text, px(size), &[run], None).width)
}

/// Seconds since the Unix epoch now.
pub(super) fn now_unix() -> i64 {
    (super::settings::now_ms() / 1000) as i64
}

/// A rounded bar filled to `pct` (UsageBar): neutral, orange from 80%, red from 95%, the track's
/// colour when stale; `pace` draws a tick at that share of the bar.
fn usage_bar(pct: f64, stale: bool, pace: Option<f64>, width: f32) -> gpui::Div {
    let fill = (width as f64 * pct.clamp(0., 100.) / 100.).max(f64::from(strip::BAR_H)) as f32;
    let mut bar = div().relative().w(px(width)).h(px(strip::BAR_H)).flex_none().rounded_full().bg(rgb(strip::TRACK))
        .child(div().absolute().left_0().top_0().h(px(strip::BAR_H)).w(px(fill)).rounded_full().bg(rgb(strip::bar_fill(pct, stale))));
    if let Some(pace) = pace {
        bar = bar.child(div().absolute().top(px(-2.)).left(px((width - 1.5) * pace.clamp(0., 1.) as f32)).w(px(1.5)).h(px(strip::BAR_H + 4.)).bg(rgb(0xd9e8e1)));
    }
    bar
}

/// The same bar taking the width it is given (the Usage window's rows).
fn usage_bar_flex(pct: f64, stale: bool, pace: Option<f64>) -> gpui::Div {
    let mut bar = div().relative().w_full().h(px(strip::BAR_H)).rounded_full().bg(rgb(strip::TRACK)).overflow_hidden()
        .child(div().h(px(strip::BAR_H)).w(relative(pct.clamp(0., 100.) as f32 / 100.)).min_w(px(strip::BAR_H)).rounded_full().bg(rgb(strip::bar_fill(pct, stale))));
    if let Some(pace) = pace {
        bar = bar.child(div().absolute().top_0().left(relative(pace.clamp(0., 1.) as f32)).w(px(1.5)).h(px(strip::BAR_H)).bg(rgb(0xd9e8e1)));
    }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tints_are_the_named_colours_and_nothing_else() {
        assert_eq!(tint(Some("green")), Some(0x30d158));
        assert_eq!(tint(Some("grey")), tint(Some("gray")));
        assert_eq!(tint(Some("teal")), None);
        assert_eq!(tint(None), None);
    }
}
