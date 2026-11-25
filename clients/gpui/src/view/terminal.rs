//! Terminal panes, drawn by GPUI from the client's own model of each term (ADR-0056): one
//! `TermModel` per term id, shared by every pane that shows it (a mirror), fed from this window's
//! wheeld connection. Keys, IME text, pastes, mouse reports and dropped files go back as
//! `term_input`; a pane's grid goes as `term_resize`, held while the grid moves or a window edge
//! is dragged and sent once, as the grid the move ended at.
use super::*;
use gpui::{BorderStyle, Font, Hsla, InputHandler, ScrollWheelEvent, TextAlign, TextRun, UTF16Selection, WeakEntity, WindowAppearance, fill, font, outline, point, size};
use std::path::PathBuf;
use std::time::Instant;
use wheel_gpui_core::terminal::{self as vt, CellMetrics, KeyInput, Look, MouseAction, PADDING_X, PADDING_Y, ResizeHold, RgbColor, TermModel};

/// The face the terminal draws with when a space names none: Ghostty's own, bundled.
pub const FONT: &str = "JetBrains Mono";
const BLINK_MS: u64 = 600;
/// Grids a pane keeps in its history for the hidden check.
const GRIDS_KEPT: usize = 32;

/// The four JetBrains Mono faces, added to the app's text system once at launch.
pub fn fonts() -> Vec<std::borrow::Cow<'static, [u8]>> {
    vec![
        std::borrow::Cow::Borrowed(include_bytes!("../../vendor/jetbrains-mono/JetBrainsMono-Regular.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../../vendor/jetbrains-mono/JetBrainsMono-Bold.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../../vendor/jetbrains-mono/JetBrainsMono-Italic.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../../vendor/jetbrains-mono/JetBrainsMono-BoldItalic.ttf").as_slice()),
    ]
}

/// One drawn terminal pane: the term it shows, where its body is, and its own state.
#[derive(Debug, Clone)]
pub struct TermPane {
    pub term: String,
    pub space: Option<String>,
    /// ⌘= / ⌘- on this pane, over its space's size; ⌘0 clears it.
    pub font_size: Option<f64>,
    /// The body under the title row, in window points: x, y, width, height.
    pub body: [f32; 4],
    pub cell: CellMetrics,
    pub grid: (u16, u16),
    /// Every grid the pane drew at, oldest first.
    pub grids: Vec<(u16, u16)>,
    /// Where a selecting drag began, while one goes on.
    anchor: Option<(u16, u16)>,
    /// A mouse report press is held: moves and the release go to the program.
    reporting: bool,
    /// Scrolled distance not yet a whole line.
    scroll_rest: f64,
}

#[derive(Default)]
pub struct Terms {
    pub models: HashMap<String, TermModel>,
    pub panes: HashMap<String, TermPane>,
    pub looks: HashMap<String, Look>,
    pub hold: ResizeHold,
    pub dark: bool,
    /// IME text being composed in a pane: drawn at its cursor, sent once committed.
    pub marked: Option<(String, String)>,
    /// Every `term_resize` sent, per term.
    pub sent_grids: HashMap<String, Vec<(u16, u16)>>,
    /// The viewport's text, per term, as it last drew.
    pub texts: HashMap<String, String>,
    /// The pane holding the keyboard at the last frame, for focus reports.
    pub focused: Option<String>,
    /// When the cursor last went solid: a key restarts the blink.
    blink_from: Option<Instant>,
}

impl Terms {
    pub fn look(&self, space: Option<&str>) -> Look {
        space.and_then(|s| self.looks.get(s)).cloned().unwrap_or_default()
    }
}

fn rgb_hsla(c: RgbColor) -> Hsla {
    gpui::Rgba { r: f32::from(c.r) / 255., g: f32::from(c.g) / 255., b: f32::from(c.b) / 255., a: 1. }.into()
}

