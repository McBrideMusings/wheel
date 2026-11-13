//! The window's title row as the Swift client draws it (`TitleRow.swift`, ADR-0037): the sidebar
//! button beside the traffic lights, the session menu, the mode menu, the left `[[title.button]]`
//! entries, then the window's title while the sidebar is open or the space tabs while it is
//! hidden, and at the right end the right entries, Processes, ⌘K, the pin and New Space. Every
//! button is an icon in a fixed slot, so a state change moves no neighbour; the tabs take only
//! the width left between the two groups. wheeld merges the machine's and the project's entries
//! by id before they get here; an entry marked `hidden` removes the built-in of that id.
use crate::layout::Rect;
use wheel_proto::{TitleButtonInfo, TitleSide};

/// The row's height: `TitleRow.minHeight`, the title bar the window would have.
pub const HEIGHT: f32 = 38.0;
/// The width the traffic lights take at the left end.
pub const LIGHTS_W: f32 = 72.0;
/// A button's slot (`TitleButtonStyle`: minWidth 28, minHeight 26).
pub const BUTTON_W: f32 = 28.0;
pub const BUTTON_H: f32 = 26.0;
pub const BUTTON_Y: f32 = (HEIGHT - BUTTON_H) / 2.0;
/// The left group's spacing, the right group's, the row's trailing padding, and the spacer
/// between the tabs and the right group.
pub const GAP: f32 = 6.0;
pub const RIGHT_GAP: f32 = 2.0;
pub const TRAILING: f32 = 10.0;
pub const SPACER: f32 = 8.0;
/// The window title's leading padding while the sidebar is open.
pub const TITLE_LEAD: f32 = 4.0;
/// `SpaceTabs`: every tab the same width between these, inside a track with a 2 pt inset.
pub const TAB_NARROWEST: f32 = 72.0;
pub const TAB_WIDEST: f32 = 220.0;
pub const TAB_INSET: f32 = 2.0;
pub const TAB_H: f32 = 28.0;
/// The grid's and the sidebar's inset from the window's left edge.
pub const MARGIN: f32 = 10.0;

/// The words a new session's name is made from (WheelKit `SessionNames`).
const ADJECTIVES: [&str; 25] = ["bold", "brisk", "calm", "clear", "cool", "crisp", "dusky", "eager", "faint", "gentle", "golden", "hazy", "hushed", "keen", "lucid", "mellow", "misty", "quiet", "rapid", "silver", "steady", "swift", "tidal", "warm", "wild"];
const NOUNS: [&str; 25] = ["basin", "birch", "brook", "canyon", "cedar", "cove", "delta", "dune", "fjord", "glade", "harbor", "heath", "island", "lagoon", "meadow", "mesa", "ridge", "river", "shoal", "summit", "thicket", "tundra", "valley", "willow", "grove"];

/// A session name not in `taken`, `<adjective>-<noun>` picked at random as `SessionNames.next`
/// does; a number is added only when every pair is used.
pub fn session_name(taken: &[&str]) -> String {
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut pairs: Vec<String> = ADJECTIVES.iter().flat_map(|a| NOUNS.iter().map(move |n| format!("{a}-{n}"))).collect();
    for i in (1..pairs.len()).rev() {
        pairs.swap(i, (next() % (i as u64 + 1)) as usize);
    }
    if let Some(free) = pairs.iter().find(|p| !taken.contains(&p.as_str())) {
        return free.clone();
    }
    (2..).map(|n| format!("{}-{n}", pairs[0])).find(|p| !taken.contains(&p.as_str())).unwrap()
}

/// The modes a window's body draws (`WindowMode`), with the title and icon the menu shows.
pub const MODES: [(&str, &str, &str); 2] = [("spaces", "Spaces", "square.grid.2x2"), ("desk", "Desk", "rectangle.3.group")];

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Sidebar,
    SessionMenu,
    Mode,
    /// A `[[title.button]]` entry, by id.
    Plugin(String),
    /// A space tab (the sidebar hidden).
    Space(String),
    /// The window's title (the sidebar open).
    Title,
    Processes,
    Palette,
    Pin,
    NewSpace,
}

