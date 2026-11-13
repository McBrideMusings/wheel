//! The copied pairing code (PairingWindow.swift's writeConcealed): one per app, whichever window
//! or the menu bar copied it, so the copy works with no window open. The concealed clipboard item
//! is written through a bridge half, and a minute later the code is cancelled unless a pairing
//! window shows it, from the app's own task rather than a window's redraw.
use super::*;
use super::settings::now_ms;
use serde_json::Value;
use crate::pairing::{self, Copied};
use futures::StreamExt as _;

thread_local! {
    static COPIED: RefCell<Copied> = RefCell::default();
    /// Wakes the expiry task when a copy starts its minute.
    static WAKE: RefCell<Option<futures::channel::mpsc::UnboundedSender<()>>> = const { RefCell::new(None) };
}

/// The code on the clipboard and when its minute ends (unix ms), if one is.
pub fn copied() -> Option<(String, u64)> {
    COPIED.with(|c| c.borrow().get().cloned())
}

/// Start the app's task that cancels a copied code when its minute runs out; once per app.
pub fn ensure(cx: &mut App) {
    if WAKE.with(|w| w.borrow().is_some()) {
        return;
    }
    let (wake, mut copies) = futures::channel::mpsc::unbounded::<()>();
    WAKE.with(|w| *w.borrow_mut() = Some(wake));
    cx.spawn(async move |cx| {
        while copies.next().await.is_some() {
            while let Some((_, until)) = copied() {
                let now = now_ms();
                if now < until {
                    cx.background_executor().timer(std::time::Duration::from_millis(until - now)).await;
                    continue;
                }
                cx.update(expire);
                break;
            }
        }
    })
    .detach();
}

/// Put `json` on the clipboard as a concealed item through `native`, cleared after a minute if
/// nothing else is copied. Returns the code this copy replaced, which the caller cancels on its
/// connection, and the bridge's answer.
pub fn copy(native: &Native, token: &str, json: &str) -> (Option<String>, Value) {
    let old = COPIED.with(|c| c.borrow_mut().replace(token, now_ms() + pairing::CLIPBOARD_LIFE_MS));
    let answer = native.call(serde_json::json!({"op": "clipboard_concealed", "text": json, "clear_after_ms": pairing::CLIPBOARD_LIFE_MS}));
    WAKE.with(|w| w.borrow().as_ref().map(|w| w.unbounded_send(())));
    (old, answer)
}

/// The copied code's minute ran out: cancel it on the menu bar's connection (the app's one that
/// outlives every window), unless a window's pairing window shows it.
fn expire(cx: &mut App) {
    let shown = |token: &str| crate::window_handles().iter().any(|(_, h)| h.read(cx).is_ok_and(|c| c.pairing.open && c.pairing.window_tokens.iter().any(|t| t == token)));
    let Some(token) = COPIED.with(|c| c.borrow_mut().expired(now_ms(), shown)) else { return };
    if let Some(menu) = menu_bar() {
        menu.update(cx, |m, _| m.cancel_pairing(token));
    }
}
