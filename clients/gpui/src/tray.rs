//! A space's tray: the chip over its grid while it has backgrounded panes, and the list the chip
//! opens (TrayChip.swift `TrayPlace`, `TrayChip`, `TrayList`; AppModel.swift `trayTitle`). Where
//! the chip sits is this client's own (ADR-0022): one place for every space, never sent to wheeld.
use crate::layout::{Leaf, Rect};
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use wheel_proto::Dir;

/// The chip's side and its gap from the grid's edges, as the Swift client's.
pub const SIZE: f32 = 34.;
pub const MARGIN: f32 = 8.;
/// A list row's height, the list's padding above and below its rows, and its width.
pub const ROW: f32 = 30.;
pub const PAD: f32 = 6.;
pub const LIST_W: f32 = 240.;
/// The gap between the chip and its list.
pub const GAP: f32 = 6.;
/// A row's pop-out button: its side and its inset from the row's right edge.
pub const POP: f32 = 22.;
pub const POP_INSET: f32 = 12.;
/// How far a press on the chip goes before it drags the chip instead of clicking it.
pub const DRAG_DISTANCE: f32 = 6.;

/// Which edge of the grid the chip is pinned to, and where along it (0 at the edge's start, 1 at its end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub edge: Dir,
    pub along: f32,
}

impl Default for Place {
    fn default() -> Self {
        Self { edge: Dir::Right, along: 0.5 }
    }
}

pub fn edge_name(edge: Dir) -> &'static str {
    match edge {
        Dir::Left => "left",
        Dir::Right => "right",
        Dir::Up => "up",
        Dir::Down => "down",
    }
}

impl Place {
    /// A place in the text form the Swift client's `TrayPlace.save` uses, `<edge>:<along>`; None
    /// for anything else. This client keeps it in its own file.
    pub fn parse(text: &str) -> Option<Self> {
        let (edge, along) = text.trim().split_once(':')?;
        let edge = [Dir::Left, Dir::Right, Dir::Up, Dir::Down].into_iter().find(|d| edge_name(*d) == edge)?;
        let along: f32 = along.parse().ok().filter(|a: &f32| a.is_finite())?;
        Some(Self { edge, along: along.clamp(0., 1.) })
    }
    pub fn text(&self) -> String {
        format!("{}:{}", edge_name(self.edge), self.along)
    }
    /// The chip's top left corner in a grid `w` by `h` (grid coordinates).
    pub fn origin(&self, w: f32, h: f32) -> (f32, f32) {
        let x = (w - SIZE - 2. * MARGIN).max(0.) * self.along + MARGIN;
        let y = (h - SIZE - 2. * MARGIN).max(0.) * self.along + MARGIN;
        match self.edge {
            Dir::Left => (MARGIN, y),
            Dir::Right => (w - SIZE - MARGIN, y),
            Dir::Up => (x, MARGIN),
            Dir::Down => (x, h - SIZE - MARGIN),
        }
    }
    /// The edge nearest (`x`, `y`) in a grid `w` by `h`, and where along it the point falls.
    pub fn nearest(x: f32, y: f32, w: f32, h: f32) -> Self {
        let gaps = [(Dir::Left, x), (Dir::Right, w - x), (Dir::Up, y), (Dir::Down, h - y)];
        let edge = gaps.iter().fold(gaps[0], |best, g| if g.1 < best.1 { *g } else { best }).0;
        let side = matches!(edge, Dir::Left | Dir::Right);
        let span = if side { h } else { w } - SIZE - 2. * MARGIN;
        let pos = if side { y } else { x } - SIZE / 2. - MARGIN;
        Self { edge, along: if span > 0. { (pos / span).clamp(0., 1.) } else { 0.5 } }
    }
}

/// Where the list of `rows` rows opens beside a chip at `chip`, on the side away from the edge
/// the chip is pinned to, kept inside `area` (window coordinates).
pub fn list_rect(chip: Rect, edge: Dir, rows: usize, area: Rect) -> Rect {
    let (w, h) = (LIST_W, rows as f32 * ROW + 2. * PAD);
    let (cx, cy) = (chip.x + chip.w / 2., chip.y + chip.h / 2.);
    let (x, y) = match edge {
        Dir::Right => (chip.x - GAP - w, cy - h / 2.),
        Dir::Left => (chip.x + chip.w + GAP, cy - h / 2.),
        Dir::Up => (cx - w / 2., chip.y + chip.h + GAP),
        Dir::Down => (cx - w / 2., chip.y - GAP - h),
    };
    let clamp = |v: f32, lo: f32, size: f32, room: f32| v.min(lo + room - size).max(lo);
    Rect { x: clamp(x, area.x, w, area.w), y: clamp(y, area.y, h, area.h), w, h }
}

/// The name a tray row shows for a pane: its agent or page, never a status.
pub fn title(leaf: &Leaf) -> String {
    match leaf.kind.as_str() {
        "empty" => "Empty pane".into(),
        "terminal" => leaf.harness.as_deref().map_or_else(|| "Terminal".into(), crate::settings::agent_title),
        "web" | "plugin" => leaf.label.clone(),
        "browser" => {
            let url = leaf.url.as_deref().unwrap_or(&leaf.label);
            url.split_once("://").map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
                .map(|host| host.rsplit_once('@').map_or(host, |(_, h)| h))
                .map(|host| host.split(':').next().unwrap_or(host))
                .filter(|host| !host.is_empty()).unwrap_or(url).to_owned()
        }
        "agent" => crate::settings::agent_title(leaf.harness.as_deref().unwrap_or("agent")),
        _ => "Pane".into(),
    }
}