/// One thing drawn in the row: a button in its slot, a tab, or the title.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub hit: Hit,
    pub rect: Rect,
    /// A tab's or the title's text, a button's tooltip.
    pub label: String,
    pub active: bool,
    /// The SF Symbol a button draws; empty for ⌘K, a tab and the title.
    pub icon: String,
    pub badge: Option<String>,
    pub tint: Option<String>,
    pub enabled: bool,
    /// Whether a press does anything: an entry with no command is an indicator.
    pub pressable: bool,
}

/// The row as laid out for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub chips: Vec<Chip>,
    /// The tabs' rounded track while the sidebar is hidden and the window has spaces.
    pub track: Option<Rect>,
    pub tab_width: f32,
    /// The tabs run past the track and the strip is shifted to keep the active one in view.
    pub scrolls: bool,
    /// Every tab's rectangle in session order at its shifted place, including tabs the track
    /// scrolls out of view (`chips` holds only the ones it shows): a space drop counts them all.
    pub tabs: Vec<Rect>,
}

/// The id a chip's frame is recorded under (`GET /geometry` `title_buttons`), as the Swift app names them.
pub fn frame_id(hit: &Hit) -> Option<String> {
    Some(match hit {
        Hit::Sidebar => "sidebar".into(),
        Hit::SessionMenu => "session".into(),
        Hit::Mode => "mode".into(),
        Hit::Plugin(id) => format!("plugin-{id}"),
        Hit::Processes => "processes".into(),
        Hit::Palette => "palette".into(),
        Hit::Pin => "pin".into(),
        Hit::NewSpace => "new-space".into(),
        Hit::Space(_) | Hit::Title => return None,
    })
}

/// The width of each of `count` tabs in a strip `available` points wide (`SpaceTabs.tabWidth`).
pub fn tab_width(count: usize, available: f32) -> f32 {
    if count == 0 {
        return TAB_WIDEST;
    }
    ((available - 2. * TAB_INSET) / count as f32).floor().clamp(TAB_NARROWEST, TAB_WIDEST)
}

fn slot(x: f32) -> Rect {
    Rect { x, y: BUTTON_Y, w: BUTTON_W, h: BUTTON_H }
}

fn button(hit: Hit, x: f32, icon: &str, label: &str) -> Chip {
    Chip { hit, rect: slot(x), label: label.to_owned(), active: false, icon: icon.to_owned(), badge: None, tint: None, enabled: true, pressable: true }
}

fn plugin(info: &TitleButtonInfo, x: f32) -> Chip {
    Chip {
        hit: Hit::Plugin(info.id.clone()),
        rect: slot(x),
        label: if info.tip.is_empty() { info.id.clone() } else { info.tip.clone() },
        active: false,
        icon: info.icon.clone(),
        badge: info.badge.clone(),
        tint: info.tint.clone(),
        enabled: info.enabled,
        pressable: info.has_command,
    }
}

