//! The spaces sidebar (SpacesSidebar.swift, SpaceMenu.swift, SpaceDrag.swift and WheelKit's
//! SpaceDropping.swift), as pure geometry and rules: where each row is, where a dragged space or
//! pane would land, what a space's menu offers. The view draws it and the hidden check reads it.
use crate::layout::Rect;

pub const WIDTH: f32 = 220.0;
pub const ROW_H: f32 = 26.0;
pub const ROW_GAP: f32 = 2.0;
pub const PAD: f32 = 6.0;
/// The close button's square at the right of a row.
pub const CLOSE_W: f32 = 18.0;
/// The gap between the sidebar and the grid.
pub const GAP: f32 = 8.0;
/// How far a press moves before it is a drag, as SpacePress counts it.
pub const DRAG_DISTANCE: f32 = 6.0;
/// The hover preview opens after this long over a row; it closes this long after leaving.
pub const PREVIEW_OPEN_MS: u64 = 250;
pub const PREVIEW_CLOSE_MS: u64 = 100;
pub const PREVIEW_W: f32 = 360.0;
pub const PREVIEW_GRID_H: f32 = 210.0;

/// One space's row as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub rect: Rect,
    pub close: Rect,
}

/// The rows of `spaces` (ids in session order) in a sidebar at `sidebar`.
pub fn rows(spaces: &[String], sidebar: Rect) -> Vec<Row> {
    spaces.iter().enumerate().map(|(i, id)| {
        let rect = Rect { x: sidebar.x + PAD, y: sidebar.y + PAD + i as f32 * (ROW_H + ROW_GAP), w: sidebar.w - 2.0 * PAD, h: ROW_H };
        let close = Rect { x: rect.x + rect.w - CLOSE_W - 4.0, y: rect.y + (ROW_H - CLOSE_W) / 2.0, w: CLOSE_W, h: CLOSE_W };
        Row { id: id.clone(), rect, close }
    }).collect()
}

pub fn contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// The row under a point, if any.
pub fn row_at(rows: &[Row], x: f32, y: f32) -> Option<&Row> {
    rows.iter().find(|r| contains(r.rect, x, y))
}

/// Where a space released at `y` goes: above the first row whose middle is below the pointer,
/// else after the last (SpaceDropping.target's sidebar case).
pub fn drop_index(rows: &[Row], y: f32) -> usize {
    rows.iter().position(|r| y < r.rect.y + r.rect.h / 2.0).unwrap_or(rows.len())
}

/// The insertion bar's place for a drop at `index` while `dragged` is held in `held`, or none
/// where the drop changes nothing: just before or just after the dragged item itself
/// (SpaceDropping.barIndex).
pub fn bar_index(index: usize, held: &[String], dragged: &str) -> Option<usize> {
    match held.iter().position(|id| id == dragged) {
        Some(at) if index == at || index == at + 1 => None,
        _ => Some(index),
    }
}

/// The `index` a `session_move` takes for a drop at `index` counted in the list as it stands:
/// inserting past the space's own slot shifts the others up by one once it is taken out
/// (AppModel.place).
pub fn move_index(index: usize, held: &[String], dragged: &str) -> usize {
    match held.iter().position(|id| id == dragged) {
        Some(was) if was < index => index - 1,
        _ => index,
    }
}

/// Where the bar is drawn for a drop at `bar` among `rows`: over the top edge of that row, or
/// under the last row's bottom edge.
pub fn bar_rect(rows: &[Row], bar: usize) -> Option<Rect> {
    let y = if bar < rows.len() { rows[bar].rect.y - 2.0 } else { rows.last()?.rect.y + rows.last()?.rect.h };
    let first = rows.first()?;
    Some(Rect { x: first.rect.x, y: y - 1.0, w: first.rect.w, h: 3.0 })
}

/// A space held by its row (SpaceDrag): nothing moves until it is released.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceDrag {
    pub space: String,
    /// Whether a held mouse button drives it; the debug route's drags have none.
    pub by_pointer: bool,
    pub target: Option<SpaceDropTarget>,
    /// Where the pointer is in the window the drag started in.
    pub at: (f32, f32),
    /// Where the pointer is on screen (top left origin), once the windows are measured.
    pub screen: Option<(f32, f32)>,
    /// The window the pointer is in and where in it.
    pub chip: Option<(String, (f32, f32))>,
}

