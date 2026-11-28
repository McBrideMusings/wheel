//! The rest of WheelUI's components drawn by GPUI (wheel-4d1o.3): Table, Form, Section,
//! LabeledContent, Grid, ZStack, the presented Modal, Alert and ConfirmationDialog, Popover, the
//! value controls (ChoicePicker, CheckBox, Stepper, Slider, ColorPicker, DateTimeInput), Tabs,
//! Menu, Toolbar, the navigation and split views, ViewThatFits, Gauge, Chart, ShareLink,
//! PasteButton and the Unsupported placeholder, with the defaults `SystemControls.swift`,
//! `Basics.swift` and `Table.swift` give them. Canvas and the embeds, VideoStream among them, draw elsewhere.
use super::plugin::{Entry, Fit, Pick};
use super::plugin_draw::{ACCENT, BODY, Draw, Env, SECONDARY, el_id, greedy, pane_color};
use super::*;
use gpui::{AvailableSpace, GlobalElementId, InspectorElementId, LayoutId, Size, Style};
use serde_json::{Value as Json, json};
use wheel_luau::tree::{ViewNode, text as json_text};

const ROW_H: f32 = 24.;
const CONTROL_BG: u32 = 0xffffff2e;
const SELECTED: u32 = 0x0058d0;

/// Which of a ViewThatFits' children, measured at their ideal sizes, fits the space offered:
/// the first that does along the axes `in` names, else the last.
pub(super) fn pick(sizes: &[(f32, f32)], width: Option<f32>, height: Option<f32>, axis: &str) -> usize {
    let (across, down) = match axis { "horizontal" => (true, false), "vertical" => (false, true), _ => (true, true) };
    sizes
        .iter()
        .position(|&(w, h)| (!across || width.is_none_or(|o| w <= o + 0.5)) && (!down || height.is_none_or(|o| h <= o + 0.5)))
        .unwrap_or(sizes.len().saturating_sub(1))
}

/// Text the way Finder orders it (`localizedStandardCompare`): case folded, runs of digits by value.
pub(super) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s.trim_start_matches('0').to_owned()
                };
                let (m, n) = (run(&mut x), run(&mut y));
                let o = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if o.is_ne() {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o.is_ne() {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// A Table row's cell for column `key`, with its index among the row's cells.
fn cell_of<'a>(row: &'a ViewNode, key: &str) -> Option<(usize, &'a serde_json::Map<String, Json>)> {
    row.props.get("cells")?.as_array()?.iter().enumerate().filter_map(|(i, c)| c.as_object().map(|c| (i, c))).find(|(_, c)| c.get("key").and_then(Json::as_str) == Some(key))
}

/// Orders two rows by column `key`'s cells: numbers by value, text the way Finder does.
fn row_cmp(a: &ViewNode, b: &ViewNode, key: &str) -> std::cmp::Ordering {
    let value = |r: &ViewNode| -> (Option<f64>, String) {
        let c = cell_of(r, key).map(|(_, c)| c);
        match c.and_then(|c| c.get("sort")) {
            Some(Json::Number(n)) => (n.as_f64(), String::new()),
            Some(Json::String(s)) => (None, s.clone()),
            _ => (None, c.and_then(|c| c.get("text")).map(json_text).unwrap_or_default()),
        }
    };
    match (value(a), value(b)) {
        ((Some(m), _), (Some(n), _)) => m.partial_cmp(&n).unwrap_or(std::cmp::Ordering::Equal),
        ((_, s), (_, t)) => natural_cmp(&s, &t),
    }
}

/// One line of a Table: a section's title, or a row by its node id.
#[derive(Clone)]
pub(super) enum Line {
    Title(String),
    Row(String),
}

/// The lines of Table `n`'s sections, each section's rows sorted by `sort` when the view sorts.
pub(super) fn table_lines(tree: &wheel_luau::tree::ViewTree, n: &ViewNode, sort: Option<&(String, bool)>) -> Vec<Line> {
    let mut lines = vec![];
    for sec in n.props.get("sections").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_object) {
        let mut rows: Vec<&ViewNode> = sec.get("rows").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_str).filter_map(|id| tree.node(id)).collect();
        if let Some((key, asc)) = sort {
            rows.sort_by(|a, b| if *asc { row_cmp(a, b, key) } else { row_cmp(b, a, key) });
        }
        if let Some(title) = sec.get("title").and_then(Json::as_str).filter(|t| !t.is_empty()) {
            lines.push(Line::Title(format!("{title}  {}", sec.get("count").and_then(Json::as_f64).unwrap_or(0.) as i64)));
        }
        lines.extend(rows.into_iter().map(|r| Line::Row(r.id.clone())));
    }
    lines
}

fn tone(cell: &serde_json::Map<String, Json>, dim: bool) -> u32 {
    let name = match cell.get("tone").and_then(Json::as_str) {
        Some("ok") => "green",
        Some("warn") => "orange",
        Some("bad") => "red",
        Some("accent" | "focus") => "accentColor",
        Some("purple") => "purple",
        Some("muted" | "faint" | "todo") => "secondary",
        _ if dim => "secondary",
        _ => "primary",
    };
    pane_color(name).unwrap_or(0xffffffd9)
}

/// `#RRGGBB` as RGBA, or None.
fn hex(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#')?;
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten().map(|c| (c << 8) | 0xff)
}

/// A DateTimeInput's value as the Mac's numeric date picker shows it: `11/6/2026`, `2:30 PM`.
pub(super) fn date_text(value: &str, date: bool, time: bool) -> String {
    let (d, t) = match value.split_once('T') { Some((d, t)) => (d, t), None if value.contains(':') => ("", value), None => (value, "") };
    let mut out = vec![];
    if date {
        let p: Vec<&str> = d.split('-').collect();
        if let [y, m, day] = p[..] {
            out.push(format!("{}/{}/{y}", m.trim_start_matches('0'), day.trim_start_matches('0')));
        }
    }
    if time {
        let p: Vec<u32> = t.split(':').filter_map(|s| s.parse().ok()).collect();
        if let [h, m, ..] = p[..] {
            let (h12, ampm) = match h { 0 => (12, "AM"), 1..=11 => (h, "AM"), 12 => (12, "PM"), _ => (h - 12, "PM") };
            out.push(format!("{h12}:{m:02} {ampm}"));
        }
    }
    out.join(", ")
}