/// The cell of `family` at `points`, as Ghostty measures it at the window's scale.
fn cell_for(window: &Window, family: &str, points: f64, line_height: f64) -> CellMetrics {
    let ts = window.text_system();
    let id = ts.resolve_font(&font(family.to_owned()));
    let scale = f64::from(window.scale_factor());
    let s = px(points as f32);
    let advance = ts.advance(id, s, 'm').map(|a| f64::from(f32::from(a.width))).unwrap_or(points * 0.6);
    let line = f64::from(f32::from(ts.ascent(id, s))) + f64::from(f32::from(ts.descent(id, s))).abs();
    CellMetrics::from_face(advance * scale, line * scale, scale, line_height)
}

fn family(look: &Look) -> String {
    if look.font_family.is_empty() { FONT.to_owned() } else { look.font_family.clone() }
}

impl Client {
    /// A terminal pane for the debug state: what it shows, its grids, and the look it draws in.
    pub(super) fn term_json(&self, pane: &str) -> serde_json::Value {
        let Some(p) = self.terms.panes.get(pane) else { return serde_json::Value::Null };
        let look = self.terms.look(p.space.as_deref());
        serde_json::json!({
            "term": p.term,
            "space": p.space,
            "viewport": self.terms.texts.get(&p.term),
            "cols": p.grid.0, "rows": p.grid.1,
            "grids": p.grids.iter().map(|g| [g.0, g.1]).collect::<Vec<_>>(),
            "sent_grids": self.terms.sent_grids.get(&p.term).map(|g| g.iter().map(|g| [g.0, g.1]).collect::<Vec<_>>()).unwrap_or_default(),
            "body": p.body,
            "cell": [p.cell.width, p.cell.height],
            "focused": self.terms.focused.as_deref() == Some(pane),
            "font_family": family(&look),
            "font_size": p.font_size.unwrap_or(look.font_size),
            "theme": format!("{:?}", look.theme).to_lowercase(),
            "cursor": format!("{:?}", look.cursor).to_lowercase(),
            "cursor_blink": look.cursor_blink,
            "dark": self.terms.dark,
            "attached": self.terms.models.contains_key(&p.term),
            "marked": self.terms.marked.as_ref().filter(|(m, _)| m == pane).map(|(_, t)| t),
        })
    }

    /// A term's stream from wheeld: fed to the model showing it, and the terminal's answers to the
    /// program's queries sent back. True when it was a term's.
    pub(super) fn term_message(&mut self, message: &ServerMsg) -> bool {
        if !matches!(message, ServerMsg::TermAttached { .. } | ServerMsg::TermOutput { .. } | ServerMsg::TermExited { .. }) {
            return false;
        }
        let mut replies = vec![];
        for model in self.terms.models.values_mut() {
            if model.receive(message) {
                replies.extend(model.take_replies());
            }
        }
        for reply in replies {
            self.send(&reply);
        }
        true
    }

    /// A new connection streams nothing until each term is attached again; the replay redraws it.
    pub(super) fn reattach_terms(&self) {
        for model in self.terms.models.values() {
            self.send(&model.attach());
        }
    }

    /// A space's `terminal.*` settings: its panes redraw in the new look.
    pub(super) fn set_term_look(&mut self, space: &str, look: Look) {
        if self.terms.looks.get(space) == Some(&look) {
            return;
        }
        self.terms.looks.insert(space.to_owned(), look.clone());
        let dark = self.terms.dark;
        let terms: HashSet<String> = self.terms.panes.values().filter(|p| p.space.as_deref() == Some(space)).map(|p| p.term.clone()).collect();
        for term in terms {
            if let Some(model) = self.terms.models.get_mut(&term) {
                let _ = model.restyle(&look, dark);
            }
        }
    }

    /// A window edge is dragged or let go (`appkit::live_resize`).
    pub fn set_live_resize(&mut self, on: bool) {
        self.terms.hold.set_live_resize(on);
    }

