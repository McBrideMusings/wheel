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
    /// Where the pointer is, for the chip that follows it.
    pub at: (f32, f32),
}

/// Where releasing a dragged space would put it. The Swift client also drops on other windows,
/// the popover's cards and the desktop; this window is the only one here (wheel-ynjo.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceDropTarget {
    /// Between this window's sidebar rows: `index` counts the rows above the bar, in the list as
    /// it was before the drag.
    Sidebar(usize),
    /// The window's body: the space goes to the end of the window.
    Window,
}

/// What releasing at (`x`, `y`) would do while the sidebar is at `sidebar` with `rows`.
pub fn space_target(sidebar: Rect, rows: &[Row], x: f32, y: f32) -> SpaceDropTarget {
    if contains(sidebar, x, y) { SpaceDropTarget::Sidebar(drop_index(rows, y)) } else { SpaceDropTarget::Window }
}

/// The words under a space drag (SpaceDropping.hint).
pub fn space_hint(target: &SpaceDropTarget, space: &str, window: &str) -> String {
    match target {
        SpaceDropTarget::Sidebar(_) => format!("Place “{space}” here in “{window}”"),
        SpaceDropTarget::Window => format!("Move “{space}” to “{window}”"),
    }
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
        assert_eq!(space_target(bar(), &rows, 20., 400.), SpaceDropTarget::Sidebar(3));
        assert_eq!(space_target(bar(), &rows, 600., 400.), SpaceDropTarget::Window);
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
