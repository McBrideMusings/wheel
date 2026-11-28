//! What a `ui.Canvas` node says, read as WheelUI's `Canvas.swift` and `CanvasCommands.swift` read
//! it, with no drawing: a recorded Canvas 2D command list replayed into device-space paints, and
//! an `ops` scene with its camera, hit test and label cut. The client paints the result.
use serde_json::{Map, Value as Json};

use crate::CanvasFont;

/// A 2D affine transform `[a, b, c, d, tx, ty]`: x' = a·x + c·y + tx, y' = b·x + d·y + ty.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f64; 6]);

impl Affine {
    pub const IDENTITY: Self = Self([1., 0., 0., 1., 0., 0.]);

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, tx, ty] = self.0;
        (a * x + c * y + tx, b * x + d * y + ty)
    }

    /// `other` applied first, then `self` (Core Graphics' `other.concatenating(self)`).
    fn then(&self, other: &Self) -> Self {
        let [a, b, c, d, tx, ty] = self.0;
        let [a2, b2, c2, d2, tx2, ty2] = other.0;
        Self([a2 * a + b2 * c, a2 * b + b2 * d, c2 * a + d2 * c, c2 * b + d2 * d, tx2 * a + ty2 * c + tx, tx2 * b + ty2 * d + ty])
    }

    /// How much the transform scales a length: the square root of its determinant.
    pub fn scale(&self) -> f64 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }

    /// Whether it turns or skews: text drawn under it cannot stay upright.
    pub fn turns(&self) -> bool {
        self.0[1].abs() > 1e-9 || self.0[2].abs() > 1e-9
    }
}

/// One segment of a path in device space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Quad(f64, f64, f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

/// A colour a script names: one of the page's tones, or RGBA in 0...1; either times `alpha`.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Fill(Vec<Seg>, Color),
    Stroke(Vec<Seg>, Color, f64),
    Text { text: String, font: CanvasFont, color: Color, transform: Affine, x: f64, y: f64 },
    Image { data: String, width: usize, height: usize, rect: [f64; 4], transform: Affine, alpha: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Color {
    Tone(String, f64),
    Rgba([f64; 4]),
}

impl Color {
    fn times(&self, alpha: f64) -> Self {
        match self {
            Self::Tone(t, a) => Self::Tone(t.clone(), a * alpha),
            Self::Rgba([r, g, b, a]) => Self::Rgba([*r, *g, *b, a * alpha]),
        }
    }
}

/// The tones a Canvas colour may name (`canvasColor` in Canvas.swift).
pub const TONES: [&str; 12] = ["ok", "warn", "bad", "muted", "fg", "faint", "accent", "purple", "line", "group", "fill", "accentFill"];

/// RGBA in 0...1 for a CSS colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()` and a few names.
pub fn css_rgba(s: &str) -> Option<[f64; 4]> {
    let t = s.trim().to_lowercase();
    match t.as_str() {
        "black" => return Some([0., 0., 0., 1.]),
        "white" => return Some([1., 1., 1., 1.]),
        "red" => return Some([1., 0., 0., 1.]),
        "green" => return Some([0., 128. / 255., 0., 1.]),
        "blue" => return Some([0., 0., 1., 1.]),
        "transparent" => return Some([0., 0., 0., 0.]),
        _ => {}
    }
    if let Some(hex) = t.strip_prefix('#') {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let b: Vec<u32> = hex.chars().filter_map(|c| c.to_digit(16)).collect();
        let v = |hi: u32, lo: u32| f64::from(hi * 16 + lo) / 255.;
        return match b.len() {
            3 | 4 => Some([v(b[0], b[0]), v(b[1], b[1]), v(b[2], b[2]), if b.len() == 4 { v(b[3], b[3]) } else { 1. }]),
            6 | 8 => Some([v(b[0], b[1]), v(b[2], b[3]), v(b[4], b[5]), if b.len() == 8 { v(b[6], b[7]) } else { 1. }]),
            _ => None,
        };
    }
    for prefix in ["rgba(", "rgb("] {
        if let Some(inner) = t.strip_prefix(prefix).and_then(|r| r.strip_suffix(')')) {
            let parts: Option<Vec<f64>> = inner.split(',').map(|p| p.trim().parse::<f64>().ok()).collect();
            let v = parts?;
            return match v.len() {
                3 => Some([v[0] / 255., v[1] / 255., v[2] / 255., 1.]),
                4 => Some([v[0] / 255., v[1] / 255., v[2] / 255., v[3].clamp(0., 1.)]),
                _ => None,
            };
        }
    }
    None
}

/// A fillStyle or strokeStyle: a tone, else a CSS colour; None when it is neither.
pub fn style_color(s: &str) -> Option<Color> {
    if !s.starts_with('#') && TONES.contains(&s) {
        return Some(Color::Tone(s.to_owned(), 1.));
    }
    css_rgba(s).map(Color::Rgba)
}

/// One recorded command (`CanvasCommand` in CanvasCommands.swift).
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    FillStyle(String),
    StrokeStyle(String),
    LineWidth(f64),
    Font(String),
    GlobalAlpha(f64),
    BeginPath,
    ClosePath,
    Fill,
    Stroke,
    Save,
    Restore,
    MoveTo(f64, f64),
    LineTo(f64, f64),
    BezierCurveTo(f64, f64, f64, f64, f64, f64),
    QuadraticCurveTo(f64, f64, f64, f64),
    Arc(f64, f64, f64, f64, f64, bool),
    Rect(f64, f64, f64, f64),
    RoundRect(f64, f64, f64, f64, f64),
    Translate(f64, f64),
    Scale(f64, f64),
    Rotate(f64),
    FillText(String, f64, f64),
    DrawImage(String, usize, usize, f64, f64, f64, f64),
}

