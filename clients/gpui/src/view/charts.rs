//! A reply's charts: a ```chart block holds one JSON object with the page Chart component's props,
//! `mark` (bar, line, point or area; bar when absent) and `data` [{x, y, series}], and is drawn in
//! the code block's frame with gpui-component's plot layer, bars and areas stacked by series as
//! Swift Charts stacks them. A block that does not parse keeps its source with the reason under it.
//! `/state` lists every chart a reply drew under `chat_charts`, by agent.
use super::*;
use gpui_component::plot::{IntoPlot, Plot, label::{PlotLabel, Text}};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A chart's spec parsed once, or the parse error.
type Parsed = Rc<Result<Rc<Spec>, String>>;

/// The plot's height in points, legend and header aside.
pub const HEIGHT: f32 = 200.;

/// The series colours, in the order series first appear.
const PALETTE: [u32; 6] = [0x7fb89a, 0x6aa6d6, 0xd9a55b, 0xc47fb8, 0xd07a6a, 0x9a8fd6];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Bar,
    Line,
    Point,
    Area,
}

impl Mark {
    fn name(self) -> &'static str {
        match self {
            Mark::Bar => "bar",
            Mark::Line => "line",
            Mark::Point => "point",
            Mark::Area => "area",
        }
    }
}

/// A parsed chart: its mark, its x categories and series in first-seen order, and each
/// (x, series) value, summed when a row repeats one.
#[derive(Debug, PartialEq)]
pub struct Spec {
    pub mark: Mark,
    pub xs: Vec<String>,
    pub series: Vec<String>,
    pub values: BTreeMap<(usize, usize), f64>,
    pub rows: usize,
}

/// Whether a fenced block's language names a chart.
pub fn is_chart(language: Option<&str>) -> bool {
    language.is_some_and(|l| l.trim().eq_ignore_ascii_case("chart"))
}

/// A JSON value as the text a category shows: a string as written, a number without a trailing `.0`.
fn label(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.as_f64().map(|f| fmt(f, if f.fract() == 0. { 1. } else { 0.01 })).unwrap_or_else(|| n.to_string())),
        _ => None,
    }
}

/// The chart in `source`, or why it cannot be drawn. The reasons match WheelKit's `ChartFence`.
pub fn parse(source: &str) -> Result<Spec, String> {
    let v: Value = serde_json::from_str(source).map_err(|_| "it is not JSON".to_owned())?;
    let Value::Object(o) = v else { return Err("it must be one JSON object with mark and data".into()) };
    let mark = match o.get("mark") {
        None | Some(Value::Null) => Mark::Bar,
        Some(Value::String(m)) => match m.as_str() {
            "bar" => Mark::Bar,
            "line" => Mark::Line,
            "point" => Mark::Point,
            "area" => Mark::Area,
            other => return Err(format!("mark \"{other}\" is not bar, line, point or area")),
        },
        Some(_) => return Err("mark must be bar, line, point or area".into()),
    };
    let Some(Value::Array(data)) = o.get("data") else { return Err("data must be a list of {x, y, series} rows".into()) };
    if data.is_empty() {
        return Err("data has no rows".into());
    }
    let (mut xs, mut series, mut values) = (Vec::<String>::new(), Vec::<String>::new(), BTreeMap::new());
    for (i, row) in data.iter().enumerate() {
        let n = i + 1;
        let x = row.get("x").and_then(label).ok_or_else(|| format!("row {n} has no x"))?;
        let y = row.get("y").and_then(Value::as_f64).ok_or_else(|| format!("row {n} has no number y"))?;
        let s = match row.get("series") {
            None | Some(Value::Null) => String::new(),
            Some(v) => label(v).ok_or_else(|| format!("row {n}'s series is not text"))?,
        };
        let xi = xs.iter().position(|e| *e == x).unwrap_or_else(|| {
            xs.push(x);
            xs.len() - 1
        });
        let si = series.iter().position(|e| *e == s).unwrap_or_else(|| {
            series.push(s);
            series.len() - 1
        });
        *values.entry((xi, si)).or_insert(0.) += y;
    }
    Ok(Spec { mark, xs, series, values, rows: data.len() })
}

