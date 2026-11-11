//! Which key runs which action. A port of apple/WheelKit/Sources/WheelKit/Keymap.swift and the
//! key reading in apple/Mac/Sources/KeyBindings.swift: a chord is spelled as wheeld stores a
//! `keybind` trigger (`ctrl+opt+shift+cmd+<key>`), the app's built-in bindings have the setting
//! applied over them entry by entry, and a GPUI keystroke is read back into that spelling.
use gpui::Keystroke;
use std::collections::HashMap;

/// Modifier bits, in the order they are written and printed: ⌃⌥⇧⌘.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub opt: bool,
    pub shift: bool,
    pub cmd: bool,
}
impl Modifiers {
    const ORDER: [(&'static str, &'static str); 4] = [("ctrl", "⌃"), ("opt", "⌥"), ("shift", "⇧"), ("cmd", "⌘")];
    /// The (name, symbol) of each modifier that is on, in `ORDER`.
    fn held(self) -> impl Iterator<Item = (&'static str, &'static str)> {
        Self::ORDER.into_iter().zip([self.ctrl, self.opt, self.shift, self.cmd]).filter(|(_, on)| *on).map(|(names, _)| names)
    }
    fn set(&mut self, i: usize) {
        match i {
            0 => self.ctrl = true,
            1 => self.opt = true,
            2 => self.shift = true,
            _ => self.cmd = true,
        }
    }
}

/// One key and its modifiers, in wheeld's spelling: the key a lowercase letter, a digit,
/// `f1`–`f12` or a name (`enter`, `arrow_left`, `bracket_left`, …).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub key: String,
    pub modifiers: Modifiers,
}

/// The named keys that type a character, with the character they type unshifted.
const PRINTING: [(&str, char); 11] = [
    ("comma", ','), ("period", '.'), ("slash", '/'), ("semicolon", ';'), ("quote", '\''), ("backquote", '`'),
    ("minus", '-'), ("equal", '='), ("backslash", '\\'), ("bracket_left", '['), ("bracket_right", ']'),
];
/// The keys that type no character, with what a menu prints for each.
const NAMED: [(&str, &str); 14] = [
    ("enter", "↩"), ("tab", "⇥"), ("space", "Space"), ("escape", "⎋"), ("backspace", "⌫"), ("delete", "⌦"),
    ("arrow_up", "↑"), ("arrow_down", "↓"), ("arrow_left", "←"), ("arrow_right", "→"),
    ("home", "↖"), ("end", "↘"), ("page_up", "⇞"), ("page_down", "⇟"),
];
/// What Shift makes a US keyboard's printing keys type, by the key's name: GPUI reports a shifted
/// punctuation or digit key as that character with Shift cleared.
const SHIFTED: [(char, &str); 21] = [
    ('!', "1"), ('@', "2"), ('#', "3"), ('$', "4"), ('%', "5"), ('^', "6"), ('&', "7"), ('*', "8"), ('(', "9"), (')', "0"),
    ('+', "equal"), ('_', "minus"), ('{', "bracket_left"), ('}', "bracket_right"), ('"', "quote"), (':', "semicolon"),
    ('|', "backslash"), ('<', "comma"), ('>', "period"), ('?', "slash"), ('~', "backquote"),
];

