//! The window's header: a row of sessions and a row of that session's spaces (ADR-0014), as
//! chips of fixed size. Every chip has a rectangle the view draws and a hidden check clicks.
use crate::layout::Rect;

pub const SESSION_W: f32 = 130.0;
pub const SPACE_W: f32 = 96.0;
pub const NEW_WINDOW_W: f32 = 110.0;
pub const CHIP_H: f32 = 22.0;
pub const GAP: f32 = 6.0;
pub const MARGIN: f32 = 10.0;
/// Where the connection banner sits, left of the New Window button on the sessions row.
pub const BANNER_W: f32 = 380.0;
pub const SESSIONS_Y: f32 = 6.0;
pub const SPACES_Y: f32 = 32.0;
/// The header's height: both rows and the room under them.
pub const HEIGHT: f32 = 60.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Session(String),
    Space(String),
    NewWindow,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub hit: Hit,
    pub rect: Rect,
    pub label: String,
    pub active: bool,
}

/// Where the banner is drawn for a window `width` points wide.
pub fn banner_rect(width: f32) -> Rect {
    Rect { x: width - MARGIN - NEW_WINDOW_W - GAP - BANNER_W, y: SESSIONS_Y, w: BANNER_W, h: CHIP_H }
}
pub fn new_window_rect(width: f32) -> Rect {
    Rect { x: width - MARGIN - NEW_WINDOW_W, y: SESSIONS_Y, w: NEW_WINDOW_W, h: CHIP_H }
}

/// The chips that fit in `room` points when each takes `each` plus a gap, the active one always
/// among them: the list starts at the first chip and shifts only as far as the active one needs.
fn visible(count: usize, room: f32, each: f32, active: Option<usize>) -> std::ops::Range<usize> {
    let cap = (((room + GAP) / (each + GAP)).floor().max(0.0) as usize).min(count);
    let start = active.map_or(0, |a| (a + 1).saturating_sub(cap));
    start..(start + cap).min(count)
}

/// Every chip of the header. `sessions` and `spaces` are (id, name) in order.
pub fn chips(width: f32, sessions: &[(String, String)], spaces: &[(String, String)], session: Option<&str>, space: Option<&str>) -> Vec<Chip> {
    let mut out = vec![];
    let row = |items: &[(String, String)], y: f32, each: f32, room: f32, active: Option<&str>, hit: &dyn Fn(String) -> Hit, out: &mut Vec<Chip>| {
        let at = items.iter().position(|(id, _)| Some(id.as_str()) == active);
        let shown = visible(items.len(), room, each, at);
        for (slot, (id, name)) in items[shown.clone()].iter().enumerate() {
            out.push(Chip {
                hit: hit(id.clone()),
                rect: Rect { x: MARGIN + slot as f32 * (each + GAP), y, w: each, h: CHIP_H },
                label: name.clone(),
                active: Some(id.as_str()) == active,
            });
        }
    };
    row(sessions, SESSIONS_Y, SESSION_W, banner_rect(width).x - GAP - MARGIN, session, &Hit::Session, &mut out);
    row(spaces, SPACES_Y, SPACE_W, width - 2.0 * MARGIN, space, &Hit::Space, &mut out);
    out.push(Chip { hit: Hit::NewWindow, rect: new_window_rect(width), label: "New Window".into(), active: false });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(prefix: &str, n: usize) -> Vec<(String, String)> {
        (1..=n).map(|i| (format!("{prefix}{i}"), format!("{prefix} {i}"))).collect()
    }

    #[test]
    fn sessions_and_spaces_sit_on_their_own_rows_and_new_window_at_the_right() {
        let c = chips(1200., &named("n", 2), &named("s", 3), Some("n2"), Some("s1"));
        assert_eq!(c.len(), 2 + 3 + 1);
        assert_eq!(c[0].rect, Rect { x: 10., y: 6., w: 130., h: 22. });
        assert_eq!(c[1].rect.x, 146.);
        assert!(c[1].active && !c[0].active);
        assert_eq!(c[2].rect, Rect { x: 10., y: 32., w: 96., h: 22. });
        assert_eq!(c[5].hit, Hit::NewWindow);
        assert_eq!(c[5].rect.x + c[5].rect.w, 1190.);
    }

    #[test]
    fn controls_keep_their_place_whatever_the_counts() {
        let few = chips(1200., &named("n", 1), &named("s", 1), None, None);
        let many = chips(1200., &named("n", 4), &named("s", 30), Some("n1"), Some("s1"));
        assert_eq!(few.last().unwrap().rect, many.last().unwrap().rect);
        assert_eq!(few[0].rect, many[0].rect);
    }

    #[test]
    fn an_overflowing_row_keeps_the_active_chip_in_view_and_inside_its_room() {
        let spaces = named("s", 30);
        let c = chips(1200., &[], &spaces, None, Some("s30"));
        let row: Vec<_> = c.iter().filter(|c| c.rect.y == SPACES_Y).collect();
        assert!(row.iter().any(|c| c.active));
        assert!(row.last().unwrap().rect.x + SPACE_W <= 1190.);
        let first = chips(1200., &[], &spaces, None, Some("s1"));
        assert_eq!(first.iter().filter(|c| c.rect.y == SPACES_Y).next().unwrap().label, "s 1");
    }

    #[test]
    fn session_chips_stop_before_the_banner() {
        let c = chips(900., &named("n", 9), &[], Some("n1"), None);
        let last = c.iter().filter(|c| c.rect.y == SESSIONS_Y && c.hit != Hit::NewWindow).last().unwrap();
        assert!(last.rect.x + last.rect.w <= banner_rect(900.).x);
    }
}
