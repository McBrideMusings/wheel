//! A plugin pane's WheelUI tree drawn by GPUI (wheel-4d1o.2): Column, Row, Text, Divider, Button,
//! TextField, List, Icon, Image, Link, ProgressView, Card and their modifiers, with the defaults
//! WheelUI's `Basics.swift` and `Modifiers.swift` give them; `plugin_controls.rs` draws the rest.
//! A name neither draws draws nothing, as WheelUI's catalog does for a name it lacks.
use super::*;
use super::plugin::PluginHost;
use serde_json::{Map, Value as Json};
use wheel_luau::tree::{ViewNode, ViewTree, child_id, text as json_text};

/// The page's body text, as WheelUI sets it on the whole tree "so a page reads the same on a phone".
pub(super) const BODY: f32 = 13.;
/// Swift's `.secondary` and `.tertiary` label colours in the dark appearance.
pub(super) const SECONDARY: u32 = 0xffffff8c;
const TERTIARY: u32 = 0xffffff40;
pub(super) const ACCENT: u32 = 0x0a84ff;

/// A colour name a modifier may give (`paneColor`, Modifiers.swift), as RGBA in the dark
/// appearance the app draws in; None for any other name, which shows the warning marker.
pub fn pane_color(name: &str) -> Option<u32> {
    let rgb = |c: u32| (c << 8) | 0xff;
    Some(match name {
        "primary" => 0xffffffd9,
        "secondary" => SECONDARY,
        "tertiary" => TERTIARY,
        "black" => 0x000000ff,
        "white" => 0xffffffff,
        "tint" | "accentColor" => rgb(ACCENT),
        "separator" => 0xffffff1a,
        "windowBackground" => rgb(0x323232),
        "controlBackground" | "textBackground" => rgb(0x1e1e1e),
        other => rgb(title::tint(Some(other))?),
    })
}

/// A weight name (`fontWeight` in Modifiers.swift); anything else is regular.
fn weight(name: Option<&str>) -> FontWeight {
    FontWeight(match name {
        Some("ultraLight") => 100.,
        Some("thin") => 200.,
        Some("light") => 300.,
        Some("medium") => 500.,
        Some("semibold") => 600.,
        Some("bold") => 700.,
        Some("heavy") => 800.,
        Some("black") => 900.,
        _ => 400.,
    })
}

/// A SwiftUI text style's size and weight on macOS.
fn text_style(name: &str) -> Option<(f32, f32)> {
    Some(match name {
        "largeTitle" => (26., 400.),
        "title" => (22., 400.),
        "title2" => (17., 400.),
        "title3" => (15., 400.),
        "headline" => (13., 700.),
        "subheadline" => (11., 400.),
        "body" => (13., 400.),
        "callout" => (12., 400.),
        "footnote" | "caption" | "caption2" => (10., 400.),
        _ => return None,
    })
}

/// A Material icon name WheelUI maps to an SF Symbol; any other name is an SF Symbol's own.
fn symbol_name(name: &str) -> &str {
    match name {
        "check" => "checkmark",
        "close" => "xmark",
        "add" => "plus",
        "delete" => "trash",
        "edit" => "pencil",
        "home" => "house",
        "search" => "magnifyingglass",
        "settings" => "gearshape",
        "info" => "info.circle",
        "warning" => "exclamationmark.triangle",
        "play" => "play.fill",
        "pause" => "pause.fill",
        "refresh" => "arrow.clockwise",
        "send" => "paperplane",
        "share" => "square.and.arrow.up",
        other => other,
    }
}

/// `padding`'s argument as top, trailing, bottom, leading: a number for every side, `[v, h]`,
/// `[t, r, b, l]`, or a table of sides; None for any other shape.
pub fn padding(arg: &Json) -> Option<[f32; 4]> {
    let n = |v: &Json| v.as_f64().map(|f| f as f32);
    match arg {
        Json::Number(_) => n(arg).map(|v| [v; 4]),
        Json::Array(a) if a.len() == 2 => Some([n(&a[0])?, n(&a[1])?, n(&a[0])?, n(&a[1])?]),
        Json::Array(a) if a.len() == 4 => Some([n(&a[0])?, n(&a[1])?, n(&a[2])?, n(&a[3])?]),
        Json::Object(d) => {
            let side = |k: &str, axis: &str| d.get(k).and_then(n).or_else(|| d.get(axis).and_then(n)).unwrap_or(0.);
            Some([side("top", "vertical"), side("trailing", "horizontal"), side("bottom", "vertical"), side("leading", "horizontal")])
        }
        _ => None,
    }
}

