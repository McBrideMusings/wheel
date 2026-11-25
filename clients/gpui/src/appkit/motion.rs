//! The system's Reduce Motion, as the app's global [`ReduceMotion`]: every window observes it,
//! and it changes when the setting does or when the hidden check stands in for it.
use futures::StreamExt as _;
use gpui::{App, Global};
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
use objc2_foundation::{NSNotification, NSOperationQueue};
use std::ptr::NonNull;

/// Whether motion is reduced now, and the check's stand-in for the system setting (None reads the system's).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReduceMotion {
    pub on: bool,
    stand_in: Option<bool>,
}
impl Global for ReduceMotion {}

fn system() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// Whether motion is reduced now: false before [`watch`] runs.
pub fn on(cx: &App) -> bool {
    cx.try_global::<ReduceMotion>().is_some_and(|m| m.on)
}

/// Read the setting and follow it for as long as the app runs.
pub fn watch(cx: &mut App) {
    cx.set_global(ReduceMotion { on: system(), stand_in: None });
    let (changed, mut changes) = futures::channel::mpsc::unbounded::<()>();
    let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
        let _ = changed.unbounded_send(());
    });
    // SAFETY: the block only sends on a channel, from the main queue it is given.
    let observer = unsafe {
        NSWorkspace::sharedWorkspace().notificationCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        )
    };
    cx.spawn(async move |cx| {
        // The observer lives while this task does: as long as the app.
        let _observer = observer;
        while changes.next().await.is_some() {
            cx.update(|cx| {
                let m = *cx.global::<ReduceMotion>();
                let on = m.stand_in.unwrap_or_else(system);
                if on != m.on {
                    cx.set_global(ReduceMotion { on, ..m });
                }
            });
        }
    })
    .detach();
}

/// The check's stand-in for the system setting; None hands it back. Whether motion is reduced now.
pub fn stand_in(set: Option<bool>, cx: &mut App) -> bool {
    let on = set.unwrap_or_else(system);
    cx.set_global(ReduceMotion { on, stand_in: set });
    on
}