/// Where releasing a dragged space would put it (SpaceDropping.SpaceDropTarget).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceDropTarget {
    /// Between `window`'s sidebar rows: `index` counts the rows above the bar, in the list as it
    /// was before the drag.
    Sidebar { window: String, index: usize },
    /// Between `window`'s tabs while its sidebar is hidden: `index` counts the tabs left of the
    /// bar.
    Tabs { window: String, index: usize },
    /// A window's body: the space goes to the end of the window's session.
    Window(String),
    /// A session's card in the menu bar popover: `index` counts the card's rows above the bar.
    Card { session: String, index: usize },
    /// No Wheel window under the pointer: the space opens in a little window.
    Desktop,
}

/// A main window as the pointer meets it, in screen points with the origin at the top left of
/// the main screen (DropWindow).
#[derive(Clone, Debug, PartialEq)]
pub struct DropWindow {
    pub id: String,
    pub frame: Rect,
    /// The sidebar's rectangle while the window shows it.
    pub sidebar: Option<Rect>,
    /// The sidebar's rows, top to bottom.
    pub rows: Vec<Rect>,
    /// The tab strip's rectangle while the window shows tabs instead of a sidebar.
    pub tab_strip: Option<Rect>,
    /// The tabs, left to right, at their place in the strip, scrolled out of it or not.
    pub tabs: Vec<Rect>,
}

/// A card in the popover: one session's spaces (DropCard).
#[derive(Clone, Debug, PartialEq)]
pub struct DropCard {
    pub session: String,
    pub frame: Rect,
    pub rows: Vec<Rect>,
}

/// The menu bar popover while it is open (DropPopover).
#[derive(Clone, Debug, PartialEq)]
pub struct DropPopover {
    pub frame: Rect,
    pub cards: Vec<DropCard>,
}

/// The bar goes above the first rectangle whose middle is below `y`, else after the last.
fn index_above(rows: &[Rect], y: f32) -> usize {
    rows.iter().position(|r| y < r.y + r.h / 2.0).unwrap_or(rows.len())
}

/// The bar goes left of the first rectangle whose middle is right of `x`, else after the last.
pub fn index_left(tabs: &[Rect], x: f32) -> usize {
    tabs.iter().position(|r| x < r.x + r.w / 2.0).unwrap_or(tabs.len())
}

/// What releasing at (`x`, `y`) would do (SpaceDropping.target). `windows` runs front to back
/// and the first under the pointer decides; the popover, when open, is in front of them all.
pub fn space_target(x: f32, y: f32, windows: &[DropWindow], popover: Option<&DropPopover>) -> SpaceDropTarget {
    if let Some(popover) = popover.filter(|p| contains(p.frame, x, y)) {
        // Between the cards the nearer one is the intent.
        let card = popover.cards.iter().find(|c| contains(c.frame, x, y))
            .or_else(|| popover.cards.iter().min_by(|a, b| (a.frame.y + a.frame.h / 2.0 - y).abs().total_cmp(&(b.frame.y + b.frame.h / 2.0 - y).abs())));
        if let Some(card) = card {
            return SpaceDropTarget::Card { session: card.session.clone(), index: index_above(&card.rows, y) };
        }
    }
    let Some(window) = windows.iter().find(|w| contains(w.frame, x, y)) else { return SpaceDropTarget::Desktop };
    match window.sidebar {
        Some(bar) if contains(bar, x, y) => SpaceDropTarget::Sidebar { window: window.id.clone(), index: index_above(&window.rows, y) },
        _ => match window.tab_strip {
            Some(strip) if contains(strip, x, y) => SpaceDropTarget::Tabs { window: window.id.clone(), index: index_left(&window.tabs, x) },
            _ => SpaceDropTarget::Window(window.id.clone()),
        },
    }
}

/// The words under a space drag (SpaceDropping.hint).
pub fn space_hint(target: &SpaceDropTarget, space: &str, session_name: impl Fn(&str) -> String, window_title: impl Fn(&str) -> String) -> String {
    match target {
        SpaceDropTarget::Sidebar { window, .. } | SpaceDropTarget::Tabs { window, .. } => format!("Place “{space}” here in “{}”", window_title(window)),
        SpaceDropTarget::Window(window) => format!("Move “{space}” to “{}”", window_title(window)),
        SpaceDropTarget::Card { session, .. } => format!("Move “{space}” to the “{}” session", session_name(session)),
        SpaceDropTarget::Desktop => format!("Release to open “{space}” in a little window"),
    }
}

