//! A plugin pane's `ui.Canvas` drawn by GPUI (wheel-4d1o.4), as WheelUI's `Canvas.swift` and
//! `CanvasCommands.swift` draw it. A `commands` list a script recorded is replayed with Canvas 2D
//! semantics (wheel-luau's `canvas::replay`), reports its size as `canvas-size:<key>` and raises
//! `pointerAction`; an `ops` scene gets a camera (drag to pan, scroll to zoom, Fit + −), hit
//! regions that hover, tip, click and double-click, and its size, focus and key actions.
use super::plugin_draw::{ACCENT, Draw, Env, SECONDARY, el_id, greedy, pane_color};
use super::*;
use gpui::{Bounds, DispatchPhase, FillOptions, Hitbox, HitboxBehavior, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, PathStyle, Pixels, Point, ScrollDelta, ScrollWheelEvent, StrokeOptions, TextAlign, TextRun, canvas, point};
use serde_json::{Value as Json, json};
use std::sync::Arc;
use wheel_luau::CanvasFont;
use wheel_luau::canvas::{self as cv, Camera, Color, Paint, Scene, Seg};
use wheel_luau::tree::ViewNode;

const FG: u32 = 0xffffffd9;
const TERTIARY: u32 = 0xffffff40;
/// NSColor.gridColor and controlBackgroundColor in the dark appearance (`separatorStrong`, `group`).
const LINE: u32 = 0x1a1a1aff;
const GROUP: u32 = 0x1e1e1eff;

/// A Canvas tone as RGBA (`canvasColor` in Canvas.swift, WebTheme's dark colours).
fn tone(name: &str) -> Option<u32> {
    let accent = (ACCENT << 8) | 0xff;
    Some(match name {
        "ok" => pane_color("green")?,
        "warn" => pane_color("orange")?,
        "bad" => pane_color("red")?,
        "purple" => pane_color("purple")?,
        "muted" => SECONDARY,
        "fg" => FG,
        "faint" => TERTIARY,
        "accent" => accent,
        "line" => LINE,
        "group" => GROUP,
        // Color.secondary at 12%.
        "fill" => 0xffffff11,
        "accentFill" => (accent & 0xffffff00) | 0x26,
        _ => return None,
    })
}

fn hsla(rgba: u32, alpha: f64) -> gpui::Hsla {
    let mut c = gpui::rgba(rgba);
    c.a *= alpha.clamp(0., 1.) as f32;
    c.into()
}

/// A replayed paint's colour; an unknown tone is the label colour.
fn color(c: &Color) -> gpui::Hsla {
    match c {
        Color::Tone(t, a) => hsla(tone(t).unwrap_or(FG), *a),
        Color::Rgba([r, g, b, a]) => gpui::Rgba { r: *r as f32, g: *g as f32, b: *b as f32, a: *a as f32 }.into(),
    }
}

/// A scene op's colour: a tone or `#rrggbb[aa]`, else none.
fn scene_color(name: Option<&str>, alpha: f64) -> Option<gpui::Hsla> {
    let name = name.filter(|n| !n.is_empty())?;
    if let Some(t) = tone(name) {
        return Some(hsla(t, alpha));
    }
    let hex = name.strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    let rgba = match hex.len() { 6 => (v << 8) | 0xff, 8 => v, _ => return None };
    Some(hsla(rgba, alpha))
}

fn pt(origin: Point<Pixels>, x: f64, y: f64) -> Point<Pixels> {
    point(origin.x + px(x as f32), origin.y + px(y as f32))
}

/// `segs` offset to `origin` and filled (non-zero winding, as Core Graphics fills) or stroked.
fn paint_segs(segs: &[Seg], origin: Point<Pixels>, fill: Option<gpui::Hsla>, stroke: Option<(gpui::Hsla, f64, bool, &[f64])>, window: &mut Window) {
    let build = |mut b: PathBuilder| {
        for s in segs {
            match *s {
                Seg::Move(x, y) => b.move_to(pt(origin, x, y)),
                Seg::Line(x, y) => b.line_to(pt(origin, x, y)),
                Seg::Quad(a, c, x, y) => b.curve_to(pt(origin, x, y), pt(origin, a, c)),
                Seg::Cubic(a, c, d, e, x, y) => b.cubic_bezier_to(pt(origin, x, y), pt(origin, a, c), pt(origin, d, e)),
                Seg::Close => b.close(),
            }
        }
        b.build().ok()
    };
    if !segs.iter().any(|s| !matches!(s, Seg::Move(..) | Seg::Close)) {
        return;
    }
    if let Some(c) = fill
        && let Some(p) = build(PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero())))
    {
        window.paint_path(p, c);
    }
    if let Some((c, width, round, dash)) = stroke
        && width > 0.
    {
        let mut o = StrokeOptions::default().with_line_width(width as f32);
        if round {
            o = o.with_line_cap(lyon_tessellation_cap()).with_line_join(lyon_tessellation_join());
        }
        let mut b = PathBuilder::stroke(px(width as f32)).with_style(PathStyle::Stroke(o));
        // GPUI's dasher advances by each length: a zero, negative or sub-point pattern never ends, so it draws solid.
        if !dash.is_empty() && dash.iter().all(|d| d.is_finite() && *d > 0.) && dash.iter().sum::<f64>() >= 1. {
            b = b.dash_array(&dash.iter().map(|d| px(*d as f32)).collect::<Vec<_>>());
        }
        if let Some(p) = build(b) {
            window.paint_path(p, c);
        }
    }
}