/// What a node's children inherit from it, as SwiftUI's environment carries it: the parts a GPUI
/// div cannot pass down itself.
#[derive(Clone, Copy)]
pub(super) struct Env {
    /// The colour and size an Icon's symbol is drawn in.
    pub color: u32,
    pub size: f32,
    pub disabled: bool,
    /// The parent lays its children out in a row, so a greedy width grows along the row.
    pub in_row: bool,
    /// Inside a Form, where a picker shows its label and a Section its grouped box.
    pub in_form: bool,
}

/// One draw of one pane's tree.
pub(super) struct Draw<'a> {
    pub pane: &'a str,
    pub tree: &'a ViewTree,
    pub host: &'a PluginHost,
    pub controls: &'a Rc<crate::view::scoped::Scoped<Control>>,
    pub symbols: &'a title::Symbols,
    /// The width the page is drawn at: a Table drops its `wide` columns under 640 pt.
    pub width: f32,
}

pub(super) fn el_id(pane: &str, id: &str, tag: &str) -> gpui::ElementId {
    gpui::ElementId::Name(format!("plugin-{pane}-{id}-{tag}").into())
}

/// Takes the width it is offered: along the row in a Row, across the column in a Column. A
/// stretch, not a percentage: a percentage of a parent its content sizes resolves to nothing.
pub(super) fn greedy(d: gpui::Div, env: Env) -> gpui::Div {
    if env.in_row { d.flex_1().min_w_0() } else { d.self_stretch().min_w_0() }
}

/// Whether node `n` takes the width it is offered, as its SwiftUI view does: a Column (WheelUI
/// frames it to the full width), a Card, a field, a horizontal Divider, a List, a bar, and a Row
/// a justify spacer pushes to its edges.
fn takes_width(n: &ViewNode) -> bool {
    match n.component.as_str() {
        "Column" | "Card" | "TextField" | "List" | "ViewThatFits" | "Table" | "Form" | "Section" | "LabeledContent" | "Slider" | "Tabs" | "Chart" | "Toolbar" | "NavigationSplitView" | "NavigationStack" | "HSplitView" | "VSplitView" => true,
        "Gauge" => !matches!(n.string("style"), Some("accessoryCircular" | "accessoryCircularCapacity")),
        "Divider" => n.string("axis") != Some("vertical"),
        "ProgressView" => n.number("value").is_some(),
        "Row" => matches!(n.string("justify").unwrap_or("start"), "start" | "center" | "end" | "spaceEvenly" | "spaceAround"),
        _ => false,
    }
}

impl Client {
    /// Previewed plugin pane `pane`'s tree drawn `w` × `h`, or None when it is not previewed.
    pub(super) fn plugin_element(&mut self, pane: &str, w: f32, h: f32, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.flush_plugin_copies(cx);
        self.sync_plugin_fields(pane, window, cx);
        let host = self.plugin_previews.get(pane)?;
        let w = host.width.unwrap_or(w);
        host.drawn.set(w);
        let d = Draw { pane, tree: &host.pane.tree, host, controls: &self.controls, symbols: &self.symbols, width: w };
        let env = Env { color: 0xffffffd9, size: BODY, disabled: false, in_row: false, in_form: false };
        let mut page = div().flex().flex_col().w(px(w)).text_size(px(BODY)).line_height(gpui::relative(1.23)).text_color(rgba(env.color));
        if let Some(error) = host.pane.error() {
            page = page.child(div().p_2().text_color(rgba(pane_color("red").unwrap())).child(error.to_owned()));
        }
        if let Some(root) = host.pane.tree.root() {
            page = page.child(d.node(root, env, cx));
        }
        let fills = host.pane.fills_pane();
        let body = if fills {
            page.h(px(h)).overflow_hidden().into_any_element()
        } else {
            div().id(el_id(pane, "root", "scroll")).w(px(w)).h(px(h)).overflow_y_scroll().track_scroll(&host.scroll).child(page).into_any_element()
        };
        Some(div().relative().w(px(w)).h(px(h)).child(body).children(d.overlays(cx)).into_any_element())
    }

