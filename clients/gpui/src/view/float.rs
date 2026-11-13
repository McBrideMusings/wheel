//! The two windows a space drag draws outside every Wheel window (SpaceDrag.swift): the chip
//! naming the dragged space, which follows the pointer across every window and the desktop
//! (DragChipWindow), and over the desktop the preview card of the space it will open in a little
//! window (DesktopDropOverlay, SpaceCardView). Each is one app-wide window, opened on first use
//! and kept, deaf to the pointer; the bridge's `float` op places, shows and hides it.
use crate::{layout::Rect, native::{Bridge, Native}, sidebar};
use futures::StreamExt as _;
use gpui::{App, AppContext as _, Bounds, Context, FontWeight, IntoElement, ParentElement as _, Render, Styled as _, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point, px, rgb, size};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};

const CHIP_BG: u32 = 0x22302a;
const CARD_BG: u32 = 0x17201c;
const CARD_BORDER: u32 = 0x2c3d34;
const PANE_BG: u32 = 0x22302a;
/// The chip's widest; a longer name is cut.
const CHIP_MAX_W: f32 = 240.0;
const CHIP_H: f32 = 28.0;
/// The card's name row and the inset round its grid.
const CARD_HEADER: f32 = 28.0;
const CARD_INSET: f32 = 8.0;

/// What the drag wants drawn outside its windows now.
#[derive(Clone, Debug, PartialEq)]
pub enum Want {
    Nothing,
    /// The chip naming `name`, beside the pointer at `at` (screen points, top left origin).
    Chip { name: String, at: (f32, f32) },
    /// The preview card of `name`, whose panes sit at `panes` inside the card's grid, centred on `at`.
    Card { name: String, panes: Vec<Rect>, at: (f32, f32) },
}

/// The grid inside the card, where `Want::Card`'s pane rectangles are laid.
pub fn card_grid() -> Rect {
    Rect { x: 0., y: 0., w: sidebar::CARD_W - 2. * CARD_INSET, h: sidebar::CARD_H - CARD_HEADER - 1. - 2. * CARD_INSET }
}

pub struct FloatView {
    content: Want,
}
impl Render for FloatView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match &self.content {
            Want::Chip { name, .. } => div().size_full().child(
                div().max_w(px(CHIP_MAX_W)).h(px(CHIP_H)).px(px(10.)).flex().items_center().rounded_md().bg(rgb(CHIP_BG)).border_1().border_color(rgb(CARD_BORDER))
                    .text_sm().text_color(rgb(0xd9e8e1)).whitespace_nowrap().overflow_hidden().child(name.clone()),
            ),
            Want::Card { name, panes, .. } => {
                let grid = card_grid();
                let mut cells = div().relative().w(px(grid.w)).h(px(grid.h));
                for r in panes {
                    cells = cells.child(div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).rounded_sm().bg(rgb(PANE_BG)).border_1().border_color(rgb(CARD_BORDER)));
                }
                div().size_full().rounded(px(10.)).bg(rgb(CARD_BG)).border_1().border_color(rgb(CARD_BORDER)).flex().flex_col().overflow_hidden()
                    .child(div().h(px(CARD_HEADER)).px(px(10.)).flex().items_center().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xd9e8e1)).whitespace_nowrap().overflow_hidden().child(name.clone()))
                    .child(div().h(px(1.)).bg(rgb(CARD_BORDER)))
                    .child(div().p(px(CARD_INSET)).child(cells))
            }
            Want::Nothing => div(),
        }
    }
}

/// One floating window, its bridge half, and what it was last asked to do.
pub struct Float {
    pub handle: WindowHandle<FloatView>,
    pub native: Rc<Native>,
    pub shown: bool,
    pub frame: Option<Rect>,
    pub text: String,
}

#[derive(Default)]
struct Floats {
    chip: Option<Float>,
    card: Option<Float>,
    /// What was last applied, and by which window's drag.
    want: Option<(String, Want)>,
}

