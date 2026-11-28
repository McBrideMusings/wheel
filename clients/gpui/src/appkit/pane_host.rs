//! The native pane host of one window (wheel-kq6t.1): a slot view per native pane, a subview of
//! GPUI's content view that holds the pane's content: a browser or a plain web page made here
//! (`web.rs`), or what the bridge adds (an artifact page, a plugin view). Here each slot is placed,
//! hidden, faded and masked around the cards and the overlay drawn over it, given the keyboard,
//! and reached by the check's synthetic pointer. A press that lands in a slot reaches the GPUI
//! view as a `pressed` frame, since GPUI never sees it.
use super::{main_thread, web::Pages};
use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSResponder, NSView};
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::{NSPoint, NSProcessInfo, NSRect, NSSize};
use objc2_quartz_core::CAShapeLayer;
use objc2_web_kit::WKWebView;
use serde_json::{Map, Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
};
use wheel_gpui_core::native::{Host, Mouse, MouseKind};

#[derive(Default)]
pub struct SlotIvars {
    /// The holes in the slot's own coordinates: neither drawn nor hit.
    holes: RefCell<Vec<NSRect>>,
    /// The cards over the slot as GPUI placed them (x, y, w, h from the window's top left).
    holes_top: RefCell<Vec<[f64; 4]>>,
    /// How many rects the mask keeps: 1 with no hole over the slot.
    shown: Cell<usize>,
}

define_class!(
    /// A pane's container. Its holes (an overlay's, and the cards drawn over it) neither draw nor hit-test.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WheelPaneSlot"]
    #[ivars = SlotIvars]
    pub struct Slot;

    impl Slot {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            let local = self.convertPoint_fromView(point, unsafe { self.superview() }.as_deref());
            if self.ivars().holes.borrow().iter().any(|h| contains(*h, local)) {
                return std::ptr::null_mut();
            }
            // SAFETY: NSView's own hitTest:, which returns an autoreleased view or nil.
            unsafe { msg_send![super(self), hitTest: point] }
        }
    }
);

fn contains(r: NSRect, p: NSPoint) -> bool {
    p.x >= r.origin.x && p.x < r.origin.x + r.size.width && p.y >= r.origin.y && p.y < r.origin.y + r.size.height
}

fn intersection(a: NSRect, b: NSRect) -> Option<NSRect> {
    let (x0, y0) = (a.origin.x.max(b.origin.x), a.origin.y.max(b.origin.y));
    let (x1, y1) = ((a.origin.x + a.size.width).min(b.origin.x + b.size.width), (a.origin.y + a.size.height).min(b.origin.y + b.size.height));
    (x1 > x0 && y1 > y0).then(|| NSRect::new(NSPoint::new(x0, y0), NSSize::new(x1 - x0, y1 - y0)))
}

/// What is left of `bounds` once every hole is taken away, as rects, so two holes that overlap stay holes.
pub fn remainder(bounds: NSRect, holes: &[NSRect]) -> Vec<NSRect> {
    let mut left = vec![bounds];
    for hole in holes {
        left = left
            .into_iter()
            .flat_map(|r| {
                let Some(h) = intersection(r, *hole) else { return vec![r] };
                let (rx, ry, rw, rh) = (r.origin.x, r.origin.y, r.size.width, r.size.height);
                let (hx, hy, hw, hh) = (h.origin.x, h.origin.y, h.size.width, h.size.height);
                [
                    (rx, ry, rw, hy - ry),
                    (rx, hy + hh, rw, ry + rh - (hy + hh)),
                    (rx, hy, hx - rx, hh),
                    (hx + hw, hy, rx + rw - (hx + hw), hh),
                ]
                .into_iter()
                .filter(|&(_, _, w, h)| w > 0. && h > 0.)
                .map(|(x, y, w, h)| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)))
                .collect()
            })
            .collect();
    }
    left
}

impl Slot {
    fn new() -> Retained<Self> {
        let this = main_thread().alloc::<Self>().set_ivars(SlotIvars::default());
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setWantsLayer(true);
        this.ivars().shown.set(1);
        this
    }

    /// Cut `rects` (the root's coordinates) out of the slot.
    fn cut(&self, rects: &[NSRect]) {
        let superview = unsafe { self.superview() };
        let bounds = self.bounds();
        let holes: Vec<NSRect> = rects.iter().map(|r| self.convertRect_fromView(*r, superview.as_deref())).filter(|h| intersection(*h, bounds).is_some()).collect();
        let left = remainder(bounds, &holes);
        *self.ivars().holes.borrow_mut() = holes;
        self.ivars().shown.set(left.len());
        let path = CGMutablePath::new();
        for r in &left {
            // SAFETY: a null transform adds the rect as it is.
            unsafe { CGMutablePath::add_rect(Some(&path), std::ptr::null(), *r) };
        }
        let mask = CAShapeLayer::new();
        mask.setPath(Some(&path));
        if let Some(layer) = self.layer() {
            // SAFETY: a fresh shape layer no other layer holds.
            unsafe { layer.setMask(Some(&mask)) };
        }
    }

