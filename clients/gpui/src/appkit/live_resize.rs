//! Whether a window edge is being dragged, as the app's global [`LiveResize`]: AppKit posts
//! `NSWindowWillStartLiveResize` and `NSWindowDidEndLiveResize`, and every window's terminals hold
//! their resizes while it is on. The hidden check stands in for the drag, which it cannot make.
use futures::StreamExt as _;
use gpui::{App, Global};
use objc2_app_kit::{NSWindowDidEndLiveResizeNotification, NSWindowWillStartLiveResizeNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};
use std::ptr::NonNull;

/// Whether some window is being resized by its edge now, and the check's stand-in for that.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiveResize {
    pub on: bool,
    stand_in: Option<bool>,
}
impl Global for LiveResize {}

/// Whether a window edge is being dragged: false before [`watch`] runs.
pub fn on(cx: &App) -> bool {
    cx.try_global::<LiveResize>().is_some_and(|l| l.on)
}

/// Follow every window's live resizes for as long as the app runs.
pub fn watch(cx: &mut App) {
    cx.set_global(LiveResize::default());
    let (changed, mut changes) = futures::channel::mpsc::unbounded::<bool>();
    let center = NSNotificationCenter::defaultCenter();
    let queue = NSOperationQueue::mainQueue();
    let observers = [(unsafe { NSWindowWillStartLiveResizeNotification }, true), (unsafe { NSWindowDidEndLiveResizeNotification }, false)].map(|(name, on)| {
        let changed = changed.clone();
        let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
            let _ = changed.unbounded_send(on);
        });
        // SAFETY: the block only sends on a channel, from the main queue it is given.
        unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, Some(&queue), &block) }
    });
    cx.spawn(async move |cx| {
        // The observers live while this task does: as long as the app.
        let _observers = observers;
        while let Some(on) = changes.next().await {
            cx.update(|cx| {
                let l = *cx.global::<LiveResize>();
                if l.stand_in.is_none() && l.on != on {
                    cx.set_global(LiveResize { on, ..l });
                }
            });
        }
    })
    .detach();
}

/// The check's stand-in for a window edge held (`Some(true)`), let go (`Some(false)`), or handed
/// back to AppKit (`None`, which reads as not resizing).
pub fn stand_in(set: Option<bool>, cx: &mut App) {
    cx.set_global(LiveResize { on: set.unwrap_or(false), stand_in: set });
}