/// The commands in `list` that parse, in order; a command with the wrong arguments is dropped.
pub fn commands(list: &[Json]) -> Vec<Command> {
    list.iter().filter_map(command).collect()
}

fn command(item: &Json) -> Option<Command> {
    let a = item.as_array()?;
    let name = a.first()?.as_str()?;
    let rest = &a[1..];
    let n = |k: usize| -> Option<Vec<f64>> {
        if rest.len() != k {
            return None;
        }
        rest.iter().map(Json::as_f64).collect()
    };
    let none = |c: Command| rest.is_empty().then_some(c);
    use Command::*;
    Some(match name {
        "set" => {
            if rest.len() != 2 {
                return None;
            }
            let v = &rest[1];
            match rest[0].as_str()? {
                "fillStyle" => FillStyle(v.as_str()?.to_owned()),
                "strokeStyle" => StrokeStyle(v.as_str()?.to_owned()),
                "font" => Font(v.as_str()?.to_owned()),
                "lineWidth" => LineWidth(v.as_f64()?),
                "globalAlpha" => GlobalAlpha(v.as_f64()?),
                _ => return None,
            }
        }
        "beginPath" => none(BeginPath)?,
        "closePath" => none(ClosePath)?,
        "fill" => none(Fill)?,
        "stroke" => none(Stroke)?,
        "save" => none(Save)?,
        "restore" => none(Restore)?,
        "moveTo" => n(2).map(|v| MoveTo(v[0], v[1]))?,
        "lineTo" => n(2).map(|v| LineTo(v[0], v[1]))?,
        "bezierCurveTo" => n(6).map(|v| BezierCurveTo(v[0], v[1], v[2], v[3], v[4], v[5]))?,
        "quadraticCurveTo" => n(4).map(|v| QuadraticCurveTo(v[0], v[1], v[2], v[3]))?,
        "arc" => {
            if rest.len() != 6 {
                return None;
            }
            let ccw = rest[5].as_bool()?;
            let v: Vec<f64> = rest[..5].iter().map(Json::as_f64).collect::<Option<_>>()?;
            Arc(v[0], v[1], v[2], v[3], v[4], ccw)
        }
        "rect" => n(4).map(|v| Rect(v[0], v[1], v[2], v[3]))?,
        "roundRect" => n(5).map(|v| RoundRect(v[0], v[1], v[2], v[3], v[4]))?,
        "translate" => n(2).map(|v| Translate(v[0], v[1]))?,
        "scale" => n(2).map(|v| Scale(v[0], v[1]))?,
        "rotate" => n(1).map(|v| Rotate(v[0]))?,
        "fillText" => {
            if rest.len() != 3 {
                return None;
            }
            FillText(rest[0].as_str()?.to_owned(), rest[1].as_f64()?, rest[2].as_f64()?)
        }
        "drawImage" => {
            if rest.len() != 7 {
                return None;
            }
            let data = rest[0].as_str()?.to_owned();
            let v: Vec<f64> = rest[1..].iter().map(Json::as_f64).collect::<Option<_>>()?;
            // The sizes the Luau side allows: whole numbers 1...2048.
            let side = |s: f64| (1. ..=2048.).contains(&s) && s == s.round();
            if !side(v[0]) || !side(v[1]) {
                return None;
            }
            DrawImage(data, v[0] as usize, v[1] as usize, v[2], v[3], v[4], v[5])
        }
        _ => return None,
    })
}

#[derive(Clone)]
struct State {
    transform: Affine,
    fill: Color,
    stroke: Color,
    line_width: f64,
    alpha: f64,
    font: CanvasFont,
}