    /// Every drawn terminal pane gets a model (attached on first sight) and its grid; the grid goes
    /// to wheeld unless a move holds it; a term no space shows any more is detached.
    pub(super) fn sync_terms(&mut self, layers: &[Layer], window: &Window) {
        self.terms.hold.set_moving(self.resizes_wanted());
        let dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
        if dark != self.terms.dark {
            self.terms.dark = dark;
            for pane in self.terms.panes.values() {
                let look = self.terms.look(pane.space.as_deref());
                if let Some(model) = self.terms.models.get_mut(&pane.term) {
                    let _ = model.restyle(&look, dark);
                }
            }
        }
        let space = self.space_id.clone();
        let mut out = vec![];
        let mut drawn: Vec<String> = vec![];
        for p in layers.iter().flat_map(|l| l.leaves.iter()) {
            let Some(term) = p.leaf.term.clone().filter(|_| p.leaf.kind == "terminal") else { continue };
            if self.quarantine.isolates_pane(&p.leaf.id) {
                continue;
            }
            let look = self.terms.look(space.as_deref());
            let pane = self.terms.panes.entry(p.leaf.id.clone()).or_insert_with(|| TermPane {
                term: term.clone(), space: space.clone(), font_size: None, body: [0.; 4], cell: CellMetrics { width: 1., height: 1. }, grid: (0, 0), grids: vec![], anchor: None, reporting: false, scroll_rest: 0.,
            });
            if pane.term != term {
                pane.term = term.clone();
                pane.anchor = None;
            }
            pane.space = space.clone();
            pane.cell = cell_for(window, &family(&look), pane.font_size.unwrap_or(look.font_size), look.line_height);
            let h = (p.rect.h - TITLE).max(1.);
            pane.body = [p.rect.x, p.rect.y + TITLE, p.rect.w, h];
            let grid = vt::grid_for(f64::from(p.rect.w), f64::from(h), pane.cell);
            if pane.grid != grid {
                pane.grid = grid;
                pane.grids.push(grid);
                if pane.grids.len() > GRIDS_KEPT {
                    pane.grids.remove(0);
                }
            }
            if !self.terms.models.contains_key(&term)
                && let Ok(model) = TermModel::new(&term, grid.0, grid.1, &look, dark)
            {
                out.push(model.attach());
                self.terms.models.insert(term.clone(), model);
            }
            drawn.push(p.leaf.id.clone());
        }
        // A mirrored term takes the grid of the pane with the keyboard, else the first drawn.
        if !self.terms.hold.held() {
            let mut sized: HashSet<String> = HashSet::new();
            let first = self.focus.current.iter().filter(|f| drawn.contains(f)).cloned().chain(drawn.iter().cloned()).collect::<Vec<_>>();
            for id in first {
                let Some(pane) = self.terms.panes.get(&id) else { continue };
                if !sized.insert(pane.term.clone()) {
                    continue;
                }
                let (term, (cols, rows), cell) = (pane.term.clone(), pane.grid, pane.cell);
                if let Some(msg) = self.terms.models.get_mut(&term).and_then(|m| m.resize(cols, rows, cell)) {
                    self.terms.sent_grids.entry(term).or_default().push((cols, rows));
                    out.push(msg);
                }
            }
        }
        // Focus reports: the pane holding the keyboard now, against the last frame's.
        let keyboard = self.focus.current.clone().filter(|f| drawn.contains(f) && self.handle.is_focused(window));
        if keyboard != self.terms.focused {
            for (pane, gained) in [(self.terms.focused.clone(), false), (keyboard.clone(), true)] {
                if let Some(model) = pane.and_then(|p| self.terms.panes.get(&p)).and_then(|p| self.terms.models.get(&p.term)) {
                    let bytes = model.encode_focus(gained);
                    if !bytes.is_empty() {
                        out.push(model.input(&bytes));
                    }
                }
            }
            self.terms.focused = keyboard;
            self.terms.blink_from = Some(Instant::now());
        }
        if !self.awaiting_layout {
            let alive: HashSet<&str> = self.spaces.iter().flat_map(|s| s.shown()).filter_map(|l| l.term.as_deref()).collect();
            let gone: Vec<String> = self.terms.models.keys().filter(|t| !alive.contains(t.as_str())).cloned().collect();
            for term in gone {
                if let Some(model) = self.terms.models.remove(&term) {
                    out.push(model.detach());
                }
                self.terms.texts.remove(&term);
                self.terms.sent_grids.remove(&term);
            }
            let panes: HashSet<&str> = self.spaces.iter().flat_map(|s| s.shown()).map(|l| l.id.as_str()).collect();
            self.terms.panes.retain(|id, _| panes.contains(id.as_str()));
        }
        for msg in out {
            self.send(&msg);
        }
    }