thread_local! {
    static FLOATS: RefCell<Floats> = RefCell::default();
}

/// The window `owner`'s drag wants `want` drawn: the change is applied after the caller's update,
/// since opening or drawing a window inside another window's update cannot reach the app. A
/// `Nothing` from a window that does not own what is drawn changes nothing.
pub fn set(owner: &str, want: Want, bridge: &Rc<Bridge>, hidden: bool, cx: &mut App) {
    let changed = FLOATS.with(|f| {
        let mut f = f.borrow_mut();
        let same = match &f.want {
            Some((o, w)) => (o == owner && *w == want) || (o != owner && want == Want::Nothing),
            None => want == Want::Nothing,
        };
        if !same {
            f.want = (want != Want::Nothing).then(|| (owner.to_owned(), want.clone()));
        }
        !same
    });
    if changed {
        let bridge = bridge.clone();
        cx.defer(move |cx| apply(want, &bridge, hidden, cx));
    }
}

/// The window `owner` closed: what its drag drew outside its windows goes with it.
pub fn release(owner: &str) {
    let owned = FLOATS.with(|f| {
        let mut f = f.borrow_mut();
        let owned = f.want.as_ref().is_some_and(|(o, _)| o == owner);
        if owned {
            f.want = None;
        }
        owned
    });
    if owned {
        hide(|f| &mut f.chip);
        hide(|f| &mut f.card);
    }
}

fn apply(want: Want, bridge: &Rc<Bridge>, hidden: bool, cx: &mut App) {
    let (chip, card) = match &want {
        Want::Nothing => (None, None),
        Want::Chip { at, .. } => (Some(*at), None),
        Want::Card { at, .. } => (None, Some(*at)),
    };
    if chip.is_none() {
        hide(|f| &mut f.chip);
    }
    if card.is_none() {
        hide(|f| &mut f.card);
    }
    let name = match &want {
        Want::Chip { name, .. } | Want::Card { name, .. } => name.clone(),
        Want::Nothing => return,
    };
    let is_card = card.is_some();
    let Some((handle, native, was_shown)) = ensure(is_card, bridge, hidden, cx) else {
        // The next move asks again.
        FLOATS.with(|f| f.borrow_mut().want = None);
        eprintln!("wheel-gpui: space drag: the {} window did not open", if is_card { "card" } else { "chip" });
        return;
    };
    // The chip is as wide as its name, up to its widest.
    let Ok(width) = handle.update(cx, |view, window, cx| {
        view.content = want.clone();
        cx.notify();
        if is_card {
            return sidebar::CARD_W;
        }
        let style = window.text_style();
        let shaped = window.text_system().shape_line(name.clone().into(), px(14.), &[style.to_run(name.len())], None);
        (f32::from(shaped.width) + 22.).min(CHIP_MAX_W)
    }) else {
        FLOATS.with(|f| f.borrow_mut().want = None);
        return;
    };
    let frame = match (chip, card) {
        (Some((x, y)), _) => {
            let screen = native.call(json!({"op": "screen_at", "point": [x, y]}));
            let visible = rect_of(&screen["frame"]).unwrap_or(Rect { x: f32::MIN / 4., y: f32::MIN / 4., w: f32::MAX / 2., h: f32::MAX / 2. });
            let (ox, oy) = sidebar::chip_origin(x, y, (width, CHIP_H), visible);
            Rect { x: ox, y: oy, w: width, h: CHIP_H }
        }
        (_, Some((x, y))) => sidebar::card_frame(x, y),
        _ => return,
    };
    native.call(json!({"op": "float", "frame": [frame.x, frame.y, frame.w, frame.h], "show": true, "level": if is_card { "card" } else { "chip" }, "grow": is_card && !was_shown}));
    FLOATS.with(|f| {
        let mut f = f.borrow_mut();
        if let Some(float) = if is_card { f.card.as_mut() } else { f.chip.as_mut() } {
            float.shown = true;
            float.frame = Some(frame);
            float.text = name;
        }
    });
}