impl KeyChord {
    pub fn new(key: &str, modifiers: Modifiers) -> Self {
        Self { key: key.to_owned(), modifiers }
    }
    /// `shift+cmd+d` read back; None for anything not in the stored spelling.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts: Vec<&str> = text.split('+').collect();
        let key = parts.pop()?;
        if !is_key(key) {
            return None;
        }
        let mut modifiers = Modifiers::default();
        let mut next = 0;
        for part in parts {
            // Each modifier once, in order.
            let i = (next..4).find(|&i| Modifiers::ORDER[i].0 == part)?;
            modifiers.set(i);
            next = i + 1;
        }
        Some(Self::new(key, modifiers))
    }
    /// `shift+cmd+d`, as wheeld stores it.
    pub fn text(&self) -> String {
        let mut parts: Vec<&str> = self.modifiers.held().map(|(name, _)| name).collect();
        parts.push(&self.key);
        parts.join("+")
    }
    /// `⇧⌘D`, as a menu prints it.
    pub fn symbol(&self) -> String {
        let mods: String = self.modifiers.held().map(|(_, symbol)| symbol).collect();
        let key = NAMED.iter().find(|(k, _)| *k == self.key).map(|(_, s)| (*s).to_owned())
            .or_else(|| PRINTING.iter().find(|(k, _)| *k == self.key).map(|(_, c)| c.to_string()))
            .unwrap_or_else(|| self.key.to_uppercase());
        format!("{mods}{key}")
    }
    /// The chord a GPUI keystroke is, or None for a key no chord can name. GPUI names the keys
    /// that type nothing (`enter`, `left`, `pageup`, …) and gives a printing key as the character
    /// its unshifted position types, except that a shifted punctuation or digit key arrives as the
    /// shifted character with Shift cleared.
    pub fn from_keystroke(k: &Keystroke) -> Option<Self> {
        let mut modifiers = Modifiers { ctrl: k.modifiers.control, opt: k.modifiers.alt, shift: k.modifiers.shift, cmd: k.modifiers.platform };
        let key = match k.key.as_str() {
            "up" => "arrow_up".to_owned(),
            "down" => "arrow_down".to_owned(),
            "left" => "arrow_left".to_owned(),
            "right" => "arrow_right".to_owned(),
            "pageup" => "page_up".to_owned(),
            "pagedown" => "page_down".to_owned(),
            name @ ("enter" | "tab" | "space" | "escape" | "backspace" | "delete" | "home" | "end") => name.to_owned(),
            function if function.len() > 1 && function.starts_with('f') && function[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => function.to_owned(),
            other => {
                let c = other.chars().next().filter(|_| other.chars().count() == 1)?;
                match SHIFTED.iter().find(|(shifted, _)| *shifted == c) {
                    Some((_, name)) => {
                        modifiers.shift = true;
                        (*name).to_owned()
                    }
                    None => name_of(c)?,
                }
            }
        };
        Some(Self::new(&key, modifiers))
    }
}

/// The key a printing character is on: `d`, `1`, `bracket_left` for `[`.
fn name_of(c: char) -> Option<String> {
    if c.is_ascii_alphanumeric() {
        return Some(c.to_ascii_lowercase().to_string());
    }
    PRINTING.iter().find(|(_, p)| *p == c).map(|(n, _)| (*n).to_owned())
}
fn is_key(key: &str) -> bool {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return c.is_ascii_lowercase() || c.is_ascii_digit();
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=12).contains(&n) && key == format!("f{n}");
    }
    NAMED.iter().any(|(k, _)| *k == key) || PRINTING.iter().any(|(k, _)| *k == key)
}