    /// The cell under a window point in `pane`, clamped to its grid.
    fn term_cell(&self, pane: &str, at: Point<Pixels>) -> Option<(u16, u16)> {
        let p = self.terms.panes.get(pane)?;
        let x = (f64::from(f32::from(at.x) - p.body[0]) - PADDING_X) / p.cell.width;
        let y = (f64::from(f32::from(at.y) - p.body[1]) - PADDING_Y) / p.cell.height;
        let clamp = |v: f64, n: u16| v.floor().clamp(0., f64::from(n.saturating_sub(1))) as u16;
        Some((clamp(x, p.grid.0), clamp(y, p.grid.1)))
    }

    /// A mouse event's bytes for the program, when it reads the mouse.
    fn term_report(&mut self, pane: &str, action: MouseAction, button: Option<vt::MouseButton>, at: Point<Pixels>, m: gpui::Modifiers) -> bool {
        let Some(p) = self.terms.panes.get(pane) else { return false };
        let (body, cell) = (p.body, p.cell);
        let Some(model) = self.terms.models.get_mut(&p.term) else { return false };
        if !model.tracks_mouse() {
            return false;
        }
        let bytes = model.encode_mouse(action, button, f64::from(f32::from(at.x) - body[0]), f64::from(f32::from(at.y) - body[1]), f64::from(body[2]), f64::from(body[3]), cell, m.shift, m.control, m.alt);
        if !bytes.is_empty() {
            let msg = model.input(&bytes);
            self.send(&msg);
        }
        true
    }

    fn term_mouse_down(&mut self, pane: &str, e: &MouseDownEvent) {
        self.focus_pane(pane);
        if !e.modifiers.shift && self.term_report(pane, MouseAction::Press, Some(vt::MouseButton::Left), e.position, e.modifiers) {
            if let Some(p) = self.terms.panes.get_mut(pane) {
                p.reporting = true;
                p.anchor = None;
            }
            return;
        }
        let Some(cell) = self.term_cell(pane, e.position) else { return };
        let Some(p) = self.terms.panes.get_mut(pane) else { return };
        p.reporting = false;
        let Some(model) = self.terms.models.get_mut(&p.term) else { return };
        match e.click_count {
            1 => {
                model.clear_selection();
                p.anchor = Some(cell);
            }
            n => {
                p.anchor = None;
                let Ok(frame) = model.frame() else { return };
                let Some(row) = frame.rows.get(usize::from(cell.1)) else { return };
                let (from, to) = if n == 2 { vt::word_at(row, cell.0) } else { (0, frame.cols.saturating_sub(1)) };
                let _ = model.select((from, cell.1), (to, cell.1));
            }
        }
    }