fn hide(slot: impl Fn(&mut Floats) -> &mut Option<Float>) {
    FLOATS.with(|f| {
        let mut f = f.borrow_mut();
        if let Some(float) = slot(&mut f).as_mut().filter(|fl| fl.shown) {
            let r = float.frame.unwrap_or(Rect { x: 0., y: 0., w: 1., h: 1. });
            float.native.call(json!({"op": "float", "frame": [r.x, r.y, r.w, r.h], "show": false}));
            float.shown = false;
        }
    });
}

/// The chip's or the card's window, opened the first time it is wanted; whether it was showing.
fn ensure(card: bool, bridge: &Rc<Bridge>, hidden: bool, cx: &mut App) -> Option<(WindowHandle<FloatView>, Rc<Native>, bool)> {
    let held = FLOATS.with(|f| {
        let f = f.borrow();
        let slot = if card { &f.card } else { &f.chip };
        slot.as_ref().map(|fl| (fl.handle, fl.native.clone(), fl.shown))
    });
    // A window something else closed (the check closes every window) opens again.
    match held {
        Some((handle, _, _)) if handle.update(cx, |_, _, _| ()).is_ok() => return held,
        Some(_) => FLOATS.with(|f| {
            let mut f = f.borrow_mut();
            if card { f.card = None } else { f.chip = None }
        }),
        None => {}
    }
    let (w, h) = if card { (sidebar::CARD_W, sidebar::CARD_H) } else { (CHIP_MAX_W, CHIP_H) };
    let bounds = Bounds { origin: point(px(0.), px(0.)), size: size(px(w), px(h)) };
    let handle = cx.open_window(
        WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), kind: WindowKind::PopUp, titlebar: None, focus: false, show: false, is_movable: false, window_background: WindowBackgroundAppearance::Transparent, ..Default::default() },
        |_, cx| cx.new(|_| FloatView { content: Want::Nothing }),
    ).ok()?;
    let native = Native::new(bridge);
    let (frames, mut incoming) = futures::channel::mpsc::unbounded::<String>();
    handle.update(cx, |_, window, _| -> Option<()> {
        let RawWindowHandle::AppKit(raw) = window.window_handle().ok()?.as_raw() else { return None };
        native.attach_to(raw.ns_view.as_ptr(), hidden, frames);
        Some(())
    }).ok()??;
    // The bridge's frames for this window (its Reduce Motion) are read and dropped.
    cx.spawn(async move |_| while incoming.next().await.is_some() {}).detach();
    FLOATS.with(|f| {
        let mut f = f.borrow_mut();
        let float = Float { handle, native: native.clone(), shown: false, frame: None, text: String::new() };
        if card { f.card = Some(float) } else { f.chip = Some(float) }
    });
    Some((handle, native, false))
}

fn rect_of(v: &Value) -> Option<Rect> {
    let a: Vec<f32> = v.as_array()?.iter().map(|n| n.as_f64().unwrap_or(0.) as f32).collect();
    (a.len() == 4).then(|| Rect { x: a[0], y: a[1], w: a[2], h: a[3] })
}

/// The chip and the card as last asked, each with its window's frame as the bridge measures it,
/// for the check.
pub fn state() -> Value {
    FLOATS.with(|f| {
        let f = f.borrow();
        let one = |fl: &Option<Float>| fl.as_ref().map(|fl| json!({
            "shown": fl.shown, "asked": fl.frame.map(|r| [r.x, r.y, r.w, r.h]), "text": fl.text,
            "frame": fl.native.call(json!({"op": "frame"}))["frame"].clone(),
        }));
        json!({"chip": one(&f.chip), "card": one(&f.card)})
    })
}

/// The card's window, to draw it for a picture.
pub fn card_window() -> Option<gpui::AnyWindowHandle> {
    FLOATS.with(|f| f.borrow().card.as_ref().map(|fl| fl.handle.into()))
}