fn lyon_tessellation_cap() -> lyon::tessellation::LineCap {
    lyon::tessellation::LineCap::Round
}

fn lyon_tessellation_join() -> lyon::tessellation::LineJoin {
    lyon::tessellation::LineJoin::Round
}

thread_local! {
    /// The family the monospaced system font resolves to here (SF Mono, which `measureText` measures), found once.
    static MONO: std::cell::OnceCell<&'static str> = const { std::cell::OnceCell::new() };
    /// Decoded drawImage pixels by size and data, so a redraw decodes nothing it has decoded before.
    static IMAGES: RefCell<(HashMap<String, Arc<gpui::RenderImage>>, Vec<String>, usize)> = RefCell::new(Default::default());
}

/// The GPUI font that draws the face `CanvasFont::measure` measures.
fn gpui_font(f: &CanvasFont, window: &Window) -> gpui::Font {
    let family = match f.design {
        wheel_luau::Design::Monospaced => MONO.with(|m| *m.get_or_init(|| if window.text_system().all_font_names().iter().any(|f| f == "SF Mono") { "SF Mono" } else { "Menlo" })),
        wheel_luau::Design::Serif => "New York",
        _ => ".SystemUIFont",
    };
    let mut font = gpui::font(family);
    font.weight = FontWeight(f.weight.clamp(100, 900) as f32);
    if f.italic {
        font.style = gpui::FontStyle::Italic;
    }
    font
}

fn shape(text: &str, font: gpui::Font, size: f32, color: gpui::Hsla, window: &Window) -> gpui::ShapedLine {
    let run = TextRun { len: text.len(), font, color, background_color: None, underline: None, strikethrough: None };
    window.text_system().shape_line(text.to_owned().into(), px(size), &[run], None)
}

/// RGBA pixels from base64, as BGRA for GPUI, scaled by nearest neighbour to `dw` × `dh` device
/// pixels (Swift draws them uninterpolated) and faded by `alpha`; at most 64 MB are kept.
fn image(data: &str, w: usize, h: usize, dw: usize, dh: usize, alpha: f64) -> Option<Arc<gpui::RenderImage>> {
    use base64::Engine;
    let key = format!("{w}x{h}>{dw}x{dh}@{alpha:.3}:{data}");
    if let Some(hit) = IMAGES.with(|c| c.borrow().0.get(&key).cloned()) {
        return Some(hit);
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok().filter(|b| b.len() == w * h * 4)?;
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let sy = (y * h / dh).min(h - 1);
        for x in 0..dw {
            let sx = (x * w / dw).min(w - 1);
            let s = &bytes[(sy * w + sx) * 4..][..4];
            let o = &mut out[(y * dw + x) * 4..][..4];
            o.copy_from_slice(&[s[2], s[1], s[0], (f64::from(s[3]) * alpha).round() as u8]);
        }
    }
    let buffer = image::RgbaImage::from_raw(dw as u32, dh as u32, out)?;
    let made = Arc::new(gpui::RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)]));
    IMAGES.with(|c| {
        let (store, order, bytes) = &mut *c.borrow_mut();
        store.insert(key.clone(), made.clone());
        order.push(key);
        *bytes += dw * dh * 4;
        while *bytes > 64 << 20 && order.len() > 1 {
            let old = order.remove(0);
            if let Some(img) = store.remove(&old) {
                let s = img.size(0);
                *bytes -= (s.width.0 * s.height.0 * 4) as usize;
            }
        }
    });
    Some(made)
}

/// What a Canvas keeps between frames, by node id, as Swift keeps it in the stage's `@State`.
#[derive(Default)]
pub struct CanvasView {
    /// The command list last replayed and its paints.
    played: Option<(Json, Rc<Vec<Paint>>)>,
    /// The size last reported as `canvas-size:<key>`.
    reported: Option<(f32, f32)>,
    pressed: bool,
    last: Option<(f32, f32)>,
    /// A move or drag step waiting for the next frame: kind, x, y, dx, dy.
    pending: Option<(String, f32, f32, f32, f32)>,
    armed: bool,
    // The scene's camera and pointer.
    cam: Camera,
    drag: Option<(Camera, f32, f32)>,
    moved: bool,
    hover: Option<(f32, f32)>,
    size: (f32, f32),
    last_focus: String,
    last_fit: String,
    shown: bool,
    reported_w: Option<f64>,
    last_tap: Option<cv::Hit>,
    cuts: HashMap<String, String>,
    focus: Option<gpui::FocusHandle>,
    /// For a check: each label drawn, its width as drawn and as `measureText` gives it.
    pub texts: Vec<(String, f32, f64)>,
    /// For a check: the first rect hits in view, as (id, screen rect).
    hits: Vec<(String, [f64; 4])>,
}

impl CanvasView {
    fn camera(&self, scene: &Scene) -> Camera {
        scene.effective(self.cam, f64::from(self.size.0), f64::from(self.size.1))
    }