/// The SF Symbol a tray row draws for a pane's kind (SpacesView.swift `PaneKind.icon`).
pub fn icon(kind: &str) -> &'static str {
    match kind {
        "empty" => "rectangle.dashed",
        "terminal" => "terminal",
        "web" | "plugin" => "doc.text",
        "browser" => "safari",
        "agent" => "bubble.left.and.text.bubble.right",
        _ => "questionmark.square.dashed",
    }
}

/// The chip's place, read from `file` and written back as it moves; kept in memory only without one.
pub struct Store {
    pub place: Place,
    file: Option<PathBuf>,
}

impl Store {
    pub fn load(file: Option<PathBuf>) -> Self {
        let place = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| Place::parse(&t)).unwrap_or_default();
        Self { place, file }
    }
    pub fn default_file() -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/wheel-gpui/tray-place")
    }
    /// The one place every window of this process reads and writes for `file`.
    pub fn shared(file: Option<PathBuf>) -> Rc<RefCell<Store>> {
        thread_local! {
            static SHARED: RefCell<std::collections::HashMap<Option<PathBuf>, Rc<RefCell<Store>>>> = RefCell::default();
        }
        SHARED.with(|s| s.borrow_mut().entry(file.clone()).or_insert_with(|| Rc::new(RefCell::new(Store::load(file)))).clone())
    }
    pub fn set(&mut self, place: Place) {
        self.place = place;
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(error) = std::fs::write(file, place.text()) {
            eprintln!("wheel-gpui: the tray chip's place was not saved to {}: {error}", file.display());
        }
    }
}

/// A press on the chip: where it went down and where the pointer is; `moved` once it went
/// `DRAG_DISTANCE`, after which the release places the chip instead of opening the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub start: (f32, f32),
    pub at: (f32, f32),
    pub moved: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(kind: &str) -> Leaf {
        Leaf { id: "p".into(), kind: kind.into(), label: String::new(), term: None, url: None, scope: None, agent: None, harness: None, home: None, status: None }
    }

    #[test]
    fn a_place_reads_back_the_edge_colon_along_form() {
        assert_eq!(Place::parse("left:0.25"), Some(Place { edge: Dir::Left, along: 0.25 }));
        assert_eq!(Place::parse("down:7"), Some(Place { edge: Dir::Down, along: 1. }));
        assert_eq!(Place::parse("middle:0.5"), None);
        assert_eq!(Place::parse("up:NaN"), None);
        assert_eq!(Place::parse(&Place { edge: Dir::Up, along: 0.75 }.text()), Some(Place { edge: Dir::Up, along: 0.75 }));
    }

    #[test]
    fn the_chip_sits_on_its_edge_inside_the_margin() {
        assert_eq!(Place::default().origin(1000., 600.), (958., 283.));
        assert_eq!(Place { edge: Dir::Left, along: 0. }.origin(1000., 600.), (8., 8.));
        assert_eq!(Place { edge: Dir::Down, along: 1. }.origin(1000., 600.), (958., 558.));
        assert_eq!(Place { edge: Dir::Up, along: 0.5 }.origin(1000., 600.), (483., 8.));
    }

    #[test]
    fn a_drop_picks_the_nearest_edge_and_the_spot_along_it() {
        assert_eq!(Place::nearest(990., 300., 1000., 600.), Place { edge: Dir::Right, along: 0.5 });
        assert_eq!(Place::nearest(500., 595., 1000., 600.).edge, Dir::Down);
        assert_eq!(Place::nearest(0., 5., 1000., 600.), Place { edge: Dir::Left, along: 0. });
        // A drop puts the chip's centre where the pointer let go.
        let p = Place::nearest(500., 10., 1000., 600.);
        let (x, _) = p.origin(1000., 600.);
        assert!((x + SIZE / 2. - 500.).abs() < 0.01, "{p:?} puts the chip at {x}");
    }

    #[test]
    fn the_list_opens_away_from_the_edge_and_stays_in_the_grid() {
        let area = Rect { x: 0., y: 40., w: 1000., h: 600. };
        let chip = Rect { x: 958., y: 323., w: SIZE, h: SIZE };
        let list = list_rect(chip, Dir::Right, 2, area);
        assert_eq!((list.x, list.w, list.h), (958. - GAP - LIST_W, LIST_W, 2. * ROW + 2. * PAD));
        let top = Rect { x: 8., y: 48., w: SIZE, h: SIZE };
        let below = list_rect(top, Dir::Up, 1, area);
        assert_eq!((below.x, below.y), (0., 48. + SIZE + GAP));
    }

    #[test]
    fn a_row_names_the_agent_or_the_page() {
        assert_eq!(title(&leaf("terminal")), "Terminal");
        assert_eq!(title(&Leaf { harness: Some("codex".into()), ..leaf("terminal") }), "Codex");
        assert_eq!(title(&Leaf { harness: Some("claude".into()), ..leaf("agent") }), "Claude");
        assert_eq!(title(&Leaf { label: "notes".into(), ..leaf("web") }), "notes");
        assert_eq!(title(&Leaf { url: Some("https://user@example.com:8080/a?b".into()), ..leaf("browser") }), "example.com");
        assert_eq!(title(&Leaf { url: Some("about:blank".into()), ..leaf("browser") }), "about:blank");
        assert_eq!(title(&leaf("empty")), "Empty pane");
        assert_eq!(title(&leaf("gizmo")), "Pane");
    }
}