impl Draw<'_> {
    fn part(&self, n: &ViewNode, i: usize) -> Option<&ViewNode> {
        n.parts().get(i).and_then(|id| self.tree.node(id))
    }

    fn embedded(&self, n: &ViewNode, key: &str) -> Option<&ViewNode> {
        n.embedded.get(key).and_then(|r| self.tree.node(&r.id))
    }

    fn label(text: impl Into<gpui::SharedString>) -> gpui::Div {
        div().whitespace_nowrap().child(text.into())
    }

    /// A component outside `plugin_draw.rs`'s basic set, or None for a name nothing draws.
    pub(super) fn control(&self, n: &ViewNode, inner: Env, outer: Env, cx: &mut Context<Client>) -> Option<gpui::AnyElement> {
        let row = |inner| Env { in_row: true, ..inner };
        let col = |inner| Env { in_row: false, ..inner };
        Some(match n.component.as_str() {
            "ViewThatFits" => self.fits(n, col(inner), outer, cx),
            "Table" => self.table(n, outer, cx),
            "Form" => {
                let mut f = greedy(div().flex().flex_col().gap(px(10.)), outer);
                for k in self.kids(n.parts()) {
                    f = f.child(self.node(k, Env { in_form: true, ..col(inner) }, cx));
                }
                f.into_any_element()
            }
            "Section" => {
                let mut s = greedy(div().flex().flex_col().gap(px(6.)), outer);
                let header = match self.embedded(n, "header") {
                    Some(h) => Some(self.node(h, col(inner), cx)),
                    None => n.string("title").filter(|t| !t.is_empty()).map(|t| div().font_weight(FontWeight::SEMIBOLD).child(t.to_owned()).into_any_element()),
                };
                s = s.children(header);
                let mut body = div().flex().flex_col().gap(px(6.));
                if inner.in_form {
                    body = body.p(px(10.)).rounded(px(8.)).bg(rgba(0xffffff0d)).border_1().border_color(rgba(0xffffff14));
                }
                for k in self.kids(n.parts()) {
                    body = body.child(self.node(k, col(inner), cx));
                }
                s = s.child(body);
                if let Some(f) = self.embedded(n, "footer") {
                    s = s.child(div().text_size(px(11.)).text_color(rgba(SECONDARY)).child(self.node(f, col(inner), cx)));
                }
                s.into_any_element()
            }
            "LabeledContent" => {
                let mut r = greedy(div().flex().flex_row().items_center().gap(px(8.)), outer).child(Self::label(n.string("title").unwrap_or("").to_owned())).child(div().flex_1());
                r = match n.props.get("value") {
                    Some(v) if v.is_string() => r.child(div().text_color(rgba(SECONDARY)).child(json_text(v))),
                    _ => r.children(self.kids(n.parts()).into_iter().map(|k| self.node(k, row(inner), cx))),
                };
                r.into_any_element()
            }
            "Grid" => self.grid(n, inner, cx),
            // A GridRow outside a Grid lays its parts out in a row, as SwiftUI's does.
            "GridRow" => div().flex().flex_row().items_center().gap(px(8.)).children(self.kids(n.parts()).into_iter().map(|k| self.node(k, row(inner), cx))).into_any_element(),
            "ZStack" => {
                let (h, v) = align(n.string("alignment").unwrap_or("center"));
                let mut z = div().grid().grid_cols(1).grid_rows(1);
                for k in self.kids(n.parts()) {
                    let cell = div().col_start(1).row_start(1).flex().flex_col();
                    z = z.child(v(h(cell)).child(self.node(k, col(inner), cx)));
                }
                z.into_any_element()
            }
            "Modal" => {
                // Presented, it draws in the layer over the page (`overlays`); with a trigger, the trigger opens a sheet there.
                if n.bool("presented").is_some() || n.parts().len() < 2 {
                    return Some(div().into_any_element());
                }
                let trigger = self.part(n, 0).map(|t| self.node(t, col(inner), cx));
                let (pane, id) = (self.pane.to_owned(), n.id.clone());
                div().id(el_id(self.pane, &n.id, "trigger")).children(trigger).on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_toggle_open(&pane, &id);
                    cx.notify();
                })).into_any_element()
            }
            "Popover" => self.popover(n, inner, cx),
            // Script-owned: `presented` draws its card in the layer over the page.
            "Alert" | "ConfirmationDialog" => div().into_any_element(),
            "ChoicePicker" => self.choice(n, inner, outer, cx),
            "CheckBox" => self.check_box(n, inner, cx),
            "Stepper" => self.stepper(n, inner, cx),
            "Slider" => self.slider(n, outer, cx),
            "ColorPicker" => {
                let swatch = n.value().and_then(Json::as_str).and_then(hex).unwrap_or((ACCENT << 8) | 0xff);
                // hack: draws the colour well only; NSColorPanel is wheel-4d1o.9.
                div().flex().flex_row().items_center().gap(px(8.)).child(Self::label(n.string("title").unwrap_or("").to_owned()))
                    .child(div().w(px(44.)).h(px(22.)).p(px(3.)).rounded(px(5.)).bg(rgba(CONTROL_BG)).child(div().size_full().rounded(px(3.)).bg(rgba(swatch))))
                    .into_any_element()
            }
            "DateTimeInput" => {
                let value = n.value().map(json_text).unwrap_or_default();
                let shown = date_text(&value, n.bool("enableDate").unwrap_or(true), n.bool("enableTime").unwrap_or(false));
                // hack: shows the value only; editing it is wheel-4d1o.9.
                div().flex().flex_row().items_center().gap(px(8.)).child(Self::label(n.string("label").unwrap_or("").to_owned()))
                    .child(div().px(px(6.)).py(px(2.)).rounded(px(5.)).bg(rgba(CONTROL_BG)).whitespace_nowrap().child(shown))
                    .into_any_element()
            }
            "Tabs" => self.tabs(n, inner, outer, cx),
            "Menu" => self.menu_button(n, inner, cx),
            "Toolbar" => {
                let placement = n.string("placement").unwrap_or("automatic");
                let mut bar = div().flex().flex_row().items_center().gap(px(8.)).py(px(4.));
                bar = match placement { "principal" => bar.justify_center(), "navigation" | "cancellationAction" => bar.justify_start(), _ => bar.justify_end() };
                for k in self.kids(n.parts().into_iter().skip(1)) {
                    bar = bar.child(self.node(k, row(inner), cx));
                }
                greedy(div().flex().flex_col(), outer).child(greedy(bar, Env { in_row: false, ..outer })).children(self.part(n, 0).map(|c| self.node(c, col(inner), cx))).into_any_element()
            }
            "NavigationSplitView" => {
                let side = self.part(n, 0).map(|c| self.node(c, col(inner), cx));
                let detail = self.part(n, 1).map(|c| self.node(c, col(inner), cx));
                greedy(div().flex().flex_row().items_start(), outer)
                    .child(div().w(px(220.)).flex_none().flex().flex_col().p(px(8.)).bg(rgba(0xffffff08)).children(side))
                    .child(div().w(px(1.)).self_stretch().bg(rgba(pane_color("separator").unwrap())))
                    .child(div().flex_1().min_w_0().flex().flex_col().p(px(8.)).children(detail))
                    .into_any_element()
            }
            // The title goes to the window's toolbar, which a pane does not have.
            "NavigationStack" => greedy(div().flex().flex_col(), outer).children(self.part(n, 0).map(|c| self.node(c, col(inner), cx))).into_any_element(),
            "HSplitView" | "VSplitView" => {
                let across = n.component == "HSplitView";
                // hack: the parts share the space equally; dragging a divider is wheel-4d1o.9.
                let mut s = greedy(div().flex(), outer);
                s = if across { s.flex_row() } else { s.flex_col().flex_1().min_h_0() };
                for (i, k) in self.kids(n.parts()).into_iter().enumerate() {
                    if i > 0 {
                        let line = div().bg(rgba(pane_color("separator").unwrap()));
                        s = s.child(if across { line.w(px(1.)).self_stretch() } else { line.h(px(1.)).self_stretch() });
                    }
                    let slot = div().flex_1().min_w_0().min_h_0().flex().flex_col();
                    s = s.child(slot.child(self.node(k, Env { in_row: false, ..inner }, cx)));
                }
                s.into_any_element()
            }
            "Gauge" => self.gauge(n, outer),
            "Chart" => self.chart(n, outer),
            "Canvas" => self.canvas(n, outer, cx),
            "ShareLink" => {
                let title = n.string("title").unwrap_or("Share").to_owned();
                // hack: draws the button; the sharing picker is wheel-4d1o.9.
                self.push_button(n, "square.and.arrow.up", title, inner, None, cx)
            }
            "PasteButton" => {
                let (pane, id) = (self.pane.to_owned(), n.id.clone());
                let on = move |this: &mut Client, cx: &mut Context<Client>| {
                    let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                    if !text.is_empty() {
                        this.plugin_raise(&pane, &id, "action", &json!({"text": text}), cx);
                    }
                };
                self.push_button(n, "doc.on.clipboard", "Paste".into(), inner, Some(Box::new(on)), cx)
            }
            "Unsupported" | "AudioPlayer" => {
                let name = match n.component.as_str() { "Unsupported" => n.string("name").unwrap_or("?").to_owned(), other => other.to_owned() };
                div().flex().flex_row().items_center().gap(px(4.)).p(px(6.)).rounded(px(6.)).border_1().border_dashed().border_color(rgba(0xffffff46)).text_size(px(12.)).text_color(rgba(SECONDARY))
                    .children(title::symbol(self.symbols, "questionmark.square.dashed", 12., SECONDARY >> 8))
                    .child(format!("Unsupported component: {name}"))
                    .into_any_element()
            }
            _ => return None,
        })
    }

    /// A bordered push button with a symbol and a title; `on` runs on a press.
    #[allow(clippy::type_complexity)]
    fn push_button(&self, n: &ViewNode, symbol: &str, title: String, inner: Env, on: Option<Box<dyn Fn(&mut Client, &mut Context<Client>)>>, cx: &mut Context<Client>) -> gpui::AnyElement {
        let b = div().id(el_id(self.pane, &n.id, "push")).flex().flex_row().items_center().gap(px(4.)).px(px(8.)).py(px(3.)).rounded(px(5.)).bg(rgba(CONTROL_BG))
            .children(title::symbol(self.symbols, symbol, 12., inner.color >> 8)).child(Self::label(title));
        match on {
            Some(on) if !inner.disabled => b.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                on(this, cx);
                cx.notify();
            })).into_any_element(),
            _ => b.into_any_element(),
        }
    }

    // MARK: ViewThatFits

    fn fits(&self, n: &ViewNode, inner: Env, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let kids = self.kids(n.parts());
        if kids.is_empty() {
            return div().into_any_element();
        }
        let copy = |tag: &str, i: usize, k: &ViewNode, cx: &mut Context<Client>| div().id(el_id(self.pane, &n.id, &format!("{tag}{i}"))).flex().flex_col().child(self.node(k, inner, cx)).into_any_element();
        let measure = kids.iter().enumerate().map(|(i, k)| copy("measure", i, k, cx)).collect();
        let finals = kids.iter().enumerate().map(|(i, k)| Some(copy("fit", i, k, cx))).collect();
        Fits { id: el_id(self.pane, &n.id, "fits"), node: n.id.clone(), axis: n.string("in").unwrap_or("both").to_owned(), in_row: outer.in_row, measure, finals, cache: self.host.fits.clone() }.into_any_element()
    }

    // MARK: Table

    fn table(&self, n: &ViewNode, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let narrow = self.width < 640.;
        let cols: Vec<serde_json::Map<String, Json>> = n.props.get("columns").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_object).filter(|c| !(narrow && c.get("wide").and_then(Json::as_bool) == Some(true))).cloned().collect();
        let sortable = cols.iter().any(|c| c.get("sortable").and_then(Json::as_bool) == Some(true));
        let page_sorts = !n.action("sortAction").is_empty();
        let sort = if page_sorts {
            n.props.get("sort").and_then(Json::as_object).map(|s| (s.get("key").and_then(Json::as_str).unwrap_or("").to_owned(), s.get("asc").and_then(Json::as_bool).unwrap_or(true)))
        } else {
            self.host.sorts.get(&n.id).cloned()
        };
        let count = table_lines(self.tree, n, if page_sorts { None } else { sort.as_ref() }).len();
        let width_of = |c: &serde_json::Map<String, Json>| c.get("width").and_then(Json::as_f64).map(|w| w as f32);
        let sized = |d: gpui::Div, c: &serde_json::Map<String, Json>| match width_of(c) { Some(w) => d.w(px(w)).min_w(px(w.min(40.))), None => d.flex_1().min_w_0() };
        let mut header = div().flex().flex_row().h(px(28.)).items_center().border_b_1().border_color(rgba(pane_color("separator").unwrap())).text_size(px(12.)).text_color(rgba(SECONDARY));
        for (ci, c) in cols.iter().enumerate() {
            let key = c.get("key").and_then(Json::as_str).unwrap_or("").to_owned();
            let label = c.get("label").and_then(Json::as_str).unwrap_or("").to_owned();
            let mut h = sized(div(), c).id(el_id(self.pane, &n.id, &format!("head{ci}"))).flex().flex_row().items_center().gap(px(4.)).px(px(6.)).overflow_hidden().child(Self::label(label));
            let arrow = sort.as_ref().filter(|(k, _)| sortable && *k == key).map(|(_, asc)| if *asc { "chevron.up" } else { "chevron.down" });
            h = h.children(arrow.and_then(|a| title::symbol(self.symbols, a, 9., SECONDARY >> 8)));
            if sortable && c.get("sortable").and_then(Json::as_bool) == Some(true) {
                let (pane, id) = (self.pane.to_owned(), n.id.clone());
                h = h.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_table_sort(&pane, &id, &key, cx);
                    cx.notify();
                }));
            }
            header = header.child(h);
        }
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        let rows = gpui::uniform_list(el_id(self.pane, &n.id, "rows"), count, cx.processor(move |this: &mut Client, range: Range<usize>, _, cx| this.plugin_table_rows(&pane, &id, range, cx)))
            .flex_1().min_h_0().track_scroll(&self.host.table_scroll(&n.id));
        // A click below the last row clears a selection the page owns.
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        let rows = div().id(el_id(self.pane, &n.id, "ground")).flex_1().min_h_0().flex().flex_col().child(rows).on_click(cx.listener(move |this, _, _, cx| {
            this.plugin_table_click(&pane, &id, "", 1, false, false, cx);
            cx.notify();
        }));
        let empty = (count == 0).then(|| div().absolute().inset_0().flex().items_center().justify_center().text_color(rgba(SECONDARY)).child(n.string("empty").unwrap_or("Nothing matches the search and filters.").to_owned()));
        greedy(div().relative().flex().flex_col().flex_1().min_h(px(120.)), outer).child(header).child(rows).children(empty).child(recorded(self.pane, &format!("{}#rows", n.id), self.controls)).into_any_element()
    }

    /// Lines `range` of Table `n`, drawn for its list.
    pub(super) fn table_rows(&self, n: &ViewNode, range: Range<usize>, cx: &mut Context<Client>) -> Vec<gpui::AnyElement> {
        let narrow = self.width < 640.;
        let cols: Vec<&serde_json::Map<String, Json>> = n.props.get("columns").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_object).filter(|c| !(narrow && c.get("wide").and_then(Json::as_bool) == Some(true))).collect();
        let page_sorts = !n.action("sortAction").is_empty();
        let lines = table_lines(self.tree, n, if page_sorts { None } else { self.host.sorts.get(&n.id) });
        let selects = !n.action("selectAction").is_empty();
        let selected: Vec<&str> = n.props.get("selected").and_then(Json::as_array).into_iter().flatten().filter_map(Json::as_str).collect();
        let first = cols.first().and_then(|c| c.get("key")).and_then(Json::as_str).unwrap_or("");
        let env = Env { color: 0xffffffd9, size: BODY, disabled: false, in_row: true, in_form: false };
        let sized = |d: gpui::Div, c: &serde_json::Map<String, Json>| match c.get("width").and_then(Json::as_f64) { Some(w) => d.w(px(w as f32)).min_w(px((w as f32).min(40.))), None => d.flex_1().min_w_0() };
        let mut out = vec![];
        for ix in range {
            let Some(line) = lines.get(ix) else { break };
            let mut r = div().id(el_id(self.pane, &n.id, &format!("line{ix}"))).w_full().flex().flex_row().items_center().h(px(ROW_H)).when(ix % 2 == 1, |d| d.bg(rgba(0xffffff08)));
            match line {
                Line::Title(title) => {
                    for c in &cols {
                        let key = c.get("key").and_then(Json::as_str).unwrap_or("");
                        r = r.child(sized(div().px(px(6.)).overflow_hidden(), c).font_weight(FontWeight::SEMIBOLD).when(key == first, |d| d.child(Self::label(title.clone()))));
                    }
                }
                Line::Row(row_id) => {
                    let Some(row) = self.tree.node(row_id) else { continue };
                    let on = selects && selected.contains(&row.row_id.as_str());
                    if on {
                        r = r.bg(rgb(SELECTED)).rounded(px(4.));
                    }
                    for c in &cols {
                        r = r.child(sized(div().px(px(6.)).overflow_hidden().flex().flex_row().items_center(), c).child(self.cell(n, row, c, env, cx)));
                    }
                    let (pane, table, rid, row_node) = (self.pane.to_owned(), n.id.clone(), row.row_id.clone(), row.id.clone());
                    r = r.on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.plugin_table_click(&pane, &table, &rid, e.click_count(), e.modifiers().platform, e.modifiers().shift, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }));
                    let (pane, table) = (self.pane.to_owned(), n.id.clone());
                    r = r.on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| {
                        this.plugin_row_menu(&pane, &table, &row_node, cx);
                    }));
                }
            }
            out.push(r.into_any_element());
        }
        out
    }

    fn cell(&self, table: &ViewNode, row: &ViewNode, c: &serde_json::Map<String, Json>, env: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let key = c.get("key").and_then(Json::as_str).unwrap_or("");
        let found = cell_of(row, key);
        let mut out = div().flex().flex_row().items_center().min_w_0();
        if c.get("tree").and_then(Json::as_bool) == Some(true) {
            out = out.child(div().w(px(row.number("depth").unwrap_or(0.) as f32 * 18.)).flex_none());
            if row.bool("kids") == Some(true) {
                let (pane, id, path) = (self.pane.to_owned(), table.id.clone(), row.string("path").unwrap_or("").to_owned());
                let fold = table.action("foldAction");
                out = out.child(div().id(el_id(self.pane, &row.id, "fold")).w(px(16.)).flex_none().cursor_pointer().child(if row.bool("collapsed") == Some(true) { "▸" } else { "▾" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !fold.is_empty() {
                            this.plugin_raise(&pane, &id, fold, &json!({"path": path}), cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    })));
            } else {
                out = out.child(div().w(px(16.)).flex_none());
            }
        }
        if let Some(view) = found.and_then(|(i, _)| self.embedded(row, &format!("cells.{i}.view"))) {
            return out.child(self.node(view, env, cx)).into_any_element();
        }
        let cell = found.map(|(_, c)| c.clone()).unwrap_or_default();
        let text = cell.get("text").map(json_text).unwrap_or_default();
        let mut t = div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgba(tone(&cell, row.bool("dim") == Some(true)))).child(text);
        t = if cell.get("mono").and_then(Json::as_bool) == Some(true) {
            t.text_size(px(11.)).font_family("Menlo")
        } else if cell.get("chip").and_then(Json::as_bool) == Some(true) {
            t.text_size(px(12.)).font_weight(FontWeight::SEMIBOLD)
        } else {
            t.text_size(px(13.))
        };
        out.child(t).into_any_element()
    }

    // MARK: Grid

    fn grid(&self, n: &ViewNode, inner: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let kids = self.kids(n.parts());
        let cols = kids.iter().map(|k| if k.component == "GridRow" { k.parts().len() } else { 1 }).max().unwrap_or(1).max(1);
        let (h, v) = align(n.string("alignment").unwrap_or("leading"));
        let hs = n.number("horizontalSpacing").unwrap_or(8.) as f32;
        let vs = n.number("verticalSpacing").unwrap_or(8.) as f32;
        let mut g = div().grid().grid_cols_max_content(cols as u16).gap_x(px(hs)).gap_y(px(vs));
        for k in kids {
            if k.component == "GridRow" {
                let parts = self.kids(k.parts());
                let len = parts.len();
                for (i, p) in parts.into_iter().enumerate() {
                    let cell = v(h(div().flex().flex_col()));
                    // The last cell of a short row spans what is left, as SwiftUI leaves the rest empty.
                    let cell = if i + 1 == len && len < cols { cell.col_span((cols - i) as u16) } else { cell };
                    g = g.child(cell.child(self.node(p, Env { in_row: false, ..inner }, cx)));
                }
            } else {
                g = g.child(div().col_span_full().child(self.node(k, Env { in_row: false, ..inner }, cx)));
            }
        }
        g.into_any_element()
    }

    // MARK: Popover

    fn popover(&self, n: &ViewNode, inner: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let label = self.part(n, 0).map(|l| self.node(l, Env { in_row: false, ..inner }, cx));
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        let mut b = div().relative().child(div().id(el_id(self.pane, &n.id, "popover")).cursor_pointer().text_color(rgb(ACCENT)).children(label).on_click(cx.listener(move |this, _, _, cx| {
            this.plugin_toggle_open(&pane, &id);
            cx.notify();
        })));
        if self.host.open.contains(&n.id) && let Some(content) = self.part(n, 1) {
            let card = div().id(el_id(self.pane, &n.id, "popover-card")).occlude().p(px(12.)).rounded(px(10.)).bg(rgb(0x323232)).border_1().border_color(rgba(0xffffff1f)).shadow_lg()
                .flex().flex_col().items_start().child(self.node(content, Env { in_row: false, ..inner }, cx)).child(recorded(self.pane, &format!("{}#popover", n.id), self.controls));
            let edge = n.string("arrowEdge").unwrap_or("bottom");
            let at = div().absolute();
            let at = match edge { "top" => at.bottom_full().left_0(), "leading" => at.right_full().top_0(), "trailing" => at.left_full().top_0(), _ => at.top_full().left_0() };
            b = b.child(at.child(deferred(anchored().child(div().pt(px(6.)).child(card))).with_priority(1)));
        }
        b.into_any_element()
    }

    // MARK: value controls

    fn check_box(&self, n: &ViewNode, inner: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let on = n.value().and_then(Json::as_bool).unwrap_or(false);
        let mut boxed = div().size(px(14.)).rounded(px(3.)).flex().items_center().justify_center();
        boxed = if on { boxed.bg(rgb(ACCENT)).children(title::symbol(self.symbols, "checkmark", 9., 0xffffff)) } else { boxed.bg(rgba(CONTROL_BG)).border_1().border_color(rgba(0xffffff40)) };
        let c = div().id(el_id(self.pane, &n.id, "check")).flex().flex_row().items_center().gap(px(6.)).child(boxed).child(Self::label(n.string("label").unwrap_or("").to_owned()));
        if inner.disabled {
            return c.opacity(0.4).into_any_element();
        }
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        c.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
            this.plugin_set(&pane, &id, Json::Bool(!on), cx);
            cx.notify();
        })).into_any_element()
    }

    fn stepper(&self, n: &ViewNode, inner: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let value = n.value().and_then(Json::as_f64).unwrap_or(0.);
        let step = n.number("step").unwrap_or(1.);
        let range = n.number("min").zip(n.number("max"));
        let half = |dir: f64, sym: &str, cx: &mut Context<Client>| {
            let next = value + dir * step;
            let next = match range { Some((lo, hi)) => next.clamp(lo, hi), None => next };
            let (pane, id) = (self.pane.to_owned(), n.id.clone());
            div().id(el_id(self.pane, &n.id, if dir > 0. { "up" } else { "down" })).h(px(11.)).w(px(14.)).flex().items_center().justify_center().cursor_pointer()
                .children(title::symbol(self.symbols, sym, 7., inner.color >> 8))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_set(&pane, &id, json!(next), cx);
                    cx.notify();
                }))
        };
        let arrows = div().flex().flex_col().rounded(px(4.)).bg(rgba(CONTROL_BG)).child(half(1., "chevron.up", cx)).child(half(-1., "chevron.down", cx));
        div().flex().flex_row().items_center().gap(px(6.)).child(Self::label(n.string("title").unwrap_or("").to_owned())).child(arrows).into_any_element()
    }

    fn slider(&self, n: &ViewNode, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let lo = n.number("min").unwrap_or(0.);
        let hi = n.number("max").unwrap_or(100.);
        let value = n.value().and_then(Json::as_f64).unwrap_or(lo);
        let frac = if hi > lo { ((value - lo) / (hi - lo)).clamp(0., 1.) as f32 } else { 0. };
        let track_id = format!("{}#track", n.id);
        let knob = div().absolute().top(px(-6.)).left(gpui::relative(frac)).ml(px(-8.)).size(px(16.)).rounded_full().bg(rgb(0xe6e6e6)).shadow_sm();
        let track = div().relative().h(px(4.)).rounded(px(2.)).bg(rgba(0xffffff1a)).child(div().h_full().rounded(px(2.)).bg(rgb(ACCENT)).w(gpui::relative(frac))).child(knob);
        let (pane, id, tid) = (self.pane.to_owned(), n.id.clone(), track_id.clone());
        let set = move |this: &mut Client, x: Pixels, cx: &mut Context<Client>| {
            let bounds = this.controls.borrow().iter().rev().find(|c| c.agent == pane && c.id == tid).map(|c| c.bounds);
            if let Some(b) = bounds.filter(|b| b.size.width > px(0.)) {
                let f = ((x - b.origin.x) / b.size.width).clamp(0., 1.) as f64;
                this.plugin_set(&pane, &id, json!(lo + f * (hi - lo)), cx);
                cx.notify();
            }
        };
        let set = Rc::new(set);
        let (down, moved) = (set.clone(), set);
        // A drag keeps tracking outside the slider until the button comes up, as AppKit's does.
        let area = div().id(el_id(self.pane, &n.id, "slider")).flex_1().min_w(px(60.)).py(px(8.)).child(track.child(recorded(self.pane, &track_id, self.controls)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| down(this, e.position.x, cx)))
            .on_drag(SliderDrag, |_, _, _, cx| cx.new(|_| gpui::Empty))
            .on_drag_move(cx.listener(move |this, e: &gpui::DragMoveEvent<SliderDrag>, _, cx| moved(this, e.event.position.x, cx)));
        greedy(div().flex().flex_row().items_center().gap(px(8.)), outer).child(Self::label(n.string("label").unwrap_or("").to_owned())).child(area).into_any_element()
    }

    fn choice(&self, n: &ViewNode, inner: Env, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let options: Vec<(String, String)> = n.props.get("options").and_then(Json::as_array).into_iter().flatten()
            .map(|o| (o.get("label").map(json_text).unwrap_or_default(), o.get("value").map(json_text).unwrap_or_default())).collect();
        let value: Vec<String> = n.value().and_then(Json::as_array).into_iter().flatten().map(json_text).collect();
        let multiple = n.string("variant") == Some("multipleSelection");
        let style = n.string("displayStyle").unwrap_or("");
        let label = n.string("label").unwrap_or("").to_owned();
        let filter = if n.bool("filterable") == Some(true) && !inner.in_form { self.host.fields.get(&format!("{}#filter", n.id)) } else { None };
        let needle = filter.map(|(_, _, t)| t.to_lowercase()).unwrap_or_default();
        let shown: Vec<&(String, String)> = options.iter().filter(|(l, _)| needle.is_empty() || l.to_lowercase().contains(&needle)).collect();
        let toggled = |v: &str| -> Json {
            if multiple {
                let mut next = value.clone();
                match next.iter().position(|x| x == v) { Some(i) => { next.remove(i); } None => next.push(v.to_owned()) }
                json!(next)
            } else {
                json!([v])
            }
        };
        let set = |tag: String, next: Json, el: gpui::Stateful<gpui::Div>, cx: &mut Context<Client>| {
            let (pane, id) = (self.pane.to_owned(), n.id.clone());
            let _ = tag;
            el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                this.plugin_set(&pane, &id, next.clone(), cx);
                cx.notify();
            }))
        };
        let mut out = div().flex().flex_col().items_start().gap(px(6.));
        if let Some((_, input, _)) = filter {
            out = greedy(out, outer).child(div().w(px(180.)).px(px(6.)).py(px(2.)).rounded(px(5.)).bg(rgba(0x8e8e9329)).border_1().border_color(rgba(0x8e8e9373)).child(Input::new(input).appearance(false).p_0()));
        }
        let body: gpui::AnyElement = if inner.in_form && multiple {
            let mut c = greedy(div().flex().flex_col(), outer);
            for (i, (l, v)) in shown.iter().enumerate() {
                let row = div().id(el_id(self.pane, &n.id, &format!("opt{i}"))).flex().flex_row().items_center().py(px(3.)).child(Self::label(l.clone())).child(div().flex_1())
                    .children(value.contains(v).then(|| title::symbol(self.symbols, "checkmark", 11., ACCENT)).flatten());
                c = c.child(set(v.clone(), toggled(v), row, cx));
            }
            c.into_any_element()
        } else if multiple && style == "chips" {
            let mut r = div().flex().flex_row().flex_wrap().gap(px(6.));
            for (i, (l, v)) in shown.iter().enumerate() {
                let on = value.contains(v);
                let chip = div().id(el_id(self.pane, &n.id, &format!("opt{i}"))).px(px(8.)).py(px(3.)).rounded(px(5.)).when(on, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff))).when(!on, |d| d.bg(rgba(CONTROL_BG))).child(Self::label(l.clone()));
                r = r.child(set(v.clone(), toggled(v), chip, cx));
            }
            r.into_any_element()
        } else if multiple {
            let mut c = div().flex().flex_col().gap(px(4.));
            for (i, (l, v)) in shown.iter().enumerate() {
                let on = value.contains(v);
                let mut boxed = div().size(px(14.)).rounded(px(3.)).flex().items_center().justify_center();
                boxed = if on { boxed.bg(rgb(ACCENT)).children(title::symbol(self.symbols, "checkmark", 9., 0xffffff)) } else { boxed.bg(rgba(CONTROL_BG)).border_1().border_color(rgba(0xffffff40)) };
                let row = div().id(el_id(self.pane, &n.id, &format!("opt{i}"))).flex().flex_row().items_center().gap(px(6.)).child(boxed).child(Self::label(l.clone()));
                c = c.child(set(v.clone(), toggled(v), row, cx));
            }
            c.into_any_element()
        } else if style == "chips" && !inner.in_form {
            let mut seg = div().flex().flex_row().p(px(2.)).rounded(px(6.)).bg(rgba(0xffffff14));
            for (i, (l, v)) in shown.iter().enumerate() {
                let on = value.contains(v);
                let s = div().id(el_id(self.pane, &n.id, &format!("opt{i}"))).px(px(10.)).py(px(2.)).rounded(px(5.)).when(on, |d| d.bg(rgba(0xffffff33))).child(Self::label(l.clone()));
                seg = seg.child(set(v.clone(), json!([v]), s, cx));
            }
            seg.into_any_element()
        } else {
            let current = options.iter().find(|(_, v)| value.contains(v)).map(|(l, _)| l.clone()).unwrap_or_default();
            let picks: Vec<Entry> = shown.iter().map(|(l, v)| Entry::new(l.clone(), value.contains(v), Pick::Set(n.id.clone(), json!([v])))).collect();
            let menu = self.popup(n, current, picks, inner, cx);
            if inner.in_form { div().flex().flex_row().items_center().gap(px(8.)).child(Self::label(label)).child(menu).into_any_element() } else { menu }
        };
        out.child(body).into_any_element()
    }

    /// A pop-up button showing `title` whose press opens a menu of `items`.
    fn popup(&self, n: &ViewNode, title: String, items: Vec<Entry>, inner: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let b = div().id(el_id(self.pane, &n.id, "popup")).flex().flex_row().items_center().gap(px(6.)).pl(px(8.)).pr(px(4.)).py(px(2.)).rounded(px(5.)).bg(rgba(CONTROL_BG))
            .child(Self::label(title)).children(title::symbol(self.symbols, "chevron.up.chevron.down", 9., inner.color >> 8));
        if inner.disabled {
            return b.opacity(0.4).into_any_element();
        }
        let pane = self.pane.to_owned();
        b.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.plugin_open_menu(&pane, items.clone(), cx))).into_any_element()
    }

    /// Menu `n`'s children as menu items: Buttons, CheckBoxes, Dividers and nested Menus.
    fn entries(&self, n: &ViewNode) -> Vec<Entry> {
        let mut items = vec![];
        for k in self.kids(n.parts()) {
            match k.component.as_str() {
                "Button" => items.push(Entry::new(self.texts(k), false, Pick::Press(k.id.clone()))),
                "CheckBox" => {
                    let on = k.value().and_then(Json::as_bool).unwrap_or(false);
                    items.push(Entry::new(k.string("label").unwrap_or("").to_owned(), on, Pick::Set(k.id.clone(), Json::Bool(!on))));
                }
                "Divider" => items.push(Entry::new(String::new(), false, Pick::Separator)),
                "Menu" => items.push(Entry { title: k.string("title").unwrap_or("").to_owned(), on: false, pick: Pick::Separator, submenu: self.entries(k) }),
                _ => {}
            }
        }
        items
    }

    fn menu_button(&self, n: &ViewNode, inner: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let items = self.entries(n);
        let b = div().id(el_id(self.pane, &n.id, "menu")).flex().flex_row().items_center().gap(px(6.)).pl(px(8.)).pr(px(4.)).py(px(2.)).rounded(px(5.)).bg(rgba(CONTROL_BG))
            .child(Self::label(n.string("title").unwrap_or("").to_owned())).children(title::symbol(self.symbols, "chevron.down", 9., inner.color >> 8));
        let pane = self.pane.to_owned();
        b.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.plugin_open_menu(&pane, items.clone(), cx))).into_any_element()
    }

    /// Every Text inside node `n`, joined: a menu item's title.
    fn texts(&self, n: &ViewNode) -> String {
        let mut out = vec![];
        let mut stack = vec![n];
        while let Some(k) = stack.pop() {
            if k.component == "Text" {
                out.push(k.props.get("text").map(json_text).unwrap_or_default());
            }
            stack.extend(self.kids(k.parts()).into_iter().rev());
        }
        out.join(" ")
    }

    fn tabs(&self, n: &ViewNode, inner: Env, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let titles: Vec<String> = n.props.get("tabs").and_then(Json::as_array).into_iter().flatten().map(|t| t.get("title").map(json_text).unwrap_or_default()).collect();
        let at = self.host.tabs.get(&n.id).copied().unwrap_or(0).min(titles.len().saturating_sub(1));
        let mut bar = div().flex().flex_row().p(px(2.)).rounded(px(6.)).bg(rgba(0xffffff14));
        for (i, t) in titles.iter().enumerate() {
            let (pane, id) = (self.pane.to_owned(), n.id.clone());
            bar = bar.child(div().id(el_id(self.pane, &n.id, &format!("tab{i}"))).px(px(10.)).py(px(2.)).rounded(px(5.)).when(i == at, |d| d.bg(rgba(0xffffff33))).cursor_pointer().child(Self::label(t.clone()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(h) = this.plugin_previews.get_mut(&pane) {
                        h.tabs.insert(id.clone(), i);
                    }
                    cx.notify();
                })));
        }
        let content = self.part(n, at).map(|c| self.node(c, Env { in_row: false, ..inner }, cx));
        greedy(div().flex().flex_col().items_center(), outer)
            .child(div().relative().top(px(11.)).child(bar))
            .child(div().self_stretch().flex().flex_col().pt(px(20.)).px(px(12.)).pb(px(12.)).rounded(px(8.)).bg(rgba(0xffffff0d)).border_1().border_color(rgba(0xffffff14)).children(content))
            .into_any_element()
    }

    fn gauge(&self, n: &ViewNode, outer: Env) -> gpui::AnyElement {
        let (lo, hi) = (n.number("min").unwrap_or(0.), n.number("max").unwrap_or(1.));
        let v = n.number("value").unwrap_or(0.);
        let frac = if hi > lo { ((v - lo) / (hi - lo)).clamp(0., 1.) as f32 } else { 0. };
        let title = n.string("title").unwrap_or("").to_owned();
        match n.string("style") {
            Some("accessoryCircular" | "accessoryCircularCapacity") => {
                let ring = canvas(|_, _, _| (), move |b, _, window, _| {
                    let (c, r) = (b.center(), b.size.width.min(b.size.height) / 2. - px(3.));
                    let at = |f: f32| {
                        let a = std::f32::consts::PI * (0.75 + 1.5 * f);
                        gpui::point(c.x + r * a.cos(), c.y + r * a.sin())
                    };
                    let arc = |to: f32, color: gpui::Hsla, window: &mut Window| {
                        if to <= 0. {
                            return;
                        }
                        let mut p = gpui::PathBuilder::stroke(px(5.));
                        p.move_to(at(0.));
                        p.arc_to(gpui::point(r, r), px(0.), to * 1.5 > 1., true, at(to));
                        if let Ok(path) = p.build() {
                            window.paint_path(path, color);
                        }
                    };
                    arc(1., rgba(0xffffff26).into(), window);
                    arc(frac, rgb(ACCENT).into(), window);
                }).size(px(44.));
                div().flex().flex_col().items_center().gap(px(2.)).child(div().relative().size(px(44.)).child(ring)
                    .child(div().absolute().inset_0().flex().items_center().justify_center().text_size(px(11.)).child(format!("{}", (v * 100.).round() / 100.))))
                    .when(!title.is_empty(), |d| d.child(div().text_size(px(11.)).child(title))).into_any_element()
            }
            _ => {
                let bar = div().h(px(6.)).rounded(px(3.)).bg(rgba(0xffffff1a)).child(div().h_full().rounded(px(3.)).bg(rgb(ACCENT)).w(gpui::relative(frac)));
                greedy(div().flex().flex_col().gap(px(4.)), outer).when(!title.is_empty(), |d| d.child(title)).child(bar).into_any_element()
            }
        }
    }

    fn chart(&self, n: &ViewNode, outer: Env) -> gpui::AnyElement {
        let mark = match n.string("mark") { Some(m @ ("line" | "point" | "area")) => m, _ => "bar" };
        let data: Vec<Json> = n.props.get("data").and_then(Json::as_array).into_iter().flatten()
            .map(|d| json!({"x": d.get("x").cloned().unwrap_or(json!("")), "y": d.get("y").cloned().unwrap_or(json!(0)), "series": d.get("series").cloned().unwrap_or(json!(""))})).collect();
        match charts::parse(&json!({"mark": mark, "data": data}).to_string()) {
            Ok(spec) => {
                let spec = Rc::new(spec);
                let mut legend = div().flex().flex_wrap().gap_3().text_size(px(11.)).text_color(rgba(SECONDARY));
                for (si, s) in spec.series.iter().enumerate().filter(|(_, s)| !s.is_empty()) {
                    legend = legend.child(div().flex().items_center().gap_1().child(div().size(px(8.)).rounded_sm().bg(charts::series_color(si))).child(s.clone()));
                }
                greedy(div().flex().flex_col().gap(px(4.)), outer).child(div().h(px(120.)).w_full().child(charts::plot(spec))).child(legend).into_any_element()
            }
            Err(e) => div().text_color(rgba(SECONDARY)).child(e).into_any_element(),
        }
    }

    // MARK: the layer over the page

    /// What draws over the page: presented Modals, opened sheets, and Alerts and dialogs.
    pub(super) fn overlays(&self, cx: &mut Context<Client>) -> Option<gpui::AnyElement> {
        let env = Env { color: 0xffffffd9, size: BODY, disabled: false, in_row: false, in_form: false };
        if !page_overlaid(self.tree, self.host) {
            return None;
        }
        let mut modals: Vec<&ViewNode> = self.tree.modals().collect();
        let mut sheets: Vec<&ViewNode> = self.tree.all().filter(|n| is_open_sheet(n, self.host)).collect();
        let mut alerts: Vec<&ViewNode> = self.tree.all().filter(|n| is_presented_alert(n)).collect();
        sheets.sort_by(|a, b| a.id.cmp(&b.id));
        alerts.sort_by(|a, b| a.id.cmp(&b.id));
        modals.dedup_by(|a, b| a.id == b.id);
        let mut layer = div().absolute().inset_0();
        for m in modals {
            layer = layer.child(self.presented(m, env, cx));
        }
        for s in sheets {
            layer = layer.child(self.sheet(s, env, cx));
        }
        for a in alerts {
            layer = layer.child(self.alert(a, cx));
        }
        Some(layer.into_any_element())
    }

    fn scrim(&self, n: &ViewNode, alpha: u32, dismiss: bool, cx: &mut Context<Client>) -> gpui::Stateful<gpui::Div> {
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        let s = div().id(el_id(self.pane, &n.id, "scrim")).absolute().inset_0().occlude().bg(rgba(alpha));
        if dismiss {
            s.on_click(cx.listener(move |this, _, _, cx| {
                this.plugin_raise(&pane, &id, "dismissAction", &json!({}), cx);
                cx.notify();
            }))
        } else {
            s
        }
    }

    fn presented(&self, n: &ViewNode, env: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let dismiss = !n.action("dismissAction").is_empty();
        let mut content = div().id(el_id(self.pane, &n.id, "content")).flex().flex_col().items_start().gap(px(8.)).p(px(18.)).when(dismiss, |d| d.pr(px(40.))).overflow_y_scroll();
        for k in self.kids(n.parts()) {
            content = content.child(self.node(k, env, cx));
        }
        let actions: Vec<&ViewNode> = (0..).map_while(|i| self.embedded(n, &format!("actions.{i}"))).collect();
        let mut card = div().id(el_id(self.pane, &n.id, "card")).relative().occlude().flex().flex_col().max_h_full().rounded(px(12.)).bg(rgba(pane_color("controlBackground").unwrap())).border_1().border_color(rgba(0xffffff1a)).shadow_xl()
            .on_click(|_, _, cx| cx.stop_propagation()).child(content);
        if !actions.is_empty() {
            let mut foot = div().flex().flex_row().justify_end().gap(px(8.)).p(px(10.));
            for a in actions {
                foot = foot.child(self.node(a, Env { in_row: true, ..env }, cx));
            }
            card = card.child(div().h(px(1.)).bg(rgba(pane_color("separator").unwrap()))).child(foot);
        }
        if dismiss {
            let (pane, id) = (self.pane.to_owned(), n.id.clone());
            card = card.child(div().id(el_id(self.pane, &n.id, "close")).absolute().top(px(6.)).right(px(6.)).p(px(8.)).cursor_pointer().tooltip(crate::tooltip::tip("Close"))
                .children(title::symbol(self.symbols, "xmark", 12., SECONDARY >> 8)).child(recorded(self.pane, &format!("{}#close", n.id), self.controls))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.plugin_raise(&pane, &id, "dismissAction", &json!({}), cx);
                    cx.notify();
                })));
        }
        card = card.child(recorded(self.pane, &format!("{}#card", n.id), self.controls));
        div().absolute().inset_0().child(self.scrim(n, 0x00000066, dismiss, cx)).child(div().absolute().inset(px(24.)).flex().flex_col().justify_center().child(card)).into_any_element()
    }

    fn sheet(&self, n: &ViewNode, env: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let content = self.part(n, 1).map(|c| self.node(c, env, cx));
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        let done = div().id(el_id(self.pane, &n.id, "done")).px(px(10.)).py(px(3.)).rounded(px(5.)).bg(rgb(ACCENT)).text_color(rgb(0xffffff)).cursor_pointer().child("Done")
            .child(recorded(self.pane, &format!("{}#done", n.id), self.controls))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.plugin_toggle_open(&pane, &id);
                cx.notify();
            }));
        let bar = div().flex().flex_row().items_center().h(px(38.)).px(px(10.)).border_b_1().border_color(rgba(pane_color("separator").unwrap()))
            .child(div().flex_1()).child(div().font_weight(FontWeight::SEMIBOLD).child(n.string("title").unwrap_or("").to_owned())).child(div().flex_1().flex().justify_end().child(done));
        let card = div().id(el_id(self.pane, &n.id, "sheet")).occlude().flex().flex_col().min_w(px(380.)).min_h(px(420.)).max_h_full().rounded(px(10.)).bg(rgb(0x2a2a2a)).border_1().border_color(rgba(0xffffff1a)).shadow_xl()
            .child(bar).child(div().flex().flex_col().items_start().flex_1().children(content)).child(recorded(self.pane, &format!("{}#sheet", n.id), self.controls));
        div().absolute().inset_0().child(self.scrim(n, 0x00000033, false, cx)).child(div().absolute().inset_0().flex().flex_col().items_center().pt(px(8.)).child(card)).into_any_element()
    }

    fn alert(&self, n: &ViewNode, cx: &mut Context<Client>) -> gpui::AnyElement {
        let mut buttons: Vec<(String, Option<String>, String)> = n.props.get("buttons").and_then(Json::as_array).into_iter().flatten().enumerate()
            .map(|(i, b)| (b.get("title").map(json_text).unwrap_or_else(|| "OK".into()), b.get("role").and_then(Json::as_str).map(str::to_owned), format!("buttons.{i}.action"))).collect();
        if buttons.is_empty() && n.component == "Alert" {
            buttons.push(("OK".into(), None, String::new()));
        }
        if n.component == "ConfirmationDialog" && !buttons.iter().any(|(_, r, _)| r.as_deref() == Some("cancel")) {
            buttons.push(("Cancel".into(), Some("cancel".into()), String::new()));
        }
        let stacked = buttons.len() > 2;
        let mut row = div().flex().gap(px(8.)).mt(px(6.));
        row = if stacked { row.flex_col() } else { row.flex_row() };
        let default = buttons.iter().position(|(_, r, _)| r.is_none());
        for (i, (title, role, key)) in buttons.into_iter().enumerate() {
            let (pane, id) = (self.pane.to_owned(), n.id.clone());
            let mut b = div().id(el_id(self.pane, &n.id, &format!("button{i}"))).flex_1().flex().justify_center().py(px(5.)).rounded(px(6.)).cursor_pointer().child(Self::label(title));
            b = match role.as_deref() {
                Some("destructive") => b.bg(rgba(CONTROL_BG)).text_color(rgba(pane_color("red").unwrap())),
                _ if Some(i) == default => b.bg(rgb(ACCENT)).text_color(rgb(0xffffff)),
                _ => b.bg(rgba(CONTROL_BG)),
            };
            b = b.child(recorded(self.pane, &format!("{}#button{i}", n.id), self.controls)).on_click(cx.listener(move |this, _, _, cx| {
                if !key.is_empty() {
                    this.plugin_raise(&pane, &id, &key, &json!({}), cx);
                }
                this.plugin_raise(&pane, &id, "dismissAction", &json!({}), cx);
                cx.notify();
            }));
            row = row.child(b);
        }
        let card = div().id(el_id(self.pane, &n.id, "alert")).occlude().w(px(260.)).flex().flex_col().items_center().gap(px(4.)).p(px(16.)).rounded(px(12.)).bg(rgb(0x323232)).border_1().border_color(rgba(0xffffff1f)).shadow_xl()
            .child(div().font_weight(FontWeight::BOLD).text_center().child(n.string("title").unwrap_or("").to_owned()))
            .when(n.string("message").is_some_and(|m| !m.is_empty()), |d| d.child(div().text_size(px(11.)).text_center().child(n.string("message").unwrap_or("").to_owned())))
            .child(row).child(recorded(self.pane, &format!("{}#card", n.id), self.controls));
        div().absolute().inset_0().child(self.scrim(n, 0x00000033, false, cx)).child(div().absolute().inset_0().flex().items_center().justify_center().child(card)).into_any_element()
    }
}

