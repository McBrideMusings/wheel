//! Which icon buttons a pane's header draws (HeaderControls.swift): each control's place for each
//! kind of pane, the person's choice of top bar, ⋯ menu or off, kept in this client's own file
//! (the header is this client's drawing, so no server holds it), and the level a narrow header
//! falls to, where it drops controls group by group and the ⋯ menu offers each one it dropped.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

/// A control a pane's header can draw as an icon button, in the order Settings lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    SwitchView,
    Conversations,
    Info,
    SplitRight,
    SplitDown,
    Pop,
    Background,
    Zoom,
    Close,
}

impl Control {
    pub const ALL: [Control; 9] = [Control::SwitchView, Control::Conversations, Control::Info, Control::SplitRight, Control::SplitDown, Control::Pop, Control::Background, Control::Zoom, Control::Close];

    /// The name the Swift client stores it under.
    pub fn name(self) -> &'static str {
        match self {
            Control::SwitchView => "switchView",
            Control::Conversations => "conversations",
            Control::Info => "info",
            Control::SplitRight => "splitRight",
            Control::SplitDown => "splitDown",
            Control::Pop => "pop",
            Control::Background => "background",
            Control::Zoom => "zoom",
            Control::Close => "close",
        }
    }
    pub fn named(name: &str) -> Option<Control> {
        Control::ALL.into_iter().find(|c| c.name() == name)
    }
    pub fn title(self) -> &'static str {
        match self {
            Control::SwitchView => "Chat / terminal switch",
            Control::Conversations => "Conversations",
            Control::Info => "Pane info",
            Control::SplitRight => "Split right",
            Control::SplitDown => "Split down",
            Control::Pop => "Pop out",
            Control::Background => "Background",
            Control::Zoom => "Zoom",
            Control::Close => "Close",
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Control::SwitchView => "bubble.left.and.text.bubble.right",
            Control::Conversations => "clock.arrow.circlepath",
            Control::Info => "info.circle",
            Control::SplitRight => "square.split.2x1",
            Control::SplitDown => "square.split.1x2",
            Control::Pop => "rectangle.on.rectangle",
            Control::Background => "tray.and.arrow.down",
            Control::Zoom => "arrow.up.left.and.arrow.down.right",
            Control::Close => "xmark",
        }
    }
    /// The id the header's button and its menu item go by (`header_buttons`, `/panemenu`).
    pub fn id(self) -> &'static str {
        match self {
            Control::SwitchView => "switch-view",
            Control::SplitRight => "split-right",
            Control::SplitDown => "split-down",
            other => other.name(),
        }
    }
    /// The header level at which a narrow header stops drawing this control.
    pub fn drop_level(self) -> u8 {
        match self {
            Control::Conversations | Control::Info | Control::Pop | Control::Background => 1,
            Control::SwitchView => 2,
            Control::SplitRight | Control::SplitDown => 3,
            Control::Zoom => 4,
            Control::Close => u8::MAX,
        }
    }
    /// Whether panes of `kind` (a kind name, `unknown` for one this client does not know) have it.
    pub fn applies(self, kind: &str) -> bool {
        match self {
            Control::SwitchView => kind == "terminal" || kind == "agent",
            Control::Conversations => kind == "agent",
            Control::Info | Control::SplitRight | Control::SplitDown => kind != "unknown",
            Control::Pop | Control::Background | Control::Zoom | Control::Close => true,
        }
    }
}

/// The levels a header can fall to: 0 draws every control, 4 only the menu and Close.
pub const LEVELS: std::ops::RangeInclusive<u8> = 0..=4;

/// The pane kinds Settings sets a header up for, in its order.
pub const KINDS: [&str; 5] = ["terminal", "agent", "web", "browser", "empty"];

/// The name a pane kind's header is set up under: one of `KINDS`, else `unknown`.
pub fn kind_name(kind: &str) -> &str {
    KINDS.into_iter().find(|k| *k == kind).unwrap_or("unknown")
}

/// Where a control is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Bar,
    Menu,
    Off,
}

impl Placement {
    pub const ALL: [Placement; 3] = [Placement::Bar, Placement::Menu, Placement::Off];
    pub fn name(self) -> &'static str {
        match self {
            Placement::Bar => "bar",
            Placement::Menu => "menu",
            Placement::Off => "off",
        }
    }
    pub fn named(name: &str) -> Option<Placement> {
        Placement::ALL.into_iter().find(|p| p.name() == name)
    }
    pub fn title(self) -> &'static str {
        match self {
            Placement::Bar => "Top bar",
            Placement::Menu => "⋯ menu",
            Placement::Off => "Off",
        }
    }
}

