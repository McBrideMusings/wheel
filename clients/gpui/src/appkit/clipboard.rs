//! The general pasteboard: the pairing code as a concealed item that is cleared after its minute
//! unless something else was copied, and the plain text, save and restore the check uses.
use objc2::{rc::Retained, runtime::{NSObjectProtocol, ProtocolObject}};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString};
use objc2_foundation::{NSActivityOptions, NSArray, NSData, NSProcessInfo, NSString};
use std::cell::RefCell;

/// The concealed-pasteboard marker clipboard managers honour: an item that carries it is not recorded.
const CONCEALED: &str = "org.nspasteboard.ConcealedType";

/// The concealed text waiting to be cleared, and the activity that keeps the system from napping
/// the app past the clear.
struct Pending {
    text: String,
    activity: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

fn string_type() -> &'static NSString {
    // SAFETY: an AppKit constant.
    unsafe { NSPasteboardTypeString }
}

/// Write `text` as a concealed item; `clear_concealed` removes it later. A new write replaces the
/// one waiting to be cleared.
pub fn write_concealed(text: &str) {
    let board = NSPasteboard::generalPasteboard();
    let concealed = NSString::from_str(CONCEALED);
    board.clearContents();
    // SAFETY: two pasteboard types and no owner.
    unsafe { board.declareTypes_owner(&NSArray::from_slice(&[string_type(), &*concealed]), None) };
    board.setString_forType(&NSString::from_str(text), string_type());
    board.setData_forType(Some(&NSData::new()), &concealed);
    let info = NSProcessInfo::processInfo();
    let activity = info.beginActivityWithOptions_reason(NSActivityOptions::UserInitiatedAllowingIdleSystemSleep, &NSString::from_str("clearing the pairing code from the clipboard"));
    if let Some(old) = PENDING.with(|p| p.borrow_mut().replace(Pending { text: text.to_owned(), activity })) {
        // SAFETY: the token beginActivity handed back, ended once.
        unsafe { info.endActivity(&old.activity) };
    }
}

/// Clear the concealed item written last, if the pasteboard still holds it: Some(true) when it was
/// cleared, Some(false) when something else was copied since and was left alone, None when no item waits.
/// The pasteboard's change count is no test: a clipboard manager or the system touches it after a write.
pub fn clear_concealed() -> Option<bool> {
    let pending = PENDING.with(|p| p.borrow_mut().take())?;
    // SAFETY: the token beginActivity handed back, ended once.
    unsafe { NSProcessInfo::processInfo().endActivity(&pending.activity) };
    let held = text().is_some_and(|t| t == pending.text);
    if held {
        NSPasteboard::generalPasteboard().clearContents();
    }
    Some(held)
}

/// Whether the general pasteboard carries the concealed marker.
pub fn concealed() -> bool {
    NSPasteboard::generalPasteboard().types().is_some_and(|t| t.iter().any(|t| t.to_string() == CONCEALED))
}

/// The general pasteboard's text.
pub fn text() -> Option<String> {
    NSPasteboard::generalPasteboard().stringForType(string_type()).map(|s| s.to_string())
}

/// Replace the general pasteboard's contents with `text`.
pub fn set_text(text: &str) {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    board.setString_forType(&NSString::from_str(text), string_type());
}

/// Every item the general pasteboard held, each as its types and data, put back when this drops,
/// so a check leaves the person's pasteboard as it found it on every exit, an early error included.
pub struct Saved(Vec<Vec<(Retained<NSString>, Retained<NSData>)>>);

pub fn save() -> Saved {
    let items = NSPasteboard::generalPasteboard().pasteboardItems().map(|a| a.to_vec()).unwrap_or_default();
    Saved(items.iter().map(|item| item.types().iter().filter_map(|t| item.dataForType(&t).map(|d| (t, d))).collect()).collect())
}

impl Drop for Saved {
    fn drop(&mut self) {
        restore(&self.0);
    }
}

fn restore(saved: &[Vec<(Retained<NSString>, Retained<NSData>)>]) {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    let items: Vec<_> = saved
        .iter()
        .map(|types| {
            let item = NSPasteboardItem::new();
            for (t, data) in types {
                item.setData_forType(data, t);
            }
            ProtocolObject::from_retained(item)
        })
        .collect();
    board.writeObjects(&NSArray::from_retained_slice(&items));
}
