//! The process card (ProcessesView.swift) as the client draws it: the title row's Processes
//! button opens a card over the window with a filter field, a row per process (name, pane, CPU)
//! and, under the chosen row, its command line, directory and a Stop button. The model is
//! `crate::procs`.
use super::*;
use super::settings::{BOX, DIM, LINE, PANEL, TEXT, button, click, now_ms};
use crate::procs::{REFRESH_MS, detail};

impl Client {
    /// The button: open the card for the space shown (and list), or close it.
    pub fn toggle_processes(&mut self) {
        if self.procs.open.is_some() {
            self.close_processes();
        } else if let Some(space) = self.space_id.clone() {
            self.open_processes(&space);
        }
    }
    pub fn open_processes(&mut self, space: &str) {
        self.procs.open_for(space);
        self.log(format!("processes card opened for space {space}"));
        self.request_procs(space);
        self.sync_overlay();
    }
    pub fn close_processes(&mut self) {
        if let Some(space) = self.procs.open.clone() {
            self.log(format!("processes card closed for space {space}"));
        }
        self.procs.close();
        self.sync_overlay();
    }
    /// Ask wheeld for the processes under `space`; the answer lands in `procs.by_space`.
    pub fn request_procs(&mut self, space: &str) {
        // Asked-at moves even when the link is down, so an open card waits its 2 s before trying again.
        self.procs.asked_at = now_ms();
        if self.link != Link::Up {
            return;
        }
        let req = self.req();
        self.send(&ClientMsg::SpaceProcs { req: Some(req), space: space.to_owned() });
    }
    /// Stop one of those processes, then list again so it leaves the card.
    pub fn stop_proc(&mut self, space: &str, pid: u32, force: bool) {
        self.log(format!("process {pid} of space {space} asked to stop"));
        self.send(&ClientMsg::ProcStop { req: None, space: space.to_owned(), pid, force });
        self.procs.asked_at = 0;
    }
    /// A `procs` reply. Returns whether it was one.
    pub fn on_procs_message(&mut self, message: &ServerMsg) -> bool {
        let ServerMsg::Procs { space, procs, .. } = message else { return false };
        self.log(format!("space {space} processes: {}", procs.iter().map(|p| format!("{}={}@{}", p.pid, p.name, p.pane.as_deref().unwrap_or("detached"))).collect::<Vec<_>>().join(" ")));
        self.procs.listed(space, procs.clone());
        true
    }
    /// An open card asks again every 2 s, and a stop asks at once.
    pub fn tick_procs(&mut self, cx: &mut Context<Self>) {
        let Some(space) = self.procs.open.clone() else { return };
        let now = now_ms();
        if self.procs.due(now) {
            self.request_procs(&space);
        }
        let wait = (self.procs.asked_at + REFRESH_MS).saturating_sub(now).max(50);
        self.redraw_in(wait, cx);
    }
    /// A key while the card is open: it types into the filter; Esc closes the card. The card
    /// takes every key.
    pub fn procs_key(&mut self, key: &str, text: Option<&str>, chord: bool) {
        match key {
            "escape" => self.close_processes(),
            "backspace" if !chord => {
                self.procs.filter.pop();
            }
            _ if !chord => {
                if let Some(t) = text.filter(|t| !t.chars().any(char::is_control)) {
                    self.procs.filter.push_str(t);
                }
            }
            _ => {}
        }
    }

    pub(super) fn processes_overlay(&mut self, width: f32, height: f32, cx: &mut Context<Self>) -> gpui::Div {
        let space = self.procs.open.clone().unwrap_or_default();
        let rows: Vec<wheel_proto::ProcInfo> = self.procs.rows(&space).into_iter().cloned().collect();
        let chosen = self.procs.chosen;
        let field: gpui::Div = if self.procs.filter.is_empty() {
            div().text_color(rgb(DIM)).child("Filter")
        } else {
            div().child(self.procs.filter.clone())
        };
        let mut list = div().flex().flex_col().gap(px(2.)).w_full().max_h(px(320.)).overflow_hidden();
        if rows.is_empty() {
            list = list.child(div().w_full().min_h(px(60.)).flex().items_center().justify_center().text_color(rgb(DIM)).child("No processes are reporting activity"));
        }
        for p in rows {
            let (pid, open) = (p.pid, chosen == Some(p.pid));
            let head = div().flex().items_center().gap_2().w_full().cursor_pointer().py(px(3.))
                .on_mouse_down(MouseButton::Left, click(cx, move |this| this.procs.toggle_chosen(pid)))
                .child(div().font_weight(FontWeight::MEDIUM).whitespace_nowrap().overflow_hidden().child(p.name.clone()))
                .child(div().text_color(rgb(DIM)).whitespace_nowrap().overflow_hidden().child(p.pane.clone().unwrap_or_else(|| "detached".into())))
                .child(div().flex_1())
                .child(div().text_color(rgb(DIM)).font_family("Menlo").child(format!("{:.1}%", p.cpu)));
            let mut row = div().flex().flex_col().gap_1().w_full().child(head);
            if open {
                let space_for_stop = space.clone();
                let mut details = div().flex().flex_col().gap_1().pl_2().text_xs()
                    .child(div().text_color(rgb(DIM)).child(detail(&p)))
                    .child(div().font_family("Menlo").child(p.command.clone()));
                if let Some(cwd) = &p.cwd {
                    details = details.child(div().text_color(rgb(DIM)).child(cwd.clone()));
                }
                details = details.child(div().flex().child(button("Stop", click(cx, move |this| this.stop_proc(&space_for_stop, pid, false))).text_color(rgb(0xff6b6b))));
                row = row.child(details);
            }
            list = list.child(row);
        }
        let card = div().w(px(380f32.min(width - 32.))).flex().flex_col().gap_2().p_3().rounded_md().bg(rgb(PANEL)).border_1().border_color(rgb(LINE)).text_color(rgb(TEXT)).text_sm()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(div().w_full().flex().justify_between().items_center()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Processes"))
                .child(div().px_2().rounded_sm().cursor_pointer().bg(rgb(BOX)).child("×").on_mouse_down(MouseButton::Left, click(cx, |this| this.close_processes()))))
            .child(div().w_full().px_2().py_1().rounded_sm().bg(rgb(BOX)).border_1().border_color(rgb(LINE)).child(field))
            .child(list);
        // The card hangs under its button, as the Swift popover does; a press on the dim closes it.
        let anchor = self.header_chips().into_iter().find(|c| c.hit == Hit::Processes).map(|c| c.rect)
            .unwrap_or(Rect { x: width - header::TRAILING - header::BUTTON_W, y: header::BUTTON_Y, w: header::BUTTON_W, h: header::BUTTON_H });
        div().absolute().left_0().top_0().w(px(width)).h(px(height)).bg(rgba(0x00000055))
            .on_mouse_down(MouseButton::Left, click(cx, |this| this.close_processes()))
            .child(div().absolute().top(px(anchor.y + anchor.h + 6.)).right(px(width - anchor.x - anchor.w)).child(card))
    }
}
