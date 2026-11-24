//! An agent pane's body (its transcript, request card, composer and status line) as a view of its
//! own, drawn cached: a frame that changes nothing it shows (a working arc turning, another pane's
//! scroll) replays it instead of laying it out again. The window's frame and every pane's header
//! still draw each frame.
use super::{Client, Placed};
use gpui::{AnyView, AppContext as _, Context, Entity, IntoElement, Render, StyleRefinement, WeakEntity, Window, px};
use std::collections::HashMap;

pub struct PaneBody {
    client: WeakEntity<Client>,
    pane: String,
}

impl Render for PaneBody {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = self.pane.clone();
        self.client
            .update(cx, |client, cx| client.draw_body(&pane, window, cx))
            .unwrap_or_else(|_| gpui::div())
    }
}

/// The bodies a window has made, by pane, and how many times each has drawn.
#[derive(Default)]
pub struct Bodies {
    views: HashMap<String, Entity<PaneBody>>,
    /// Where each body's pane is placed this frame, read when the body draws.
    pub placed: HashMap<String, Placed>,
    /// Each body's composer height when it last drew: the pane's selection band ends above it.
    pub composer_h: HashMap<String, f32>,
    pub draws: HashMap<String, u64>,
    /// The panes whose bodies this frame drew or replayed.
    seen: std::collections::HashSet<String>,
}

impl Bodies {
    /// The cached view for `p`'s body, made on its first frame.
    pub fn view(&mut self, p: &Placed, inner: (f32, f32), cx: &mut Context<Client>) -> AnyView {
        self.placed.insert(p.leaf.id.clone(), p.clone());
        self.seen.insert(p.leaf.id.clone());
        let client = cx.weak_entity();
        let entity = self.views.entry(p.leaf.id.clone()).or_insert_with(|| {
            let pane = p.leaf.id.clone();
            cx.new(|_| PaneBody { client, pane })
        });
        let mut style = StyleRefinement::default();
        style.size.width = Some(px(inner.0).into());
        style.size.height = Some(px(inner.1).into());
        AnyView::from(entity.clone()).cached(style)
    }
    /// Every body draws again on the next frame: whatever changed in the window may be in it.
    pub fn invalidate(&self, cx: &mut gpui::App) {
        for view in self.views.values() {
            view.update(cx, |_, cx| cx.notify());
        }
    }
    pub fn begin_frame(&mut self) {
        self.seen.clear();
    }
    /// Drop the bodies of panes this frame no longer draws, and return the panes it does.
    pub fn end_frame(&mut self) -> std::collections::HashSet<String> {
        let drawn = std::mem::take(&mut self.seen);
        self.keep(&drawn);
        drawn
    }
    fn keep(&mut self, drawn: &std::collections::HashSet<String>) {
        self.views.retain(|pane, _| drawn.contains(pane));
        self.placed.retain(|pane, _| drawn.contains(pane));
        self.composer_h.retain(|pane, _| drawn.contains(pane));
    }
}