/// The commands run once with Canvas 2D semantics into the paints they make: the path is kept in
/// device space (each point mapped by the transform current when it was added), and save and
/// restore push and pop the transform and the styles. Text and images keep their transform.
pub fn replay(commands: &[Command]) -> Vec<Paint> {
    let fg = Color::Tone("fg".into(), 1.);
    let mut st = State { transform: Affine::IDENTITY, fill: fg.clone(), stroke: fg, line_width: 1., alpha: 1., font: CanvasFont::default() };
    let mut stack: Vec<State> = Vec::new();
    let mut path: Vec<Seg> = Vec::new();
    let mut paints = Vec::new();
    for c in commands {
        let t = st.transform;
        let p = |x: f64, y: f64| t.apply(x, y);
        match c {
            Command::FillStyle(s) => {
                if let Some(col) = style_color(s) {
                    st.fill = col;
                }
            }
            Command::StrokeStyle(s) => {
                if let Some(col) = style_color(s) {
                    st.stroke = col;
                }
            }
            Command::LineWidth(w) => {
                if *w > 0. && w.is_finite() {
                    st.line_width = *w;
                }
            }
            Command::Font(f) => st.font = CanvasFont::parse(f),
            Command::GlobalAlpha(a) => {
                if (0. ..=1.).contains(a) {
                    st.alpha = *a;
                }
            }
            Command::BeginPath => path.clear(),
            Command::ClosePath => {
                if !path.is_empty() {
                    path.push(Seg::Close);
                }
            }
            Command::Fill => paints.push(Paint::Fill(path.clone(), st.fill.times(st.alpha))),
            Command::Stroke => paints.push(Paint::Stroke(path.clone(), st.stroke.times(st.alpha), st.line_width * t.scale())),
            Command::Save => stack.push(st.clone()),
            Command::Restore => {
                if let Some(s) = stack.pop() {
                    st = s;
                }
            }
            Command::MoveTo(x, y) => {
                let (x, y) = p(*x, *y);
                path.push(Seg::Move(x, y));
            }
            Command::LineTo(x, y) => {
                let (x, y) = p(*x, *y);
                path.push(if path.is_empty() { Seg::Move(x, y) } else { Seg::Line(x, y) });
            }
            Command::BezierCurveTo(a, b, cc, d, x, y) => {
                let (a, b) = p(*a, *b);
                if path.is_empty() {
                    path.push(Seg::Move(a, b));
                }
                let (cc, d) = p(*cc, *d);
                let (x, y) = p(*x, *y);
                path.push(Seg::Cubic(a, b, cc, d, x, y));
            }
            Command::QuadraticCurveTo(a, b, x, y) => {
                let (a, b) = p(*a, *b);
                if path.is_empty() {
                    path.push(Seg::Move(a, b));
                }
                let (x, y) = p(*x, *y);
                path.push(Seg::Quad(a, b, x, y));
            }
            Command::Arc(x, y, r, a0, a1, ccw) => {
                if *r < 0. {
                    continue;
                }
                let (fx, fy) = p(x + r * a0.cos(), y + r * a0.sin());
                path.push(if path.is_empty() { Seg::Move(fx, fy) } else { Seg::Line(fx, fy) });
                for [c1x, c1y, c2x, c2y, ex, ey] in arc_cubics(*x, *y, *r, *a0, *a1, *ccw) {
                    let (c1x, c1y) = p(c1x, c1y);
                    let (c2x, c2y) = p(c2x, c2y);
                    let (ex, ey) = p(ex, ey);
                    path.push(Seg::Cubic(c1x, c1y, c2x, c2y, ex, ey));
                }
            }
            Command::Rect(x, y, w, h) => {
                let (x0, y0) = p(*x, *y);
                path.push(Seg::Move(x0, y0));
                for (px, py) in [(x + w, *y), (x + w, y + h), (*x, y + h)] {
                    let (px, py) = p(px, py);
                    path.push(Seg::Line(px, py));
                }
                path.push(Seg::Close);
                path.push(Seg::Move(x0, y0));
            }
            Command::RoundRect(x, y, w, h, r) => {
                let (rx, ry, rw, rh) = (x.min(x + w), y.min(y + h), w.abs(), h.abs());
                let rr = r.min(rw / 2.).min(rh / 2.).max(0.);
                for s in round_rect(rx, ry, rw, rh, rr) {
                    path.push(map_seg(&t, s));
                }
                let (x0, y0) = p(*x, *y);
                path.push(Seg::Move(x0, y0));
            }
            Command::Translate(x, y) => st.transform = t.then(&Affine([1., 0., 0., 1., *x, *y])),
            Command::Scale(x, y) => st.transform = t.then(&Affine([*x, 0., 0., *y, 0., 0.])),
            Command::Rotate(a) => st.transform = t.then(&Affine([a.cos(), a.sin(), -a.sin(), a.cos(), 0., 0.])),
            Command::FillText(text, x, y) => paints.push(Paint::Text { text: text.clone(), font: st.font, color: st.fill.times(st.alpha), transform: t, x: *x, y: *y }),
            Command::DrawImage(data, iw, ih, x, y, w, h) => paints.push(Paint::Image { data: data.clone(), width: *iw, height: *ih, rect: [*x, *y, *w, *h], transform: t, alpha: st.alpha }),
        }
    }
    paints
}