    /// Gives each TextField of `pane`'s tree an input, and the script's value when it changed.
    fn sync_plugin_fields(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.plugin_previews.get_mut(pane) else { return };
        host.prune_view_state();
        let mut fields: Vec<(String, u64, String, String, Option<String>)> = host.pane.tree.all().filter(|n| n.component == "TextField")
            .map(|n| (n.id.clone(), n.serial, n.value().map(json_text).unwrap_or_default(), n.string("label").unwrap_or("").to_owned(), n.string("variant").map(str::to_owned)))
            .collect();
        // A filterable ChoicePicker's filter is the view's own text, kept as typed (`#filter`).
        for n in host.pane.tree.all().filter(|n| n.component == "ChoicePicker" && n.bool("filterable") == Some(true)) {
            let id = format!("{}#filter", n.id);
            let typed = host.fields.get(&id).map(|(_, _, t)| t.clone()).unwrap_or_default();
            fields.push((id, n.serial, typed, "Filter".into(), None));
        }
        host.fields.retain(|id, (serial, _, _)| fields.iter().any(|(i, s, ..)| i == id && s == serial));
        for (id, serial, value, label, variant) in fields {
            let host = self.plugin_previews.get_mut(pane).unwrap();
            let input = match host.fields.get(&id) {
                Some((_, input, synced)) if *synced == value => continue,
                Some((_, input, _)) => input.clone(),
                None => {
                    let long = variant.as_deref() == Some("longText");
                    let masked = variant.as_deref() == Some("obscured");
                    let input = cx.new(|cx| InputState::new(window, cx).multi_line(long).masked(masked).placeholder(label));
                    let (pane, node) = (pane.to_owned(), id.clone());
                    cx.subscribe_in(&input, window, move |this: &mut Self, input, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            let text = input.read(cx).value().to_string();
                            if let Some((_, _, synced)) = this.plugin_previews.get_mut(&pane).and_then(|h| h.fields.get_mut(&node)) {
                                if *synced == text {
                                    return;
                                }
                                *synced = text.clone();
                            }
                            if !node.ends_with("#filter") {
                                this.plugin_set(&pane, &node, Json::String(text), cx);
                            }
                            cx.notify();
                        }
                    })
                    .detach();
                    input
                }
            };
            input.update(cx, |state, cx| {
                state.set_value("", window, cx);
                state.insert(value.clone(), window, cx);
            });
            self.plugin_previews.get_mut(pane).unwrap().fields.insert(id, (serial, input, value));
        }
    }
}

