//! A terminal pane's state, held by the client itself: a term attached over the client's own
//! wheeld connection, fed into ghostty-vt, read back as styled rows, and the keys, pastes, mouse
//! and focus events a program in it reads, encoded as Ghostty encodes them. The look a space's
//! `terminal.*` settings give it (`Look`) restyles a model in place, without a new attach.

use base64::Engine as _;
use ghostty_vt::key::{self, Mods, OptionAsAlt};
use ghostty_vt::render::{CellIterator, CursorVisualStyle, RenderState, RowIterator};
use ghostty_vt::screen::CellWide;
use ghostty_vt::style::{Palette as VtPalette, RgbColor};
use ghostty_vt::selection::{FormatOptions, Selection};
use ghostty_vt::terminal::{CursorStyle, Event, Mode, Options, Point, PointCoordinate, ScrollViewport, Terminal};
use ghostty_vt::{focus, mouse, paste};
use wheel_proto::{ClientMsg, ServerMsg, SettingInfo};

/// Points of the terminal's own background around the grid, on the left and right, as the Swift
/// terminal's `window-padding-x`.
pub const PADDING_X: f64 = 10.0;
/// Points above and below the grid (`window-padding-y`).
pub const PADDING_Y: f64 = 8.0;
/// Lines of history a model keeps, as many as wheeld's own screen.
const SCROLLBACK_LINES: usize = 10_000;

/// `#rrggbb`, as the themes spell their colours.
const fn hex(v: u32) -> RgbColor {
    RgbColor { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Standard,
    Solarized,
    Gruvbox,
    Github,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

/// How a space's terminals draw, from its `terminal.*` settings as wheeld resolved them. A key
/// missing from the list, or a value this build does not know, keeps the default.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// Empty is the terminal's own font.
    pub font_family: String,
    /// Points.
    pub font_size: f64,
    /// A multiple of the font's own row height.
    pub line_height: f64,
    pub theme: Theme,
    pub cursor: CursorShape,
    pub cursor_blink: bool,
}

impl Default for Look {
    fn default() -> Self {
        Look { font_family: String::new(), font_size: 14.0, line_height: 1.0, theme: Theme::Standard, cursor: CursorShape::Block, cursor_blink: true }
    }
}

impl Look {
    pub fn from_settings(settings: &[SettingInfo]) -> Look {
        let value = |key: &str| settings.iter().find(|s| s.key == key).map(|s| s.value.as_str());
        let mut look = Look::default();
        if let Some(v) = value("terminal.font.family") {
            look.font_family = v.to_owned();
        }
        if let Some(v) = value("terminal.font.size").and_then(|v| v.parse().ok()) {
            look.font_size = v;
        }
        if let Some(v) = value("terminal.line_height").and_then(|v| v.parse().ok()) {
            look.line_height = v;
        }
        match value("terminal.theme") {
            Some("default") => look.theme = Theme::Standard,
            Some("solarized") => look.theme = Theme::Solarized,
            Some("gruvbox") => look.theme = Theme::Gruvbox,
            Some("github") => look.theme = Theme::Github,
            _ => {}
        }
        match value("terminal.cursor.style") {
            Some("block") => look.cursor = CursorShape::Block,
            Some("bar") => look.cursor = CursorShape::Bar,
            Some("underline") => look.cursor = CursorShape::Underline,
            _ => {}
        }
        if let Some(v) = value("terminal.cursor.blink") {
            look.cursor_blink = v == "true";
        }
        look
    }

    /// The colours this look draws with in the light or dark appearance.
    pub fn palette(&self, dark: bool) -> &'static Palette {
        match (self.theme, dark) {
            (Theme::Standard, true) => &AFTERGLOW,
            (Theme::Standard, false) => &ALABASTER,
            (Theme::Solarized, true) => &SOLARIZED_DARK,
            (Theme::Solarized, false) => &SOLARIZED_LIGHT,
            (Theme::Gruvbox, true) => &GRUVBOX_DARK,
            (Theme::Gruvbox, false) => &GRUVBOX_LIGHT,
            (Theme::Github, true) => &GITHUB_DARK,
            (Theme::Github, false) => &GITHUB_LIGHT,
        }
    }
}

/// One theme's colours in one appearance: the page, the text, ANSI colours 0 to 15, and the
/// cursor and selection colours when the theme names them.
#[derive(Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: RgbColor,
    pub foreground: RgbColor,
    pub ansi: [RgbColor; 16],
    pub cursor: Option<RgbColor>,
    pub selection: Option<RgbColor>,
}

