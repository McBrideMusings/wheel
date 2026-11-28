//! Game controllers, read with GameController and sent to every stream that takes input in the key
//! window, as WheelUI's `StreamControllers`. Everything here runs on the main thread, where
//! GameController calls its handlers by default.
use super::StreamEmbed;
use objc2::rc::Retained;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};
use objc2_game_controller::{GCController, GCControllerDidConnectNotification, GCControllerDidDisconnectNotification, GCControllerPlayerIndex, GCExtendedGamepad};
use serde_json::{Value, json};
use std::{cell::RefCell, ptr::NonNull, rc::Weak};

thread_local! {
    static SESSIONS: RefCell<Option<Vec<Weak<StreamEmbed>>>> = const { RefCell::new(None) };
}

/// Send controller events to `embed` while it takes input in the key window; the first call starts watching.
pub fn add(embed: Weak<StreamEmbed>) {
    let first = SESSIONS.with_borrow_mut(|s| {
        let first = s.is_none();
        let list = s.get_or_insert_with(Vec::new);
        list.retain(|w| w.strong_count() > 0);
        list.push(embed);
        first
    });
    if !first {
        return;
    }
    // SAFETY: GameController's own list and notifications, observed on the main queue.
    unsafe {
        GCController::controllers().iter().for_each(|c| watch(&c));
        let center = NSNotificationCenter::defaultCenter();
        let main = NSOperationQueue::mainQueue();
        let connected = block2::RcBlock::new(|_: NonNull<NSNotification>| GCController::controllers().iter().for_each(|c| watch(&c)));
        let gone = block2::RcBlock::new(|note: NonNull<NSNotification>| {
            let pad = note.as_ref().object().and_then(|o| o.downcast::<GCController>().ok());
            if let Some(pad) = pad {
                broadcast(&json!({"kind": "disconnected", "controller": pad.playerIndex().0}));
            }
        });
        // The observers last as long as the app: nothing removes them.
        std::mem::forget(center.addObserverForName_object_queue_usingBlock(Some(GCControllerDidConnectNotification), None, Some(&main), &connected));
        std::mem::forget(center.addObserverForName_object_queue_usingBlock(Some(GCControllerDidDisconnectNotification), None, Some(&main), &gone));
    }
}

fn watch(pad: &GCController) {
    // SAFETY: a connected controller, on the main thread.
    unsafe {
        if pad.playerIndex() == GCControllerPlayerIndex::IndexUnset {
            let used: Vec<isize> = GCController::controllers().iter().map(|c| c.playerIndex().0).collect();
            if let Some(free) = (0..4).find(|i| !used.contains(i)) {
                pad.setPlayerIndex(GCControllerPlayerIndex(free));
            }
        }
        let Some(gamepad) = pad.extendedGamepad() else { return };
        let owner: Retained<GCController> = Retained::retain(pad as *const GCController as *mut GCController).expect("a live controller");
        let handler = block2::RcBlock::new(move |g: NonNull<GCExtendedGamepad>, _| {
            broadcast(&event(g.as_ref(), owner.playerIndex().0));
        });
        gamepad.setValueChangedHandler(block2::RcBlock::as_ptr(&handler));
    }
}

fn broadcast(event: &Value) {
    let sessions: Vec<_> = SESSIONS.with_borrow(|s| s.iter().flatten().filter_map(Weak::upgrade).collect());
    for s in sessions.iter().filter(|s| s.wants_controllers()) {
        s.send(super::frames::Input::Controller, event);
    }
}

/// A controller's whole state: every button pressed or not, and each stick and trigger from -1 to 1 (triggers 0 to 1).
fn event(g: &GCExtendedGamepad, index: isize) -> Value {
    // SAFETY: a live gamepad profile on the main thread.
    unsafe {
        let pressed = |b: Option<Retained<objc2_game_controller::GCControllerButtonInput>>| b.is_some_and(|b| b.isPressed());
        let d = g.dpad();
        json!({
            "kind": "state", "controller": index,
            "buttons": {
                "a": g.buttonA().isPressed(), "b": g.buttonB().isPressed(), "x": g.buttonX().isPressed(), "y": g.buttonY().isPressed(),
                "lb": g.leftShoulder().isPressed(), "rb": g.rightShoulder().isPressed(),
                "l3": pressed(g.leftThumbstickButton()), "r3": pressed(g.rightThumbstickButton()),
                "menu": g.buttonMenu().isPressed(), "options": pressed(g.buttonOptions()), "home": pressed(g.buttonHome()),
                "up": d.up().isPressed(), "down": d.down().isPressed(), "left": d.left().isPressed(), "right": d.right().isPressed(),
            },
            "axes": {
                "lx": g.leftThumbstick().xAxis().value(), "ly": g.leftThumbstick().yAxis().value(),
                "rx": g.rightThumbstick().xAxis().value(), "ry": g.rightThumbstick().yAxis().value(),
                "lt": g.leftTrigger().value(), "rt": g.rightTrigger().value(),
            },
        })
    }
}