/// A SwiftUI alignment name as the horizontal and vertical placement of a flex column's content.
#[allow(clippy::type_complexity)]
fn align(name: &str) -> (fn(gpui::Div) -> gpui::Div, fn(gpui::Div) -> gpui::Div) {
    let (h, v): (fn(gpui::Div) -> gpui::Div, fn(gpui::Div) -> gpui::Div) = match name {
        "leading" => (|d| d.items_start(), |d| d.justify_center()),
        "trailing" => (|d| d.items_end(), |d| d.justify_center()),
        "top" => (|d| d.items_center(), |d| d.justify_start()),
        "bottom" => (|d| d.items_center(), |d| d.justify_end()),
        "topLeading" => (|d| d.items_start(), |d| d.justify_start()),
        "topTrailing" => (|d| d.items_end(), |d| d.justify_start()),
        "bottomLeading" => (|d| d.items_start(), |d| d.justify_end()),
        "bottomTrailing" => (|d| d.items_end(), |d| d.justify_end()),
        "firstTextBaseline" => (|d| d.items_start(), |d| d.justify_start()),
        _ => (|d| d.items_center(), |d| d.justify_center()),
    };
    (h, v)
}

/// What a Slider's drag carries: nothing, it only keeps the drag going outside the slider.
struct SliderDrag;