    fn zoom(&mut self, scene: &Scene, f: f64, about: (f32, f32)) {
        let mut c = self.camera(scene);
        let s = (c.s * f).clamp(0.002, 8.);
        let k = s / c.s;
        let (px, py) = (f64::from(about.0), f64::from(about.1));
        c.x = px - (px - c.x) * k;
        c.y = py - (py - c.y) * k;
        c.s = s;
        c.auto = false;
        self.cam = c;
    }

    fn pan(&mut self, scene: &Scene, dx: f64, dy: f64) {
        let mut c = self.camera(scene);
        c.x += dx;
        c.y += dy;
        c.auto = false;
        self.cam = c;
    }

    /// For a check: the camera, the size, the hover and the labels as drawn.
    pub fn json(&self) -> Json {
        json!({
            "size": [self.size.0, self.size.1], "reported": self.reported.map(|(w, h)| [w, h]),
            "camera": {"x": self.cam.x, "y": self.cam.y, "s": self.cam.s, "auto": self.cam.auto},
            "hover": self.hover.map(|(x, y)| [x, y]), "paints": self.played.as_ref().map_or(0, |p| p.1.len()),
            "hits": self.hits.iter().map(|(id, r)| json!({"id": id, "rect": r})).collect::<Vec<_>>(),
            "texts": self.texts.iter().map(|(t, d, m)| json!({"text": t, "drawn": d, "measured": m})).collect::<Vec<_>>(),
        })
    }
}

type Shared = Rc<RefCell<CanvasView>>;

/// Raises action `key` of node `id` in pane `pane`, as the bridge's `tree.raise` does.
fn raise(client: &gpui::WeakEntity<Client>, pane: &str, id: &str, key: &str, context: Json, cx: &mut App) {
    let (pane, id, key) = (pane.to_owned(), id.to_owned(), key.to_owned());
    client.update(cx, |this, cx| {
        this.plugin_raise(&pane, &id, &key, &context, cx);
        cx.notify();
    }).ok();
}

fn local(e: Point<Pixels>, b: Bounds<Pixels>) -> (f32, f32) {
    (f32::from(e.x - b.origin.x), f32::from(e.y - b.origin.y))
}

/// A scroll's (dx, dy) in points, as the Swift stage reads `scrollingDelta`: a line is 8 points,
/// and ⌃ makes the vertical one 6 times larger.
fn scroll_delta(e: &ScrollWheelEvent) -> (f32, f32) {
    let (dx, dy) = match e.delta {
        ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
        ScrollDelta::Lines(l) => (l.x * 8., l.y * 8.),
    };
    (dx, if e.modifiers.control { dy * 6. } else { dy })
}

