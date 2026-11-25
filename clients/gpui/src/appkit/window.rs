//! Where a GPUI window sits on screen, in AppKit (WindowManager.swift's moves, DragChipWindow,
//! DesktopDropOverlay and the menu bar popover's fit). Every frame is [x, y, w, h] in points with
//! the origin at the main screen's top left, y growing down, as the client measures. A view is the
//! window's content view (the GPUI view `raw_window_handle` names); a null one is a window the
//! client has not attached yet, and every call on it does nothing.
//!
//! A call that resizes or moves a window does so after it returns, from the main queue: GPUI's
//! resize callback borrows the app, which the caller holds while it runs.
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::{MainThreadMarker, MainThreadOnly as _, rc::Retained};
use objc2_app_kit::{NSAnimatablePropertyContainer as _, NSAnimationContext, NSApplication, NSColor, NSScreen, NSView, NSWindow, NSFloatingWindowLevel, NSNormalWindowLevel, NSPopUpMenuWindowLevel, NSWindowCollectionBehavior};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::ffi::c_void;

/// A window's content frame and its place among the app's windows front to back (-1 off screen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub frame: [f64; 4],
    pub order: isize,
    pub visible: bool,
}

/// The view behind `ptr`, retained, when it is in a window. The pointer is a live content view
/// GPUI owns (its window's raw handle), or null.
fn view(ptr: *mut c_void) -> Option<Retained<NSView>> {
    // SAFETY: a non-null `ptr` is the NSView of a window GPUI holds open.
    let view = unsafe { Retained::retain(ptr.cast::<NSView>()) }?;
    view.window().is_some().then_some(view)
}

/// The main screen's top edge in AppKit's screen coordinates (0 on a host with no screen).
fn main_top(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm).firstObject().map_or(0., |s| s.frame().origin.y + s.frame().size.height)
}

/// `r` (AppKit screen coordinates, y up) as a top-left frame.
fn top_left(r: NSRect, top: f64) -> [f64; 4] {
    [r.origin.x, top - (r.origin.y + r.size.height), r.size.width, r.size.height]
}

/// A top-left frame as an AppKit screen rectangle.
fn appkit_rect([x, y, w, h]: [f64; 4], top: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, top - y - h), NSSize::new(w, h))
}

/// `view`'s rectangle on screen.
fn screen_rect(view: &NSView, window: &NSWindow) -> NSRect {
    window.convertRectToScreen(view.convertRect_toView(view.bounds(), None))
}

/// Run `f` with `view` and its window on the main queue, once the caller has returned.
fn later(view: Retained<NSView>, f: impl FnOnce(&NSView, &NSWindow, MainThreadMarker) + Send + 'static) {
    let mtm = view.mtm();
    let bound = MainThreadBound::new(view, mtm);
    DispatchQueue::main().exec_async(move || {
        // SAFETY: the main queue runs on the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let view = bound.get(mtm);
        if let Some(window) = view.window() {
            f(view, &window, mtm);
        }
    });
}

/// Where the window holding `view` shows its content, and its place front to back.
pub fn frame(ptr: *mut c_void) -> Option<Placed> {
    let view = view(ptr)?;
    let window = view.window()?;
    let mtm = view.mtm();
    let order = NSApplication::sharedApplication(mtm).orderedWindows().iter().position(|w| *w == *window).map_or(-1, |i| i as isize);
    Some(Placed { frame: top_left(screen_rect(&view, &window), main_top(mtm)), order, visible: window.isVisible() })
}

/// `view`'s own rectangle on screen (the popover's, which a space drag meets).
pub fn view_frame(ptr: *mut c_void) -> Option<[f64; 4]> {
    let view = view(ptr)?;
    let window = view.window()?;
    Some(top_left(screen_rect(&view, &window), main_top(view.mtm())))
}

/// The frame that puts the content's top left at (`x`, `y`), keeping the window's size.
fn moved(view: &NSView, window: &NSWindow, [x, y]: [f64; 2], top: f64) -> NSRect {
    let content = screen_rect(view, window);
    let mut frame = window.frame();
    let (left, above) = (frame.origin.x - content.origin.x, (frame.origin.y + frame.size.height) - (content.origin.y + content.size.height));
    frame.origin.x = x + left;
    frame.origin.y = top - y + above - frame.size.height;
    frame
}

/// Put the content's top left corner at `point`. Whether the window was there to move.
pub fn move_to(ptr: *mut c_void, point: [f64; 2]) -> bool {
    let Some(view) = view(ptr) else { return false };
    later(view, move |view, window, mtm| window.setFrame_display(moved(view, window, point, main_top(mtm)), true));
    true
}

/// Size the window to `width` by `height` points of content (None keeps that side), keeping its
/// top edge: the menu bar popover grows down from under the item.
pub fn fit(ptr: *mut c_void, width: Option<f64>, height: Option<f64>) -> bool {
    let Some(view) = view(ptr) else { return false };
    later(view, move |_, window, _| {
        let mut frame = window.frame();
        let top = frame.origin.y + frame.size.height;
        let now = window.contentLayoutRect().size;
        let content = NSRect::new(NSPoint::ZERO, NSSize::new(width.unwrap_or(now.width), height.unwrap_or(now.height)));
        frame.size = window.frameRectForContentRect(content).size;
        frame.origin.y = top - frame.size.height;
        window.setFrame_display(frame, true);
    });
    true
}