// The default theme is the one the Swift terminal drew (libghostty-spm's `TerminalTheme.default`):
// Afterglow when dark, Alabaster when light.
static AFTERGLOW: Palette = Palette {
    background: hex(0x212121),
    foreground: hex(0xd0d0d0),
    ansi: [
        hex(0x151515), hex(0xac4142), hex(0x7e8e50), hex(0xe4b567), hex(0x6c99bb), hex(0x9f4e86), hex(0x7dd5cf), hex(0xd0d0d0),
        hex(0x505050), hex(0xac4142), hex(0x7e8e50), hex(0xe4b567), hex(0x6c99bb), hex(0x9f4e86), hex(0x7dd5cf), hex(0xf5f5f5),
    ],
    cursor: Some(hex(0xd0d0d0)),
    selection: Some(hex(0x303030)),
};
static ALABASTER: Palette = Palette {
    background: hex(0xf7f7f7),
    foreground: hex(0x000000),
    ansi: [
        hex(0x000000), hex(0xaa3731), hex(0x448c27), hex(0xcb8800), hex(0x325cc0), hex(0x7a3e9d), hex(0x0083b2), hex(0xf7f7f7),
        hex(0x777777), hex(0xf03e31), hex(0x60cb00), hex(0xffbc5d), hex(0x007acc), hex(0xe64ce6), hex(0x00aacb), hex(0xf7f7f7),
    ],
    cursor: Some(hex(0x007acc)),
    selection: Some(hex(0xc9d0d9)),
};
const SOLARIZED_ANSI: [RgbColor; 16] = [
    hex(0x073642), hex(0xdc322f), hex(0x859900), hex(0xb58900), hex(0x268bd2), hex(0xd33682), hex(0x2aa198), hex(0xeee8d5),
    hex(0x002b36), hex(0xcb4b16), hex(0x586e75), hex(0x657b83), hex(0x839496), hex(0x6c71c4), hex(0x93a1a1), hex(0xfdf6e3),
];
static SOLARIZED_DARK: Palette = Palette { background: hex(0x002b36), foreground: hex(0x839496), ansi: SOLARIZED_ANSI, cursor: None, selection: None };
static SOLARIZED_LIGHT: Palette = Palette { background: hex(0xfdf6e3), foreground: hex(0x657b83), ansi: SOLARIZED_ANSI, cursor: None, selection: None };
static GRUVBOX_DARK: Palette = Palette {
    background: hex(0x282828),
    foreground: hex(0xebdbb2),
    ansi: [
        hex(0x282828), hex(0xcc241d), hex(0x98971a), hex(0xd79921), hex(0x458588), hex(0xb16286), hex(0x689d6a), hex(0xa89984),
        hex(0x928374), hex(0xfb4934), hex(0xb8bb26), hex(0xfabd2f), hex(0x83a598), hex(0xd3869b), hex(0x8ec07c), hex(0xebdbb2),
    ],
    cursor: None,
    selection: None,
};
static GRUVBOX_LIGHT: Palette = Palette {
    background: hex(0xfbf1c7),
    foreground: hex(0x3c3836),
    ansi: [
        hex(0xfbf1c7), hex(0xcc241d), hex(0x98971a), hex(0xd79921), hex(0x458588), hex(0xb16286), hex(0x689d6a), hex(0x7c6f64),
        hex(0x928374), hex(0x9d0006), hex(0x79740e), hex(0xb57614), hex(0x076678), hex(0x8f3f71), hex(0x427b58), hex(0x3c3836),
    ],
    cursor: None,
    selection: None,
};
static GITHUB_DARK: Palette = Palette {
    background: hex(0x0d1117),
    foreground: hex(0xc9d1d9),
    ansi: [
        hex(0x484f58), hex(0xff7b72), hex(0x3fb950), hex(0xd29922), hex(0x58a6ff), hex(0xbc8cff), hex(0x39c5cf), hex(0xb1bac4),
        hex(0x6e7681), hex(0xffa198), hex(0x56d364), hex(0xe3b341), hex(0x79c0ff), hex(0xd2a8ff), hex(0x56d4dd), hex(0xf0f6fc),
    ],
    cursor: None,
    selection: None,
};
static GITHUB_LIGHT: Palette = Palette {
    background: hex(0xffffff),
    foreground: hex(0x24292f),
    ansi: [
        hex(0x24292f), hex(0xcf222e), hex(0x116329), hex(0x4d2d00), hex(0x0969da), hex(0x8250df), hex(0x1b7c83), hex(0x6e7781),
        hex(0x57606a), hex(0xa40e26), hex(0x1a7f37), hex(0x633c01), hex(0x218bff), hex(0xa475f9), hex(0x3192aa), hex(0x8c959f),
    ],
    cursor: None,
    selection: None,
};

/// One cell's size in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f64,
    pub height: f64,
}

impl CellMetrics {
    /// A cell from the font's unrounded advance and line height in pixels at `scale` pixels per
    /// point: Ghostty rounds each to whole pixels (`font/Metrics.zig`), then `line_height`
    /// stretches the row as `adjust-cell-height` does.
    pub fn from_face(advance_px: f64, line_px: f64, scale: f64, line_height: f64) -> CellMetrics {
        let height = (line_px.round() * line_height).round();
        CellMetrics { width: advance_px.round() / scale, height: height / scale }
    }
}

/// The grid a pane of `width` x `height` points holds: whole cells inside the padding, at
/// least one of each.
pub fn grid_for(width: f64, height: f64, cell: CellMetrics) -> (u16, u16) {
    let fit = |space: f64, size: f64| ((space / size).floor().clamp(1.0, u16::MAX as f64)) as u16;
    (fit(width - 2.0 * PADDING_X, cell.width), fit(height - 2.0 * PADDING_Y, cell.height))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Narrow,
    /// The first of two cells a wide character takes.
    Wide,
    /// The second cell of a wide character, or the gap a wide character left at a line's end:
    /// nothing to draw.
    Spacer,
}

/// One cell as it draws: its text, its colours resolved through the palette (`bg` is `None`
/// on the page's own background), and its attributes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub fg: RgbColor,
    pub bg: Option<RgbColor>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub width: Width,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Column and row in the viewport; `None` when the viewport is scrolled away from it.
    pub at: Option<(u16, u16)>,
    pub visible: bool,
    pub blinking: bool,
    pub shape: CursorVisualStyle,
}

/// What to draw: every row of the viewport, which rows changed since the last frame, the
/// cursor and the page's colours.
#[derive(Clone, Debug)]
pub struct Frame {
    pub cols: u16,
    pub rows: Vec<Vec<Cell>>,
    pub dirty: Vec<bool>,
    pub cursor: Cursor,
    pub background: RgbColor,
    pub foreground: RgbColor,
    pub cursor_color: Option<RgbColor>,
}