    fn term_mouse_move(&mut self, pane: &str, e: &MouseMoveEvent) -> bool {
        let Some(p) = self.terms.panes.get(pane) else { return false };
        let (reporting, anchor) = (p.reporting, p.anchor);
        let held = e.pressed_button == Some(MouseButton::Left);
        if reporting || (!held && anchor.is_none()) {
            // Button-event and any-event tracking (1002, 1003) want the moves; the encoder drops the rest.
            self.term_report(pane, MouseAction::Motion, held.then_some(vt::MouseButton::Left), e.position, e.modifiers);
            return false;
        }
        let (Some(anchor), true) = (anchor, held) else { return false };
        let Some(cell) = self.term_cell(pane, e.position) else { return false };
        let term = p.term.clone();
        if let Some(model) = self.terms.models.get_mut(&term) {
            let _ = model.select(anchor, cell);
        }
        true
    }

    fn term_mouse_up(&mut self, pane: &str, e: &MouseUpEvent) {
        let Some(p) = self.terms.panes.get_mut(pane) else { return };
        let reporting = std::mem::take(&mut p.reporting);
        p.anchor = None;
        if reporting {
            self.term_report(pane, MouseAction::Release, Some(vt::MouseButton::Left), e.position, e.modifiers);
        }
    }

    fn term_scroll(&mut self, pane: &str, e: &ScrollWheelEvent) {
        let Some(p) = self.terms.panes.get_mut(pane) else { return };
        let height = p.cell.height;
        p.scroll_rest += f64::from(f32::from(e.delta.pixel_delta(px(height as f32)).y)) / height;
        let lines = p.scroll_rest.trunc() as isize;
        if lines == 0 {
            return;
        }
        p.scroll_rest -= lines as f64;
        let term = p.term.clone();
        let button = if lines > 0 { vt::MouseButton::WheelUp } else { vt::MouseButton::WheelDown };
        let mut reported = false;
        for _ in 0..lines.unsigned_abs() {
            reported = self.term_report(pane, MouseAction::Press, Some(button), e.position, e.modifiers);
            if !reported {
                break;
            }
        }
        if reported {
            return;
        }
        let Some(model) = self.terms.models.get_mut(&term) else { return };
        if model.wheel_sends_arrows() {
            let key = if lines > 0 { "up" } else { "down" };
            let mut bytes = vec![];
            for _ in 0..lines.unsigned_abs() {
                bytes.extend(model.encode_key(&KeyInput { key, ..Default::default() }));
            }
            let msg = model.input(&bytes);
            self.send(&msg);
        } else {
            model.scroll(Some(-lines));
        }
    }

    /// Text the keyboard layout or an input method made, typed into the pane's term.
    fn term_text(&mut self, pane: &str, text: &str) {
        self.terms.marked = None;
        self.terms.blink_from = Some(Instant::now());
        let Some(model) = self.terms.panes.get(pane).and_then(|p| self.terms.models.get_mut(&p.term)) else { return };
        model.scroll(None);
        let msg = model.input(text.as_bytes());
        self.send(&msg);
    }

    /// Text typed through the debug server, as the keyboard types it.
    pub(super) fn term_type(&mut self, pane: &str, text: &str) -> bool {
        if !self.terms.panes.contains_key(pane) {
            return false;
        }
        self.term_text(pane, text);
        true
    }

    /// Files dropped on a terminal pane type their shell-escaped paths.
    pub(super) fn term_drop(&mut self, pane: &str, paths: &[PathBuf]) -> bool {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if paths.is_empty() || !self.terms.panes.contains_key(pane) {
            return false;
        }
        self.focus_pane(pane);
        self.term_text(pane, &vt::drop_text(&paths));
        true
    }

