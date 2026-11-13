//! The Pane info popover (PaneInfoView.swift): the header's ⓘ button, or Pane Info in the ⋯ menu,
//! opens it under that pane's header with the fields wheeld built for the pane, each with a button
//! that copies its value. The fields come from `pane_info` (AppModel.requestPaneInfo).
use super::*;
use super::settings::{DIM, LINE, PANEL, TEXT};
use crate::tooltip::tip;
use wheel_proto::InfoField;
use serde_json::Value;

/// The popover's width (`PaneInfoView` is framed 360 wide), and the label column's.
const WIDTH: f32 = 360.;
const LABEL_W: f32 = 84.;

/// The popover open over one pane: its request, and what wheeld answered (none while it waits).
#[derive(Clone, Debug, Default)]
pub struct PaneInfo {
    pub pane: String,
    req: u64,
    pub fields: Option<Vec<InfoField>>,
    /// wheeld's refusal, shown where the spinner was.
    pub failure: Option<String>,
    /// The label of the field copied last.
    pub copied: Option<String>,
}

impl Client {
    /// Open the popover over `pane`, or close it when it is open there already (the button toggles),
    /// and ask wheeld what the pane is; the last answer is dropped first so no earlier values show.
    pub fn toggle_pane_info(&mut self, pane: &str) {
        if self.pane_info.as_ref().is_some_and(|i| i.pane == pane) {
            return self.close_pane_info();
        }
        self.close_pane_menu();
        let req = self.req();
        self.log(format!("pane info opened for {pane}"));
        self.pane_info = Some(PaneInfo { pane: pane.to_owned(), req, ..Default::default() });
        self.send(&ClientMsg::PaneInfo { req: Some(req), pane: pane.to_owned() });
        self.sync_overlay();
    }
    pub fn close_pane_info(&mut self) {
        if self.pane_info.take().is_some() {
            self.sync_overlay();
        }
    }
    /// Put field `ix`'s value on the clipboard (PaneInfoView.copy).
    pub fn copy_info_field(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(info) = self.pane_info.as_mut() else { return };
        let Some(field) = info.fields.as_ref().and_then(|f| f.get(ix)).cloned() else { return };
        info.copied = Some(field.label.clone());
        self.log(format!("pane info: copied {}, {} character(s)", field.label, field.value.chars().count()));
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(field.value));
    }
    /// wheeld's answer to the popover's `pane_info`, or its refusal.
    pub(super) fn on_pane_info_message(&mut self, message: &ServerMsg) -> bool {
        let Some(info) = self.pane_info.as_mut() else { return false };
        match message {
            ServerMsg::PaneInfo { req, fields, .. } if *req == info.req => {
                info.fields = Some(fields.clone());
                true
            }
            ServerMsg::Error { req: Some(req), message, .. } if *req == info.req => {
                info.failure = Some(message.clone());
                true
            }
            _ => false,
        }
    }
    /// The popover for `/state`.
    pub fn pane_info_json(&self) -> Value {
        let Some(i) = &self.pane_info else { return Value::Null };
        serde_json::json!({
            "pane": i.pane, "failure": i.failure, "copied": i.copied,
            "fields": i.fields.as_ref().map(|f| f.iter().map(|f| serde_json::json!({"label": f.label, "value": f.value, "hint": f.hint})).collect::<Vec<_>>()),
        })
    }
    pub(super) fn pane_info_overlay(&mut self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let info = self.pane_info.clone().expect("drawn only while open");
        let mut card = div().w(px(WIDTH)).flex().flex_col().gap(px(10.)).p(px(14.)).rounded_md().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT)).text_sm()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Pane info"));
        match (&info.fields, &info.failure) {
            (Some(fields), _) => {
                for (ix, field) in fields.iter().enumerate() {
                    let copy = self.symbol("doc.on.doc", 11., 0x8fa89b).unwrap_or_else(|| div().text_xs().child("copy").into_any_element());
                    let mut value = div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                        .child(div().text_xs().font_family("Menlo").child(field.value.clone()));
                    if let Some(hint) = &field.hint {
                        value = value.child(div().text_xs().text_color(rgb(DIM)).child(hint.clone()));
                    }
                    card = card.child(div().flex().items_start().gap(px(12.))
                        .child(div().w(px(LABEL_W)).flex_none().text_color(rgb(0x8fa89b)).child(field.label.clone()))
                        .child(value)
                        .child(div().id(gpui::ElementId::Name(format!("pane-info-copy-{ix}").into())).flex_none().cursor_pointer().tooltip(tip(&format!("Copy {}", field.label)))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.copy_info_field(ix, cx);
                                cx.notify();
                            }))
                            .child(copy)));
                }
            }
            (None, Some(failure)) => card = card.child(div().text_xs().text_color(rgb(DIM)).child(failure.clone())),
            (None, None) => card = card.child(div().text_xs().text_color(rgb(DIM)).child("Loading…")),
        }
        // Under the pane's header, at its right edge, as the Swift popover hangs from the header.
        let (leaves, _) = self.scene();
        let r = leaves.iter().find(|l| l.leaf.id == info.pane).map(|l| l.rect).unwrap_or(Rect { x: width - WIDTH - 8., y: HEADER, w: WIDTH, h: 0. });
        let left = (r.x + r.w - WIDTH - 8.).clamp(4., (width - WIDTH - 4.).max(4.));
        let top = (r.y + TITLE + 4.).min((height - 200.).max(HEADER));
        div().absolute().left_0().top_0().w(px(width)).h(px(height))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.close_pane_info();
                cx.notify();
            }))
            .child(div().absolute().left(px(left)).top(px(top)).child(card))
    }
}