impl Draw<'_> {
    fn canvas_state(&self, id: &str) -> Shared {
        self.host.canvases.borrow_mut().entry(id.to_owned()).or_default().clone()
    }

    pub(super) fn canvas(&self, n: &ViewNode, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        if let Some(list) = n.props.get("commands").and_then(Json::as_array) {
            self.command_canvas(n, list, outer, cx)
        } else {
            self.scene_canvas(n, outer, cx)
        }
    }

    /// A Canvas drawn from a recorded command list: `height` tall, as wide as it is offered,
    /// clipped to its rect, with no camera, controls, background or border.
    fn command_canvas(&self, n: &ViewNode, list: &[Json], outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let height = n.number("height").unwrap_or(240.) as f32;
        let key = n.string("canvasKey").unwrap_or("canvas").to_owned();
        let pointer = n.action("pointerAction").to_owned();
        let state = self.canvas_state(&n.id);
        {
            let mut s = state.borrow_mut();
            if s.played.as_ref().is_none_or(|(c, _)| c.as_array().map(Vec::as_slice) != Some(list)) {
                let paints = Rc::new(cv::replay(&cv::commands(list)));
                s.played = Some((Json::Array(list.to_vec()), paints));
            }
        }
        let paints = state.borrow().played.as_ref().map(|p| p.1.clone()).unwrap_or_default();
        let client = cx.entity().downgrade();
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        let stage = canvas(
            {
                let (state, client, pane) = (state.clone(), client.clone(), pane.clone());
                move |bounds, window, cx| {
                    let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    let mut s = state.borrow_mut();
                    if s.reported != Some(size) {
                        s.reported = Some(size);
                        let (client, pane, key) = (client.clone(), pane.clone(), key.clone());
                        cx.defer(move |cx| {
                            client.update(cx, |this, cx| {
                                this.plugin_canvas_size(&pane, &key, f64::from(size.0), f64::from(size.1));
                                cx.notify();
                            }).ok();
                        });
                    }
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                }
            },
            move |bounds, hitbox: Hitbox, window, cx| {
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    let mut texts = Vec::new();
                    for p in paints.iter() {
                        paint_command(p, bounds.origin, &mut texts, window, cx);
                    }
                    state.borrow_mut().texts = texts;
                });
                if pointer.is_empty() {
                    return;
                }
                // Moves and drag steps wait for the next frame and merge; every other kind sends what waits, then itself.
                let send: Rc<dyn Fn(&str, (f32, f32), f32, f32, &mut Window, &mut App)> = {
                    let (state, client, pane, id, pointer) = (state.clone(), client.clone(), pane.clone(), id.clone(), pointer.clone());
                    Rc::new(move |kind, p, dx, dy, _window, cx| {
                        let deliver = |k: &str, x: f32, y: f32, dx: f32, dy: f32, cx: &mut App| {
                            raise(&client, &pane, &id, &pointer, json!({"kind": k, "x": x, "y": y, "button": 0, "dx": dx, "dy": dy}), cx);
                        };
                        let mut s = state.borrow_mut();
                        if kind != "move" && kind != "drag" {
                            let waiting = s.pending.take();
                            drop(s);
                            if let Some((k, x, y, dx, dy)) = waiting {
                                deliver(&k, x, y, dx, dy, cx);
                            }
                            deliver(kind, p.0, p.1, dx, dy, cx);
                            return;
                        }
                        let merged = match s.pending.take() {
                            Some((k, _, _, wx, wy)) if k == kind => Some((kind.to_owned(), p.0, p.1, wx + dx, wy + dy)),
                            Some(other) => {
                                s.pending = Some((kind.to_owned(), p.0, p.1, dx, dy));
                                drop(s);
                                deliver(&other.0, other.1, other.2, other.3, other.4, cx);
                                s = state.borrow_mut();
                                None
                            }
                            None => Some((kind.to_owned(), p.0, p.1, dx, dy)),
                        };
                        if let Some(m) = merged {
                            s.pending = Some(m);
                        }
                        if !s.armed {
                            s.armed = true;
                            let (state, client, pane, id, pointer) = (state.clone(), client.clone(), pane.clone(), id.clone(), pointer.clone());
                            cx.spawn(async move |cx| {
                                cx.background_executor().timer(std::time::Duration::from_millis(16)).await;
                                let waiting = {
                                    let mut s = state.borrow_mut();
                                    s.armed = false;
                                    s.pending.take()
                                };
                                if let Some((k, x, y, dx, dy)) = waiting {
                                    cx.update(|cx| raise(&client, &pane, &id, &pointer, json!({"kind": k, "x": x, "y": y, "button": 0, "dx": dx, "dy": dy}), cx));
                                }
                            }).detach();
                        }
                    })
                };
                {
                    let (state, send, hitbox) = (state.clone(), send.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble || e.button != gpui::MouseButton::Left || !hitbox.is_hovered(window) {
                            return;
                        }
                        let p = local(e.position, bounds);
                        {
                            let mut s = state.borrow_mut();
                            s.pressed = true;
                            s.last = Some(p);
                        }
                        send("down", p, 0., 0., window, cx);
                    });
                }
                {
                    let (state, send, hitbox) = (state.clone(), send.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let p = local(e.position, bounds);
                        let (pressed, last) = { let s = state.borrow(); (s.pressed, s.last) };
                        if pressed {
                            let from = last.unwrap_or(p);
                            if p != from {
                                state.borrow_mut().last = Some(p);
                                send("drag", p, p.0 - from.0, p.1 - from.1, window, cx);
                            }
                        } else if hitbox.is_hovered(window) {
                            send("move", p, 0., 0., window, cx);
                        }
                    });
                }
                {
                    let (state, send) = (state.clone(), send.clone());
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble || e.button != gpui::MouseButton::Left || !state.borrow().pressed {
                            return;
                        }
                        {
                            let mut s = state.borrow_mut();
                            s.pressed = false;
                            s.last = None;
                        }
                        send("up", local(e.position, bounds), 0., 0., window, cx);
                    });
                }
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                        return;
                    }
                    let (dx, dy) = scroll_delta(e);
                    send("wheel", local(e.position, bounds), dx, dy, window, cx);
                    cx.stop_propagation();
                });
            },
        );
        greedy(div().flex().flex_col(), outer).h(px(height)).child(stage.size_full()).into_any_element()
    }

    /// A Canvas drawn from an `ops` scene: a camera the person moves over the scene, hit regions,
    /// and Fit, + and − in its top right corner.
    fn scene_canvas(&self, n: &ViewNode, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let viewport = self.host.viewport.get();
        let height = n.number("height").map_or_else(|| (n.number("minHeight").unwrap_or(480.)).max(f64::from(viewport) - n.number("reserve").unwrap_or(0.)), |h| h) as f32;
        let scene = Rc::new(Scene::parse(&n.props));
        let state = self.canvas_state(&n.id);
        let focus = state.borrow_mut().focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let client = cx.entity().downgrade();
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        let stage = canvas(
            {
                let (state, scene, client, pane, id) = (state.clone(), scene.clone(), client.clone(), pane.clone(), id.clone());
                move |bounds, window, cx| {
                    let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    let mut s = state.borrow_mut();
                    let resized = s.size != size;
                    s.size = size;
                    if !s.shown {
                        s.shown = true;
                        s.reported_w = Some(scene.size_known);
                    }
                    // fitKey resets the camera; a new focusKey moves it to the focus rect.
                    if scene.fit_key != s.last_fit {
                        s.last_fit = scene.fit_key.clone();
                        s.cam = Camera::default();
                    }
                    if let Some(f) = scene.focus
                        && scene.focus_key != s.last_focus
                    {
                        s.last_focus = scene.focus_key.clone();
                        s.cam = match scene.focus_zoom {
                            Some(z) => scene.focus_camera(f, z, f64::from(size.0), f64::from(size.1)),
                            None => Scene::fitted(f, f64::from(size.0), f64::from(size.1)),
                        };
                    }
                    if resized && !scene.size_action.is_empty() {
                        let q = (f64::from(size.0) / 10.).round() * 10.;
                        if (q - s.reported_w.unwrap_or(scene.size_known)).abs() >= 20. {
                            s.reported_w = Some(q);
                            let (client, pane, id, key) = (client.clone(), pane.clone(), id.clone(), scene.size_action.clone());
                            cx.defer(move |cx| raise(&client, &pane, &id, &key, json!({"value": format!("{}", q as i64)}), cx));
                        }
                    }
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                }
            },
            {
                let (state, scene) = (state.clone(), scene.clone());
                move |bounds, hitbox: Hitbox, window, cx| {
                    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                        let mut s = state.borrow_mut();
                        draw_scene(&scene, &mut s, bounds.origin, window, cx);
                    });
                    scene_events(&scene, &state, &client, &pane, &id, bounds, hitbox, window);
                }
            },
        );
        let s = state.borrow();
        let c = s.camera(&scene);
        let tip = s.hover.filter(|_| s.drag.is_none()).and_then(|p| scene.hit(f64::from(p.0), f64::from(p.1), c).map(|i| (p, i))).filter(|(_, i)| !scene.hits[*i].tip.is_empty())
            .map(|(p, i)| tooltip(&scene.hits[i], p, s.size));
        drop(s);
        let control = |label: &'static str, tag: &str, act: Rc<dyn Fn(&mut CanvasView, &Scene)>| {
            let (state, scene) = (state.clone(), scene.clone());
            div().id(el_id(self.pane, &n.id, tag)).occlude().relative().child(recorded(self.pane, &format!("{}#{tag}", n.id), self.controls)).px(px(8.)).h(px(24.)).flex().items_center().text_size(px(12.)).text_color(rgba(FG))
                .bg(rgba(GROUP)).rounded(px(6.)).border_1().border_color(rgba(LINE)).child(label)
                .on_click(cx.listener(move |_, _, _, cx| {
                    act(&mut state.borrow_mut(), &scene);
                    cx.notify();
                }))
        };
        let controls = scene.controls.then(|| {
            let mid = |s: &CanvasView| (s.size.0 / 2., s.size.1 / 2.);
            div().absolute().top(px(8.)).right(px(8.)).flex().flex_row().gap(px(4.))
                .child(control("Fit", "fit", Rc::new(|s, _| s.cam = Camera::default())))
                .child(control("+", "in", Rc::new(move |s, sc| { let m = mid(s); s.zoom(sc, 1.4, m) })))
                .child(control("−", "out", Rc::new(move |s, sc| { let m = mid(s); s.zoom(sc, 1. / 1.4, m) })))
        });
        let keys = {
            let (state, scene, pane, id) = (state.clone(), scene.clone(), self.pane.to_owned(), n.id.clone());
            cx.listener(move |this, e: &gpui::KeyDownEvent, _, cx| {
                let raised = match e.keystroke.key.as_str() {
                    "escape" => Some((&scene.escape_action, Json::Object(Default::default()))),
                    "enter" => Some((&scene.enter_action, Json::Object(scene.enter_context.clone()))),
                    _ => None,
                };
                if let Some((key, context)) = raised {
                    if !key.is_empty() {
                        this.plugin_raise(&pane, &id, key, &context, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    return;
                }
                let mut s = state.borrow_mut();
                let mid = (s.size.0 / 2., s.size.1 / 2.);
                match e.keystroke.key.as_str() {
                    "left" => s.pan(&scene, 70., 0.),
                    "right" => s.pan(&scene, -70., 0.),
                    "up" => s.pan(&scene, 0., 70.),
                    "down" => s.pan(&scene, 0., -70.),
                    "+" | "=" => s.zoom(&scene, 1.3, mid),
                    "-" => s.zoom(&scene, 1. / 1.3, mid),
                    "0" => s.cam = Camera::default(),
                    _ => return,
                }
                drop(s);
                cx.stop_propagation();
                cx.notify();
            })
        };
        let stage = div().id(el_id(self.pane, &n.id, "stage")).track_focus(&focus).relative().size_full().overflow_hidden()
            .bg(gpui::rgba(0xffffff09)).rounded(px(8.)).border_1().border_color(rgba(LINE))
            .on_mouse_down(gpui::MouseButton::Left, { let focus = focus.clone(); move |_, window, cx| focus.focus(window, cx) })
            .on_key_down(keys)
            .child(stage.size_full()).children(tip).children(controls);
        greedy(div().flex().flex_col(), outer).h(px(height)).child(stage).into_any_element()
    }
}

