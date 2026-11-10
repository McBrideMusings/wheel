//! The window's content: a header with the spaces, the active space's split tree drawn as
//! panes and draggable dividers, and the real Ghostty and WKWebView views on the first
//! terminal and first web pane. Every other kind is drawn as unimplemented.
use crate::{daemon::Endpoint, layout::*, native::Native};
use gpui::{Context, MouseButton, MouseMoveEvent, MouseUpEvent, Window, div, prelude::*, px, rgb};
use serde_json::json;
use std::{collections::HashMap, rc::Rc, sync::mpsc};

pub const TITLE: f32 = 22.0;
pub const HEADER: f32 = 34.0;

pub struct Client {
    native: Rc<Native>,
    out: mpsc::Sender<String>,
    endpoint: Endpoint,
    spaces: Vec<Space>,
    /// The space shown: kept across layouts by id.
    space_id: Option<String>,
    /// Ratios being dragged, until the daemon confirms them in its next layout.
    overrides: HashMap<String, f64>,
    drag: Option<(String, bool, Rect)>,
    /// Panes drawn by the native views: (web, terminal).
    pub native_panes: (Option<String>, Option<String>),
    attached_term: Option<String>,
    loaded_web: Option<String>,
    viewport: (f32, f32),
}

impl Client {
    pub fn new(native: Rc<Native>, out: mpsc::Sender<String>, endpoint: Endpoint, prefer_space: Option<String>) -> Self {
        Self {
            native, out, endpoint, spaces: vec![], space_id: prefer_space, overrides: HashMap::new(), drag: None,
            native_panes: (None, None), attached_term: None, loaded_web: None, viewport: (1200., 720.),
        }
    }
    /// The daemon's layout replaces ours, dragged ratios included.
    pub fn apply(&mut self, spaces: Vec<Space>) {
        if !spaces.iter().any(|s| Some(&s.id) == self.space_id.as_ref()) {
            self.space_id = spaces.first().map(|s| s.id.clone());
        }
        self.spaces = spaces;
        self.overrides.clear();
    }
    fn space(&self) -> Option<&Space> {
        self.spaces.iter().find(|s| Some(&s.id) == self.space_id.as_ref())
    }
    pub fn scene(&self) -> (Vec<Placed>, Vec<Divider>) {
        let (mut leaves, mut dividers) = (vec![], vec![]);
        if let Some(space) = self.space() {
            let area = Rect { x: 0., y: HEADER, w: self.viewport.0, h: self.viewport.1 - HEADER };
            place(&space.root, area, &self.overrides, &mut leaves, &mut dividers);
        }
        (leaves, dividers)
    }
    pub fn tree_ratio(&self, split: &str) -> Option<f64> {
        fn find(t: &Tree, id: &str) -> Option<f64> {
            match t {
                Tree::Split { id: i, ratio, a, b, .. } => if i == id { Some(*ratio) } else { find(a, id).or_else(|| find(b, id)) },
                _ => None,
            }
        }
        self.space().and_then(|s| find(&s.root, split))
    }
    pub fn request_status(&self, req: u64) {
        let _ = self.out.send(json!({"op":"status","req":req}).to_string());
    }
    fn end_drag(&mut self) {
        if let Some((split, _, _)) = self.drag.take() {
            if let Some(ratio) = self.overrides.get(&split) {
                let _ = self.out.send(json!({"op":"split_ratio","split":split,"ratio":ratio}).to_string());
            }
        }
    }
    fn move_drag(&mut self, x: f32, y: f32) {
        if let Some((split, horizontal, parent)) = &self.drag {
            self.overrides.insert(split.clone(), drag_ratio(*horizontal, *parent, x, y));
        }
    }
    /// Point the native views at the panes they draw: attach the terminal and load the page
    /// when the pane changes, move each view onto its pane's body.
    fn sync_native(&mut self, leaves: &[Placed]) {
        let web = leaves.iter().find(|p| p.leaf.kind == "web");
        let term = leaves.iter().find(|p| p.leaf.kind == "terminal");
        self.native_panes = (web.map(|p| p.leaf.id.clone()), term.map(|p| p.leaf.id.clone()));
        let term_id = term.and_then(|p| p.leaf.term.clone());
        if term_id != self.attached_term {
            if let Some(id) = &term_id {
                self.native.call(json!({"op":"attach_term","id":id,"url":self.endpoint.ws}));
            }
            self.attached_term = term_id;
        }
        if self.native_panes.0 != self.loaded_web {
            if let Some(pane) = &self.native_panes.0 {
                self.native.call(json!({"op":"load","url":self.endpoint.artifact(pane)}));
            }
            self.loaded_web = self.native_panes.0.clone();
        }
        for (index, placed) in [(0, web), (1, term)] {
            unsafe {
                match placed {
                    Some(p) => (self.native.layout)(index, f64::from(p.rect.x), f64::from(p.rect.y + TITLE), f64::from(p.rect.w), f64::from((p.rect.h - TITLE).max(1.))),
                    None => (self.native.layout)(index, 0., 0., 1., 1.),
                }
            }
            self.native.call(json!({"op":"slot","index":index,"hidden":placed.is_none()}));
        }
    }
}

impl Render for Client {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = window.viewport_size();
        self.viewport = (f32::from(v.width), f32::from(v.height));
        let (leaves, dividers) = self.scene();
        self.sync_native(&leaves);
        let native_ids = [self.native_panes.0.clone(), self.native_panes.1.clone()];
        let mut root = div().relative().size_full().bg(rgb(0x0e1311)).text_color(rgb(0xd9e8e1)).text_sm()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    this.move_drag(f32::from(e.position.x), f32::from(e.position.y));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                this.end_drag();
                cx.notify();
            }));
        let mut header = div().absolute().left(px(10.)).top(px(6.)).h(px(HEADER - 8.)).flex().gap_3().items_center();
        if self.spaces.is_empty() {
            header = header.child("waiting for wheeld");
        }
        for s in &self.spaces {
            let (id, active) = (s.id.clone(), Some(&s.id) == self.space_id.as_ref());
            header = header.child(
                div().px_2().rounded_sm().cursor_pointer()
                    .bg(rgb(if active { 0x31483e } else { 0x17201c }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.space_id = Some(id.clone());
                        this.overrides.clear();
                        cx.notify();
                    }))
                    .child(s.name.clone()),
            );
        }
        root = root.child(header);
        for p in &leaves {
            let r = p.rect;
            let native = native_ids.iter().any(|id| id.as_deref() == Some(&p.leaf.id));
            let body = if native { String::new() } else { format!("unimplemented: {} pane\n{}", p.leaf.kind, p.leaf.label) };
            root = root.child(
                div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).bg(rgb(0x17201c))
                    .border_1().border_color(rgb(0x2c3d34))
                    .child(div().h(px(TITLE)).px_2().bg(rgb(0x22302a)).text_xs().child(format!("{} · {}", p.leaf.kind, p.leaf.id)))
                    .child(div().p_2().text_color(rgb(0x8fa89b)).child(body)),
            );
        }
        for d in dividers {
            let (split, horizontal, parent, r) = (d.split.clone(), d.horizontal, d.parent, d.rect);
            let bar = div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).bg(rgb(0x3d5247))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.drag = Some((split.clone(), horizontal, parent));
                    cx.notify();
                }));
            root = root.child(if horizontal { bar.cursor_col_resize() } else { bar.cursor_row_resize() });
        }
        root
    }
}
