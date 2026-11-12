//! The tray chip over the shown space's grid and the list it opens (TrayChip.swift): one row per
//! backgrounded pane, which puts the pane back in the grid; its pop-out button pops it out.
use super::*;
use crate::tooltip::tip;
use crate::tray::{self, Drag, Place};
use serde_json::{Value, json};
use wheel_proto::PaneSite;

impl Client {
    /// The panes in the shown space's tray, in the order wheeld lists them; none while the grid
    /// draws nothing of its own (a little window shows the space, the desk is up).
    pub fn tray_panes(&self) -> Vec<Leaf> {
        if self.space_away() || self.mode == "desk" {
            return vec![];
        }
        self.space().map(|s| s.backgrounded.iter().flat_map(leaves_of).cloned().collect()).unwrap_or_default()
    }
    /// Where the chip draws (window coordinates): at its place on the grid's edge, or under the
    /// pointer while it is dragged; None while the space has nothing in its tray.
    pub fn tray_chip_rect(&self) -> Option<Rect> {
        if self.tray_panes().is_empty() {
            return None;
        }
        let area = self.grid_area();
        let (x, y) = match self.tray_drag.as_ref().filter(|d| d.moved) {
            Some(d) => (d.at.0 - tray::SIZE / 2., d.at.1 - tray::SIZE / 2.),
            None => {
                let (x, y) = self.tray_store.borrow().place.origin(area.w, area.h);
                (area.x + x, area.y + y)
            }
        };
        Some(Rect { x, y, w: tray::SIZE, h: tray::SIZE })
    }
    /// The list while it is open: beside the chip, away from its edge.
    pub fn tray_list_rect(&self) -> Option<Rect> {
        if !self.tray_is_open() || self.tray_drag.is_some() {
            return None;
        }
        let chip = self.tray_chip_rect()?;
        Some(tray::list_rect(chip, self.tray_store.borrow().place.edge, self.tray_panes().len(), self.grid_area()))
    }
    /// What the chip and its list cover, cut out of every native view so they draw over terminals and pages.
    pub(super) fn tray_holes(&self) -> Vec<Rect> {
        self.tray_chip_rect().into_iter().chain(self.tray_list_rect()).collect()
    }
    /// The list closes, as a click away from a popover closes it.
    pub fn close_tray(&mut self) {
        if let Some(space) = self.tray_open.take() {
            self.log(format!("tray list {space}: closed"));
        }
    }
    /// The list is open for the space shown, which still has a pane in its tray (AppModel.trayOpen).
    pub fn tray_is_open(&self) -> bool {
        self.tray_open.is_some() && self.tray_open == self.space_id && !self.tray_panes().is_empty()
    }
    fn toggle_tray(&mut self) {
        if self.tray_is_open() {
            self.close_tray();
        } else if let Some(space) = self.space_id.clone().filter(|_| !self.tray_panes().is_empty()) {
            self.log(format!("tray list {space}: opened"));
            self.tray_open = Some(space);
        }
    }
    /// A press on the chip: a click opens or closes the list, a drag moves the chip.
    pub fn tray_press(&mut self, x: f32, y: f32) {
        self.tray_drag = Some(Drag { start: (x, y), at: (x, y), moved: false });
    }
    pub(super) fn tray_move(&mut self, x: f32, y: f32) {
        let Some(drag) = &mut self.tray_drag else { return };
        drag.at = (x, y);
        if !drag.moved && (x - drag.start.0).hypot(y - drag.start.1) >= tray::DRAG_DISTANCE {
            drag.moved = true;
            self.tray_open = None;
        }
    }
    /// The release at (`x`, `y`): a drag pins the chip to the edge nearest where it was let go, for
    /// every space; a click toggles the list.
    pub(super) fn tray_release(&mut self, x: f32, y: f32) {
        let Some(drag) = self.tray_drag.take() else { return };
        if !drag.moved {
            self.toggle_tray();
            return;
        }
        if self.tray_panes().is_empty() {
            return;
        }
        let area = self.grid_area();
        let place = Place::nearest(x - area.x, y - area.y, area.w, area.h);
        self.tray_store.borrow_mut().set(place);
        self.log(format!("tray chip: {}", place.text()));
    }
    /// A tray row: the pane goes to `site`, and the list closes.
    pub fn tray_choose(&mut self, pane: &str, site: PaneSite) {
        self.close_tray();
        self.set_site(pane, site);
    }
    /// The chip and, while open, the list, over the grid and its cards.
    pub(super) fn tray_elements(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let Some(chip) = self.tray_chip_rect() else { return vec![] };
        let panes = self.tray_panes();
        let needs_you = panes.iter().any(|l| matches!(l.status, Some(PaneStatus::NeedsYou { .. })));
        let icon = self.symbol("square.stack", 15., 0xd9e8e1).unwrap_or_else(|| div().child("▤").into_any_element());
        let mut out = vec![];
        let mut el = div().id("tray-chip").absolute().left(px(chip.x)).top(px(chip.y)).w(px(chip.w)).h(px(chip.h)).occlude()
            .rounded(px(9.)).bg(rgb(0x1b2621)).border_1().border_color(rgb(0x3d5247)).shadow_md()
            .flex().items_center().justify_center().cursor_pointer().child(icon)
            .tooltip(tip(if needs_you { "Backgrounded panes, one needs you" } else { "Backgrounded panes" }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.tray_press(f32::from(e.position.x), f32::from(e.position.y));
                cx.notify();
            }))
            // A release before the next frame draws the capture layer lands on the chip itself.
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, cx| this.window_mouse_up(e, cx)));
        if needs_you {
            el = el.child(div().absolute().top(px(-3.)).right(px(-3.)).w(px(10.)).h(px(10.)).rounded_full().bg(rgb(0xff9f0a)));
        }
        out.push(el.into_any_element());
        let Some(list) = self.tray_list_rect() else { return out };
        let mut card = div().id("tray-list").absolute().left(px(list.x)).top(px(list.y)).w(px(list.w)).h(px(list.h)).occlude()
            .py(px(tray::PAD)).flex().flex_col().rounded_md().bg(rgb(0x17201c)).border_1().border_color(rgb(0x3d5247)).shadow_lg()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()));
        for leaf in &panes {
            let (row_pane, pop_pane) = (leaf.id.clone(), leaf.id.clone());
            let kind = self.symbol(tray::icon(&leaf.kind), 12., 0x8fa89b).unwrap_or_else(|| div().into_any_element());
            let pop = self.symbol("rectangle.on.rectangle", 11., 0xb7cbc1).unwrap_or_else(|| div().text_xs().child("⧉").into_any_element());
            card = card.child(div().id(gpui::ElementId::Name(format!("tray-row-{}", leaf.id).into())).h(px(tray::ROW)).flex_none().pl_3().pr(px(tray::POP_INSET)).flex().items_center().gap_2()
                .hover(|s| s.bg(rgb(0x22302a))).cursor_pointer()
                // A row and its button act on release, as a button does.
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tray_choose(&row_pane, PaneSite::Grid);
                    cx.notify();
                }))
                .child(div().w(px(18.)).flex_none().flex().justify_center().child(kind))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(tray::title(leaf)))
                .child(div().id(gpui::ElementId::Name(format!("tray-pop-{}", leaf.id).into())).w(px(tray::POP)).h(px(tray::POP)).flex_none().rounded_sm().flex().items_center().justify_center()
                    .hover(|s| s.bg(rgb(0x31483e))).tooltip(tip("Pop out")).child(pop)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.tray_choose(&pop_pane, PaneSite::Popped);
                        cx.notify();
                    }))));
        }
        out.push(card.into_any_element());
        out
    }
    /// The tray as `/state` reports it: the chip, its place, the list and each row's rect.
    pub(super) fn tray_json(&self) -> Value {
        let at = |r: Rect| json!([r.x, r.y, r.w, r.h]);
        let place = self.tray_store.borrow().place;
        let list = self.tray_list_rect();
        let rows: Vec<Value> = self.tray_panes().iter().enumerate().map(|(i, l)| {
            let row = list.map(|r| Rect { x: r.x, y: r.y + tray::PAD + i as f32 * tray::ROW, w: r.w, h: tray::ROW });
            let pop = row.map(|r| Rect { x: r.x + r.w - tray::POP_INSET - tray::POP, y: r.y + (tray::ROW - tray::POP) / 2., w: tray::POP, h: tray::POP });
            json!({"pane": l.id, "title": tray::title(l), "icon": tray::icon(&l.kind), "frame": row.map(at), "pop": pop.map(at)})
        }).collect();
        json!({"chip": self.tray_chip_rect().map(at), "open": list.is_some(), "place": tray::edge_name(place.edge), "along": place.along,
            "list": list.map(at), "rows": rows, "dragging": self.tray_drag.as_ref().map(|d| d.moved)})
    }
}