fn map_seg(t: &Affine, s: Seg) -> Seg {
    let p = |x, y| t.apply(x, y);
    match s {
        Seg::Move(x, y) => { let (x, y) = p(x, y); Seg::Move(x, y) }
        Seg::Line(x, y) => { let (x, y) = p(x, y); Seg::Line(x, y) }
        Seg::Quad(a, b, x, y) => { let (a, b) = p(a, b); let (x, y) = p(x, y); Seg::Quad(a, b, x, y) }
        Seg::Cubic(a, b, c, d, x, y) => { let (a, b) = p(a, b); let (c, d) = p(c, d); let (x, y) = p(x, y); Seg::Cubic(a, b, c, d, x, y) }
        Seg::Close => Seg::Close,
    }
}

/// The circular arc from `a0` to `a1` (clockwise on screen unless `ccw`, as Canvas 2D sweeps) as
/// cubic Béziers of at most a quarter turn each, each `[c1x, c1y, c2x, c2y, x, y]`.
pub fn arc_cubics(cx: f64, cy: f64, r: f64, a0: f64, a1: f64, ccw: bool) -> Vec<[f64; 6]> {
    use std::f64::consts::TAU;
    let mut sweep = a1 - a0;
    if !ccw {
        if sweep >= TAU { sweep = TAU } else { sweep = sweep.rem_euclid(TAU) }
        if sweep == 0. && a1 != a0 { sweep = TAU }
    } else {
        if -sweep >= TAU { sweep = -TAU } else { sweep = -((-sweep).rem_euclid(TAU)) }
        if sweep == 0. && a1 != a0 { sweep = -TAU }
    }
    let n = ((sweep.abs() / (TAU / 4.)).ceil() as usize).max(1);
    let step = sweep / n as f64;
    let k = 4. / 3. * (step / 4.).tan();
    let mut out = Vec::with_capacity(n);
    let mut a = a0;
    for _ in 0..n {
        let b = a + step;
        let (ca, sa, cb, sb) = (a.cos(), a.sin(), b.cos(), b.sin());
        out.push([cx + r * (ca - k * sa), cy + r * (sa + k * ca), cx + r * (cb + k * sb), cy + r * (sb - k * cb), cx + r * cb, cy + r * sb]);
        a = b;
    }
    if sweep == 0. { out.clear() }
    out
}

/// A rounded rectangle as segments, clockwise from its top-left corner's end.
pub fn round_rect(x: f64, y: f64, w: f64, h: f64, r: f64) -> Vec<Seg> {
    if r <= 0. {
        return vec![Seg::Move(x, y), Seg::Line(x + w, y), Seg::Line(x + w, y + h), Seg::Line(x, y + h), Seg::Close];
    }
    let k = r * 0.552_284_749_8;
    let (x1, y1) = (x + w, y + h);
    vec![
        Seg::Move(x + r, y),
        Seg::Line(x1 - r, y),
        Seg::Cubic(x1 - r + k, y, x1, y + r - k, x1, y + r),
        Seg::Line(x1, y1 - r),
        Seg::Cubic(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1),
        Seg::Line(x + r, y1),
        Seg::Cubic(x + r - k, y1, x, y1 - r + k, x, y1 - r),
        Seg::Line(x, y + r),
        Seg::Cubic(x, y + r - k, x + r - k, y, x + r, y),
        Seg::Close,
    ]
}

/// An ellipse in `[x, y, w, h]` as four cubics.
pub fn ellipse(x: f64, y: f64, w: f64, h: f64) -> Vec<Seg> {
    let (rx, ry) = (w / 2., h / 2.);
    let (cx, cy) = (x + rx, y + ry);
    let (kx, ky) = (rx * 0.552_284_749_8, ry * 0.552_284_749_8);
    vec![
        Seg::Move(cx + rx, cy),
        Seg::Cubic(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry),
        Seg::Cubic(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy),
        Seg::Cubic(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry),
        Seg::Cubic(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy),
        Seg::Close,
    ]
}

// MARK: the `ops` scene (Canvas.swift)