    fn is_or_holds(&self, view: &NSView) -> bool {
        let me: &NSView = self;
        std::ptr::eq(view, me) || view.isDescendantOf(me)
    }
}

/// One window's slots, by pane id.
pub struct PaneHost {
    root: Retained<NSView>,
    frames: UnboundedSender<String>,
    slots: RefCell<HashMap<String, Retained<Slot>>>,
    /// The rect cut out of every slot while the palette or a sheet is open, the root's coordinates.
    overlay: Cell<Option<NSRect>>,
    /// The view the check's last press or move reached: its drag and release go there too.
    mouse_target: RefCell<Option<Retained<NSView>>>,
    mouse_number: Cell<isize>,
    press_monitor: RefCell<Option<Retained<AnyObject>>>,
    /// The browsers and plain web pages in this window's slots.
    pages: Rc<Pages>,
}

impl PaneHost {
    /// The host of GPUI's content view `root`; a press in a slot goes to `frames`.
    pub fn new(root: *mut c_void, frames: UnboundedSender<String>) -> Rc<Self> {
        // SAFETY: GPUI's content view of a live window, retained while the host lives.
        let root = unsafe { Retained::retain(root.cast::<NSView>()) }.expect("the window's content view");
        let host = Rc::new(Self {
            root,
            pages: Pages::new(frames.clone()),
            frames,
            slots: RefCell::default(),
            overlay: Cell::new(None),
            mouse_target: RefCell::default(),
            mouse_number: Cell::new(0),
            press_monitor: RefCell::default(),
        });
        // A press lands on a native view before GPUI sees it, so the GPUI view hears of it here
        // and closes what a press away closes (its popovers), as a Swift popover closes on one.
        let weak = Rc::downgrade(&host);
        let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the monitor a live event for the length of the call.
            let e = unsafe { event.as_ref() };
            if let Some(host) = weak.upgrade() {
                host.heard_press(e);
            }
            event.as_ptr()
        });
        let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
        // SAFETY: the block returns the event it was given, which AppKit then delivers.
        *host.press_monitor.borrow_mut() = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
        host
    }

    fn heard_press(&self, event: &NSEvent) {
        let mtm = main_thread();
        let (Some(window), Some(superview)) = (self.root.window(), unsafe { self.root.superview() }) else { return };
        if !event.window(mtm).is_some_and(|w| std::ptr::eq(&*w, &*window)) {
            return;
        }
        let at = superview.convertPoint_fromView(event.locationInWindow(), None);
        let hit = self.root.hitTest(at);
        self.pressed(hit.as_deref(), self.root.convertPoint_fromView(at, Some(&superview)));
    }

    /// A press reached `hit` at `point` (the root's coordinates): when that lies in a slot, say so
    /// with the point from the window's top left.
    fn pressed(&self, hit: Option<&NSView>, point: NSPoint) {
        let Some(pane) = hit.and_then(|v| self.owner(v)) else { return };
        let y = if self.root.isFlipped() { point.y } else { self.root.bounds().size.height - point.y };
        let _ = self.frames.unbounded_send(json!({"event": "pressed", "pane": pane, "point": [point.x, y]}).to_string());
    }

    /// The pane whose slot `view` is or lies inside.
    fn owner(&self, view: &NSView) -> Option<String> {
        self.slots.borrow().iter().find(|(_, s)| s.is_or_holds(view)).map(|(id, _)| id.clone())
    }

    /// `[x, y, w, h]` from the window's top left as a rect in the root's coordinates.
    fn rect_from_top(&self, a: [f64; 4]) -> NSRect {
        let y = if self.root.isFlipped() { a[1] } else { self.root.bounds().size.height - a[1] - a[3] };
        NSRect::new(NSPoint::new(a[0], y), NSSize::new(a[2], a[3]))
    }

    fn point_rect_from_top(&self, p: [f64; 2]) -> NSPoint {
        NSPoint::new(p[0], if self.root.isFlipped() { p[1] } else { self.root.bounds().size.height - p[1] })
    }

    /// What is cut out of `slot`: the overlay's rect and the cards over it, in the root's coordinates.
    fn cuts(&self, slot: &Slot) -> Vec<NSRect> {
        self.overlay.get().into_iter().chain(slot.ivars().holes_top.borrow().iter().map(|a| self.rect_from_top(*a))).collect()
    }

    /// The WKWebView a slot holds, made here or by the bridge.
    fn web_view(slot: &Slot) -> Option<Retained<WKWebView>> {
        slot.subviews().firstObject().and_then(|v| v.downcast::<WKWebView>().ok())
    }

    /// The view the window's hit test reaches at `point` (the window's top left).
    fn view_at(&self, point: NSPoint) -> Option<Retained<NSView>> {
        let superview = unsafe { self.root.superview() };
        self.root.hitTest(self.root.convertPoint_toView(point, superview.as_deref()))
    }
}