impl Draw<'_> {
    pub(super) fn kids<'n>(&'n self, ids: impl IntoIterator<Item = &'n str>) -> Vec<&'n ViewNode> {
        ids.into_iter().filter_map(|id| self.tree.node(id)).collect()
    }

    /// Node `n` with its modifiers, as WheelUI's `PaneNodeView` wraps it.
    pub(super) fn node(&self, n: &ViewNode, env: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        // What the node's own modifiers set for everything inside it.
        let mut inner = env;
        for m in n.modifiers() {
            let Some((name, arg)) = m.iter().next() else { continue };
            match name.as_str() {
                "foregroundStyle" => if let Some(c) = arg.as_str().and_then(pane_color) { inner.color = c },
                "font" => if let Some((size, _)) = font_of(arg) { inner.size = size },
                "disabled" if arg.as_bool() == Some(true) => inner.disabled = true,
                _ => {}
            }
        }
        // Inside the node's own wrapper, a column, a greedy piece stretches across; only the wrapper sits in the parent.
        let body = self.component(n, Env { in_row: false, ..inner }, Env { in_row: false, ..env }, cx);
        let (modified, wide) = self.modify(n, body, env, cx);
        // A borderless Button lights up under the pointer unless a `hoverEffect` modifier says otherwise.
        let lit = n.component == "Button" && n.string("variant") == Some("borderless") && !n.modifiers().any(|m| m.contains_key("hoverEffect")) && !inner.disabled;
        let outer = |d: gpui::Div| if wide { greedy(d, env) } else { d };
        let wrapped = outer(div().relative().flex().flex_col().items_start()).child(modified).child(recorded(self.pane, &n.id, self.controls));
        // A disabled node and everything in it ignores the pointer, as SwiftUI's `disabled` does.
        let wrapped = if inner.disabled && !env.disabled { wrapped.child(div().id(el_id(self.pane, &n.id, "disabled")).absolute().inset_0().occlude()) } else { wrapped };
        if lit { outer(div().flex().flex_col().items_start()).id(el_id(self.pane, &n.id, "hover")).rounded(px(6.)).hover(|s| s.bg(rgba(0xffffff29))).child(wrapped).into_any_element() } else { wrapped.into_any_element() }
    }

    /// The component itself, inside its modifiers: `inner` is what its children inherit and
    /// `outer` where it sits.
    fn component(&self, n: &ViewNode, inner: Env, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        match n.component.as_str() {
            "Column" | "Row" => self.stack(n, inner, outer, cx),
            "Text" => {
                let raw = n.props.get("text").map(json_text).unwrap_or_default();
                let t = div().child(raw);
                let t = match n.string("variant") {
                    Some("h1") => t.text_size(px(28.)).font_weight(FontWeight::BOLD),
                    Some("h2") => t.text_size(px(20.)).font_weight(FontWeight::BOLD),
                    Some("h3") => t.text_size(px(17.)).font_weight(FontWeight::SEMIBOLD),
                    Some("h4") => t.text_size(px(15.)).font_weight(FontWeight::BOLD),
                    Some("h5") => t.text_size(px(14.)).font_weight(FontWeight::SEMIBOLD),
                    Some("caption") => t.text_size(px(12.)).text_color(rgba(SECONDARY)),
                    _ => t,
                };
                t.into_any_element()
            }
            "Divider" => {
                let line = div().bg(rgba(pane_color("separator").unwrap()));
                if n.string("axis") == Some("vertical") { line.w(px(1.)).h(px(18.)) } else { greedy(line.h(px(1.)), outer) }.into_any_element()
            }
            "Button" => self.button(n, inner, cx),
            "TextField" => {
                let field = match self.host.fields.get(&n.id) {
                    Some((_, input, _)) => Input::new(input).appearance(false).p_0().into_any_element(),
                    None => div().into_any_element(),
                };
                let boxed = div().px(px(10.)).py(px(8.)).rounded(px(8.)).bg(rgba(0x8e8e9329)).border_1().border_color(rgba(0x8e8e9373)).child(field);
                let boxed = if n.string("variant") == Some("longText") { boxed.min_h(px(60.)) } else { boxed };
                greedy(boxed, outer).into_any_element()
            }
            "List" => self.list(n, inner, outer, cx),
            "Icon" => {
                let name = symbol_name(n.string("name").unwrap_or(""));
                title::symbol(self.symbols, name, inner.size, inner.color >> 8).unwrap_or_else(|| div().into_any_element())
            }
            "Image" => {
                let Some(url) = n.string("url") else { return div().into_any_element() };
                let max = match n.string("variant") {
                    Some("icon") => Some(24.),
                    Some("avatar") => Some(40.),
                    Some("smallFeature") => Some(50.),
                    Some("mediumFeature") => Some(150.),
                    Some("largeFeature") => Some(400.),
                    _ => None,
                };
                let fit = if n.string("fit") == Some("cover") { gpui::ObjectFit::Cover } else { gpui::ObjectFit::Contain };
                let image = gpui::img(url.to_owned()).object_fit(fit).w_full();
                let image = if n.string("variant") == Some("avatar") { image.rounded(px(20.)) } else { image };
                let frame = div().child(image);
                match max { Some(m) => greedy(frame.max_w(px(m)), outer), None => greedy(frame, outer) }.into_any_element()
            }
            "Link" => {
                let Some(dest) = n.string("destination").filter(|d| has_scheme(d)) else { return div().into_any_element() };
                let title = n.string("title").unwrap_or(dest).to_owned();
                let dest = dest.to_owned();
                div().id(el_id(self.pane, &n.id, "link")).cursor_pointer().text_color(rgb(ACCENT)).child(title)
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&dest))).into_any_element()
            }
            "ProgressView" => {
                let title = n.string("title").unwrap_or("").to_owned();
                let mut col = div().flex().flex_col().gap(px(4.));
                match n.number("value") {
                    Some(v) => {
                        let total = n.number("total").unwrap_or(1.);
                        let frac = if total > 0. { (v / total).clamp(0., 1.) as f32 } else { 0. };
                        if !title.is_empty() {
                            col = col.child(title);
                        }
                        let bar = div().h(px(6.)).rounded(px(3.)).bg(rgba(0xffffff1a)).child(div().h_full().rounded(px(3.)).bg(rgb(ACCENT)).w(gpui::relative(frac)));
                        greedy(col.child(bar), outer).into_any_element()
                    }
                    // hack: a still ring stands in for the spinning one until the cutover's motion pass (wheel-4d1o.7).
                    None => col.items_center().child(div().size(px(16.)).rounded_full().border_2().border_color(rgba(SECONDARY))).when(!title.is_empty(), |c| c.child(title)).into_any_element(),
                }
            }
            "Card" => {
                let first = n.parts().first().and_then(|id| self.tree.node(id));
                let mut card = div().p(px(10.)).rounded(px(6.)).bg(rgba(0xffffff0d)).border_1().border_color(rgba(0xffffff14));
                card = greedy(card, outer);
                if let Some(child) = first {
                    card = card.child(self.node(child, inner, cx));
                }
                card.into_any_element()
            }
            _ => self.control(n, inner, outer, cx).unwrap_or_else(|| div().into_any_element()),
        }
    }

    fn stack(&self, n: &ViewNode, inner: Env, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let row = n.component == "Row";
        let spacing = n.number("spacing").unwrap_or(8.) as f32;
        let justify = n.string("justify").unwrap_or("start");
        let align = n.string("align");
        let mut s = div().flex().gap(px(spacing));
        s = if row {
            match align { Some("start") => s.flex_row().items_start(), Some("end") => s.flex_row().items_end(), _ => s.flex_row().items_center() }
        } else {
            let s = greedy(s.flex_col(), outer);
            match align { Some("center") => s.items_center(), Some("end") => s.items_end(), _ => s.items_start() }
        };
        let spacer = || div().flex_1().min_w_0().min_h_0();
        let before = matches!(justify, "end" | "center" | "spaceEvenly" | "spaceAround");
        let between = matches!(justify, "spaceBetween" | "spaceEvenly" | "spaceAround");
        let after = row && matches!(justify, "start" | "center" | "spaceEvenly" | "spaceAround");
        // A Row with a spacer at an edge takes the width it is offered (the spacer is greedy).
        if row && (before || after) {
            s = greedy(s, outer);
        }
        // The gap goes between spacers too; SwiftUI's stack spacing does the same.
        let kids = self.kids(n.parts());
        if before && !kids.is_empty() {
            s = s.child(spacer());
        }
        for (i, k) in kids.iter().enumerate() {
            if i > 0 && between {
                s = s.child(spacer());
            }
            s = s.child(self.node(k, Env { in_row: row, ..inner }, cx));
        }
        if after {
            s = s.child(spacer());
        }
        s.into_any_element()
    }

    fn button(&self, n: &ViewNode, inner: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let label = n.parts().first().and_then(|id| self.tree.node(id)).map(|k| self.node(k, Env { in_row: false, ..inner }, cx));
        let copied = self.host.copied.get(&n.id).is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(1500));
        let mut b = div().id(el_id(self.pane, &n.id, "button")).relative().flex().flex_row().items_center().justify_center();
        b = match n.string("variant") {
            Some("primary") => b.px(px(8.)).py(px(3.)).rounded(px(5.)).bg(rgb(ACCENT)).text_color(rgb(0xffffff)),
            Some("borderless") => b.text_color(rgb(ACCENT)),
            _ => b.px(px(8.)).py(px(3.)).rounded(px(5.)).bg(rgba(0xffffff2e)),
        };
        if let Some(label) = label {
            b = b.child(div().when(copied, |d| d.opacity(0.)).child(label));
        }
        if copied && let Some(check) = title::symbol(self.symbols, "checkmark", inner.size, inner.color >> 8) {
            b = b.child(div().absolute().inset_0().flex().items_center().justify_center().child(check));
        }
        if inner.disabled {
            return b.opacity(0.4).into_any_element();
        }
        let (pane, id) = (self.pane.to_owned(), n.id.clone());
        b.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
            this.plugin_press(&pane, &id, cx);
            cx.notify();
        })).into_any_element()
    }

    fn list(&self, n: &ViewNode, inner: Env, outer: Env, cx: &mut Context<Client>) -> gpui::AnyElement {
        let horizontal = n.string("direction") == Some("horizontal");
        let spacing = n.number("spacing").unwrap_or(8.) as f32;
        let parts = n.parts();
        // `scrollTo` names a keyed child by its key, or a place among the parts from 1.
        let target = match n.props.get("scrollTo") {
            Some(Json::String(k)) => Some(child_id(&n.id, k)),
            Some(Json::Number(v)) => v.as_f64().and_then(|f| parts.get((f as usize).wrapping_sub(1))).map(|s| s.to_string()),
            _ => None,
        };
        let (handle, last) = self.host.scrolled.borrow().get(&n.id).cloned().unwrap_or_default();
        if target != last && let Some(ix) = target.as_ref().and_then(|t| parts.iter().position(|p| p == t)) {
            handle.scroll_to_top_of_item(ix);
        }
        let l = div().flex().gap(px(spacing));
        let l = if horizontal { greedy(l.flex_row().items_center(), outer) } else { greedy(l.flex_col().items_start().size_full(), outer) };
        let l = l.id(el_id(self.pane, &n.id, "list")).track_scroll(&handle);
        let mut l = if horizontal { l.overflow_x_scroll() } else { l.overflow_y_scroll() };
        for k in self.kids(parts) {
            l = l.child(self.node(k, Env { in_row: horizontal, ..inner }, cx));
        }
        // The next draw scrolls only when `scrollTo` changes, and keeps what the person scrolled.
        self.host.scrolled.borrow_mut().insert(n.id.clone(), (handle, target));
        l.into_any_element()
    }

    /// `body` inside node `n`'s modifiers, in their order: the first is innermost. Also whether the
    /// result takes the width it is offered: each wrapper passes its content's greed on, and a
    /// frame sets its own.
    fn modify(&self, n: &ViewNode, body: gpui::AnyElement, env: Env, cx: &mut Context<Client>) -> (gpui::AnyElement, bool) {
        let mut el = body;
        let mut wide = takes_width(n);
        for (i, m) in n.modifiers().enumerate() {
            let Some((name, arg)) = m.iter().next() else { continue };
            let inner_wide = wide;
            if name == "frame" && let Some(d) = arg.as_object() {
                if d.contains_key("width") {
                    wide = false;
                }
                if d.get("maxWidth").and_then(Json::as_str) == Some("infinity") {
                    wide = true;
                }
            }
            el = match self.modifier(n, i, name, arg, el, env, inner_wide, cx) {
                Ok(e) => e,
                Err(e) => div().relative().when(wide, |d| greedy(d, Env { in_row: false, ..env })).child(e).child(div().absolute().top_0().right_0().text_size(px(10.)).text_color(rgba(pane_color("red").unwrap())).child(format!("⚠︎ {name}"))).into_any_element(),
            };
        }
        (el, wide)
    }

    /// One modifier around `el`; Err gives `el` back when the name or its argument is unknown.
    #[allow(clippy::too_many_arguments)]
    fn modifier(&self, n: &ViewNode, i: usize, name: &str, arg: &Json, el: gpui::AnyElement, env: Env, wide: bool, cx: &mut Context<Client>) -> Result<gpui::AnyElement, gpui::AnyElement> {
        // A wrapper hugs its content, or takes the width when its content does.
        let wrap = || {
            let d = div().flex().flex_col().items_start();
            if wide { greedy(d, Env { in_row: false, ..env }) } else { d }
        };
        let color = |a: &Json| a.as_str().and_then(pane_color);
        Ok(match name {
            "font" => match font_of(arg) {
                Some((size, w)) => {
                    let mut d = wrap().text_size(px(size)).font_weight(FontWeight(w));
                    if let Some(design) = arg.get("design").and_then(Json::as_str) {
                        d = match design { "monospaced" => d.font_family("Menlo"), "serif" => d.font_family("New York"), _ => d };
                    }
                    d.child(el).into_any_element()
                }
                None => return Err(el),
            },
            "fontWeight" => wrap().font_weight(weight(arg.as_str())).child(el).into_any_element(),
            "foregroundStyle" => match color(arg) { Some(c) => wrap().text_color(rgba(c)).child(el).into_any_element(), None => return Err(el) },
            "background" => match color(arg) { Some(c) => wrap().bg(rgba(c)).child(el).into_any_element(), None => return Err(el) },
            "tint" => match color(arg) { Some(_) => el, None => return Err(el) },
            "padding" => match padding(arg) {
                Some([t, r, b, l]) => wrap().pt(px(t)).pr(px(r)).pb(px(b)).pl(px(l)).child(el).into_any_element(),
                None => return Err(el),
            },
            "frame" => match arg.as_object() { Some(d) => frame(d, el, Env { in_row: false, ..env }, env.in_row, wide), None => return Err(el) },
            "overlay" => match arg.get("stroke").and_then(color) {
                Some(c) => {
                    let r = arg.get("cornerRadius").and_then(Json::as_f64).unwrap_or(0.) as f32;
                    wrap().relative().child(el).child(div().absolute().inset_0().rounded(px(r)).border_1().border_color(rgba(c))).into_any_element()
                }
                None => return Err(el),
            },
            "clipShape" => match arg.get("cornerRadius").and_then(Json::as_f64) {
                Some(r) => wrap().rounded(px(r as f32)).overflow_hidden().child(el).into_any_element(),
                None => return Err(el),
            },
            "disabled" => if arg.as_bool() == Some(true) { wrap().opacity(0.4).child(el).into_any_element() } else { el },
            "opacity" => match arg.as_f64() { Some(o) => wrap().opacity(o as f32).child(el).into_any_element(), None => return Err(el) },
            "offset" => {
                let at = |k: &str| arg.get(k).and_then(Json::as_f64).unwrap_or(0.) as f32;
                wrap().relative().left(px(at("x"))).top(px(at("y"))).child(el).into_any_element()
            }
            "lineLimit" => match arg.as_f64().filter(|f| *f >= 1.) { Some(l) => wrap().line_clamp(l as usize).text_ellipsis().child(el).into_any_element(), None => return Err(el) },
            "fixedSize" => wrap().flex_none().whitespace_nowrap().child(el).into_any_element(),
            "help" => match arg.as_str() {
                Some(t) => wrap().id(el_id(self.pane, &n.id, &format!("help{i}"))).child(el).tooltip(crate::tooltip::tip(t)).into_any_element(),
                None => return Err(el),
            },
            "onHover" if arg.is_object() => {
                let (pane, id, key) = (self.pane.to_owned(), n.id.clone(), format!("modifiers.{i}.onHover"));
                wrap().id(el_id(self.pane, &n.id, &format!("onhover{i}"))).child(el).on_hover(cx.listener(move |this, over: &bool, _, cx| {
                    this.plugin_raise(&pane, &id, &key, &serde_json::json!({"hovering": over}), cx);
                    cx.notify();
                })).into_any_element()
            }
            "hoverEffect" => match arg.as_str() {
                Some("none") => el,
                Some("highlight") => wrap().id(el_id(self.pane, &n.id, &format!("hl{i}"))).rounded(px(6.)).hover(|s| s.bg(rgba(0xffffff29))).child(el).into_any_element(),
                _ => return Err(el),
            },
            "pointerStyle" => match arg.as_str() { Some("link") => wrap().id(el_id(self.pane, &n.id, &format!("ptr{i}"))).cursor_pointer().child(el).into_any_element(), _ => return Err(el) },
            // What SwiftUI applies to text inside that this slice draws as it is; none marks.
            "monospacedDigit" | "truncationMode" | "multilineTextAlignment" | "textCase" | "textSelection" | "tracking" | "accessibilityLabel" | "layoutPriority" | "animation" => el,
            _ => return Err(el),
        })
    }
}