/// ViewThatFits as SwiftUI lays it out: each child measured at its ideal size, the first that
/// fits the space offered drawn.
struct Fits {
    id: gpui::ElementId,
    node: String,
    axis: String,
    in_row: bool,
    /// Copies of the children laid out only to be measured, under ids of their own.
    measure: Vec<gpui::AnyElement>,
    /// The children drawn, one of them each frame.
    finals: Vec<Option<gpui::AnyElement>>,
    cache: Rc<RefCell<HashMap<String, Fit>>>,
}

impl IntoElement for Fits {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Fits {
    type RequestLayoutState = Vec<(f32, f32)>;
    type PrepaintState = Option<gpui::AnyElement>;

    fn id(&self) -> Option<gpui::ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, Vec<(f32, f32)>) {
        let ideal = Size { width: AvailableSpace::MaxContent, height: AvailableSpace::MaxContent };
        let sizes: Vec<(f32, f32)> = self.measure.iter_mut().map(|m| {
            let s = m.layout_as_root(ideal, window, cx);
            (f32::from(s.width), f32::from(s.height))
        }).collect();
        let mut style = Style::default();
        if self.in_row {
            style.flex_grow = 1.;
            style.flex_shrink = 1.;
            style.flex_basis = gpui::relative(0.).into();
        } else {
            style.align_self = Some(gpui::AlignSelf::Stretch);
        }
        style.min_size.width = px(0.).into();
        let (measured, axis, cache, node) = (sizes.clone(), self.axis.clone(), self.cache.clone(), self.node.clone());
        let layout = window.request_measured_layout(style, move |known, space, _, _| {
            let offered = |k: Option<Pixels>, s: AvailableSpace| k.map(f32::from).or(match s { AvailableSpace::Definite(p) => Some(f32::from(p)), _ => None });
            let (w, h) = (offered(known.width, space.width), offered(known.height, space.height));
            let at = match (w, space.width) { (None, AvailableSpace::MinContent) => measured.len() - 1, _ => pick(&measured, w, h, &axis) };
            let (iw, ih) = measured[at];
            let height = cache.borrow().get(&node).filter(|f| f.choice == at && w.is_some_and(|w| (f.width - w).abs() < 0.5)).map_or(ih, |f| f.height);
            gpui::size(px(w.unwrap_or(iw)), px(height))
        });
        (layout, sizes)
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: gpui::Bounds<Pixels>, sizes: &mut Vec<(f32, f32)>, window: &mut Window, cx: &mut App) -> Option<gpui::AnyElement> {
        let w = f32::from(bounds.size.width);
        let at = pick(sizes, Some(w), Some(f32::from(bounds.size.height)).filter(|_| self.axis != "horizontal"), &self.axis);
        let chosen = self.finals.get_mut(at)?.take()?;
        let mut el = div().w(bounds.size.width).flex().flex_col().child(chosen).into_any_element();
        let size = el.layout_as_root(Size { width: AvailableSpace::Definite(bounds.size.width), height: AvailableSpace::MaxContent }, window, cx);
        let h = f32::from(size.height);
        let stale = self.cache.borrow().get(&self.node).is_none_or(|f| f.choice != at || (f.width - w).abs() >= 0.5 || (f.height - h).abs() >= 0.5);
        if stale {
            self.cache.borrow_mut().insert(self.node.clone(), Fit { choice: at, width: w, height: h });
            // The height this frame laid out was the ideal one; the next lays out the one drawn.
            if (h - f32::from(bounds.size.height)).abs() >= 0.5 {
                window.request_animation_frame();
            }
        }
        el.prepaint_at(bounds.origin, window, cx);
        Some(el)
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: gpui::Bounds<Pixels>, _: &mut Vec<(f32, f32)>, el: &mut Option<gpui::AnyElement>, window: &mut Window, cx: &mut App) {
        if let Some(el) = el {
            el.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_that_fits_picks_the_first_that_fits_else_the_last() {
        let sizes = [(640., 30.), (200., 60.)];
        assert_eq!(pick(&sizes, Some(700.), None, "horizontal"), 0);
        assert_eq!(pick(&sizes, Some(639.), None, "horizontal"), 1);
        assert_eq!(pick(&sizes, Some(100.), None, "horizontal"), 1);
        assert_eq!(pick(&sizes, None, None, "horizontal"), 0);
        // Only the axes `in` names count.
        assert_eq!(pick(&sizes, Some(100.), Some(40.), "vertical"), 0);
        assert_eq!(pick(&sizes, Some(700.), Some(20.), "both"), 1);
    }

    #[test]
    fn natural_order_reads_numbers_by_value() {
        use std::cmp::Ordering::*;
        assert_eq!(natural_cmp("file2", "file10"), Less);
        assert_eq!(natural_cmp("Beta", "alpha"), Greater);
        assert_eq!(natural_cmp("a007", "a7"), Equal);
        assert_eq!(natural_cmp("a", "ab"), Less);
    }

    #[test]
    fn dates_read_as_the_mac_shows_them() {
        assert_eq!(date_text("2026-11-06", true, false), "11/6/2026");
        assert_eq!(date_text("2026-11-06T14:30:00", false, true), "2:30 PM");
        assert_eq!(date_text("2026-11-06T00:05:00", true, true), "11/6/2026, 12:05 AM");
    }

    #[test]
    fn colours_read_from_hex() {
        assert_eq!(hex("#FF8800"), Some(0xff8800ff));
        assert_eq!(hex("FF8800"), None);
        assert_eq!(hex("#F80"), None);
    }
}

/// A Modal opened by its own trigger, drawn as a sheet over the page.
fn is_open_sheet(n: &ViewNode, host: &super::plugin::PluginHost) -> bool {
    n.component == "Modal" && n.bool("presented").is_none() && host.open.contains(&n.id) && n.parts().len() >= 2
}

fn is_presented_alert(n: &ViewNode) -> bool {
    matches!(n.component.as_str(), "Alert" | "ConfirmationDialog") && n.bool("presented") == Some(true)
}

/// Whether anything draws over `host`'s page: a presented Modal, an opened sheet, an Alert or a dialog.
pub(super) fn page_overlaid(tree: &wheel_luau::tree::ViewTree, host: &super::plugin::PluginHost) -> bool {
    tree.modals().next().is_some() || tree.all().any(|n| is_open_sheet(n, host) || is_presented_alert(n))
}