/// `v` written with as many decimals as `step` needs.
fn fmt(v: f64, step: f64) -> String {
    let decimals = if step >= 1. { 0 } else { (-step.log10()).ceil().clamp(0., 6.) as usize };
    let s = format!("{v:.decimals$}");
    if s == "-0" { "0".into() } else { s }
}

impl Spec {
    /// Each x's (bottom, top) per series: stacked for bars and areas, from zero for lines and points.
    fn spans(&self) -> BTreeMap<(usize, usize), (f64, f64)> {
        let mut spans = BTreeMap::new();
        for xi in 0..self.xs.len() {
            let (mut up, mut down) = (0., 0.);
            for si in 0..self.series.len() {
                let Some(&v) = self.values.get(&(xi, si)) else { continue };
                let stacked = matches!(self.mark, Mark::Bar | Mark::Area);
                let span = if !stacked {
                    (0., v)
                } else if v >= 0. {
                    up += v;
                    (up - v, up)
                } else {
                    down += v;
                    (down - v, down)
                };
                spans.insert((xi, si), span);
            }
        }
        spans
    }

    /// The y axis: its low and high ends and its tick step, a 1, 2 or 5 times a power of ten
    /// giving about four steps, always including zero.
    pub fn axis(&self) -> (f64, f64, f64) {
        let spans = self.spans();
        let lo = spans.values().map(|s| s.0.min(s.1)).fold(0., f64::min);
        let hi = spans.values().map(|s| s.0.max(s.1)).fold(0., f64::max);
        let range = if hi > lo { hi - lo } else { 1. };
        let raw = range / 4.;
        let mag = 10f64.powf(raw.log10().floor());
        let step = [1., 2., 5., 10.].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10. * mag);
        ((lo / step).floor() * step, (hi / step).ceil() * step, step)
    }

    /// What `/state` lists for the chart.
    fn json(&self) -> Value {
        json!({"mark": self.mark.name(), "rows": self.rows, "x": self.xs, "series": self.series})
    }
}

/// The plot: axis, grid and marks, painted into its bounds.
#[derive(IntoPlot)]
pub(super) struct ChartPlot {
    spec: Rc<Spec>,
}

/// The plot of `spec` for a plugin pane's Chart.
pub(super) fn plot(spec: Rc<Spec>) -> ChartPlot {
    ChartPlot { spec }
}

/// Series `si`'s colour, as its legend shows it.
pub(super) fn series_color(si: usize) -> gpui::Hsla {
    color(si, 1.)
}

fn color(si: usize, alpha: f32) -> gpui::Hsla {
    let mut c: gpui::Hsla = rgb(PALETTE[si % PALETTE.len()]).into();
    c.a = alpha;
    c
}