/// One replayed paint at `origin`; a label records its drawn and measured widths in `texts`.
fn paint_command(p: &Paint, origin: Point<Pixels>, texts: &mut Vec<(String, f32, f64)>, window: &mut Window, cx: &mut App) {
    match p {
        Paint::Fill(segs, c) => paint_segs(segs, origin, Some(color(c)), None, window),
        Paint::Stroke(segs, c, w) => paint_segs(segs, origin, None, Some((color(c), *w, false, &[])), window),
        Paint::Text { text, font, color: c, transform, x, y } => {
            if text.is_empty() {
                return;
            }
            // hack: text under a rotate or skew draws upright at its mapped origin; GPUI lays glyphs out on a level line. Upgrade when a shipped script rotates text.
            let k = transform.scale();
            let (dx, dy) = transform.apply(*x, *y);
            let line = shape(text, gpui_font(font, window), (font.size * k) as f32, color(c), window);
            // Canvas 2D puts (x, y) on the alphabetic baseline at the text's left end.
            let top = pt(origin, dx, dy - f64::from(f32::from(line.ascent)));
            let _ = line.paint(top, line.ascent + line.descent, TextAlign::Left, None, window, cx);
            texts.push((text.clone(), f32::from(line.width) / k.max(1e-9) as f32, font.measure(text).width));
        }
        Paint::Image { data, width, height, rect, transform, alpha } => {
            let (x0, y0) = transform.apply(rect[0], rect[1]);
            let (x1, y1) = transform.apply(rect[0] + rect[2], rect[1] + rect[3]);
            let (l, t, w, h) = (x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs());
            let sf = f64::from(window.scale_factor());
            let (dw, dh) = (((w * sf).round() as usize).clamp(1, 4096), ((h * sf).round() as usize).clamp(1, 4096));
            if let Some(img) = image(data, *width, *height, dw, dh, *alpha) {
                let b = Bounds { origin: pt(origin, l, t), size: gpui::size(px(w as f32), px(h as f32)) };
                let _ = window.paint_image(b, gpui::Corners::default(), img, 0, false);
            }
        }
    }
}