    /// A key on a terminal pane after Wheel's own keymap: the terminal's own ⌘ keys, then the bytes
    /// the program reads. A plain printable key returns false: GPUI hands it to the input handler,
    /// which types the layout's or the input method's text.
    pub(super) fn term_key(&mut self, pane: &str, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let k = &e.keystroke;
        let m = k.modifiers;
        let Some(term) = self.terms.panes.get(pane).map(|p| p.term.clone()) else { return false };
        self.terms.blink_from = Some(Instant::now());
        if m.platform && !m.control && !m.alt {
            let resize = |this: &mut Self, size: Option<f64>| {
                if let Some(p) = this.terms.panes.get_mut(pane) {
                    p.font_size = size.map(|s| s.clamp(6., 72.));
                }
                true
            };
            let current = || {
                let p = &self.terms.panes[pane];
                p.font_size.unwrap_or_else(|| self.terms.look(p.space.as_deref()).font_size)
            };
            match k.key.as_str() {
                "c" if !m.shift => {
                    if let Some(text) = self.terms.models.get(&term).and_then(|m| m.selection_text()) {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    return true;
                }
                "v" if !m.shift => {
                    let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return true };
                    if let Some(model) = self.terms.models.get_mut(&term) {
                        model.scroll(None);
                        let bytes = model.encode_paste(&text);
                        let msg = model.input(&bytes);
                        self.send(&msg);
                    }
                    return true;
                }
                "a" if !m.shift => {
                    if let Some(model) = self.terms.models.get_mut(&term) {
                        let _ = model.select_all();
                    }
                    return true;
                }
                "=" | "+" => {
                    let size = current() + 1.;
                    return resize(self, Some(size));
                }
                "-" => {
                    let size = current() - 1.;
                    return resize(self, Some(size));
                }
                "0" => return resize(self, None),
                _ => {}
            }
        }
        if self.terms.marked.as_ref().is_some_and(|(p, _)| p == pane) {
            return false;
        }
        let Some(model) = self.terms.models.get_mut(&term) else { return false };
        let named = k.key.chars().count() > 1 && k.key != "space";
        let encoded = named || m.control || m.platform || (m.alt && model.option_as_alt) || (k.key == "space" && (m.control || m.alt));
        if !encoded {
            return false;
        }
        let bytes = model.encode_key(&KeyInput { key: &k.key, text: k.key_char.as_deref(), shift: m.shift, control: m.control, alt: m.alt, command: m.platform, release: false, repeat: e.is_held });
        if bytes.is_empty() {
            // A ⌘ key the program reads nothing for goes on along the window (the menu's).
            return !m.platform;
        }
        model.scroll(None);
        let msg = model.input(&bytes);
        self.send(&msg);
        true
    }