/// The row for a window `width` points wide. `plugins` are the space's merged entries;
/// `spaces` are (id, name) of the session shown; `space` is the one shown and `space_name` its
/// name; `mode` is `spaces` or `desk`.
#[allow(clippy::too_many_arguments)]
pub fn row(width: f32, sidebar_open: bool, plugins: &[TitleButtonInfo], spaces: &[(String, String)], space: Option<&str>, space_name: &str, pinned: bool, mode: &str) -> Row {
    let hidden = |id: &str| plugins.iter().any(|p| p.hidden && p.id == id);
    let shown: Vec<&TitleButtonInfo> = plugins.iter().filter(|p| !p.hidden).collect();
    let has_space = space.is_some();
    let mut chips = vec![];
    // The left group: each item takes a slot and a gap.
    let mut x = LIGHTS_W + GAP;
    let mut left = |mut chip: Chip, chips: &mut Vec<Chip>| {
        chip.rect.x = x;
        chips.push(chip);
        x += BUTTON_W + GAP;
    };
    if !hidden("sidebar") {
        left(button(Hit::Sidebar, 0., "sidebar.left", if sidebar_open { "Hide the sidebar; spaces become tabs" } else { "Show the sidebar" }), &mut chips);
    }
    left(button(Hit::SessionMenu, 0., "rectangle.stack", "Session"), &mut chips);
    if !hidden("mode") {
        let icon = MODES.iter().find(|m| m.0 == mode).map(|m| m.2).unwrap_or("square.grid.2x2");
        left(button(Hit::Mode, 0., icon, "Mode: what this window shows"), &mut chips);
    }
    if has_space {
        for info in shown.iter().filter(|p| p.side == TitleSide::Left) {
            left(plugin(info, 0.), &mut chips);
        }
    }
    let left_end = x - GAP;
    // The right group, laid from the right edge in: New Space, the pin, ⌘K, Processes, then the right entries.
    let mut right_chips: Vec<Chip> = vec![];
    let mut rx = width - TRAILING;
    let mut right = |chip: Chip, right_chips: &mut Vec<Chip>| {
        rx -= BUTTON_W;
        let mut chip = chip;
        chip.rect.x = rx;
        right_chips.push(chip);
        rx -= RIGHT_GAP;
    };
    if !hidden("new-space") {
        right(button(Hit::NewSpace, 0., "plus", "New space from a folder"), &mut right_chips);
    }
    if !hidden("pin") {
        let mut chip = button(Hit::Pin, 0., if pinned { "pin.fill" } else { "pin" }, if pinned { "Unpin" } else { "Pin on top" });
        chip.active = pinned;
        right(chip, &mut right_chips);
    }
    if !hidden("palette") {
        right(button(Hit::Palette, 0., "", "Command palette: spaces, panes, artifacts and actions"), &mut right_chips);
    }
    if has_space {
        if !hidden("processes") {
            right(button(Hit::Processes, 0., "list.bullet.rectangle", "Processes started from this space's panes"), &mut right_chips);
        }
        for info in shown.iter().filter(|p| p.side == TitleSide::Right).rev() {
            right(plugin(info, 0.), &mut right_chips);
        }
    }
    let right_start = rx + RIGHT_GAP;
    right_chips.reverse();
    // Between the groups: the title, or the tabs.
    let mut track = None;
    let mut tab_w = TAB_WIDEST;
    let mut scrolls = false;
    let mut tabs = vec![];
    if sidebar_open {
        let x0 = left_end + GAP + TITLE_LEAD;
        chips.push(Chip { hit: Hit::Title, rect: Rect { x: x0, y: BUTTON_Y, w: (right_start - SPACER - x0).max(0.), h: BUTTON_H }, label: space_name.to_owned(), active: false, icon: String::new(), badge: None, tint: None, enabled: true, pressable: false });
    } else if !spaces.is_empty() {
        let x0 = left_end + GAP;
        let available = (right_start - SPACER - x0).max(0.);
        tab_w = tab_width(spaces.len(), available);
        let full = spaces.len() as f32 * tab_w + 2. * TAB_INSET;
        let track_w = full.min(available);
        scrolls = full > available + 0.5;
        let active_at = spaces.iter().position(|(id, _)| Some(id.as_str()) == space);
        // The strip shifts only as far as the active tab needs to be inside the track.
        let mut offset = 0f32;
        if scrolls && let Some(a) = active_at {
            let end = (a + 1) as f32 * tab_w + 2. * TAB_INSET;
            offset = (end - track_w).max(0.);
        }
        let ty = (HEIGHT - TAB_H) / 2.;
        for (i, (id, name)) in spaces.iter().enumerate() {
            let tx = x0 + TAB_INSET + i as f32 * tab_w - offset;
            tabs.push(Rect { x: tx, y: ty + TAB_INSET, w: tab_w, h: TAB_H - 2. * TAB_INSET });
            if tx + 0.5 < x0 || tx + tab_w > x0 + track_w + 0.5 {
                continue;
            }
            chips.push(Chip { hit: Hit::Space(id.clone()), rect: Rect { x: tx, y: ty + TAB_INSET, w: tab_w, h: TAB_H - 2. * TAB_INSET }, label: name.clone(), active: Some(id.as_str()) == space, icon: String::new(), badge: None, tint: None, enabled: true, pressable: true });
        }
        track = Some(Rect { x: x0, y: ty, w: track_w, h: TAB_H });
    }
    chips.extend(right_chips);
    Row { chips, track, tab_width: tab_w, scrolls, tabs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::SettingOrigin;

    fn named(prefix: &str, n: usize) -> Vec<(String, String)> {
        (1..=n).map(|i| (format!("{prefix}{i}"), format!("{prefix} {i}"))).collect()
    }
    fn entry(id: &str, side: TitleSide, has_command: bool) -> TitleButtonInfo {
        TitleButtonInfo { id: id.into(), side, hidden: false, icon: "star".into(), tip: format!("{id} tip"), tint: None, badge: None, enabled: true, has_command, origin: SettingOrigin::Machine }
    }
    fn hidden(id: &str) -> TitleButtonInfo {
        TitleButtonInfo { id: id.into(), side: TitleSide::Left, hidden: true, icon: String::new(), tip: String::new(), tint: None, badge: None, enabled: true, has_command: false, origin: SettingOrigin::Machine }
    }
    fn ids(row: &Row) -> Vec<String> {
        row.chips.iter().map(|c| match &c.hit { Hit::Space(id) => format!("tab:{id}"), Hit::Title => "title".into(), hit => frame_id(hit).unwrap() }).collect()
    }

    #[test]
    fn a_session_name_is_an_unused_pair_and_numbered_only_when_every_pair_is_taken() {
        let name = session_name(&[]);
        let (a, n) = name.split_once('-').unwrap();
        assert!(ADJECTIVES.contains(&a) && NOUNS.contains(&n), "{name}");
        let all: Vec<String> = ADJECTIVES.iter().flat_map(|a| NOUNS.iter().map(move |n| format!("{a}-{n}"))).collect();
        let taken: Vec<&str> = all.iter().map(String::as_str).collect();
        assert!(session_name(&taken).ends_with("-2"));
    }

    #[test]
    fn the_built_ins_sit_in_the_swift_order_at_fixed_slots() {
        let r = row(1200., true, &[], &named("s", 2), Some("s1"), "s 1", false, "spaces");
        assert_eq!(ids(&r), ["sidebar", "session", "mode", "title", "processes", "palette", "pin", "new-space"]);
        assert_eq!(r.chips[0].rect, Rect { x: 78., y: 6., w: 28., h: 26. });
        assert_eq!(r.chips[1].rect.x, 112.);
        assert_eq!(r.chips[2].rect.x, 146.);
        let new_space = r.chips.last().unwrap();
        assert_eq!(new_space.rect.x + new_space.rect.w, 1190.);
        assert_eq!(r.chips[r.chips.len() - 2].rect.x, 1200. - 10. - 28. - 2. - 28.);
        assert_eq!(r.chips[4].rect.x, 1200. - 10. - 4. * 28. - 3. * 2.);
        assert!(r.track.is_none());
    }

    #[test]
    fn entries_take_their_side_in_order_and_hidden_ones_remove_built_ins() {
        let plugins = vec![entry("vpn", TitleSide::Right, true), hidden("pin"), entry("deploy", TitleSide::Left, false), entry("logs", TitleSide::Right, true)];
        let r = row(1200., true, &plugins, &named("s", 1), Some("s1"), "s 1", false, "spaces");
        assert_eq!(ids(&r), ["sidebar", "session", "mode", "plugin-deploy", "title", "plugin-vpn", "plugin-logs", "processes", "palette", "new-space"]);
        let deploy = &r.chips[3];
        assert_eq!(deploy.rect.x, 180.);
        assert!(!deploy.pressable && deploy.label == "deploy tip");
        let vpn = r.chips.iter().find(|c| c.hit == Hit::Plugin("vpn".into())).unwrap();
        let logs = r.chips.iter().find(|c| c.hit == Hit::Plugin("logs".into())).unwrap();
        assert_eq!(logs.rect.x - vpn.rect.x, 30.);
        // A hidden sidebar button shifts the session menu into its place; no space hides the space's buttons.
        let r = row(1200., true, &[hidden("sidebar")], &[], None, "", false, "spaces");
        assert_eq!(ids(&r), ["session", "mode", "title", "palette", "pin", "new-space"]);
        assert_eq!(r.chips[0].rect.x, 78.);
    }

    #[test]
    fn a_state_change_moves_no_neighbour() {
        let plain = row(1200., true, &[entry("vpn", TitleSide::Left, true)], &named("s", 1), Some("s1"), "s 1", false, "spaces");
        let mut badged = entry("vpn", TitleSide::Left, true);
        badged.badge = Some("12".into());
        badged.tint = Some("green".into());
        badged.icon = "lock.fill".into();
        badged.enabled = false;
        let with = row(1200., true, &[badged], &named("s", 1), Some("s1"), "s 1", true, "desk");
        let rects = |r: &Row| r.chips.iter().map(|c| (frame_id(&c.hit), c.rect)).collect::<Vec<_>>();
        assert_eq!(rects(&plain), rects(&with));
        assert_eq!(with.chips[3].badge.as_deref(), Some("12"));
        assert!(!with.chips[3].enabled);
        assert_eq!(with.chips[2].icon, "rectangle.3.group");
        assert_eq!(with.chips.iter().find(|c| c.hit == Hit::Pin).unwrap().icon, "pin.fill");
    }

    #[test]
    fn tabs_share_the_room_between_the_groups_and_keep_the_active_one_in_view() {
        let one = row(1200., false, &[], &named("s", 1), Some("s1"), "s 1", false, "spaces");
        assert_eq!(one.tab_width, 220.);
        let track = one.track.unwrap();
        assert_eq!(track.x, 180.);
        assert_eq!(track.w, 224.);
        assert_eq!(ids(&one)[3], "tab:s1");
        let many = row(900., false, &[], &named("s", 30), Some("s30"), "s 30", false, "spaces");
        assert_eq!(many.tab_width, 72.);
        assert!(many.scrolls);
        let tabs: Vec<&Chip> = many.chips.iter().filter(|c| matches!(c.hit, Hit::Space(_))).collect();
        assert!(tabs.iter().any(|c| c.active));
        let track = many.track.unwrap();
        assert!(tabs.iter().all(|c| c.rect.x >= track.x && c.rect.x + c.rect.w <= track.x + track.w + 0.5));
        assert_eq!(tabs.last().unwrap().hit, Hit::Space("s30".into()));
        // Every tab keeps a rectangle, the ones scrolled off to the left too.
        assert_eq!(many.tabs.len(), 30);
        assert!(many.tabs[0].x < track.x);
        assert_eq!(many.tabs[29], tabs.last().unwrap().rect);
        assert!(one.tabs.len() == 1 && row(1200., true, &[], &named("s", 3), Some("s1"), "s 1", false, "spaces").tabs.is_empty());
        let first = row(900., false, &[], &named("s", 30), Some("s1"), "s 1", false, "spaces");
        assert_eq!(first.chips.iter().find(|c| matches!(c.hit, Hit::Space(_))).unwrap().hit, Hit::Space("s1".into()));
        // The right group stays put whatever the tabs do.
        assert_eq!(one.chips.last().unwrap().rect.x, 1200. - 38.);
        assert_eq!(many.chips.last().unwrap().rect.x, 900. - 38.);
        assert_eq!(tab_width(3, 400.), 132.);
    }
}