/// What a drop at `index` among `held` (or at the end, for none) sends: none when it changes
/// nothing, beside the space's own slot or on a list already ending with it; else the
/// `session_move` index (none for the end).
pub fn drop_move(index: Option<usize>, held: &[String], space: &str) -> Option<Option<usize>> {
    match index {
        Some(i) => bar_index(i, held, space).map(|_| Some(move_index(i, held, space))),
        None => (held.last().map(String::as_str) != Some(space)).then_some(None),
    }
}

/// Where a little window opened by a drop on the desktop goes: its content centred on the drop,
/// which is the preview card's centre.
pub fn little_origin(x: f32, y: f32, size: (f32, f32)) -> (f32, f32) {
    (x - size.0 / 2.0, y - size.1 / 2.0)
}

/// The preview card that follows a space dragged over the desktop (SpaceCardView).
pub const CARD_W: f32 = 230.0;
pub const CARD_H: f32 = 150.0;
/// Where the chip window sits from the pointer: right of it and below it (DragChipWindow).
pub const CHIP_OFFSET: f32 = 12.0;

/// The preview card's frame, centred on the pointer, in screen points with a top left origin.
pub fn card_frame(x: f32, y: f32) -> Rect {
    Rect { x: x - CARD_W / 2.0, y: y - CARD_H / 2.0, w: CARD_W, h: CARD_H }
}

/// The chip window's top left: right of and below the pointer, kept inside `screen` (the
/// pointer's screen's visible frame, top left origin) as DragChipWindow.follow clamps it.
pub fn chip_origin(x: f32, y: f32, size: (f32, f32), screen: Rect) -> (f32, f32) {
    let ox = (x + CHIP_OFFSET).min(screen.x + screen.w - size.0).max(screen.x);
    let oy = (y + CHIP_OFFSET).min(screen.y + screen.h - size.1).max(screen.y);
    (ox, oy)
}

/// A pane held by its header (PaneMove.swift's PaneDrag), as far as the sidebar is its target:
/// a drop on a row moves the pane to the end of that space, on the sidebar's empty room makes a
/// new space for it. Its drop zones in its own space are `drop` (grid_drop.rs).
#[derive(Clone, Debug, PartialEq)]
pub struct PaneDrag {
    pub pane: String,
    pub label: String,
    pub away: Option<PaneAway>,
    /// Where it docks in its own space; never set together with `away`.
    pub drop: Option<crate::grid_drop::GridDrop>,
    /// Where the pointer is, and where in the header it took hold.
    pub at: (f32, f32),
    pub grab: (f32, f32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneAway {
    /// The end of `space`.
    End(String),
    /// A new space after the pane's own.
    NewSpace,
}

/// What releasing a pane at (`x`, `y`) would do; none outside the sidebar.
pub fn pane_away(sidebar: Rect, rows: &[Row], x: f32, y: f32) -> Option<PaneAway> {
    if !contains(sidebar, x, y) {
        return None;
    }
    Some(match row_at(rows, x, y) {
        Some(row) => PaneAway::End(row.id.clone()),
        None => PaneAway::NewSpace,
    })
}

/// The words under a pane drag (PaneAway.hint).
pub fn pane_hint(away: &PaneAway, pane: &str, space_name: impl Fn(&str) -> Option<String>) -> String {
    match away {
        PaneAway::End(space) => format!("Move {pane} to the end of “{}”", space_name(space).unwrap_or_else(|| space.clone())),
        PaneAway::NewSpace => format!("Move {pane} to a new space"),
    }
}

/// One entry of a space's context menu (SpaceMenuItem).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub id: String,
    pub title: String,
    pub enabled: bool,
    /// The block of the menu it is drawn in; a divider separates two blocks.
    pub section: u8,
    pub destructive: bool,
}

/// What right-clicking `space` offers (AppModel.spaceMenu): one flat item per other session,
/// then Move to New Session, Move Left and Right, Rename, Copy Path, Reveal, New Pane, Close
/// Other Spaces and Close Space, with Open in Little Window (Go to Little Window while `little`,
/// one is open) after the moves. `sessions` is (id, name); `held` is the home session's spaces in order.
pub fn menu(space: &str, home: Option<&str>, sessions: &[(String, String)], held: &[String], little: bool) -> Vec<MenuItem> {
    let item = |id: &str, title: String, enabled: bool, section: u8, destructive: bool| MenuItem { id: id.to_owned(), title, enabled, section, destructive };
    let index = held.iter().position(|id| id == space);
    let mut items: Vec<MenuItem> = sessions.iter().filter(|(id, _)| Some(id.as_str()) != home)
        .map(|(id, name)| item(&format!("session:{id}"), format!("Move to {name}"), true, 0, false)).collect();
    items.push(item("new-session", "Move to New Session".into(), true, 0, false));
    items.push(item("little", if little { "Go to Little Window" } else { "Open in Little Window" }.into(), true, 0, false));
    items.push(item("left", "Move Left".into(), index.is_some_and(|i| i > 0), 1, false));
    items.push(item("right", "Move Right".into(), index.is_some_and(|i| i + 1 < held.len()), 1, false));
    items.push(item("rename", "Rename Space…".into(), true, 2, false));
    items.push(item("copy-path", "Copy Path".into(), true, 2, false));
    items.push(item("reveal", "Reveal in Finder".into(), true, 2, false));
    items.push(item("new-pane", "New Pane".into(), true, 3, false));
    items.push(item("close-others", "Close Other Spaces in Window…".into(), held.len() > 1, 4, true));
    items.push(item("close", "Close Space".into(), true, 4, true));
    items
}

/// The menu's width and the height of each of its rows.
pub const MENU_W: f32 = 240.0;
pub const MENU_ROW_H: f32 = 24.0;

/// An inline rename (InlineName): the draft and the refusal under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    pub space: String,
    pub draft: String,
    pub error: Option<String>,
    /// The `space_rename` sent and not yet answered.
    pub req: Option<u64>,
}