    /// A terminal pane's body: the model's viewport drawn cell by cell, its cursor, the input
    /// method's text, and the mouse, scroll and drop handlers.
    pub(super) fn term_element(&mut self, p: &Placed, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let id = p.leaf.id.clone();
        let Some(pane) = self.terms.panes.get(&id).cloned() else { return div() };
        let look = self.terms.look(pane.space.as_deref());
        let dark = self.terms.dark;
        let selection = look.palette(dark).selection;
        let focused = self.terms.focused.as_deref() == Some(id.as_str());
        let active = window.is_window_active();
        let Some(model) = self.terms.models.get_mut(&pane.term) else { return div() };
        let Ok(frame) = model.frame() else { return div() };
        self.terms.texts.insert(pane.term.clone(), frame.text());
        // The cursor blinks only in the focused pane of the active window, so a hidden window's
        // pictures always hold a solid cursor.
        let blinks = focused && active && frame.cursor.blinking;
        let blink_on = if blinks {
            let since = self.terms.blink_from.map_or(0, |t| t.elapsed().as_millis() as u64);
            self.redraw_in(BLINK_MS - since % BLINK_MS, cx);
            (since / BLINK_MS) % 2 == 0
        } else {
            true
        };
        let marked = self.terms.marked.as_ref().filter(|(p, _)| *p == id).map(|(_, t)| t.clone());
        let fam = family(&look);
        let points = pane.font_size.unwrap_or(look.font_size) as f32;
        let cell = pane.cell;
        let handle = self.handle.clone();
        let weak = cx.entity().downgrade();
        let input_pane = id.clone();
        let paint = canvas(|_, _, _| (), move |bounds, (), window, cx| {
            let (cw, ch) = (px(cell.width as f32), px(cell.height as f32));
            window.paint_quad(fill(bounds, rgb_hsla(frame.background)));
            let x0 = bounds.origin.x + px(PADDING_X as f32);
            let y0 = bounds.origin.y + px(PADDING_Y as f32);
            let face = |bold: bool, italic: bool| Font {
                weight: if bold { gpui::FontWeight::BOLD } else { gpui::FontWeight::NORMAL },
                style: if italic { gpui::FontStyle::Italic } else { gpui::FontStyle::Normal },
                ..font(fam.clone())
            };
            let shape = |window: &mut Window, text: &str, colour: Hsla, s: &vt::Segment| {
                let run = TextRun {
                    len: text.len(),
                    font: face(s.bold, s.italic),
                    color: colour,
                    background_color: None,
                    underline: s.underline.then_some(UnderlineStyle { thickness: px(1.), color: Some(colour), wavy: false }),
                    strikethrough: s.strikethrough.then_some(StrikethroughStyle { thickness: px(1.), color: Some(colour) }),
                };
                window.text_system().shape_line(text.to_owned().into(), px(points), &[run], (!s.wide).then_some(cw))
            };
            let colour = |s: &vt::Segment| {
                let c = rgb_hsla(s.fg);
                if s.faint { c.opacity(0.5) } else { c }
            };
            for (r, row) in frame.rows.iter().enumerate() {
                let y = y0 + ch * r as f32;
                let segments = vt::segments(row, frame.background, selection);
                // Backgrounds first, one quad per run of one colour.
                let mut run: Option<(u16, u16, RgbColor)> = None;
                for s in segments.iter().map(|s| (s.col, s.cells, s.bg)).chain(std::iter::once((u16::MAX, 0, None))) {
                    match (run, s.2) {
                        (Some((from, to, c)), Some(bg)) if to == s.0 && c == bg => run = Some((from, s.0 + s.1, c)),
                        _ => {
                            if let Some((from, to, c)) = run.take() {
                                window.paint_quad(fill(Bounds { origin: point(x0 + cw * f32::from(from), y), size: size(cw * f32::from(to - from), ch) }, rgb_hsla(c)));
                            }
                            run = s.2.map(|bg| (s.0, s.0 + s.1, bg));
                        }
                    }
                }
                for s in &segments {
                    if s.text.trim().is_empty() && !s.underline && !s.strikethrough {
                        continue;
                    }
                    let line = shape(window, &s.text, colour(s), s);
                    let _ = line.paint(point(x0 + cw * f32::from(s.col), y), ch, TextAlign::Left, None, window, cx);
                }
            }
            // The cursor last, over its cell.
            let cursor_colour = rgb_hsla(frame.cursor_color.unwrap_or(frame.foreground));
            let mut cursor_bounds = Bounds { origin: point(x0, y0), size: size(cw, ch) };
            if let Some((col, row)) = frame.cursor.at {
                let cells = frame.rows.get(usize::from(row)).and_then(|r| r.get(usize::from(col)));
                let wide = cells.is_some_and(|c| c.width == vt::Width::Wide);
                cursor_bounds = Bounds { origin: point(x0 + cw * f32::from(col), y0 + ch * f32::from(row)), size: size(if wide { cw * 2. } else { cw }, ch) };
                if frame.cursor.visible && marked.is_none() {
                    use vt::CursorVisualStyle as V;
                    match (focused, frame.cursor.shape) {
                        (false, _) | (true, V::BlockHollow) => window.paint_quad(outline(cursor_bounds, cursor_colour, BorderStyle::Solid)),
                        (true, _) if !blink_on => {}
                        (true, V::Bar) => window.paint_quad(fill(Bounds { origin: cursor_bounds.origin, size: size(px(1.), ch) }, cursor_colour)),
                        (true, V::Underline) => window.paint_quad(fill(Bounds { origin: point(cursor_bounds.origin.x, cursor_bounds.origin.y + ch - px(1.)), size: size(cursor_bounds.size.width, px(1.)) }, cursor_colour)),
                        (true, _) => {
                            window.paint_quad(fill(cursor_bounds, cursor_colour));
                            // The glyph under a block cursor draws in the page's colour.
                            if let Some(c) = cells.filter(|c| !c.text.is_empty() && !c.invisible) {
                                let s = vt::Segment { col, cells: if wide { 2 } else { 1 }, text: c.text.clone(), fg: frame.background, bg: None, bold: c.bold, italic: c.italic, faint: false, underline: false, strikethrough: false, wide };
                                let line = shape(window, &s.text, rgb_hsla(frame.background), &s);
                                let _ = line.paint(cursor_bounds.origin, ch, TextAlign::Left, None, window, cx);
                            }
                        }
                    }
                }
            }
            // Text an input method is composing sits at the cursor, underlined, over the page.
            if let Some(text) = marked.as_deref().filter(|t| !t.is_empty()) {
                let s = vt::Segment { col: 0, cells: 0, text: text.to_owned(), fg: frame.foreground, bg: None, bold: false, italic: false, faint: false, underline: true, strikethrough: false, wide: true };
                let line = shape(window, text, rgb_hsla(frame.foreground), &s);
                window.paint_quad(fill(Bounds { origin: cursor_bounds.origin, size: size(line.width().max(cw), ch) }, rgb_hsla(frame.background)));
                let _ = line.paint(cursor_bounds.origin, ch, TextAlign::Left, None, window, cx);
            }
            if focused {
                window.handle_input(&handle, TermInput { client: weak.clone(), pane: input_pane.clone(), cursor: cursor_bounds }, cx);
            }
        })
        .size_full();
        let (down, moved, up, scrolled) = (id.clone(), id.clone(), id.clone(), id.clone());
        let el = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                this.term_mouse_down(&down, e);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                if this.term_mouse_move(&moved, e) {
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(move |this, e: &MouseUpEvent, _, cx| {
                this.term_mouse_up(&up, e);
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, _, cx| {
                this.term_scroll(&scrolled, e);
                cx.stop_propagation();
                cx.notify();
            }))
            .child(paint);
        let key = format!("term:{id}");
        let over = self.file_drag.as_ref().is_some_and(|(k, _)| *k == key);
        composer::drop_target(Some(&key), over, |client, key, paths| {
            if let Some(pane) = key.strip_prefix("term:") {
                client.term_drop(pane, paths);
            }
        }, el, cx)
    }
}

/// The input method's way into a terminal pane (Zed's `TerminalInputHandler`): there is no
/// editable text, only the composing text drawn at the cursor and what is committed.
struct TermInput {
    client: WeakEntity<Client>,
    pane: String,
    cursor: Bounds<Pixels>,
}

impl InputHandler for TermInput {
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut App) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }
    fn marked_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        let client = self.client.upgrade()?;
        let c = client.read(cx);
        c.terms.marked.as_ref().filter(|(p, t)| *p == self.pane && !t.is_empty()).map(|(_, t)| 0..t.encode_utf16().count())
    }
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut App) -> Option<String> {
        None
    }
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut App) {
        let pane = self.pane.clone();
        let text = text.to_owned();
        let _ = self.client.update(cx, |c, cx| {
            c.term_text(&pane, &text);
            cx.notify();
        });
    }
    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut App) {
        let pane = self.pane.clone();
        let text = text.to_owned();
        let _ = self.client.update(cx, |c, cx| {
            c.terms.marked = (!text.is_empty()).then_some((pane, text));
            cx.notify();
        });
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut App) {
        let _ = self.client.update(cx, |c, cx| {
            c.terms.marked = None;
            cx.notify();
        });
    }
    fn bounds_for_range(&mut self, _: Range<usize>, _: &mut Window, _: &mut App) -> Option<Bounds<Pixels>> {
        Some(self.cursor)
    }
    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut App) -> Option<usize> {
        None
    }
    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}