/// `font`'s argument as a size and a weight: a text style's name, or `{size, weight, design}` /
/// `{style, weight}`.
fn font_of(arg: &Json) -> Option<(f32, f32)> {
    match arg {
        Json::String(s) => text_style(s),
        Json::Object(d) => {
            let w = d.get("weight").and_then(Json::as_str).map(|w| weight(Some(w)).0);
            match (d.get("size").and_then(Json::as_f64), d.get("style").and_then(Json::as_str).and_then(text_style)) {
                (Some(size), _) => Some((size as f32, w.unwrap_or(400.))),
                (None, Some((size, sw))) => Some((size, w.unwrap_or(sw))),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `frame`'s box around `el`: its sizes, and its content placed by `alignment` (leading when
/// absent, as WheelUI's frame defaults, not SwiftUI's centre).
fn frame(d: &Map<String, Json>, el: gpui::AnyElement, env: Env, in_row: bool, wide: bool) -> gpui::AnyElement {
    let len = |k: &str| d.get(k).and_then(|v| if v.as_str() == Some("infinity") { Some(f32::INFINITY) } else { v.as_f64().map(|f| f as f32) });
    let mut b = div().flex().flex_col();
    b = match d.get("alignment").and_then(Json::as_str) {
        Some("trailing") => b.items_end().justify_center(),
        Some("top") => b.items_center().justify_start(),
        Some("bottom") => b.items_center().justify_end(),
        Some("center") => b.items_center().justify_center(),
        _ => b.items_start().justify_center(),
    };
    if wide && len("width").is_none() { b = greedy(b, env) }
    if let Some(w) = len("width") { b = b.w(px(w)).flex_none() }
    if let Some(h) = len("height") { b = b.h(px(h)) }
    if let Some(w) = len("minWidth") { b = b.min_w(px(w)) }
    if let Some(h) = len("minHeight") { b = b.min_h(px(h)) }
    match len("maxWidth") {
        Some(w) if w.is_infinite() => b = greedy(b, env),
        Some(w) => b = greedy(b.max_w(px(w)), env),
        None => {}
    }
    match len("maxHeight") {
        Some(h) if h.is_infinite() => b = if in_row { b.h_full() } else { b.flex_1().min_h_0() },
        Some(h) => b = b.max_h(px(h)),
        None => {}
    }
    b.child(el).into_any_element()
}

/// Whether `s` starts with a URL scheme, so the system can open it.
fn has_scheme(s: &str) -> bool {
    s.split_once(':').is_some_and(|(scheme, rest)| !rest.is_empty() && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_reads_every_shape() {
        assert_eq!(padding(&serde_json::json!(4)), Some([4.; 4]));
        assert_eq!(padding(&serde_json::json!([2, 6])), Some([2., 6., 2., 6.]));
        assert_eq!(padding(&serde_json::json!([1, 2, 3, 4])), Some([1., 2., 3., 4.]));
        assert_eq!(padding(&serde_json::json!({"vertical": 8, "trailing": 12})), Some([8., 12., 8., 0.]));
        assert_eq!(padding(&serde_json::json!([1, 2, 3])), None);
    }

    #[test]
    fn colours_are_the_named_ones() {
        assert_eq!(pane_color("blue"), Some(0x0a84ffff));
        assert!(pane_color("controlBackground").is_some() && pane_color("brown").is_some());
        assert_eq!(pane_color("#ff0000"), None);
        assert_eq!(pane_color("fg"), None);
    }

    #[test]
    fn fonts_take_a_style_or_a_size() {
        assert_eq!(font_of(&serde_json::json!("headline")), Some((13., 700.)));
        assert_eq!(font_of(&serde_json::json!({"size": 18, "weight": "semibold"})), Some((18., 600.)));
        assert_eq!(font_of(&serde_json::json!({"style": "title", "weight": "bold"})), Some((22., 700.)));
        assert_eq!(font_of(&serde_json::json!({"weight": "bold"})), None);
    }

    #[test]
    fn links_need_a_scheme() {
        assert!(has_scheme("https://example.com") && has_scheme("mailto:a@b.c"));
        assert!(!has_scheme("example.com") && !has_scheme(":x") && !has_scheme("1a:b"));
    }
}