impl Plot for ChartPlot {
    fn paint(&mut self, bounds: gpui::Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let spec = &self.spec;
        let (lo, hi, step) = spec.axis();
        let o = bounds.origin;
        let (left, right, top) = (44., f32::from(bounds.size.width) - 8., 8.);
        let bottom = f32::from(bounds.size.height) - 18.;
        let y_of = |v: f64| bottom - ((v - lo) / (hi - lo).max(f64::EPSILON)) as f32 * (bottom - top);
        let at = |x: f32, y: f32| gpui::point(o.x + px(x), o.y + px(y));
        let dim: gpui::Hsla = rgb(0x8fa89b).into();
        let mut labels = Vec::new();
        let mut v = lo;
        while v <= hi + step / 2. {
            let y = y_of(v);
            let mut grid = gpui::PathBuilder::stroke(px(1.));
            grid.move_to(at(left, y));
            grid.line_to(at(right, y));
            if let Ok(path) = grid.build() {
                window.paint_path(path, rgb(if v == 0. { 0x3d5247 } else { 0x22302a }));
            }
            labels.push(Text::new(fmt(v, step), gpui::point(px(left - 6.), px(y - 6.)), dim).align(gpui::TextAlign::Right));
            v += step;
        }
        let n = spec.xs.len().max(1);
        let band = (right - left) / n as f32;
        let centre = |xi: usize| left + (xi as f32 + 0.5) * band;
        let every = ((n as f32 * 44.) / (right - left)).ceil().max(1.) as usize;
        for (xi, x) in spec.xs.iter().enumerate().filter(|(i, _)| i % every == 0) {
            labels.push(Text::new(x.clone(), gpui::point(px(centre(xi)), px(bottom + 4.)), dim).align(gpui::TextAlign::Center));
        }
        let spans = spec.spans();
        match spec.mark {
            Mark::Bar => {
                let w = (band * 0.6).max(1.);
                for (&(xi, si), &(a, b)) in &spans {
                    let (y0, y1) = (y_of(a).min(y_of(b)), y_of(a).max(y_of(b)));
                    window.paint_quad(gpui::fill(gpui::Bounds::new(at(centre(xi) - w / 2., y0), gpui::size(px(w), px((y1 - y0).max(1.)))), color(si, 1.)));
                }
            }
            Mark::Point => {
                for (&(xi, si), &(_, b)) in &spans {
                    let r = 4.;
                    window.paint_quad(gpui::quad(gpui::Bounds::new(at(centre(xi) - r, y_of(b) - r), gpui::size(px(2. * r), px(2. * r))), px(r), color(si, 1.), px(0.), color(si, 1.), gpui::BorderStyle::default()));
                }
            }
            Mark::Line | Mark::Area => {
                for si in 0..spec.series.len() {
                    let pts: Vec<(usize, f64, f64)> = (0..spec.xs.len()).filter_map(|xi| spans.get(&(xi, si)).map(|&(a, b)| (xi, a, b))).collect();
                    if pts.is_empty() {
                        continue;
                    }
                    if spec.mark == Mark::Area && pts.len() > 1 {
                        let mut fill = gpui::PathBuilder::fill();
                        fill.move_to(at(centre(pts[0].0), y_of(pts[0].2)));
                        for p in &pts[1..] {
                            fill.line_to(at(centre(p.0), y_of(p.2)));
                        }
                        for p in pts.iter().rev() {
                            fill.line_to(at(centre(p.0), y_of(p.1)));
                        }
                        fill.close();
                        if let Ok(path) = fill.build() {
                            window.paint_path(path, color(si, 0.45));
                        }
                    }
                    let mut line = gpui::PathBuilder::stroke(px(2.));
                    line.move_to(at(centre(pts[0].0), y_of(pts[0].2)));
                    for p in &pts[1..] {
                        line.line_to(at(centre(p.0), y_of(p.2)));
                    }
                    if pts.len() == 1 {
                        let r = 3.;
                        window.paint_quad(gpui::quad(gpui::Bounds::new(at(centre(pts[0].0) - r, y_of(pts[0].2) - r), gpui::size(px(2. * r), px(2. * r))), px(r), color(si, 1.), px(0.), color(si, 1.), gpui::BorderStyle::default()));
                    } else if let Ok(path) = line.build() {
                        window.paint_path(path, color(si, 1.));
                    }
                }
            }
        }
        PlotLabel::new(labels).paint(&bounds, window, cx);
    }
}

/// One agent's drawn charts, by (row, nth chart block in it).
type Shown = BTreeMap<(usize, usize), Value>;

/// Every chart the transcripts parsed, by source, and what each reply showed.
#[derive(Default)]
pub struct Charts {
    parsed: RefCell<HashMap<String, Parsed>>,
    shown: RefCell<HashMap<String, Shown>>,
}

impl Charts {
    fn spec(&self, source: &str) -> Rc<Result<Rc<Spec>, String>> {
        let mut parsed = self.parsed.borrow_mut();
        if parsed.len() > 64 {
            parsed.clear();
        }
        parsed.entry(source.to_owned()).or_insert_with(|| Rc::new(parse(source).map(Rc::new))).clone()
    }

    /// Every chart each agent's replies drew, as `/state` lists them.
    pub fn json(&self) -> Value {
        Value::Object(self.shown.borrow().iter().map(|(agent, shown)| (agent.clone(), Value::Array(shown.values().cloned().collect()))).collect())
    }
}