/// Why a name is refused before it is sent (AppModel.renameSpace), or the trimmed name to send.
pub fn rename_check(text: &str, connected: bool) -> Result<String, String> {
    let name = text.trim();
    if name.is_empty() {
        return Err("A space needs a name.".into());
    }
    if !connected {
        return Err("Not connected to wheeld.".into());
    }
    Ok(name.to_owned())
}

/// Which rows of a pane stack a space draws: one per pane up to four (PaneStack.swift).
pub const STACK_LIMIT: usize = 4;
pub fn stack_layers(panes: usize) -> usize {
    panes.clamp(1, STACK_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("s{i}")).collect()
    }
    fn bar() -> Rect {
        Rect { x: 10., y: 32., w: WIDTH, h: 600. }
    }

    #[test]
    fn rows_stack_under_each_other_inside_the_sidebar() {
        let rows = rows(&ids(5), bar());
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].rect, Rect { x: 16., y: 38., w: WIDTH - 12., h: ROW_H });
        assert_eq!(rows[1].rect.y, 38. + ROW_H + ROW_GAP);
        assert!(rows[4].close.x + rows[4].close.w <= rows[4].rect.x + rows[4].rect.w);
        assert_eq!(row_at(&rows, 20., 40.).map(|r| r.id.as_str()), Some("s1"));
        assert_eq!(row_at(&rows, 20., 500.), None);
    }

    #[test]
    fn a_drop_lands_above_the_first_row_whose_middle_is_below_the_pointer() {
        let rows = rows(&ids(3), bar());
        assert_eq!(drop_index(&rows, 40.), 0);
        assert_eq!(drop_index(&rows, rows[0].rect.y + ROW_H - 1.), 1);
        assert_eq!(drop_index(&rows, 400.), 3);
    }

    fn window(id: &str, x: f32, sidebar: bool) -> DropWindow {
        let frame = Rect { x, y: 0., w: 1200., h: 720. };
        let bar = Rect { x: x + 10., y: 32., w: WIDTH, h: 600. };
        DropWindow { id: id.into(), frame, sidebar: sidebar.then_some(bar), rows: rows(&ids(3), bar).into_iter().map(|r| r.rect).collect(), tab_strip: None, tabs: vec![] }
    }

    #[test]
    fn a_window_without_its_sidebar_takes_the_space_between_its_tabs() {
        let mut w = window("w1", 0., false);
        // Three tabs 100 wide from x 80; the third starts past the strip's right edge.
        w.tab_strip = Some(Rect { x: 80., y: 0., w: 240., h: 32. });
        w.tabs = (0..3).map(|i| Rect { x: 80. + i as f32 * 100., y: 4., w: 100., h: 24. }).collect();
        let w = [w];
        assert_eq!(space_target(100., 16., &w, None), SpaceDropTarget::Tabs { window: "w1".into(), index: 0 });
        assert_eq!(space_target(140., 16., &w, None), SpaceDropTarget::Tabs { window: "w1".into(), index: 1 });
        assert_eq!(space_target(250., 16., &w, None), SpaceDropTarget::Tabs { window: "w1".into(), index: 2 });
        // Below the strip the body takes it.
        assert_eq!(space_target(140., 400., &w, None), SpaceDropTarget::Window("w1".into()));
        assert_eq!(index_left(&w[0].tabs, 500.), 3);
        assert_eq!(index_left(&[], 500.), 0);
        assert_eq!(space_hint(&SpaceDropTarget::Tabs { window: "w1".into(), index: 1 }, "x", |s| s.to_owned(), |_| "T".into()), "Place “x” here in “T”");
    }

    #[test]
    fn the_card_centres_on_the_pointer_and_the_chip_stays_on_its_screen() {
        assert_eq!(card_frame(1000., 500.), Rect { x: 885., y: 425., w: CARD_W, h: CARD_H });
        let screen = Rect { x: 0., y: 25., w: 1920., h: 1055. };
        assert_eq!(chip_origin(300., 400., (120., 24.), screen), (312., 412.));
        // At the right and bottom edges it stays inside the visible frame.
        assert_eq!(chip_origin(1900., 1070., (120., 24.), screen), (1800., 1056.));
        assert_eq!(chip_origin(-50., 0., (120., 24.), screen), (0., 25.));
    }

    #[test]
    fn the_front_window_under_the_pointer_takes_the_space() {
        let (front, back) = (window("w1", 0., true), window("w2", 600., true));
        let both = [front.clone(), back.clone()];
        assert_eq!(space_target(20., 400., &both, None), SpaceDropTarget::Sidebar { window: "w1".into(), index: 3 });
        assert_eq!(space_target(20., 40., &both, None), SpaceDropTarget::Sidebar { window: "w1".into(), index: 0 });
        assert_eq!(space_target(900., 400., &both, None), SpaceDropTarget::Window("w1".into()));
        // Past the front window's right edge the window behind it is under the pointer.
        assert_eq!(space_target(1500., 400., &both, None), SpaceDropTarget::Window("w2".into()));
        assert_eq!(space_target(620., 400., &[back.clone(), front], None), SpaceDropTarget::Sidebar { window: "w2".into(), index: 3 });
        assert_eq!(space_target(1900., 400., &both, None), SpaceDropTarget::Desktop);
        assert_eq!(space_target(20., 400., &[window("w3", 0., false)], None), SpaceDropTarget::Window("w3".into()));
        assert_eq!(space_target(20., 400., &[], None), SpaceDropTarget::Desktop);
    }

    #[test]
    fn the_popover_is_in_front_and_between_cards_the_nearer_one_counts() {
        let card = |session: &str, y: f32, n: usize| DropCard { session: session.into(), frame: Rect { x: 12., y, w: 296., h: 60. }, rows: (0..n).map(|i| Rect { x: 22., y: y + 35. + i as f32 * 24., w: 276., h: 24. }).collect() };
        let popover = DropPopover { frame: Rect { x: 0., y: 0., w: 320., h: 300. }, cards: vec![card("n1", 60., 2), card("n2", 140., 0)] };
        let w = [window("w1", 0., true)];
        assert_eq!(space_target(100., 110., &w, Some(&popover)), SpaceDropTarget::Card { session: "n1".into(), index: 1 });
        assert_eq!(space_target(100., 61., &w, Some(&popover)), SpaceDropTarget::Card { session: "n1".into(), index: 0 });
        assert_eq!(space_target(100., 135., &w, Some(&popover)), SpaceDropTarget::Card { session: "n2".into(), index: 0 });
        assert_eq!(space_target(100., 10., &w, Some(&popover)), SpaceDropTarget::Card { session: "n1".into(), index: 0 });
        assert_eq!(space_target(600., 100., &w, Some(&popover)), SpaceDropTarget::Window("w1".into()));
    }

    #[test]
    fn a_drop_into_another_list_or_its_own_sends_the_index_wheeld_counts() {
        let held = ids(3);
        // Into another session: the index as counted, the end as none.
        assert_eq!(drop_move(Some(2), &held, "x"), Some(Some(2)));
        assert_eq!(drop_move(None, &held, "x"), Some(None));
        // Within its own: past its slot counts it out; beside it or already last changes nothing.
        assert_eq!(drop_move(Some(3), &held, "s1"), Some(Some(2)));
        assert_eq!(drop_move(Some(0), &held, "s3"), Some(Some(0)));
        assert_eq!(drop_move(Some(1), &held, "s1"), None);
        assert_eq!(drop_move(Some(0), &held, "s1"), None);
        assert_eq!(drop_move(None, &held, "s3"), None);
        assert_eq!(drop_move(None, &held, "s1"), Some(None));
    }

    #[test]
    fn the_hint_names_what_releasing_does() {
        let (session, title) = (|s: &str| format!("S-{s}"), |w: &str| format!("T-{w}"));
        assert_eq!(space_hint(&SpaceDropTarget::Sidebar { window: "w".into(), index: 0 }, "x", session, title), "Place “x” here in “T-w”");
        assert_eq!(space_hint(&SpaceDropTarget::Window("w".into()), "x", session, title), "Move “x” to “T-w”");
        assert_eq!(space_hint(&SpaceDropTarget::Card { session: "n".into(), index: 0 }, "x", session, title), "Move “x” to the “S-n” session");
        assert_eq!(space_hint(&SpaceDropTarget::Desktop, "x", session, title), "Release to open “x” in a little window");
        assert_eq!(little_origin(1250., 300., (480., 330.)), (1010., 135.));
    }

    #[test]
    fn the_bar_hides_beside_the_dragged_row_and_the_move_index_counts_it_out() {
        let held = ids(4);
        assert_eq!(bar_index(1, &held, "s2"), None);
        assert_eq!(bar_index(2, &held, "s2"), None);
        assert_eq!(bar_index(0, &held, "s2"), Some(0));
        assert_eq!(bar_index(4, &held, "s2"), Some(4));
        assert_eq!(move_index(4, &held, "s2"), 3);
        assert_eq!(move_index(0, &held, "s2"), 0);
        assert_eq!(move_index(1, &held, "s4"), 1);
        let rows = rows(&held, bar());
        assert_eq!(bar_rect(&rows, 0).unwrap().y, rows[0].rect.y - 3.);
        assert_eq!(bar_rect(&rows, 4).unwrap().y, rows[3].rect.y + ROW_H - 1.);
    }

    #[test]
    fn a_pane_lands_on_a_row_or_makes_a_new_space() {
        let rows = rows(&ids(2), bar());
        assert_eq!(pane_away(bar(), &rows, 20., 40.), Some(PaneAway::End("s1".into())));
        assert_eq!(pane_away(bar(), &rows, 20., 500.), Some(PaneAway::NewSpace));
        assert_eq!(pane_away(bar(), &rows, 800., 500.), None);
        assert_eq!(pane_hint(&PaneAway::End("s1".into()), "Terminal", |_| Some("wheel".into())), "Move Terminal to the end of “wheel”");
    }

    #[test]
    fn the_menu_matches_the_swift_one() {
        let sessions = vec![("n1".to_owned(), "Main".to_owned()), ("n2".to_owned(), "Other".to_owned())];
        let items = menu("s1", Some("n1"), &sessions, &ids(3), false);
        let got: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(got, ["session:n2", "new-session", "little", "left", "right", "rename", "copy-path", "reveal", "new-pane", "close-others", "close"]);
        assert_eq!(items[2].title, "Open in Little Window");
        assert_eq!(menu("s1", Some("n1"), &sessions, &ids(3), true)[2].title, "Go to Little Window");
        assert!(!items[3].enabled && items[4].enabled);
        assert!(items[9].destructive && items[10].destructive && items[9].enabled);
        let last = menu("s3", Some("n1"), &sessions, &ids(3), false);
        assert!(last[3].enabled && !last[4].enabled);
        assert!(!menu("s1", Some("n1"), &sessions, &ids(1), false)[9].enabled);
        assert_eq!(items[0].title, "Move to Other");
    }

    #[test]
    fn a_rename_is_checked_before_it_is_sent() {
        assert_eq!(rename_check("  ", true), Err("A space needs a name.".into()));
        assert_eq!(rename_check("x", false), Err("Not connected to wheeld.".into()));
        assert_eq!(rename_check("  wheel ", true), Ok("wheel".into()));
    }

    #[test]
    fn a_stack_draws_one_layer_per_pane_up_to_four() {
        assert_eq!(stack_layers(0), 1);
        assert_eq!(stack_layers(3), 3);
        assert_eq!(stack_layers(9), 4);
    }
}