impl Frame {
    /// The viewport's text, one line per row with trailing blanks trimmed.
    pub fn text(&self) -> String {
        let lines: Vec<String> = self.rows.iter().map(|row| row.iter().map(|c| if c.text.is_empty() && c.width != Width::Spacer { " " } else { c.text.as_str() }).collect::<String>().trim_end().to_owned()).collect();
        lines.join("\n").trim_end().to_owned()
    }
}

/// A key as the GPUI view reports it: GPUI's name for the key (`"a"`, `"up"`, `"enter"`, `"f1"`),
/// the modifiers held, and the text the keyboard layout made of it (`key_char`).
#[derive(Clone, Debug, Default)]
pub struct KeyInput<'a> {
    pub key: &'a str,
    pub text: Option<&'a str>,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub command: bool,
    pub release: bool,
    pub repeat: bool,
}

/// Ghostty's key for a GPUI key name.
fn vt_key(name: &str) -> key::Key {
    use key::Key as K;
    let letters = [K::A, K::B, K::C, K::D, K::E, K::F, K::G, K::H, K::I, K::J, K::K, K::L, K::M, K::N, K::O, K::P, K::Q, K::R, K::S, K::T, K::U, K::V, K::W, K::X, K::Y, K::Z];
    let digits = [K::Digit0, K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8, K::Digit9];
    let functions = [K::F1, K::F2, K::F3, K::F4, K::F5, K::F6, K::F7, K::F8, K::F9, K::F10, K::F11, K::F12, K::F13, K::F14, K::F15, K::F16, K::F17, K::F18, K::F19, K::F20, K::F21, K::F22, K::F23, K::F24];
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return match c {
            'a'..='z' => letters[(c as u8 - b'a') as usize],
            '0'..='9' => digits[(c as u8 - b'0') as usize],
            '`' => K::Backquote,
            '\\' => K::Backslash,
            '[' => K::BracketLeft,
            ']' => K::BracketRight,
            ',' => K::Comma,
            '=' => K::Equal,
            '-' => K::Minus,
            '.' => K::Period,
            '\'' => K::Quote,
            ';' => K::Semicolon,
            '/' => K::Slash,
            ' ' => K::Space,
            _ => K::Unidentified,
        };
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=24).contains(n)) {
        return functions[n - 1];
    }
    match name {
        "up" => K::ArrowUp,
        "down" => K::ArrowDown,
        "left" => K::ArrowLeft,
        "right" => K::ArrowRight,
        "enter" => K::Enter,
        "escape" => K::Escape,
        "backspace" => K::Backspace,
        "tab" => K::Tab,
        "space" => K::Space,
        "delete" => K::Delete,
        "home" => K::Home,
        "end" => K::End,
        "pageup" => K::PageUp,
        "pagedown" => K::PageDown,
        "insert" => K::Insert,
        _ => K::Unidentified,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

/// One attached term's screen.
pub struct TermModel {
    pub id: String,
    terminal: Terminal,
    render: RenderState,
    row_iter: RowIterator,
    cell_iter: CellIterator,
    keys: key::Encoder,
    mouse: mouse::Encoder,
    /// Whether the Option key types Alt (`macos-option-as-alt`); off, it types the layout's character.
    pub option_as_alt: bool,
    /// The program's title (OSC 0/2), cleared on each attach.
    pub title: Option<String>,
    /// The shell's exit code once it exited (`None` inside for a signal).
    pub exited: Option<Option<u32>>,
    /// The grid last sent as `term_resize` since the last attach.
    sent: Option<(u16, u16)>,
    /// Whether a mouse button is held, which button-event mouse reports (mode 1002) follow.
    button_down: bool,
    /// The size the mouse encoder was last given; setting it again forgets the last cell reported.
    mouse_size: Option<mouse::EncoderSize>,
    /// The program's mouse modes the encoder last took; taking them again forgets the last cell too.
    mouse_modes: Option<[bool; 8]>,
    /// What the terminal wrote back to the program (answers to queries), waiting to be sent.
    replies: Vec<u8>,
}

impl std::fmt::Debug for TermModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermModel").field("id", &self.id).field("title", &self.title).field("exited", &self.exited).finish_non_exhaustive()
    }
}

impl TermModel {
    /// A model for term `id`, `cols` x `rows`, drawing `look` in the light or dark appearance.
    pub fn new(id: &str, cols: u16, rows: u16, look: &Look, dark: bool) -> ghostty_vt::error::Result<TermModel> {
        let mut terminal = Terminal::new(Options { cols, rows, ..Options::default() })?;
        terminal.set_scrollback_max_lines(Some(SCROLLBACK_LINES))?;
        let mut model = TermModel {
            id: id.to_owned(),
            terminal,
            render: RenderState::new()?,
            row_iter: RowIterator::new()?,
            cell_iter: CellIterator::new()?,
            keys: key::Encoder::new()?,
            mouse: mouse::Encoder::new()?,
            option_as_alt: false,
            title: None,
            exited: None,
            sent: None,
            button_down: false,
            mouse_size: None,
            mouse_modes: None,
            replies: Vec::new(),
        };
        model.restyle(look, dark)?;
        Ok(model)
    }

    /// Ask wheeld to stream this term to the connection.
    pub fn attach(&self) -> ClientMsg {
        ClientMsg::TermAttach { req: None, id: self.id.clone(), whole_lines: false }
    }

    pub fn detach(&self) -> ClientMsg {
        ClientMsg::TermDetach { id: self.id.clone() }
    }