/// The element a closed chart block in a reply draws (an open one, still streaming, stays code):
/// the chart in the code block's frame with a legend when it has named series, or the reason it
/// cannot be drawn, under which the caller draws the source as a code block.
pub(super) fn chart_el(code: &str, texts: &mut Texts, cx: &mut Context<Client>) -> Result<gpui::Div, String> {
    let parsed = texts.charts.spec(code);
    let nth = texts.chart;
    texts.chart += 1;
    let mut entry = json!({"row": texts.row, "block": nth, "drawn": false, "height": 0, "error": null});
    let el = match &*parsed {
        Ok(spec) => {
            if let (Value::Object(more), Some(e)) = (spec.json(), entry.as_object_mut()) {
                e.extend(more);
            }
            entry["drawn"] = json!(true);
            entry["height"] = json!(HEIGHT);
            let copied = code.to_owned();
            let id = format!("copy-chart-{}-{nth}", texts.row);
            let mut legend = div().flex().flex_wrap().gap_3().px_2().pb_2().text_xs().text_color(rgb(0x8fa89b));
            for (si, s) in spec.series.iter().enumerate().filter(|(_, s)| !s.is_empty()) {
                legend = legend.child(div().flex().items_center().gap_1().child(div().size(px(8.)).rounded_sm().bg(color(si, 1.))).child(s.clone()));
            }
            Ok(div().rounded_sm().bg(rgb(CODE_BG)).border_1().border_color(rgb(0x2c3d34)).flex().flex_col().overflow_hidden()
                .child(div().h(px(20.)).px_2().flex().items_center().justify_between().bg(rgb(CODE_HEADER)).text_xs().text_color(rgb(0x8fa89b))
                    .child("chart")
                    .child(div().id(gpui::ElementId::Name(id.into())).px_1().rounded_sm().cursor_pointer().bg(rgb(0x22302a)).child("Copy")
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                            cx.stop_propagation();
                        }))))
                .child(div().p_2().h(px(HEIGHT)).w_full().child(ChartPlot { spec: spec.clone() }))
                .child(legend))
        }
        Err(error) => {
            entry["error"] = json!(error);
            Err(error.clone())
        }
    };
    texts.charts.shown.borrow_mut().entry(texts.agent.clone()).or_default().insert((texts.row, nth), entry);
    el
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stack_by_series_and_bad_blocks_say_why() {
        let spec = parse(r#"{"mark":"bar","data":[{"x":"Mon","y":3,"series":"a"},{"x":"Mon","y":2,"series":"b"},{"x":"Tue","y":5,"series":"a"},{"x":2026,"y":-1}]}"#).unwrap();
        assert_eq!((spec.mark, spec.xs.clone(), spec.series.clone(), spec.rows), (Mark::Bar, vec!["Mon".into(), "Tue".into(), "2026".into()], vec!["a".into(), "b".into(), String::new()], 4));
        assert_eq!(spec.spans()[&(0, 1)], (3., 5.));
        assert_eq!(spec.axis(), (-2., 6., 2.));
        let (lo, hi, step) = parse(r#"{"data":[{"x":"a","y":0.25}]}"#).unwrap().axis();
        assert!(lo == 0. && (hi - 0.3).abs() < 1e-9 && (step - 0.1).abs() < 1e-9, "{lo} {hi} {step}");
        assert_eq!(parse("{nope").unwrap_err(), "it is not JSON");
        assert_eq!(parse("[1]").unwrap_err(), "it must be one JSON object with mark and data");
        assert_eq!(parse(r#"{"mark":"pie","data":[]}"#).unwrap_err(), "mark \"pie\" is not bar, line, point or area");
        assert_eq!(parse(r#"{"mark":"line"}"#).unwrap_err(), "data must be a list of {x, y, series} rows");
        assert_eq!(parse(r#"{"data":[]}"#).unwrap_err(), "data has no rows");
        assert_eq!(parse(r#"{"data":[{"x":"a","y":1},{"x":"b","y":"2"}]}"#).unwrap_err(), "row 2 has no number y");
        assert_eq!(parse(r#"{"data":[{"y":1}]}"#).unwrap_err(), "row 1 has no x");
        assert!(is_chart(Some("Chart")) && !is_chart(Some("json")) && !is_chart(None));
        assert_eq!((fmt(0.30000000000000004, 0.1), fmt(-0., 1.), fmt(12., 2.)), ("0.3".into(), "0".into(), "12".into()));
    }
}