fn class_name(view: &NSView) -> String {
    view.class().name().to_string_lossy().into_owned()
}

impl Host for PaneHost {
    fn slot(&self, pane: &str) -> usize {
        let mut slots = self.slots.borrow_mut();
        let slot = slots.entry(pane.to_owned()).or_insert_with(|| {
            let slot = Slot::new();
            self.root.addSubview(&slot);
            slot
        });
        Retained::as_ptr(slot) as usize
    }

    fn place(&self, pane: &str, rect: [f64; 4], hidden: bool, holes: &[[f64; 4]], alpha: f64) {
        let Some(slot) = self.slots.borrow().get(pane).cloned() else { return };
        slot.setHidden(hidden);
        slot.setAlphaValue(alpha);
        *slot.ivars().holes_top.borrow_mut() = holes.to_vec();
        slot.setFrame(self.rect_from_top(rect));
        let bounds = slot.bounds();
        for content in slot.subviews().iter() {
            content.setFrame(bounds);
        }
        slot.cut(&self.cuts(&slot));
    }

    fn overlay(&self, rect: Option<[f64; 4]>) {
        self.overlay.set(rect.map(|a| self.rect_from_top(a)));
        if rect.is_some()
            && let Some(window) = self.root.window()
        {
            window.makeFirstResponder(Some(&self.root));
        }
        for slot in self.slots.borrow().values() {
            slot.cut(&self.cuts(slot));
        }
    }

    fn focus(&self, pane: &str) -> bool {
        // A pane with no native view is drawn by GPUI: its view takes the keyboard.
        let content = self.slots.borrow().get(pane).and_then(|s| s.subviews().firstObject());
        let target: &NSResponder = content.as_deref().map_or(&self.root, |c| c);
        self.root.window().is_some_and(|w| w.makeFirstResponder(Some(target)))
    }

    fn close(&self, pane: &str) {
        self.pages.close(pane);
        if let Some(slot) = self.slots.borrow_mut().remove(pane) {
            slot.removeFromSuperview();
        }
    }

    fn detach(&self) {
        if let Some(monitor) = self.press_monitor.borrow_mut().take() {
            // SAFETY: the monitor `addLocalMonitorForEventsMatchingMask_handler` returned.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        self.pages.close_all();
        for (_, slot) in self.slots.borrow_mut().drain() {
            slot.removeFromSuperview();
        }
        self.mouse_target.borrow_mut().take();
    }

    fn mouse(&self, m: &Mouse) -> Option<String> {
        let point = self.point_rect_from_top(m.point);
        // NSWindow.sendEvent drops mouse events for an unordered window, so route as AppKit does
        // after its own step: hit test, then the view gets the NSEvent. A drag or release goes to
        // the view that took the press.
        if matches!(m.kind, MouseKind::Down | MouseKind::Move) {
            *self.mouse_target.borrow_mut() = self.view_at(point);
        }
        let target = self.mouse_target.borrow().clone();
        if m.kind == MouseKind::Down {
            self.pressed(target.as_deref(), point);
        }
        let kind = match m.kind {
            MouseKind::Down => NSEventType::LeftMouseDown,
            MouseKind::Drag => NSEventType::LeftMouseDragged,
            MouseKind::Move => NSEventType::MouseMoved,
            MouseKind::Up => NSEventType::LeftMouseUp,
        };
        let mut flags = NSEventModifierFlags::empty();
        if m.control {
            flags |= NSEventModifierFlags::Control;
        }
        if m.command {
            flags |= NSEventModifierFlags::Command;
        }
        self.mouse_number.set(self.mouse_number.get() + 1);
        let window_number = self.root.window().map_or(0, |w| w.windowNumber());
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            self.root.convertPoint_toView(point, None),
            flags,
            NSProcessInfo::processInfo().systemUptime(),
            window_number,
            None,
            self.mouse_number.get(),
            m.clicks as isize,
            if m.kind == MouseKind::Up { 0. } else { 1. },
        );
        let target = target?;
        if let Some(event) = event {
            match m.kind {
                MouseKind::Down => target.mouseDown(&event),
                MouseKind::Drag => target.mouseDragged(&event),
                MouseKind::Move => target.mouseMoved(&event),
                MouseKind::Up => target.mouseUp(&event),
            }
        }
        Some(class_name(&target))
    }