/// The scene under its camera, as `CanvasStage.draw` draws it: each op inside its zoom range and
/// the view, then the hovered hit's outline.
fn draw_scene(scene: &Scene, s: &mut CanvasView, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let c = s.camera(scene);
    let (vw, vh) = (f64::from(s.size.0), f64::from(s.size.1));
    let m = 24. / c.s;
    let seen = [-c.x / c.s - m, -c.y / c.s - m, vw / c.s + 2. * m, vh / c.s + 2. * m];
    let meets = |r: [f64; 4]| r[0] < seen[0] + seen[2] && r[0] + r[2] > seen[0] && r[1] < seen[1] + seen[3] && r[1] + r[3] > seen[1];
    let mut texts = Vec::new();
    for op in scene.ops.iter().filter(|o| c.s >= o.style.min_zoom && c.s <= o.style.max_zoom) {
        let st = &op.style;
        let fill = scene_color(st.fill.as_deref(), st.opacity);
        let stroke = scene_color(st.stroke.as_deref(), st.opacity);
        let lw = if st.px { st.width } else { st.width * c.s };
        let dash: Vec<f64> = st.dash.iter().map(|d| if st.px { *d } else { d * c.s }).collect();
        let paint = |segs: &[Seg], window: &mut Window| paint_segs(segs, origin, fill, stroke.map(|k| (k, lw, true, dash.as_slice())), window);
        match op.kind.as_str() {
            "rect" => {
                for r in op.nums.iter().filter(|r| r.len() == 4) {
                    if !meets([r[0] - m, r[1] - m, r[2] + 2. * m, r[3] + 2. * m]) {
                        continue;
                    }
                    let w = if st.wpx.is_none() { r[2] * c.s } else { cv::clamp(r[2] * c.s, st.wpx) };
                    let h = if st.hpx.is_none() { r[3] * c.s } else { cv::clamp(r[3] * c.s, st.hpx) };
                    let (x, y) = (r[0] * c.s + c.x, (r[1] + r[3] / 2.) * c.s + c.y - h / 2.);
                    paint(&cv::round_rect(x, y, w, h, (st.radius * c.s).min(w / 2.).min(h / 2.).max(0.)), window);
                }
            }
            "circle" => {
                for r in op.nums.iter().filter(|r| r.len() == 3) {
                    let rr = cv::clamp(r[2] * c.s, st.rpx);
                    if !meets([r[0] - rr / c.s, r[1] - rr / c.s, 2. * rr / c.s, 2. * rr / c.s]) {
                        continue;
                    }
                    paint(&cv::ellipse(r[0] * c.s + c.x - rr, r[1] * c.s + c.y - rr, rr * 2., rr * 2.), window);
                }
            }
            "path" => {
                let t = cv::Affine([c.s, 0., 0., c.s, c.x, c.y]);
                for (segs, b) in op.paths.iter().filter(|(_, b)| meets([b[0] - m, b[1] - m, b[2] + 2. * m, b[3] + 2. * m])) {
                    let _ = b;
                    let mapped: Vec<Seg> = segs.iter().map(|s| map(&t, *s)).collect();
                    paint(&mapped, window);
                }
            }
            "text" => {
                let size = if st.sizepx.is_some() { cv::clamp(st.size * c.s, st.sizepx) } else if st.px { st.size } else { st.size * c.s };
                let weight = match st.weight.as_str() { "bold" => 700, "semibold" => 600, "medium" => 500, _ => 400 };
                let font = CanvasFont { size, weight, italic: false, design: if st.mono { wheel_luau::Design::Monospaced } else { wheel_luau::Design::Default } };
                let gfont = gpui_font(&font, window);
                let col = scene_color(st.fill.as_deref(), st.opacity).unwrap_or_else(|| hsla(FG, st.opacity));
                let reach = st.maxw.unwrap_or(400.) + 8.;
                for (x, y, label) in &op.texts {
                    let area = if st.minx.is_none() { [x - reach, y - 20. / c.s, reach * 2., 40. / c.s] } else { [seen[0], y - 20. / c.s, seen[2], 40. / c.s] };
                    if !meets(area) {
                        continue;
                    }
                    let mut shown = label.clone();
                    if let Some(maxw) = st.maxw {
                        let limit = maxw * c.s;
                        let key = format!("{label}|{}|{size}|{}|{}", limit.round() as i64, st.weight, st.mono);
                        shown = match s.cuts.get(&key) {
                            Some(hit) => hit.clone(),
                            None => {
                                let made = cv::cut(label, limit, |t| f64::from(f32::from(shape(t, gfont.clone(), size as f32, col, window).width)));
                                if s.cuts.len() > 20_000 {
                                    s.cuts.clear();
                                }
                                s.cuts.insert(key, made.clone());
                                made
                            }
                        };
                    }
                    let line = shape(&shown, gfont.clone(), size as f32, col, window);
                    let lh = f64::from(f32::from(line.ascent + line.descent));
                    let w = f64::from(f32::from(line.width));
                    // SwiftUI draws the label's frame anchored at (x, y): its vertical middle, its leading, centre or trailing edge.
                    let ax = (x * c.s + c.x).max(st.minx.unwrap_or(f64::NEG_INFINITY));
                    let left = match st.anchor.as_str() { "middle" => ax - w / 2., "end" => ax - w, _ => ax };
                    let _ = line.paint(pt(origin, left, y * c.s + c.y - lh / 2.), line.ascent + line.descent, TextAlign::Left, None, window, cx);
                    texts.push((shown.clone(), w as f32, font.measure(&shown).width));
                }
            }
            _ => {}
        }
    }
    s.texts = texts;
    s.hits = scene.hits.iter().filter(|h| c.s >= h.min_zoom && c.s <= h.max_zoom)
        .filter_map(|h| h.rect.map(|r| (h.id.clone(), [r[0] * c.s + c.x, r[1] * c.s + c.y, r[2] * c.s, r[3] * c.s])))
        .filter(|(_, r)| r[0] >= 0. && r[1] >= 0. && r[0] + r[2] <= vw && r[1] + r[3] <= vh).take(20).collect();
    if let Some(p) = s.hover
        && let Some(i) = scene.hit(f64::from(p.0), f64::from(p.1), c)
    {
        let h = &scene.hits[i];
        let outline = if let Some(r) = h.rect {
            let (x, y, w, hh) = (r[0] * c.s + c.x - h.slop, r[1] * c.s + c.y - h.slop, r[2] * c.s + 2. * h.slop, r[3] * c.s + 2. * h.slop);
            cv::round_rect(x, y, w, hh, 3f64.min(w / 2.).min(hh / 2.).max(0.))
        } else if let Some(ci) = h.circle {
            let rr = cv::clamp(ci[2] * c.s, h.rpx) + h.slop;
            cv::ellipse(ci[0] * c.s + c.x - rr, ci[1] * c.s + c.y - rr, rr * 2., rr * 2.)
        } else {
            vec![]
        };
        paint_segs(&outline, origin, None, Some((hsla((ACCENT << 8) | 0xff, 1.), 1.5, false, &[])), window);
    }
}