/// Set `window`'s frame to `to`, over `seconds` (0 replaces an animation still running).
fn animate(window: &NSWindow, to: NSRect, seconds: f64) {
    let block = block2::RcBlock::new(move |context: std::ptr::NonNull<NSAnimationContext>| {
        // SAFETY: AppKit hands the block the current animation context.
        unsafe { context.as_ref() }.setDuration(seconds);
        window.animator().setFrame_display(to, true);
    });
    NSAnimationContext::runAnimationGroup(&block);
}

/// What a space drag's own window shows: the chip under the pointer or the desktop's preview card.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Float {
    pub frame: [f64; 4],
    pub show: bool,
    pub card: bool,
    /// Grow in from 1.6 times its size: the card's first showing, unless `still`.
    pub grow: bool,
    /// Reduce Motion is on, or the hidden check runs: no animation, and the window never comes front.
    pub still: bool,
    pub hidden: bool,
}

/// Make the window holding `view` a space drag's own window, as DragChipWindow and
/// DesktopDropOverlay make theirs (clear, deaf to the pointer, on every Space), and put its
/// content on `float.frame`, shown or ordered out.
pub fn float(ptr: *mut c_void, float: Float) -> bool {
    let Some(view) = view(ptr) else { return false };
    later(view, move |_, window, mtm| {
        window.setIgnoresMouseEvents(true);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::Stationary | NSWindowCollectionBehavior::IgnoresCycle);
        window.setLevel(if float.card { NSFloatingWindowLevel } else { NSPopUpMenuWindowLevel });
        let target = appkit_rect(float.frame, main_top(mtm));
        if float.grow && float.show && !float.still && !float.hidden {
            let [_, _, w, h] = float.frame;
            let from = NSRect::new(NSPoint::new(target.origin.x - w * 0.3, target.origin.y - h * 0.3), NSSize::new(w * 1.6, h * 1.6));
            window.setFrame_display(from, false);
            if !window.isVisible() {
                window.orderFrontRegardless();
            }
            // The animator never holds the main thread, so the drag's moves keep coming.
            animate(window, target, 0.18);
        } else {
            // A zero-length animation replaces a grow still running, so the card stays on the pointer.
            animate(window, target, 0.);
        }
        if !float.show {
            window.orderOut(None);
        } else if !float.hidden && !window.isVisible() {
            window.orderFrontRegardless();
        }
    });
    true
}

/// The visible frame of the screen holding `point` (the main screen's when none does): where a
/// drag's chip window stays. None on a host with no screen.
pub fn screen_at([x, y]: [f64; 2]) -> Option<[f64; 4]> {
    let mtm = MainThreadMarker::new()?;
    let top = main_top(mtm);
    let at = NSPoint::new(x, top - y);
    let screens = NSScreen::screens(mtm);
    let inside = |r: NSRect| at.x >= r.origin.x && at.x < r.origin.x + r.size.width && at.y >= r.origin.y && at.y < r.origin.y + r.size.height;
    let screen = screens.iter().find(|s| inside(s.frame())).or_else(|| screens.firstObject())?;
    Some(top_left(screen.visibleFrame(), top))
}

/// A little window grows from `from` (a preview card's content frame) to its own size with its
/// content's top left at `point` (WindowManager.openLittle); with `still`, it jumps there.
pub fn grow(ptr: *mut c_void, from: [f64; 4], point: [f64; 2], still: bool) -> bool {
    let Some(view) = view(ptr) else { return false };
    later(view, move |view, window, mtm| {
        let top = main_top(mtm);
        let to = moved(view, window, point, top);
        if still {
            window.setFrame_display(to, true);
        } else {
            // `from` is the card's content rectangle; the window's frame round it holds its chrome.
            window.setFrame_display(window.frameRectForContentRect(appkit_rect(from, top)), false);
            animate(window, to, 0.25);
        }
    });
    true
}

/// The title row's pin: the window floats over every other while it is on. Whether it floats now.
pub fn pin(ptr: *mut c_void, on: bool) -> bool {
    let Some(view) = view(ptr) else { return false };
    let Some(window) = view.window() else { return false };
    window.setLevel(if on { NSFloatingWindowLevel } else { NSNormalWindowLevel });
    window.level() == NSFloatingWindowLevel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_flip_between_top_left_and_appkit() {
        let top = 1000.;
        let r = appkit_rect([10., 20., 300., 200.], top);
        assert_eq!((r.origin.x, r.origin.y, r.size.width, r.size.height), (10., 780., 300., 200.));
        assert_eq!(top_left(r, top), [10., 20., 300., 200.]);
    }

    #[test]
    fn a_null_view_does_nothing() {
        let null = std::ptr::null_mut();
        assert_eq!(frame(null), None);
        assert!(!move_to(null, [0., 0.]) && !fit(null, None, None) && !pin(null, true) && !grow(null, [0.; 4], [0., 0.], true));
    }
}