    fn hit(&self, point: [f64; 2]) -> (Option<String>, Option<String>) {
        let hit = self.view_at(self.point_rect_from_top(point));
        (hit.as_deref().map(class_name), hit.as_deref().and_then(|v| self.owner(v)))
    }

    fn facts(&self) -> (Map<String, Value>, Option<[f64; 4]>) {
        let height = self.root.bounds().size.height;
        let top = |r: NSRect| [r.origin.x, if self.root.isFlipped() { r.origin.y } else { height - r.origin.y - r.size.height }, r.size.width, r.size.height];
        let panes = self
            .slots
            .borrow()
            .iter()
            .map(|(id, s)| {
                let facts = json!({
                    "hidden": s.isHidden(), "alpha": s.alphaValue(), "frame_top": top(s.frame()),
                    "masked": s.layer().is_some_and(|l| l.mask().is_some()), "holes": *s.ivars().holes_top.borrow(), "mask_rects": s.ivars().shown.get(),
                });
                (id.clone(), facts)
            })
            .collect();
        (panes, self.overlay.get().map(top))
    }

    fn open_page(&self, pane: &str, browser: bool, url: &str, remote: bool) {
        self.slot(pane);
        let Some(slot) = self.slots.borrow().get(pane).cloned() else { return };
        self.pages.open(pane, &slot, browser, url, remote);
    }

    fn open_artifact(&self, pane: &str, name: &str, scope: &str, cwd: &str, ws: &str) {
        self.slot(pane);
        let Some(slot) = self.slots.borrow().get(pane).cloned() else { return };
        self.pages.open_artifact(pane, &slot, name, scope, cwd, ws);
    }

    fn close_page(&self, pane: &str) {
        self.pages.close(pane);
    }

    fn browser(&self, pane: &str, action: &str, value: &Value) {
        self.pages.browser(pane, action, value);
    }

    fn reload(&self, pane: &str) -> bool {
        self.pages.reload(pane)
    }

    fn compose(&self, id: u64, pane: &str, base: Result<Vec<u8>, String>) {
        let view = self.slots.borrow().get(pane).and_then(|s| s.subviews().firstObject());
        match view {
            Some(view) => self.pages.compose(id, &view, base),
            None => self.pages.compose(id, &self.root, Err("the pane is not shown".into())),
        }
    }

    fn render(&self, id: u64, pane: &str, full: bool) -> bool {
        let Some(view) = self.slots.borrow().get(pane).and_then(|s| Self::web_view(s)) else { return false };
        self.pages.render(id, &view, full);
        true
    }

    fn inspect(&self, id: u64, pane: &str, action: &str, arg: Option<&str>) {
        self.pages.inspect(id, pane, action, arg);
    }

    fn snapshot(&self, directory: &str) {
        self.pages.clear_waiting();
        let shown: Vec<(String, Retained<WKWebView>)> =
            self.slots.borrow().iter().filter(|(_, s)| !s.isHidden()).filter_map(|(id, s)| Self::web_view(s).map(|v| (id.clone(), v))).collect();
        for (pane, view) in shown {
            self.pages.snapshot_to(&pane, &view, directory);
        }
    }

    fn pages(&self) -> (Map<String, Value>, Value) {
        let responder = self.root.window().and_then(|w| w.firstResponder()).and_then(|r| r.downcast::<NSView>().ok());
        self.pages.report(responder.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    #[test]
    fn holes_leave_the_rest() {
        let bounds = r(0., 0., 100., 100.);
        assert_eq!(remainder(bounds, &[]).len(), 1);
        // A hole in the middle leaves four rects whose area is the bounds' less the hole's.
        let left = remainder(bounds, &[r(40., 40., 20., 20.)]);
        assert_eq!(left.len(), 4);
        assert_eq!(left.iter().map(|r| r.size.width * r.size.height).sum::<f64>(), 10000. - 400.);
        // A hole that covers an edge leaves one rect; one outside leaves the bounds whole.
        assert_eq!(remainder(bounds, &[r(0., 0., 100., 30.)]), vec![r(0., 30., 100., 70.)]);
        assert_eq!(remainder(bounds, &[r(200., 0., 10., 10.)]), vec![bounds]);
        // Two holes that overlap stay holes: the area left is the bounds' less their union's.
        let two = remainder(bounds, &[r(10., 10., 30., 30.), r(20., 20., 30., 30.)]);
        assert_eq!(two.iter().map(|r| r.size.width * r.size.height).sum::<f64>(), 10000. - (900. + 900. - 400.));
    }
}