fn map(t: &cv::Affine, s: Seg) -> Seg {
    let p = |x, y| t.apply(x, y);
    match s {
        Seg::Move(x, y) => { let (x, y) = p(x, y); Seg::Move(x, y) }
        Seg::Line(x, y) => { let (x, y) = p(x, y); Seg::Line(x, y) }
        Seg::Quad(a, b, x, y) => { let (a, b) = p(a, b); let (x, y) = p(x, y); Seg::Quad(a, b, x, y) }
        Seg::Cubic(a, b, c, d, x, y) => { let (a, b) = p(a, b); let (c, d) = p(c, d); let (x, y) = p(x, y); Seg::Cubic(a, b, c, d, x, y) }
        Seg::Close => Seg::Close,
    }
}

/// The scene's pointer: hover, drag to pan, click a hit (or the empty ground), double-click,
/// scroll to zoom or pan, and pinch to zoom.
#[allow(clippy::too_many_arguments)]
fn scene_events(scene: &Rc<Scene>, state: &Shared, client: &gpui::WeakEntity<Client>, pane: &str, id: &str, bounds: Bounds<Pixels>, hitbox: Hitbox, window: &mut Window) {
    let fire: Rc<dyn Fn(&str, Json, &mut App)> = {
        let (client, pane, id) = (client.clone(), pane.to_owned(), id.to_owned());
        Rc::new(move |key, context, cx| raise(&client, &pane, &id, key, context, cx))
    };
    {
        let (state, scene, hitbox) = (state.clone(), scene.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble || e.button != gpui::MouseButton::Left || !hitbox.is_hovered(window) {
                return;
            }
            let p = local(e.position, bounds);
            let mut s = state.borrow_mut();
            let c = s.camera(&scene);
            s.drag = Some((c, p.0, p.1));
            s.moved = false;
            s.hover = Some(p);
        });
    }
    {
        let (state, scene, hitbox) = (state.clone(), scene.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let p = local(e.position, bounds);
            let mut s = state.borrow_mut();
            if let Some((from, x0, y0)) = s.drag {
                s.hover = Some(p);
                let (tx, ty) = (p.0 - x0, p.1 - y0);
                if tx.abs() + ty.abs() > 4. {
                    s.moved = true;
                }
                if s.moved {
                    s.cam = Camera { x: from.x + f64::from(tx), y: from.y + f64::from(ty), s: from.s, auto: false };
                }
                window.refresh();
                return;
            }
            let was = s.hover;
            s.hover = hitbox.is_hovered(window).then_some(p);
            let c = s.camera(&scene);
            let hit = |h: Option<(f32, f32)>| h.and_then(|p| scene.hit(f64::from(p.0), f64::from(p.1), c));
            // The outline and the tip follow the hit under the pointer; the frame changes only when that hit does.
            if hit(was) != hit(s.hover) || (s.hover.is_some() && hit(s.hover).is_some_and(|i| !scene.hits[i].tip.is_empty())) || was.is_some() != s.hover.is_some() {
                window.refresh();
            }
        });
    }
    {
        let (state, scene, fire) = (state.clone(), scene.clone(), fire.clone());
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != gpui::MouseButton::Left {
                return;
            }
            let mut s = state.borrow_mut();
            let Some((start, ..)) = s.drag.take() else { return };
            window.refresh();
            if s.moved {
                return;
            }
            let p = local(e.position, bounds);
            match scene.hit(f64::from(p.0), f64::from(p.1), start) {
                Some(i) => {
                    let h = scene.hits[i].clone();
                    // Swift's double tap reads the hit the tap just landed on.
                    s.last_tap = Some(h.clone());
                    if let Some(f) = h.fit {
                        s.cam = Scene::fitted(f, f64::from(s.size.0), f64::from(s.size.1));
                    } else if !h.action.is_empty() {
                        let mut context = h.context.clone();
                        context.insert("id".into(), json!(h.id));
                        drop(s);
                        fire(&h.action, Json::Object(context), cx);
                    } else {
                        drop(s);
                    }
                    if e.click_count >= 2 && !h.double_action.is_empty() {
                        let mut context = h.double_context.clone();
                        context.insert("id".into(), json!(h.id));
                        fire(&h.double_action, Json::Object(context), cx);
                    }
                }
                None if !scene.empty_action.is_empty() => {
                    drop(s);
                    fire(&scene.empty_action, Json::Object(scene.empty_context.clone()), cx);
                }
                None => {}
            }
        });
    }
    {
        let (state, scene, hitbox) = (state.clone(), scene.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                return;
            }
            let (dx, dy) = scroll_delta(e);
            let p = local(e.position, bounds);
            let mut s = state.borrow_mut();
            if scene.wheel_pans { s.pan(&scene, f64::from(dx), f64::from(dy)) } else { s.zoom(&scene, (f64::from(dy) * 0.0016).exp(), p) }
            s.hover = Some(p);
            window.refresh();
            cx.stop_propagation();
        });
    }
    {
        let (state, scene) = (state.clone(), scene.clone());
        window.on_mouse_event(move |e: &gpui::PinchEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                return;
            }
            let p = local(e.position, bounds);
            let mut s = state.borrow_mut();
            s.zoom(&scene, 1. + f64::from(e.delta), p);
            s.hover = Some(p);
            window.refresh();
        });
    }
}

/// The tip beside the pointer: each line's runs in the callout size, under the pointer, or above
/// it near the bottom.
fn tooltip(h: &cv::Hit, p: (f32, f32), size: (f32, f32)) -> gpui::AnyElement {
    let mut col = div().flex().flex_col().gap(px(2.));
    for line in &h.tip {
        let mut row = div().flex().flex_row().flex_wrap();
        for r in line {
            let mut t = div().text_size(px(12.)).text_color(rgba(r.tone.as_deref().and_then(tone).unwrap_or(FG))).child(r.text.clone());
            if r.bold {
                t = t.font_weight(FontWeight::SEMIBOLD);
            }
            if r.mono {
                t = t.font_family("Menlo");
            }
            row = row.child(t);
        }
        col = col.child(row);
    }
    let x = (p.0 + 14.).min(size.0 - 330.).max(4.);
    let y = if p.1 + 16. > size.1 - 70. { (p.1 - 70.).max(4.) } else { p.1 + 16. };
    div().absolute().left(px(x)).top(px(y)).max_w(px(320.)).px(px(9.)).py(px(5.)).bg(rgba(0x2a2a2af2)).rounded(px(6.))
        .border_1().border_color(rgba(LINE)).shadow_md().child(col).into_any_element()
}