/// Each kind's choices; a control with no choice is in the top bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// Placement by kind name, then control name.
    pub choices: BTreeMap<String, BTreeMap<String, Placement>>,
}

impl Layout {
    pub fn placement(&self, control: Control, kind: &str) -> Placement {
        self.choices.get(kind).and_then(|c| c.get(control.name())).copied().unwrap_or(Placement::Bar)
    }
    pub fn set(&mut self, control: Control, kind: &str, placement: Placement) {
        if placement == Placement::Bar {
            if let Some(choices) = self.choices.get_mut(kind) {
                choices.remove(control.name());
                if choices.is_empty() {
                    self.choices.remove(kind);
                }
            }
        } else {
            self.choices.entry(kind.to_owned()).or_default().insert(control.name().to_owned(), placement);
        }
    }
    pub fn reset(&mut self, kind: &str) {
        self.choices.remove(kind);
    }
    /// Drawn as an icon in a header of `level`.
    pub fn in_bar(&self, control: Control, kind: &str, level: u8) -> bool {
        control.applies(kind) && self.placement(control, kind) == Placement::Bar && level < control.drop_level()
    }
    /// Offered in the ⋯ menu of a header of `level`: chosen for the menu, or dropped from the bar.
    pub fn in_menu(&self, control: Control, kind: &str, level: u8) -> bool {
        control.applies(kind)
            && match self.placement(control, kind) {
                Placement::Menu => true,
                Placement::Bar => level >= control.drop_level(),
                Placement::Off => false,
            }
    }
    pub fn is_off(&self, control: Control, kind: &str) -> bool {
        self.placement(control, kind) == Placement::Off
    }
    /// The controls a header of `level` draws, in their order (the ⋯ menu button sits before Close).
    pub fn bar(&self, kind: &str, level: u8) -> Vec<Control> {
        Control::ALL.into_iter().filter(|c| self.in_bar(*c, kind, level)).collect()
    }
    /// The first level whose buttons leave the title `title_min` points in a header with
    /// `room` points for the title and the buttons (ViewThatFits picks the first row that
    /// fits); the last level when none does. `buttons` is the width `n` buttons take, and
    /// `drawn` says whether the pane has a button for a control that applies to its kind (the
    /// switch is drawn only for a pane with another view).
    pub fn level(&self, kind: &str, room: f32, title_min: f32, buttons: impl Fn(usize) -> f32, drawn: impl Fn(Control) -> bool) -> u8 {
        let count = |level: u8| Control::ALL.into_iter().filter(|c| drawn(*c) && self.in_bar(*c, kind, level)).count();
        LEVELS.into_iter().find(|level| title_min + buttons(count(*level) + 1) <= room).unwrap_or(*LEVELS.end())
    }
    /// The choices as the Swift client writes them: `{"choices": {kind: {control: placement}}}`.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({ "choices": self.choices_json() })
    }
    pub fn choices_json(&self) -> serde_json::Value {
        serde_json::Value::Object(self.choices.iter().map(|(kind, c)| (kind.clone(), serde_json::Value::Object(c.iter().map(|(k, p)| (k.clone(), p.name().into())).collect()))).collect())
    }
    /// Read what `json` wrote, keeping every entry that names a known control and placement.
    pub fn parse(text: &str) -> Layout {
        let mut layout = Layout::default();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return layout };
        for (kind, choices) in value["choices"].as_object().into_iter().flatten() {
            for (control, placement) in choices.as_object().into_iter().flatten() {
                if let (Some(control), Some(placement)) = (Control::named(control), placement.as_str().and_then(Placement::named)) {
                    layout.set(control, kind, placement);
                }
            }
        }
        layout
    }
}

/// The layout, read from `file` and written back on each change; kept in memory only without one.
pub struct Store {
    pub layout: Layout,
    file: Option<PathBuf>,
}

