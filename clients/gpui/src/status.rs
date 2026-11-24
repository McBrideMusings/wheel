//! A pane's or space's status drawn as a mark: a 9 px dot or, while a turn runs, a 12 px arc
//! turning once a second. The words, ranks and ordering are `wheel_gpui_core::status`.
use gpui::{AnyElement, Bounds, PathBuilder, Pixels, canvas, div, point, prelude::*, px, rgb};
use std::{collections::HashSet, f32::consts::PI};
use wheel_gpui_core::layout::{Space, leaves_of};
use wheel_proto::PaneStatus;
pub use wheel_gpui_core::status::*;

/// The mark for `status`, or none for a rank `shown` leaves out, its tooltip the status's
/// `label`. `id` keeps the arc's turn going from frame to frame; a working mark takes the arc's 12 px slot, every other a 9 px dot
/// centred in the same slot so a mark never moves its neighbours when its rank changes.
pub fn dot(status: &PaneStatus, shown: &HashSet<&'static str>, id: impl Into<gpui::ElementId>) -> Option<AnyElement> {
    dot_with(status, shown, id, None)
}

/// `dot`, its working arc drawn by `spinner` when given: a view of its own, so the arc's frames
/// redraw it and not the view around it.
pub fn dot_with(status: &PaneStatus, shown: &HashSet<&'static str>, id: impl Into<gpui::ElementId>, spinner: Option<gpui::AnyView>) -> Option<AnyElement> {
    if !shown.contains(rank(status)) {
        return None;
    }
    let id: gpui::ElementId = id.into();
    // Hovering the mark names its status, as the Swift client's `.help(status.label)` does.
    let slot = div().id(id.clone()).tooltip(crate::tooltip::tip(&label(status))).w(px(12.)).h(px(12.)).flex().items_center().justify_center().flex_none();
    let mark = match status {
        PaneStatus::Working => return Some(match spinner {
            Some(view) => slot.child(view),
            None => slot.child(arc(id)),
        }.into_any_element()),
        PaneStatus::NeedsYou { .. } => div().w(px(9.)).h(px(9.)).rounded_full().bg(rgb(ORANGE)),
        PaneStatus::Stopped => div().w(px(9.)).h(px(9.)).rounded_full().border(px(1.8)).border_color(rgb(RED)),
        PaneStatus::Done => div().w(px(9.)).h(px(9.)).rounded_full().bg(rgb(BLUE)),
        PaneStatus::Idle => div().w(px(9.)).h(px(9.)).rounded_full().bg(rgb(GREEN)),
    };
    Some(slot.child(mark).into_any_element())
}

/// The working arc as a view of its own: the window's one spin clock (`Client::spinner`) notifies
/// it, so a cached pane body beside it replays instead of drawing again.
pub struct Spinner {
    pub id: gpui::ElementId,
}

impl gpui::Render for Spinner {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        arc(self.id.clone())
    }
}

/// How often the arc steps: each step redraws the whole window, so it turns in eight steps a
/// second rather than once per display refresh.
pub const SPIN_STEP_MS: u64 = 125;

/// The 3/4 arc, 1.8 px wide with round ends, turning clockwise once a second in eight steps. Its
/// angle comes from the clock, so every arc in the window shows the same step.
fn arc(_id: impl Into<gpui::ElementId>) -> AnyElement {
    let steps = 1000 / SPIN_STEP_MS;
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let angle = ((ms / SPIN_STEP_MS) % steps) as f32 / steps as f32 * 2. * PI;
    div().w(px(12.)).h(px(12.))
        .child(canvas(|_, _, _| (), move |bounds: Bounds<Pixels>, _, window, _| paint_arc(bounds, angle, window)).w(px(12.)).h(px(12.)))
        .into_any_element()
}

fn paint_arc(bounds: Bounds<Pixels>, angle: f32, window: &mut gpui::Window) {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let (cx, cy, r) = (w / 2., h / 2., (w.min(h) - 1.8) / 2.);
    let mut path = PathBuilder::stroke(px(1.8));
    let start = angle;
    let end = angle + 1.5 * PI;
    path.move_to(point(px(cx + r * start.cos()), px(cy + r * start.sin())));
    path.arc_to(point(px(r), px(r)), px(0.), true, true, point(px(cx + r * end.cos()), px(cy + r * end.sin())));
    path.translate(bounds.origin);
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(BLUE));
    }
}

/// A space's agents as marks: one per agent pane, most urgent first, the 12 px slots overlapping
/// by 2 px, at most `AGENT_DOT_LIMIT` and then `+N`; the space's own status when no pane sent
/// one. None when nothing draws a mark.
pub fn space_dots(space: &Space, shown: &HashSet<&'static str>) -> Option<gpui::Div> {
    if leaves_of(&space.root).iter().all(|l| l.status.is_none()) {
        // A daemon that predates per-pane statuses sends the space's alone.
        return space.status.as_ref().and_then(|s| dot(s, shown, gpui::ElementId::Name(format!("space-dot-{}", space.id).into()))).map(|mark| div().flex().items_center().flex_none().child(mark));
    }
    let (marks, more) = agent_statuses(space, shown, AGENT_DOT_LIMIT);
    if marks.is_empty() {
        return None;
    }
    let mut row = div().flex().items_center().gap(px(3.)).flex_none();
    let mut dots = div().flex().items_center().flex_none();
    for (i, status) in marks.iter().enumerate() {
        if let Some(mark) = dot(status, shown, gpui::ElementId::Name(format!("space-dot-{}-{i}", space.id).into())) {
            dots = dots.child(if i > 0 { div().ml(px(-2.)).child(mark) } else { div().child(mark) });
        }
    }
    row = row.child(dots);
    if more > 0 {
        row = row.child(div().text_xs().text_color(rgb(0x8fa89b)).child(format!("+{more}")));
    }
    Some(row)
}