    /// Take in a message from wheeld; whether it was this term's.
    pub fn receive(&mut self, msg: &ServerMsg) -> bool {
        match msg {
            // The replay that follows draws the whole screen on a reset terminal.
            ServerMsg::TermAttached { id, .. } if *id == self.id => {
                self.terminal.reset();
                self.title = None;
                self.exited = None;
                // Another client may have sized the term since: the next resize goes whatever it is.
                self.sent = None;
                true
            }
            ServerMsg::TermOutput { id, data } if *id == self.id => {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
                    self.feed(&bytes);
                }
                true
            }
            ServerMsg::TermExited { id, code } if *id == self.id => {
                self.exited = Some(*code);
                true
            }
            _ => false,
        }
    }

    /// Write program output to the screen.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
        for event in self.terminal.events() {
            match event {
                Event::PtyWrite(bytes) => self.replies.extend_from_slice(&bytes),
                Event::TitleChanged(title) => self.title = Some(title),
                _ => {}
            }
        }
    }

    /// The terminal's answers to the program's queries, as one `term_input`, when it wrote any.
    pub fn take_replies(&mut self) -> Option<ClientMsg> {
        (!self.replies.is_empty()).then(|| {
            let bytes = std::mem::take(&mut self.replies);
            self.input(&bytes)
        })
    }

    /// `bytes` typed into the term.
    pub fn input(&self, bytes: &[u8]) -> ClientMsg {
        ClientMsg::TermInput { req: None, id: self.id.clone(), data: base64::engine::general_purpose::STANDARD.encode(bytes) }
    }

    /// Size the screen to `cols` x `rows`; the `term_resize` to send when the grid changed.
    pub fn resize(&mut self, cols: u16, rows: u16, cell: CellMetrics) -> Option<ClientMsg> {
        if self.sent == Some((cols, rows)) {
            return None;
        }
        self.terminal.resize(cols, rows, cell.width.round() as u32, cell.height.round() as u32).ok()?;
        self.sent = Some((cols, rows));
        Some(ClientMsg::TermResize { req: None, id: self.id.clone(), cols, rows })
    }

    /// Draw `look` from now on, keeping the screen: the theme's colours become the terminal's
    /// defaults (a colour the program set stays), and the cursor's shape and blink.
    pub fn restyle(&mut self, look: &Look, dark: bool) -> ghostty_vt::error::Result<()> {
        let colours = look.palette(dark);
        let mut palette = VtPalette(self.terminal.default_color_palette()?.0);
        palette.0[..16].copy_from_slice(&colours.ansi);
        let shape = match look.cursor {
            CursorShape::Block => CursorStyle::Block,
            CursorShape::Bar => CursorStyle::Bar,
            CursorShape::Underline => CursorStyle::Underline,
        };
        self.terminal
            .set_default_fg_color(Some(colours.foreground))?
            .set_default_bg_color(Some(colours.background))?
            .set_default_cursor_color(colours.cursor)?
            .set_default_color_palette(Some(palette))?
            .set_default_cursor_style(Some(shape))?
            .set_default_cursor_blink(Some(look.cursor_blink))?;
        Ok(())
    }

    /// Scroll the viewport by `rows` (negative is up, into history), or back to the bottom.
    pub fn scroll(&mut self, rows: Option<isize>) {
        self.terminal.scroll_viewport(rows.map_or(ScrollViewport::Bottom, ScrollViewport::Delta));
    }

    pub fn mode(&self, mode: Mode) -> bool {
        self.terminal.mode(mode).unwrap_or(false)
    }

    /// The viewport as it draws now; the rows it marks dirty are those changed since the last call.
    pub fn frame(&mut self) -> ghostty_vt::error::Result<Frame> {
        let Self { terminal, render, row_iter, cell_iter, .. } = self;
        let snapshot = render.update(terminal)?;
        let colours = snapshot.colors()?;
        let full = snapshot.dirty()? == ghostty_vt::render::Dirty::Full;
        let c = snapshot.cursor()?;
        let cursor = Cursor { at: c.viewport.map(|v| (v.x, v.y)), visible: c.visible, blinking: c.blinking, shape: c.visual_style };
        let cols = snapshot.cols()?;
        let mut rows = Vec::new();
        let mut dirty = Vec::new();
        {
            let mut row_it = row_iter.update(&snapshot)?;
            while let Some(row) = row_it.next() {
                dirty.push(full || row.dirty()?);
                let mut cells = Vec::with_capacity(cols as usize);
                let mut cell_it = cell_iter.update(row)?;
                while let Some(cell) = cell_it.next() {
                    let style = cell.style()?;
                    let mut text = String::new();
                    cell.graphemes_utf8(&mut text)?;
                    let width = match cell.raw_cell()?.wide()? {
                        CellWide::Narrow => Width::Narrow,
                        CellWide::Wide => Width::Wide,
                        _ => Width::Spacer,
                    };
                    cells.push(Cell {
                        text,
                        fg: cell.fg_color()?.unwrap_or(colours.foreground),
                        bg: cell.bg_color()?,
                        bold: style.bold,
                        italic: style.italic,
                        faint: style.faint,
                        underline: style.underline != ghostty_vt::style::Underline::None,
                        strikethrough: style.strikethrough,
                        inverse: style.inverse,
                        invisible: style.invisible,
                        width,
                        selected: cell.is_selected()?,
                    });
                }
                row.set_dirty(false)?;
                rows.push(cells);
            }
        }
        snapshot.clean()?;
        Ok(Frame { cols, rows, dirty, cursor, background: colours.background, foreground: colours.foreground, cursor_color: colours.cursor })
    }

    /// The bytes a key sends the program, as Ghostty encodes it for the terminal's modes now
    /// (cursor-key application mode, the Kitty keyboard flags) and `option_as_alt`. Empty for a
    /// key that sends nothing, such as a modifier alone.
    pub fn encode_key(&mut self, input: &KeyInput) -> Vec<u8> {
        let Ok(mut event) = key::Event::new() else { return Vec::new() };
        let mut mods = Mods::empty();
        for (held, m) in [(input.shift, Mods::SHIFT), (input.control, Mods::CTRL), (input.alt, Mods::ALT), (input.command, Mods::SUPER)] {
            if held {
                mods |= m;
            }
        }
        // A single character key's own character, unshifted, as the layout prints it without modifiers.
        let unshifted = {
            let mut chars = input.key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => (input.key == "space").then_some(' '),
            }
        };
        // Option typing Alt sends the key's own character, which the encoder prefixes with ESC;
        // otherwise the layout's character is the text and Option was spent making it.
        let text = if input.alt && self.option_as_alt {
            unshifted.map(|c| if input.shift { c.to_uppercase().to_string() } else { c.to_string() })
        } else {
            input.text.map(str::to_owned)
        };
        let mut consumed = Mods::empty();
        if text.is_some() {
            if input.shift {
                consumed |= Mods::SHIFT;
            }
            if input.alt && !self.option_as_alt {
                consumed |= Mods::ALT;
            }
        }
        let action = if input.release { key::Action::Release } else if input.repeat { key::Action::Repeat } else { key::Action::Press };
        event.set_action(action).set_key(vt_key(input.key)).set_mods(mods).set_consumed_mods(consumed).set_utf8(text);
        if let Some(c) = unshifted {
            event.set_unshifted_codepoint(c);
        }
        self.keys.set_options_from_terminal(&self.terminal);
        self.keys.set_macos_option_as_alt(if self.option_as_alt { OptionAsAlt::True } else { OptionAsAlt::False });
        let mut out = Vec::new();
        let _ = self.keys.encode_to_vec(&event, &mut out);
        out
    }

    /// The bytes a paste of `text` sends: wrapped in bracketed-paste markers when the program
    /// turned mode 2004 on, its newlines sent as returns otherwise.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        let mut data = text.as_bytes().to_vec();
        let mut out = vec![0u8; data.len() + 16];
        match paste::encode(&mut data, self.mode(Mode::BRACKETED_PASTE), &mut out) {
            Ok(n) => {
                out.truncate(n);
                out
            }
            Err(_) => Vec::new(),
        }
    }

    /// The bytes a mouse event at `(x, y)` points inside a `width` x `height` pane sends, when
    /// the program asked for mouse reports; empty when it did not.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_mouse(&mut self, action: MouseAction, button: Option<MouseButton>, x: f64, y: f64, width: f64, height: f64, cell: CellMetrics, shift: bool, control: bool, alt: bool) -> Vec<u8> {
        if !self.terminal.is_mouse_tracking().unwrap_or(false) {
            return Vec::new();
        }
        let Ok(mut event) = mouse::Event::new() else { return Vec::new() };
        let mut mods = Mods::empty();
        for (held, m) in [(shift, Mods::SHIFT), (control, Mods::CTRL), (alt, Mods::ALT)] {
            if held {
                mods |= m;
            }
        }
        event
            .set_action(match action {
                MouseAction::Press => mouse::Action::Press,
                MouseAction::Release => mouse::Action::Release,
                MouseAction::Motion => mouse::Action::Motion,
            })
            .set_button(button.map(|b| match b {
                MouseButton::Left => mouse::Button::Left,
                MouseButton::Middle => mouse::Button::Middle,
                MouseButton::Right => mouse::Button::Right,
                MouseButton::WheelUp => mouse::Button::Four,
                MouseButton::WheelDown => mouse::Button::Five,
            }))
            .set_mods(mods)
            .set_position(mouse::Position { x: x as f32, y: y as f32 });
        match action {
            MouseAction::Press if !matches!(button, Some(MouseButton::WheelUp | MouseButton::WheelDown)) => self.button_down = true,
            MouseAction::Release => self.button_down = false,
            _ => {}
        }
        let px = |v: f64| v.round().max(0.0) as u32;
        let size = mouse::EncoderSize {
            screen_width: px(width),
            screen_height: px(height),
            cell_width: px(cell.width).max(1),
            cell_height: px(cell.height).max(1),
            padding_top: px(PADDING_Y),
            padding_bottom: px(PADDING_Y),
            padding_left: px(PADDING_X),
            padding_right: px(PADDING_X),
        };
        // One motion report per cell the pointer enters, not per point it moves.
        let modes = [Mode::X10_MOUSE, Mode::NORMAL_MOUSE, Mode::BUTTON_MOUSE, Mode::ANY_MOUSE, Mode::UTF8_MOUSE, Mode::SGR_MOUSE, Mode::URXVT_MOUSE, Mode::SGR_PIXELS_MOUSE].map(|m| self.mode(m));
        if self.mouse_modes != Some(modes) {
            self.mouse.set_options_from_terminal(&self.terminal);
            self.mouse_modes = Some(modes);
        }
        self.mouse.set_any_button_pressed(self.button_down).set_track_last_cell(true);
        if self.mouse_size != Some(size) {
            self.mouse.set_size(size);
            self.mouse_size = Some(size);
        }
        let mut out = Vec::new();
        let _ = self.mouse.encode_to_vec(&event, &mut out);
        out
    }

    /// Select from one viewport cell (column, row) to another, inclusive; drawn through `Cell::selected`.
    pub fn select(&mut self, from: (u16, u16), to: (u16, u16)) -> ghostty_vt::error::Result<()> {
        let at = |(x, y): (u16, u16)| Point::Viewport(PointCoordinate { x, y: y as u32 });
        let selection = Selection::new(self.terminal.grid_ref(at(from))?, self.terminal.grid_ref(at(to))?, false);
        self.terminal.set_selection(Some(&selection))?;
        Ok(())
    }

    pub fn clear_selection(&mut self) {
        let _ = self.terminal.set_selection(None);
    }

    /// The selected text, as Copy puts it on the pasteboard; `None` with nothing selected.
    pub fn selection_text(&self) -> Option<String> {
        let bytes = self.terminal.format_selection_alloc(FormatOptions::new().with_trim(true)).ok()??;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The focus report a program that turned on mode 1004 reads when the pane gains or loses
    /// the keyboard; empty when it did not ask.
    pub fn encode_focus(&self, gained: bool) -> Vec<u8> {
        if !self.mode(Mode::FOCUS_EVENT) {
            return Vec::new();
        }
        let mut buf = [0u8; 8];
        let event = if gained { focus::Event::Gained } else { focus::Event::Lost };
        event.encode(&mut buf).map(|n| buf[..n].to_vec()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghostty_vt::fmt::{Format, Formatter, FormatterOptions};

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn model(cols: u16, rows: u16) -> TermModel {
        TermModel::new("t1", cols, rows, &Look::default(), true).unwrap()
    }

    /// Output with 16-colour, 256-colour and truecolour SGR and a wide character.
    const FIXTURE: &[u8] = b"plain \x1b[31mred\x1b[0m \x1b[38;5;208mamber\x1b[0m \x1b[38;2;10;20;30;48;2;200;100;50mtrue\x1b[0m\r\n\xe6\xbc\xa2x \x1b[1;3;4;9mall\x1b[0m\r\nshell$ ";
    /// A full-screen program drawing on the alternate screen.
    const FULL_SCREEN: &[u8] = b"\x1b[?1049h\x1b[2;3Hfull \x1b[44mscreen\x1b[0m";

    /// wheeld's replay of its screen (`daemon/src/pty.rs` `snapshot`), so the model is fed what
    /// an attach sends.
    fn replay(screen: &Terminal) -> Vec<u8> {
        let options = FormatterOptions::new()
            .with_format(Format::Vt)
            .with_cursor(true)
            .with_style(true)
            .with_hyperlink(true)
            .with_protection(true)
            .with_kitty_keyboard(true)
            .with_charsets(true)
            .with_modes(true)
            .with_scrolling_region(true)
            .with_tabstops(true)
            .with_keyboard(true)
            .with_pwd(true);
        Formatter::new(screen, options).unwrap().format_alloc().unwrap().to_vec()
    }

    fn plain(t: &Terminal) -> String {
        let options = FormatterOptions::new().with_format(Format::Plain).with_trim(true);
        let text = String::from_utf8(Formatter::new(t, options).unwrap().format_alloc().unwrap().to_vec()).unwrap();
        text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim_end().to_owned()
    }

    #[test]
    fn an_attach_replay_draws_what_wheelds_screen_holds() {
        let mut daemon = Terminal::new(Options { cols: 40, rows: 6, ..Options::default() }).unwrap();
        daemon.vt_write(FIXTURE);
        let mut m = model(40, 6);
        m.feed(b"stale text from before the attach");
        assert!(m.receive(&ServerMsg::TermAttached { req: None, id: "t1".into(), replay_bytes: 0 }));
        assert!(m.receive(&ServerMsg::TermOutput { id: "t1".into(), data: b64(&replay(&daemon)) }));
        assert!(!m.receive(&ServerMsg::TermOutput { id: "t2".into(), data: b64(b"other") }));
        let cursor = |t: &Terminal| Some((t.cursor_x().unwrap(), t.cursor_y().unwrap()));
        let frame = m.frame().unwrap();
        assert_eq!(frame.text(), plain(&daemon));
        assert_eq!(frame.cursor.at, cursor(&daemon));
        // Live output: a full-screen program on the alternate screen, then back to the shell.
        daemon.vt_write(FULL_SCREEN);
        m.receive(&ServerMsg::TermOutput { id: "t1".into(), data: b64(FULL_SCREEN) });
        let full = m.frame().unwrap();
        assert_eq!(full.text(), plain(&daemon));
        assert!(full.text().contains("full screen"), "{}", full.text());
        assert_eq!(full.cursor.at, cursor(&daemon));
        assert_eq!(full.rows[1][7].bg, Some(AFTERGLOW.ansi[4]));
        assert!(m.mode(Mode::ALT_SCREEN_SAVE));
        daemon.vt_write(b"\x1b[?1049l");
        m.feed(b"\x1b[?1049l");
        let frame = m.frame().unwrap();
        assert_eq!(frame.text(), plain(&daemon));
        assert_eq!(frame.cursor.at, cursor(&daemon));
        let at = |text: &str| -> (usize, usize) {
            frame.rows.iter().enumerate().find_map(|(r, row)| {
                let line: String = row.iter().map(|c| if c.text.is_empty() { " ".into() } else { c.text.clone() }).collect();
                line.find(text).map(|byte| (r, line[..byte].chars().count()))
            }).unwrap()
        };
        let cell = |text: &str| { let (r, c) = at(text); frame.rows[r][c].clone() };
        let palette = AFTERGLOW.ansi;
        assert_eq!(cell("red").fg, palette[1]);
        assert_eq!(cell("plain").fg, AFTERGLOW.foreground);
        assert_eq!(cell("plain").bg, None);
        assert_eq!(cell("amber").fg, RgbColor { r: 0xff, g: 0x87, b: 0x00 });
        assert_eq!(cell("true").fg, RgbColor { r: 10, g: 20, b: 30 });
        assert_eq!(cell("true").bg, Some(RgbColor { r: 200, g: 100, b: 50 }));
        let all = cell("all");
        assert!(all.bold && all.italic && all.underline && all.strikethrough);
        assert_eq!(frame.rows[1][0].text, "漢");
        assert_eq!(frame.rows[1][0].width, Width::Wide);
        assert_eq!(frame.rows[1][1].width, Width::Spacer);
        assert_eq!(frame.rows[1][2].text, "x");
    }

    #[test]
    fn a_frame_marks_only_the_rows_that_changed() {
        let mut m = model(20, 4);
        m.feed(b"one\r\ntwo");
        let first = m.frame().unwrap();
        assert!(first.dirty.iter().all(|d| *d));
        m.feed(b"!");
        let next = m.frame().unwrap();
        assert_eq!(next.dirty, vec![false, true, false, false]);
        assert_eq!(next.text(), "one\ntwo!");
    }

    #[test]
    fn the_shell_exiting_and_a_title_reach_the_model() {
        let mut m = model(20, 4);
        m.feed(b"\x1b]2;vim notes.md\x07");
        assert_eq!(m.title.as_deref(), Some("vim notes.md"));
        assert!(m.receive(&ServerMsg::TermExited { id: "t1".into(), code: Some(3) }));
        assert_eq!(m.exited, Some(Some(3)));
    }

    #[test]
    fn a_query_is_answered_through_term_input() {
        let mut m = model(20, 4);
        m.feed(b"\x1b[6n");
        let Some(ClientMsg::TermInput { id, data, .. }) = m.take_replies() else { panic!("no reply") };
        assert_eq!(id, "t1");
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(data).unwrap(), b"\x1b[1;1R");
        assert!(m.take_replies().is_none());
    }

    #[test]
    fn each_theme_gives_wheelkits_colours_and_the_default_the_swift_terminals() {
        // WheelKit's TerminalPalette, light then dark: background, foreground, ANSI 1 and 15.
        let wheelkit = [
            (Theme::Solarized, "#fdf6e3", "#657b83", "#dc322f", "#fdf6e3", "#002b36", "#839496"),
            (Theme::Gruvbox, "#fbf1c7", "#3c3836", "#cc241d", "#3c3836", "#282828", "#ebdbb2"),
            (Theme::Github, "#ffffff", "#24292f", "#cf222e", "#8c959f", "#0d1117", "#c9d1d9"),
            (Theme::Standard, "#f7f7f7", "#000000", "#aa3731", "#f7f7f7", "#212121", "#d0d0d0"),
        ];
        let css = |c: RgbColor| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);
        for (theme, light_bg, light_fg, light_red, light_white, dark_bg, dark_fg) in wheelkit {
            let look = Look { theme, ..Look::default() };
            let (light, dark) = (look.palette(false), look.palette(true));
            assert_eq!((css(light.background), css(light.foreground), css(light.ansi[1]), css(light.ansi[15])), (light_bg.into(), light_fg.into(), light_red.into(), light_white.into()), "{theme:?} light");
            assert_eq!((css(dark.background), css(dark.foreground)), (dark_bg.into(), dark_fg.into()), "{theme:?} dark");
        }
        assert_eq!(css(GRUVBOX_LIGHT.ansi[9]), "#9d0006");
        assert_eq!(css(GITHUB_DARK.ansi[8]), "#6e7681");
        // A model draws the look's colours: the page, the text and a palette entry.
        let mut m = TermModel::new("t1", 20, 4, &Look { theme: Theme::Gruvbox, ..Look::default() }, false).unwrap();
        m.feed(b"\x1b[32mgreen");
        let frame = m.frame().unwrap();
        assert_eq!((frame.background, frame.foreground), (GRUVBOX_LIGHT.background, GRUVBOX_LIGHT.foreground));
        assert_eq!(frame.rows[0][0].fg, GRUVBOX_LIGHT.ansi[2]);
    }

    #[test]
    fn up_arrow_follows_cursor_key_application_mode_and_option_types_alt_when_asked() {
        let mut m = model(20, 4);
        let up = KeyInput { key: "up", ..KeyInput::default() };
        assert_eq!(m.encode_key(&up), b"\x1b[A");
        m.feed(b"\x1b[?1h");
        assert_eq!(m.encode_key(&up), b"\x1bOA");
        let option_f = KeyInput { key: "f", text: Some("ƒ"), alt: true, ..KeyInput::default() };
        assert_eq!(m.encode_key(&option_f), "ƒ".as_bytes());
        m.option_as_alt = true;
        assert_eq!(m.encode_key(&option_f), b"\x1bf");
        assert_eq!(m.encode_key(&KeyInput { key: "f", text: Some("Ï"), alt: true, shift: true, ..KeyInput::default() }), b"\x1bF");
        assert_eq!(m.encode_key(&KeyInput { key: "c", text: Some("c"), control: true, ..KeyInput::default() }), b"\x03");
        assert_eq!(m.encode_key(&KeyInput { key: "a", text: Some("A"), shift: true, ..KeyInput::default() }), b"A");
        assert_eq!(m.encode_key(&KeyInput { key: "enter", text: Some("\r"), ..KeyInput::default() }), b"\r");
    }

    #[test]
    fn a_paste_is_bracketed_only_after_the_program_asks() {
        let mut m = model(20, 4);
        assert_eq!(m.encode_paste("a\nb"), b"a\rb");
        m.feed(b"\x1b[?2004h");
        assert_eq!(m.encode_paste("a\nb"), b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn mouse_and_focus_reports_go_only_to_a_program_that_asked() {
        let mut m = model(80, 24);
        let cell = CellMetrics { width: 8.0, height: 17.0 };
        assert!(m.encode_mouse(MouseAction::Press, Some(MouseButton::Left), 30.0, 20.0, 800.0, 600.0, cell, false, false, false).is_empty());
        assert!(m.encode_focus(true).is_empty());
        m.feed(b"\x1b[?1000h\x1b[?1006h\x1b[?1004h");
        // (30, 20) is past the 10 x 8 padding: column 3, row 1, one-based in SGR.
        assert_eq!(m.encode_mouse(MouseAction::Press, Some(MouseButton::Left), 30.0, 20.0, 800.0, 600.0, cell, false, false, false), b"\x1b[<0;3;1M");
        assert_eq!(m.encode_focus(true), b"\x1b[I");
        // Button-event tracking: a drag with the button held reports each cell entered, once.
        m.feed(b"\x1b[?1002h");
        assert!(m.encode_mouse(MouseAction::Motion, Some(MouseButton::Left), 50.0, 20.0, 800.0, 600.0, cell, false, false, false).len() > 0);
        assert!(m.encode_mouse(MouseAction::Motion, Some(MouseButton::Left), 51.0, 21.0, 800.0, 600.0, cell, false, false, false).is_empty());
        m.encode_mouse(MouseAction::Release, Some(MouseButton::Left), 51.0, 21.0, 800.0, 600.0, cell, false, false, false);
        assert!(m.encode_mouse(MouseAction::Motion, None, 90.0, 20.0, 800.0, 600.0, cell, false, false, false).is_empty());
        assert_eq!(m.encode_focus(false), b"\x1b[O");
    }

    #[test]
    fn the_grid_matches_the_swift_terminals() {
        // JetBrains Mono, the terminal's own font: advance 0.6 em, line 1.32 em; Retina, 2 px a point.
        let jetbrains = |size: f64| CellMetrics::from_face(0.6 * size * 2.0, 1.32 * size * 2.0, 2.0, 1.0);
        // The grids the Swift terminal reported to wheeld in the hidden check, by pane size.
        assert_eq!(grid_for(478.0, 298.0, jetbrains(14.0)), (53, 15));
        assert_eq!(grid_for(478.0, 135.0, jetbrains(14.0)), (53, 6));
        assert_eq!(grid_for(478.0, 298.0, jetbrains(18.0)), (41, 11));
        assert_eq!(grid_for(478.0, 135.0, jetbrains(18.0)), (41, 4));
        // 13 pt: 8 x 17 point cells.
        assert_eq!(jetbrains(13.0), CellMetrics { width: 8.0, height: 17.0 });
        assert_eq!(grid_for(800.0, 600.0, jetbrains(13.0)), (97, 34));
        assert_eq!(grid_for(5.0, 5.0, jetbrains(13.0)), (1, 1));
        // A line height of 1.5 stretches each row half again.
        assert_eq!(CellMetrics::from_face(15.6, 34.32, 2.0, 1.5).height, 25.5);
    }

    #[test]
    fn a_resize_is_sent_once_per_grid() {
        let mut m = model(80, 24);
        let cell = CellMetrics { width: 8.0, height: 17.0 };
        assert!(matches!(m.resize(97, 34, cell), Some(ClientMsg::TermResize { cols: 97, rows: 34, .. })));
        assert!(m.resize(97, 34, cell).is_none());
        assert_eq!(m.frame().unwrap().cols, 97);
        // After a new attach wheeld's term may be any size: the grid goes again.
        m.receive(&ServerMsg::TermAttached { req: None, id: "t1".into(), replay_bytes: 0 });
        assert!(m.resize(97, 34, cell).is_some());
    }

    #[test]
    fn a_selection_draws_and_copies() {
        let mut m = model(20, 4);
        m.feed(b"hello world\r\nsecond");
        assert_eq!(m.selection_text(), None);
        m.select((6, 0), (10, 0)).unwrap();
        assert_eq!(m.selection_text().as_deref(), Some("world"));
        let frame = m.frame().unwrap();
        assert!(!frame.rows[0][5].selected && frame.rows[0][6].selected && frame.rows[0][10].selected);
        m.clear_selection();
        assert_eq!(m.selection_text(), None);
    }

    #[test]
    fn a_changed_setting_restyles_the_model_without_a_new_attach() {
        let setting = |key: &str, value: &str| -> SettingInfo { serde_json::from_value(serde_json::json!({"key": key, "value": value, "origin": "default", "default": "", "help": ""})).unwrap() };
        let mut m = model(20, 4);
        m.feed(b"\x1b[31mkept\x1b[0m");
        let look = Look::from_settings(&[setting("terminal.theme", "solarized"), setting("terminal.cursor.style", "bar"), setting("terminal.cursor.blink", "false"), setting("terminal.font.size", "13"), setting("terminal.line_height", "1.2")]);
        assert_eq!((look.theme, look.cursor, look.cursor_blink, look.font_size, look.line_height), (Theme::Solarized, CursorShape::Bar, false, 13.0, 1.2));
        assert_eq!(Look::from_settings(&[setting("terminal.theme", "neon")]).theme, Theme::Standard);
        m.restyle(&look, true).unwrap();
        let frame = m.frame().unwrap();
        assert_eq!(frame.text(), "kept");
        assert_eq!((frame.background, frame.rows[0][0].fg), (SOLARIZED_DARK.background, SOLARIZED_DARK.ansi[1]));
        assert_eq!((frame.cursor.shape, frame.cursor.blinking), (CursorVisualStyle::Bar, false));
    }
}
