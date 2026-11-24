//! The Edit menu every Mac app has, built through GPUI's menus so each item's tag names a GPUI
//! action. Cut, Copy and Paste send `cut:`, `copy:` and `paste:` along the responder chain, so a
//! web page or terminal holding the keyboard takes them as in Safari. While GPUI's view holds it,
//! GPUI's app delegate answers them: it enables an item when the window handles its action and
//! dispatches the action, and `view.rs` runs it as the typed ⌘X, ⌘C or ⌘V. An item whose tag
//! names no action locks GPUI's app delegate on validation, so every item comes from here.
use gpui::{App, KeyBinding, Menu, MenuItem, OsAction};
use std::cell::Cell;

gpui::actions!(edit, [Cut, Copy, Paste]);

/// The key each action stands for, with ⌘ held.
pub fn key(action: &dyn gpui::Action) -> Option<&'static str> {
    let any = action.as_any();
    if any.is::<Cut>() {
        Some("x")
    } else if any.is::<Copy>() {
        Some("c")
    } else if any.is::<Paste>() {
        Some("v")
    } else {
        None
    }
}

/// Bind ⌘X, ⌘C and ⌘V to the actions and set the menu bar, once per app.
pub fn install(cx: &mut App) {
    thread_local!(static INSTALLED: Cell<bool> = const { Cell::new(false) });
    if INSTALLED.replace(true) {
        return;
    }
    // The menu items take their key equivalents from these bindings.
    cx.bind_keys([KeyBinding::new("cmd-x", Cut, None), KeyBinding::new("cmd-c", Copy, None), KeyBinding::new("cmd-v", Paste, None)]);
    // The menu bar's first menu is always the app menu.
    cx.set_menus([
        Menu::new("Wheel"),
        Menu::new("Edit").items([
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
        ]),
    ]);
}