/// The camera: a screen point is `scene · s + (x, y)`. `auto` fits `bounds` to the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub s: f64,
    pub auto: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self { x: 0., y: 0., s: 1., auto: true }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Style {
    pub fill: Option<String>,
    pub stroke: Option<String>,
    pub width: f64,
    pub opacity: f64,
    pub radius: f64,
    pub size: f64,
    pub dash: Vec<f64>,
    pub px: bool,
    pub mono: bool,
    pub weight: String,
    pub anchor: String,
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub rpx: Option<[f64; 2]>,
    pub hpx: Option<[f64; 2]>,
    pub wpx: Option<[f64; 2]>,
    pub sizepx: Option<[f64; 2]>,
    pub maxw: Option<f64>,
    pub minx: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Op {
    pub kind: String,
    pub style: Style,
    pub nums: Vec<Vec<f64>>,
    /// Each path's segments in scene units, and its bounds `[x, y, w, h]`.
    pub paths: Vec<(Vec<Seg>, [f64; 4])>,
    pub texts: Vec<(f64, f64, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TipRun {
    pub text: String,
    pub tone: Option<String>,
    pub mono: bool,
    pub bold: bool,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub id: String,
    /// The key path of its action (`hits.3.action`), or "" when it only hovers.
    pub action: String,
    pub label: String,
    pub rect: Option<[f64; 4]>,
    pub circle: Option<[f64; 3]>,
    pub slop: f64,
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub rpx: Option<[f64; 2]>,
    pub tip: Vec<Vec<TipRun>>,
    pub double_action: String,
    pub fit: Option<[f64; 4]>,
    pub context: Map<String, Json>,
    pub double_context: Map<String, Json>,
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub ops: Vec<Op>,
    pub hits: Vec<Hit>,
    pub bounds: [f64; 4],
    pub focus: Option<[f64; 4]>,
    pub focus_key: String,
    pub focus_zoom: Option<f64>,
    pub fit_key: String,
    pub controls: bool,
    pub empty_action: String,
    pub empty_context: Map<String, Json>,
    pub size_action: String,
    pub size_known: f64,
    pub escape_action: String,
    pub enter_action: String,
    pub enter_context: Map<String, Json>,
    pub wheel_pans: bool,
}

fn nums(v: Option<&Json>) -> Vec<f64> {
    v.and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_f64).collect()
}

fn range(v: Option<&Json>) -> Option<[f64; 2]> {
    let n = nums(v);
    (n.len() == 2).then(|| [n[0], n[1]])
}

fn rect_of(v: Option<&Json>) -> Option<[f64; 4]> {
    let n = nums(v);
    (n.len() == 4).then(|| [n[0], n[1], n[2], n[3]])
}

fn obj(p: &Map<String, Json>, key: &str) -> Map<String, Json> {
    p.get(key).and_then(Json::as_object).cloned().unwrap_or_default()
}

/// `key` when `p` holds a table there for the Canvas to raise, else "".
fn action_key(p: &Map<String, Json>, key: &str) -> String {
    if matches!(p.get(key), Some(Json::Object(_))) { key.to_owned() } else { String::new() }
}

impl Scene {
    pub fn parse(p: &Map<String, Json>) -> Self {
        let s = |d: &Map<String, Json>, k: &str| d.get(k).and_then(Json::as_str).map(str::to_owned);
        let f = |d: &Map<String, Json>, k: &str| d.get(k).and_then(Json::as_f64);
        let ops = p.get("ops").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_object).map(|d| {
            let style = Style {
                fill: s(d, "fill"),
                stroke: s(d, "stroke"),
                width: f(d, "width").unwrap_or(1.),
                opacity: f(d, "opacity").unwrap_or(1.),
                radius: f(d, "radius").unwrap_or(0.),
                size: f(d, "size").unwrap_or(12.),
                dash: nums(d.get("dash")),
                px: d.get("px").and_then(Json::as_bool).unwrap_or(false),
                mono: d.get("mono").and_then(Json::as_bool).unwrap_or(false),
                weight: s(d, "weight").unwrap_or_else(|| "regular".into()),
                anchor: s(d, "anchor").unwrap_or_else(|| "start".into()),
                min_zoom: f(d, "minZoom").unwrap_or(0.),
                max_zoom: f(d, "maxZoom").unwrap_or(f64::INFINITY),
                rpx: range(d.get("rpx")),
                hpx: range(d.get("hpx")),
                wpx: range(d.get("wpx")),
                sizepx: range(d.get("sizepx")),
                maxw: f(d, "maxw"),
                minx: f(d, "minx"),
            };
            let kind = s(d, "op").unwrap_or_default();
            let mut op = Op { kind, style, nums: vec![], paths: vec![], texts: vec![] };
            let list = |k: &str| d.get(k).and_then(Json::as_array).cloned().unwrap_or_default();
            match op.kind.as_str() {
                "rect" => op.nums = list("r").iter().map(|v| nums(Some(v))).collect(),
                "circle" => op.nums = list("c").iter().map(|v| nums(Some(v))).collect(),
                "path" => op.paths = list("d").iter().filter_map(Json::as_str).map(|d| { let p = svg_path(d); let b = seg_bounds(&p); (p, b) }).collect(),
                "text" => {
                    op.texts = list("t").iter().filter_map(Json::as_array).filter_map(|a| {
                        (a.len() == 3).then_some(())?;
                        Some((a[0].as_f64()?, a[1].as_f64()?, a[2].as_str()?.to_owned()))
                    }).collect()
                }
                _ => {}
            }
            op
        }).collect();
        let hits = p.get("hits").and_then(Json::as_array).into_iter().flatten().enumerated_objects().map(|(i, d)| {
            let c = nums(d.get("circle"));
            Hit {
                id: s(d, "id").unwrap_or_default(),
                action: if matches!(d.get("action"), Some(Json::Object(_))) { format!("hits.{i}.action") } else { String::new() },
                label: s(d, "label").unwrap_or_default(),
                rect: rect_of(d.get("rect")),
                circle: (c.len() == 3).then(|| [c[0], c[1], c[2]]),
                slop: f(d, "slop").unwrap_or(0.),
                min_zoom: f(d, "minZoom").unwrap_or(0.),
                max_zoom: f(d, "maxZoom").unwrap_or(f64::INFINITY),
                rpx: range(d.get("rpx")),
                tip: d.get("tip").and_then(Json::as_array).into_iter().flatten().map(|line| match line.as_str() {
                    Some(t) => vec![TipRun { text: t.to_owned(), tone: None, mono: false, bold: false }],
                    None => line.as_array().into_iter().flatten().filter_map(Json::as_object).map(|r| TipRun {
                        text: s(r, "t").unwrap_or_default(),
                        tone: s(r, "tone"),
                        mono: r.get("mono").and_then(Json::as_bool).unwrap_or(false),
                        bold: r.get("bold").and_then(Json::as_bool).unwrap_or(false),
                    }).collect(),
                }).collect(),
                double_action: if matches!(d.get("doubleAction"), Some(Json::Object(_))) { format!("hits.{i}.doubleAction") } else { String::new() },
                fit: rect_of(d.get("fit")),
                context: obj(d, "context"),
                double_context: obj(d, "doubleContext"),
            }
        }).collect();
        let (mut focus, mut focus_key, mut focus_zoom) = (None, String::new(), None);
        if let Some(fo) = p.get("focus").and_then(Json::as_object) {
            focus = Some(rect_of(fo.get("rect")).unwrap_or([f(fo, "x").unwrap_or(0.), f(fo, "y").unwrap_or(0.), f(fo, "w").unwrap_or(1.), f(fo, "h").unwrap_or(1.)]));
            focus_key = s(fo, "key").unwrap_or_default();
            focus_zoom = f(fo, "zoom");
        }
        Self {
            ops,
            hits,
            bounds: rect_of(p.get("bounds")).unwrap_or([0., 0., 100., 100.]),
            focus,
            focus_key,
            focus_zoom,
            fit_key: s(p, "fitKey").unwrap_or_default(),
            controls: p.get("controls").and_then(Json::as_bool).unwrap_or(true),
            empty_action: action_key(p, "emptyAction"),
            empty_context: obj(p, "emptyContext"),
            size_action: action_key(p, "sizeAction"),
            size_known: f(p, "sizeKnown").unwrap_or(0.),
            escape_action: action_key(p, "escapeAction"),
            enter_action: action_key(p, "enterAction"),
            enter_context: obj(p, "enterContext"),
            wheel_pans: s(p, "wheel").as_deref() == Some("pan"),
        }
    }

    /// The camera that fits `rect` into a view `w` × `h`, with a 4% margin.
    pub fn fitted(rect: [f64; 4], w: f64, h: f64) -> Camera {
        let [rx, ry, rw, rh] = rect;
        if w <= 0. || h <= 0. || rw <= 0. || rh <= 0. {
            return Camera::default();
        }
        let s = (w / rw).min(h / rh) * 0.96;
        Camera { x: (w - rw * s) / 2. - rx * s, y: (h - rh * s) / 2. - ry * s, s, auto: false }
    }

    /// The camera now: an auto camera fits the bounds to the view.
    pub fn effective(&self, cam: Camera, w: f64, h: f64) -> Camera {
        if cam.auto { Camera { auto: true, ..Self::fitted(self.bounds, w, h) } } else { cam }
    }

    /// The camera at `zoom` with `rect` centred, never showing more empty space than the bounds have.
    pub fn focus_camera(&self, rect: [f64; 4], z: f64, w: f64, h: f64) -> Camera {
        let axis = |mid: f64, view: f64, lo: f64, hi: f64| {
            if (hi - lo) * z <= view { -lo * z } else { (-lo * z).min((view - hi * z).max(view / 2. - mid * z)) }
        };
        let [bx, by, bw, bh] = self.bounds;
        Camera { x: axis(rect[0] + rect[2] / 2., w, bx, bx + bw), y: axis(rect[1] + rect[3] / 2., h, by, by + bh), s: z, auto: false }
    }

    /// The topmost hit at screen point (`px`, `py`) under camera `c`, as its index.
    pub fn hit(&self, px: f64, py: f64, c: Camera) -> Option<usize> {
        self.hits.iter().enumerate().rev().filter(|(_, h)| c.s >= h.min_zoom && c.s <= h.max_zoom).find(|(_, h)| {
            if let Some([x, y, w, hh]) = h.rect {
                let (l, t) = (x * c.s + c.x - h.slop, y * c.s + c.y - h.slop);
                px >= l && py >= t && px < l + w * c.s + 2. * h.slop && py < t + hh * c.s + 2. * h.slop
            } else if let Some([cx, cy, r]) = h.circle {
                let (dx, dy) = (px - (cx * c.s + c.x), py - (cy * c.s + c.y));
                let r = clamp(r * c.s, h.rpx) + h.slop;
                dx * dx + dy * dy <= r * r
            } else {
                false
            }
        }).map(|(i, _)| i)
    }
}

trait EnumeratedObjects<'a> {
    fn enumerated_objects(self) -> Box<dyn Iterator<Item = (usize, &'a Map<String, Json>)> + 'a>;
}

impl<'a, I: Iterator<Item = &'a Json> + 'a> EnumeratedObjects<'a> for I {
    fn enumerated_objects(self) -> Box<dyn Iterator<Item = (usize, &'a Map<String, Json>)> + 'a> {
        Box::new(self.enumerate().filter_map(|(i, v)| v.as_object().map(|o| (i, o))))
    }
}

/// A screen size clamped to [min, max] points, or itself with no range.
pub fn clamp(v: f64, range: Option<[f64; 2]>) -> f64 {
    match range { Some([lo, hi]) => v.min(hi).max(lo), None => v }
}

/// The longest prefix of `text` that, trimmed and with a trailing "…", `measure` says fits `limit`.
pub fn cut(text: &str, limit: f64, measure: impl Fn(&str) -> f64) -> String {
    if measure(text) <= limit {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let with = |n: usize| format!("{}…", chars[..n].iter().collect::<String>().trim());
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&with(mid)) <= limit { lo = mid } else { hi = mid - 1 }
    }
    if lo == 0 { "…".into() } else { with(lo) }
}

/// SVG path data with absolute M, L, C, Q and Z, as Canvas.swift's `canvasPath` reads it.
pub fn svg_path(d: &str) -> Vec<Seg> {
    let mut out = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    let skip = |i: &mut usize| while *i < b.len() && matches!(b[*i], b' ' | b',' | b'\n' | b'\t') { *i += 1 };
    let num = |i: &mut usize| -> Option<f64> {
        skip(i);
        let start = *i;
        while *i < b.len() && (b[*i].is_ascii_digit() || matches!(b[*i], b'.' | b'-' | b'+' | b'e' | b'E')) {
            *i += 1;
        }
        std::str::from_utf8(&b[start..*i]).ok()?.parse().ok()
    };
    let mut cmd = b'M';
    loop {
        skip(&mut i);
        if i >= b.len() {
            break;
        }
        let mut last = None;
        while i < b.len() && b"MLCQZmlcqz".contains(&b[i]) {
            last = Some(b[i]);
            i += 1;
            skip(&mut i);
        }
        if let Some(c) = last { cmd = c }
        match cmd {
            b'M' => match (num(&mut i), num(&mut i)) { (Some(x), Some(y)) => { out.push(Seg::Move(x, y)); cmd = b'L' } _ => return out },
            b'L' => match (num(&mut i), num(&mut i)) { (Some(x), Some(y)) => out.push(Seg::Line(x, y)), _ => return out },
            b'C' => {
                let v: Option<Vec<f64>> = (0..6).map(|_| num(&mut i)).collect();
                match v { Some(v) => out.push(Seg::Cubic(v[0], v[1], v[2], v[3], v[4], v[5])), None => return out }
            }
            b'Q' => {
                let v: Option<Vec<f64>> = (0..4).map(|_| num(&mut i)).collect();
                match v { Some(v) => out.push(Seg::Quad(v[0], v[1], v[2], v[3])), None => return out }
            }
            b'Z' | b'z' => out.push(Seg::Close),
            _ => return out,
        }
    }
    out
}

/// The bounds `[x, y, w, h]` of every point a path names, control points included.
pub fn seg_bounds(p: &[Seg]) -> [f64; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut add = |x: f64, y: f64| { x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y) };
    for s in p {
        match *s {
            Seg::Move(x, y) | Seg::Line(x, y) => add(x, y),
            Seg::Quad(a, b, x, y) => { add(a, b); add(x, y) }
            Seg::Cubic(a, b, c, d, x, y) => { add(a, b); add(c, d); add(x, y) }
            Seg::Close => {}
        }
    }
    if x0 > x1 { [0., 0., 0., 0.] } else { [x0, y0, x1 - x0, y1 - y0] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn commands_with_wrong_arguments_drop() {
        let list = json!([["moveTo", 1, 2], ["lineTo", 1], ["arc", 0, 0, 5, 0, 1, "no"], ["set", "fillStyle", "accent"], ["drawImage", "AA", 0, 1, 0, 0, 1, 1], ["fillText", "a", 3, 4]]);
        assert_eq!(commands(list.as_array().unwrap()), vec![Command::MoveTo(1., 2.), Command::FillStyle("accent".into()), Command::FillText("a".into(), 3., 4.)]);
    }

    #[test]
    fn the_path_is_in_device_space_and_restore_pops_the_transform() {
        let c = commands(json!([["save"], ["translate", 10, 20], ["scale", 2, 2], ["beginPath"], ["moveTo", 1, 1], ["lineTo", 2, 1], ["set", "lineWidth", 3], ["stroke"], ["restore"], ["beginPath"], ["lineTo", 1, 1], ["fill"]]).as_array().unwrap());
        let p = replay(&c);
        assert_eq!(p[0], Paint::Stroke(vec![Seg::Move(12., 22.), Seg::Line(14., 22.)], Color::Tone("fg".into(), 1.), 6.));
        assert_eq!(p[1], Paint::Fill(vec![Seg::Move(1., 1.)], Color::Tone("fg".into(), 1.)));
    }

    #[test]
    fn a_full_arc_ends_where_it_starts() {
        let a = arc_cubics(0., 0., 10., 0., std::f64::consts::TAU, false);
        assert_eq!(a.len(), 4);
        let [.., x, y] = a[3];
        assert!((x - 10.).abs() < 1e-9 && y.abs() < 1e-9);
        // A quarter clockwise from 0 ends at (0, r): screen-down.
        let q = arc_cubics(0., 0., 10., 0., std::f64::consts::FRAC_PI_2, false);
        assert!((q[0][4]).abs() < 1e-9 && (q[0][5] - 10.).abs() < 1e-9);
    }

    #[test]
    fn colours_read_tones_and_css() {
        assert_eq!(style_color("accent"), Some(Color::Tone("accent".into(), 1.)));
        assert_eq!(style_color("#f00"), Some(Color::Rgba([1., 0., 0., 1.])));
        assert_eq!(style_color("rgba(255, 0, 0, 0.5)"), Some(Color::Rgba([1., 0., 0., 0.5])));
        assert_eq!(style_color("nope"), None);
    }

    #[test]
    fn the_scene_hits_the_topmost_and_names_its_action() {
        let s = Scene::parse(json!({"hits": [{"id": "a", "rect": [0, 0, 10, 10], "action": {"t": 1}}, {"id": "b", "circle": [5, 5, 2]}], "bounds": [0, 0, 10, 10]}).as_object().unwrap());
        let c = Camera { x: 0., y: 0., s: 1., auto: false };
        assert_eq!(s.hit(5., 5., c), Some(1));
        assert_eq!(s.hit(1., 1., c), Some(0));
        assert_eq!(s.hits[0].action, "hits.0.action");
        assert_eq!(s.hits[1].action, "");
        assert_eq!(s.hit(20., 20., c), None);
    }

    #[test]
    fn a_label_cuts_to_its_limit() {
        let m = |s: &str| s.chars().count() as f64;
        assert_eq!(cut("abcdef", 10., m), "abcdef");
        assert_eq!(cut("abc def", 5., m), "abc…");
        assert_eq!(cut("abcdef", 0.5, m), "…");
    }

    #[test]
    fn svg_path_reads_implicit_lines() {
        assert_eq!(svg_path("M0 0 10,0 L 10 10 Z"), vec![Seg::Move(0., 0.), Seg::Line(10., 0.), Seg::Line(10., 10.), Seg::Close]);
    }
}