/// Which chord runs which action: the built-in bindings with the `keybind` setting applied over
/// them, entry by entry, the way Ghostty applies its `keybind` lines. An action has at most one
/// chord and a chord runs at most one action.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    /// By action id.
    pub chords: HashMap<String, KeyChord>,
    /// What in the setting was not applied, one line each.
    pub problems: Vec<String>,
}
impl Keymap {
    /// `defaults` is every action there is, with its built-in chord or none.
    pub fn new(defaults: &[(&str, Option<&str>)], setting: &str) -> Self {
        let mut chords: HashMap<String, KeyChord> = defaults.iter().filter_map(|(action, keys)| {
            let chord = KeyChord::parse((*keys)?).unwrap_or_else(|| panic!("built-in key {} is not in wheeld's spelling", keys.unwrap()));
            Some(((*action).to_owned(), chord))
        }).collect();
        let mut problems = vec![];
        for entry in setting.split(',').filter(|e| !e.is_empty()) {
            if entry == "clear" {
                chords.clear();
                continue;
            }
            let Some((chord, action)) = entry.split_once('=').and_then(|(head, action)| Some((KeyChord::parse(head)?, action))) else {
                problems.push(format!("{entry}: not a binding this app can read"));
                continue;
            };
            if action != "unbind" && !defaults.iter().any(|(a, _)| *a == action) {
                problems.push(format!("{entry}: Wheel has no action {action}"));
                continue;
            }
            chords.retain(|_, bound| *bound != chord);
            if action != "unbind" {
                chords.insert(action.to_owned(), chord);
            }
        }
        Self { chords, problems }
    }
    pub fn action(&self, chord: &KeyChord) -> Option<&str> {
        self.chords.iter().find(|(_, c)| *c == chord).map(|(a, _)| a.as_str())
    }
    /// The chord bound to `action`, printed as a menu prints it, or an empty string.
    pub fn symbol(&self, action: &str) -> String {
        self.chords.get(action).map(KeyChord::symbol).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(text: &str) -> Keystroke {
        Keystroke::parse(text).unwrap()
    }

    #[test]
    fn chords_round_trip_through_the_stored_spelling() {
        let chord = KeyChord::parse("shift+cmd+bracket_left").unwrap();
        assert_eq!(chord.text(), "shift+cmd+bracket_left");
        assert_eq!(chord.symbol(), "⇧⌘[");
        assert_eq!(KeyChord::parse("cmd+enter").unwrap().symbol(), "⌘↩");
        assert_eq!(KeyChord::parse("ctrl+cmd+arrow_left").unwrap().symbol(), "⌃⌘←");
        assert!(KeyChord::parse("cmd+shift+d").is_none(), "modifiers out of order");
        assert!(KeyChord::parse("cmd+D").is_none());
        assert!(KeyChord::parse("f13").is_none());
        assert!(KeyChord::parse("cmd+").is_none());
    }

    #[test]
    fn gpui_keystrokes_read_as_chords() {
        assert_eq!(KeyChord::from_keystroke(&stroke("cmd-d")).unwrap().text(), "cmd+d");
        assert_eq!(KeyChord::from_keystroke(&stroke("cmd-shift-d")).unwrap().text(), "shift+cmd+d");
        assert_eq!(KeyChord::from_keystroke(&stroke("cmd-[")).unwrap().text(), "cmd+bracket_left");
        // A shifted bracket arrives as the brace with Shift cleared.
        assert_eq!(KeyChord::from_keystroke(&stroke("cmd-{")).unwrap().text(), "shift+cmd+bracket_left");
        assert_eq!(KeyChord::from_keystroke(&stroke("ctrl-cmd-=")).unwrap().text(), "ctrl+cmd+equal");
        assert_eq!(KeyChord::from_keystroke(&stroke("alt-cmd-left")).unwrap().text(), "opt+cmd+arrow_left");
        assert_eq!(KeyChord::from_keystroke(&stroke("cmd-shift-enter")).unwrap().text(), "shift+cmd+enter");
        assert_eq!(KeyChord::from_keystroke(&stroke("cmd-/")).unwrap().text(), "cmd+slash");
        assert_eq!(KeyChord::from_keystroke(&stroke("cmd-,")).unwrap().text(), "cmd+comma");
        assert_eq!(KeyChord::from_keystroke(&stroke("cmd-1")).unwrap().text(), "cmd+1");
        assert_eq!(KeyChord::from_keystroke(&stroke("f5")).unwrap().text(), "f5");
    }

    #[test]
    fn the_setting_applies_over_the_defaults_entry_by_entry() {
        let defaults = [("split-right", Some("cmd+d")), ("split-down", Some("shift+cmd+d")), ("usage", None)];
        let map = Keymap::new(&defaults, "");
        assert_eq!(map.action(&KeyChord::parse("cmd+d").unwrap()), Some("split-right"));
        assert_eq!(map.symbol("split-down"), "⇧⌘D");
        assert_eq!(map.symbol("usage"), "");
        // A chord moved to another action leaves its old one unbound.
        let map = Keymap::new(&defaults, "cmd+d=split-down,cmd+u=usage,cmd+x=nothing,junk,shift+cmd+d=unbind");
        assert_eq!(map.action(&KeyChord::parse("cmd+d").unwrap()), Some("split-down"));
        assert_eq!(map.action(&KeyChord::parse("shift+cmd+d").unwrap()), None);
        assert_eq!(map.symbol("usage"), "⌘U");
        assert_eq!(map.problems, ["cmd+x=nothing: Wheel has no action nothing", "junk: not a binding this app can read"]);
        let map = Keymap::new(&defaults, "clear,cmd+e=split-right");
        assert_eq!(map.chords.len(), 1);
        assert_eq!(map.symbol("split-right"), "⌘E");
    }
}