impl Store {
    pub fn load(file: Option<PathBuf>) -> Self {
        let layout = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| Layout::parse(&t)).unwrap_or_default();
        Self { layout, file }
    }
    pub fn default_file() -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/wheel-gpui/header-layout.json")
    }
    /// The one layout every window of this process reads and writes for `file`.
    pub fn shared(file: Option<PathBuf>) -> Rc<RefCell<Store>> {
        thread_local! {
            static SHARED: RefCell<std::collections::HashMap<Option<PathBuf>, Rc<RefCell<Store>>>> = RefCell::default();
        }
        SHARED.with(|s| s.borrow_mut().entry(file.clone()).or_insert_with(|| Rc::new(RefCell::new(Store::load(file)))).clone())
    }
    pub fn set(&mut self, control: Control, kind: &str, placement: Placement) {
        self.layout.set(control, kind, placement);
        self.save();
    }
    pub fn reset(&mut self, kind: &str) {
        self.layout.reset(kind);
        self.save();
    }
    fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(error) = std::fs::write(file, self.layout.json().to_string()) {
            eprintln!("wheel-gpui: the pane header layout was not saved to {}: {error}", file.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(n: usize) -> f32 {
        n as f32 * 18. + (n.saturating_sub(1)) as f32 * 2.
    }

    #[test]
    fn a_narrowing_header_drops_each_group_in_turn_and_keeps_close() {
        let layout = Layout::default();
        let names = |level| layout.bar("agent", level).iter().map(|c| c.id()).collect::<Vec<_>>();
        assert_eq!(names(0), ["switch-view", "conversations", "info", "split-right", "split-down", "pop", "background", "zoom", "close"]);
        assert_eq!(names(1), ["switch-view", "split-right", "split-down", "zoom", "close"]);
        assert_eq!(names(2), ["split-right", "split-down", "zoom", "close"]);
        assert_eq!(names(3), ["zoom", "close"]);
        assert_eq!(names(4), ["close"]);
        let all = |_| true;
        // Room for the title and all ten buttons (nine and the menu) keeps level 0.
        assert_eq!(layout.level("agent", 64. + width(10), 64., width, all), 0);
        assert_eq!(layout.level("agent", 64. + width(10) - 1., 64., width, all), 1);
        assert_eq!(layout.level("agent", 64. + width(6) - 1., 64., width, all), 2);
        assert_eq!(layout.level("agent", 64. + width(5) - 1., 64., width, all), 3);
        assert_eq!(layout.level("agent", 10., 64., width, all), 4);
        // A terminal pane has no Conversations button: eight buttons fit where an agent's ten do
        // not, and the switch takes room only on a pane that has another view.
        let no_switch = |c| c != Control::SwitchView;
        assert_eq!(layout.level("terminal", 64. + width(8), 64., width, no_switch), 0);
        assert_eq!(layout.level("terminal", 64. + width(8), 64., width, all), 1);
    }

    #[test]
    fn the_menu_offers_what_the_bar_dropped_or_the_person_moved_and_off_is_in_neither() {
        let mut layout = Layout::default();
        assert!(!layout.in_menu(Control::Pop, "terminal", 0));
        assert!(layout.in_menu(Control::Pop, "terminal", 1));
        assert!(!layout.in_menu(Control::Conversations, "terminal", 4));
        layout.set(Control::Zoom, "terminal", Placement::Menu);
        layout.set(Control::Pop, "terminal", Placement::Off);
        assert!(!layout.in_bar(Control::Zoom, "terminal", 0) && layout.in_menu(Control::Zoom, "terminal", 0));
        assert!(!layout.in_bar(Control::Pop, "terminal", 0) && !layout.in_menu(Control::Pop, "terminal", 4));
        // Another kind keeps its own layout.
        assert!(layout.in_bar(Control::Zoom, "agent", 0));
        layout.set(Control::Zoom, "terminal", Placement::Bar);
        assert_eq!(layout.choices["terminal"].len(), 1);
        layout.reset("terminal");
        assert!(layout.choices.is_empty());
    }

    #[test]
    fn the_layout_reads_back_the_swift_form_and_skips_what_it_does_not_know() {
        let mut layout = Layout::default();
        layout.set(Control::SplitDown, "web", Placement::Off);
        layout.set(Control::Conversations, "agent", Placement::Menu);
        let text = layout.json().to_string();
        assert_eq!(text, r#"{"choices":{"agent":{"conversations":"menu"},"web":{"splitDown":"off"}}}"#);
        assert_eq!(Layout::parse(&text), layout);
        let read = Layout::parse(r#"{"choices":{"web":{"splitDown":"off","screenshot":"menu","zoom":"sideways"}}}"#);
        assert_eq!(read.choices["web"].len(), 1);
        assert_eq!(Layout::parse("not json"), Layout::default());
        assert_eq!(kind_name("agent"), "agent");
        assert_eq!(kind_name("future-kind"), "unknown");
        assert!(!Control::SplitRight.applies("unknown") && Control::Close.applies("unknown"));
    }
}
