//! The pane picker an empty pane draws: the quick buttons above the search field, one list of the
//! space's plugin panes and artifacts under it, and the key hints at the foot.
use super::*;
use crate::pane_picker::{self, Hit, Item, Kind};

/// The column's widest; a narrower pane fills its width less the margins.
const COLUMN: f32 = 560.;
const MARGIN: f32 = 13.;
const BUTTONS_H: f32 = 82.;
const FIELD_H: f32 = 36.;
const GAP: f32 = 10.;

impl Client {
    /// Empty pane `pane`'s picker over its body (`width` × `height`, under the pane's title row).
    pub(super) fn picker_element(&self, pane: &str, width: f32, height: f32, cx: &mut Context<Client>) -> gpui::Div {
        let picker = self.pane_pickers.get(pane).cloned().unwrap_or_default();
        let column = (width - 2. * MARGIN).clamp(0., COLUMN);
        // The buttons and the field sit at a height set by the pane's size alone, so no state moves them.
        let top = (height * 0.15).clamp(MARGIN, 118.);
        let typing = !picker.query.is_empty();
        // The field draws as holding the keys only in the pane that has them.
        let keyed = picker.field_focused && self.focus.current.as_deref() == Some(pane);
        let total = self.picker_items(pane).len();
        let hits = if typing { self.picker_hits(pane) } else { vec![] };
        let shown = if typing { hits.len() } else { total };

        let quick = self.picker_buttons();
        // A button too narrow for its name shows the symbol and its key alone.
        let named = (column - 8. * (quick.len() as f32 - 1.)) / quick.len() as f32 >= 56.;
        let mut buttons = div().flex().gap(px(8.)).h(px(BUTTONS_H)).w(px(column)).relative();
        for (n, (item, found)) in quick.into_iter().enumerate() {
            // At rest Terminal is what Return opens, so it carries the accent border.
            let ready = item.kind == Kind::Terminal && !typing && keyed;
            let (pid, open) = (pane.to_owned(), item.clone());
            let mut b = div().id(gpui::ElementId::Name(format!("picker-button-{pane}-{}", item.id).into())).flex_1().min_w(px(0.)).h_full().rounded_md()
                .flex().flex_col().items_center().justify_center().gap_1().overflow_hidden()
                .bg(rgb(0x22302a)).border_1().border_color(rgb(if ready { 0x7fb89a } else { 0x2c3d34 })).text_color(rgb(0xd9e8e1))
                .children(title::symbol(&self.native, &self.symbols, item.kind.symbol(), 18., 0xd9e8e1))
                .children(named.then(|| div().text_sm().whitespace_nowrap().child(item.name.clone())))
                .child(div().text_xs().text_color(rgb(0x8fa89b)).child(format!("⌘{}", n + 1)));
            if found {
                b = b.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.focus_pane(&pid);
                    this.picker_open(&pid, &open);
                    cx.notify();
                }));
            } else {
                b = b.opacity(0.4);
            }
            buttons = buttons.child(b);
        }
        buttons = buttons.child(recorded(pane, "picker-buttons", &self.controls));

        let caret = || div().w(px(1.)).h(px(16.)).bg(rgb(0x7fb89a));
        let mut text = div().flex_1().min_w(px(0.)).flex().items_center().overflow_hidden().whitespace_nowrap();
        if typing {
            text = text.child(div().text_color(rgb(0xd9e8e1)).child(picker.query.clone()));
            if keyed {
                text = text.child(caret());
            }
        } else {
            if keyed {
                text = text.child(caret()).child(div().w(px(6.)));
            }
            text = text.child(div().text_color(rgb(0x6f8a7d)).text_ellipsis().overflow_hidden().child(pane_picker::placeholder(total)));
        }
        let field_pane = pane.to_owned();
        let field = div().id(gpui::ElementId::Name(format!("picker-field-{pane}").into())).relative().h(px(FIELD_H)).w(px(column)).px_3().flex().items_center().gap_2().rounded_md()
            .bg(rgb(0x22302a)).border_1().border_color(rgb(if keyed { 0x7fb89a } else { 0x3d5247 })).cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.picker_focus_field(&field_pane);
                cx.notify();
            }))
            .child(text)
            .child(div().flex_none().text_xs().text_color(rgb(0x8fa89b)).child(pane_picker::count(&picker.query, shown, total)))
            .child(recorded(pane, "picker-field", &self.controls));

        let list: gpui::AnyElement = if typing && hits.is_empty() {
            div().flex_1().min_h(px(0.)).pt(px(40.)).flex().flex_col().items_center().gap_1()
                .child(div().text_color(rgb(0xd9e8e1)).child(pane_picker::no_match(&picker.query)))
                .child(div().text_xs().text_color(rgb(0x8fa89b)).child("esc clears")).into_any_element()
        } else {
            let mut list = div().id(gpui::ElementId::Name(format!("picker-list-{pane}").into())).flex_1().min_h(px(0.)).overflow_y_scroll().flex().flex_col().pb_1();
            if typing {
                for (i, hit) in hits.iter().enumerate() {
                    list = list.child(self.picker_row(pane, hit, i == picker.selected, column, cx));
                }
            } else {
                let (plugins, artifacts) = self.picker_list(pane);
                for (heading, items) in [("Plugin panes", plugins), ("Artifacts", artifacts)] {
                    if items.is_empty() {
                        continue;
                    }
                    list = list.child(div().px(px(10.)).pt(px(8.)).pb(px(3.)).flex().gap_2().text_xs().text_color(rgb(0x8fa89b))
                        .child(heading).child(items.len().to_string()));
                    for item in items {
                        let hit = Hit { item, score: 0., name_match: vec![], title_match: vec![] };
                        list = list.child(self.picker_row(pane, &hit, false, column, cx));
                    }
                }
                if let Some(error) = self.picker_artifact_error(pane) {
                    let pid = pane.to_owned();
                    list = list.child(div().px(px(10.)).pt(px(8.)).flex().flex_col().gap_1().text_xs()
                        .child(div().text_color(rgb(0xe08070)).child(format!("Couldn’t list artifacts: {error}")))
                        .child(div().id(gpui::ElementId::Name(format!("picker-retry-{pane}").into())).w_auto().px_2().py_1().rounded_sm().bg(rgb(0x31483e)).text_color(rgb(0xd9e8e1)).cursor_pointer().child("Try Again")
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.picker_relist(&pid);
                                cx.notify();
                            }))));
                }
            }
            list.into_any_element()
        };

        let mut hints = div().flex_none().h(px(26.)).w(px(column)).flex().justify_center().items_center().gap_4().overflow_hidden().whitespace_nowrap().border_t_1().border_color(rgb(0x2c3d34)).text_xs().text_color(rgb(0x8fa89b));
        for (symbol, word) in pane_picker::HINTS {
            hints = hints.child(div().flex().gap_1().child(symbol).child(word));
        }
        div().absolute().left_0().top(px(TITLE)).w(px(width)).h(px(height)).flex().flex_col().items_center().pt(px(top)).pb(px(4.)).gap(px(GAP))
            .child(buttons)
            .child(field)
            .child(div().w(px(column)).flex_1().min_h(px(0.)).flex().flex_col().child(list))
            .child(hints)
    }

    /// One list row: the kind's symbol, the name with its matched letters in the accent colour,
    /// the title under it when it differs, the kind word at the right and ↩ on the selected row.
    fn picker_row(&self, pane: &str, hit: &Hit, selected: bool, column: f32, cx: &mut Context<Client>) -> gpui::Stateful<gpui::Div> {
        let item: &Item = &hit.item;
        let lit = |text: &str, offsets: &[usize]| {
            let accent = HighlightStyle { color: Some(rgb(0x7fb89a).into()), ..Default::default() };
            StyledText::new(text.to_owned()).with_highlights(pane_picker::byte_ranges(text, offsets).into_iter().map(|r| (r, accent)).collect::<Vec<_>>())
        };
        let mut words = div().flex_1().min_w(px(0.)).flex().flex_col().overflow_hidden()
            .child(div().whitespace_nowrap().text_ellipsis().overflow_hidden().text_color(rgb(0xd9e8e1)).child(lit(&item.name, &hit.name_match)));
        if item.has_title() {
            words = words.child(div().text_xs().whitespace_nowrap().text_ellipsis().overflow_hidden().text_color(rgb(0x8fa89b)).child(lit(&item.title, &hit.title_match)));
        }
        let (pid, open) = (pane.to_owned(), item.clone());
        div().id(gpui::ElementId::Name(format!("picker-row-{pane}-{}", item.id).into())).flex_none().px(px(10.)).py(px(5.)).flex().items_center().gap_2().rounded_md().cursor_pointer()
            .border_1().border_color(rgb(if selected { 0x7fb89a } else { 0x17201c }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.focus_pane(&pid);
                this.picker_open(&pid, &open);
                cx.notify();
            }))
            .child(div().w(px(16.)).flex_none().flex().justify_center().children(title::symbol(&self.native, &self.symbols, item.kind.symbol(), 13., 0x8fa89b)))
            .child(words)
            // A narrow column gives the name the kind word's room.
            .children((column >= 240.).then(|| div().flex_none().text_xs().text_color(rgb(0x8fa89b)).child(item.kind.word())))
            .child(div().flex_none().w(px(12.)).text_xs().text_color(rgb(0x8fa89b)).child(if selected { "↩" } else { "" }))
    }
}
