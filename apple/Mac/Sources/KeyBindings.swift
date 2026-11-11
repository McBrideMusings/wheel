import AppKit
import WheelKit
import WheelUI

extension AppModel {
    /// Read every key pressed in a Wheel window against the keymap before anything else sees it,
    /// as Ghostty reads its `keybind`s: a bound key whose action can run now runs it and goes no
    /// further; a printable key then goes to the focused chat's composer (`typeIntoFocusedComposer`); any other key goes on to the menus and the pane holding the keyboard.
    func installKeyMonitor() {
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            let ran = MainActor.assumeIsolated { self?.runKey(event) == true || self?.typeIntoFocusedComposer(event) == true || self?.pasteAttachments(event) == true }
            return ran ? nil : event
        }
    }

    /// Run the action `event` is bound to; false when the keymap leaves the key to the rest of the app.
    func runKey(_ event: NSEvent) -> Bool {
        // Esc closes the shortcut sheet wherever the keyboard is, a terminal included.
        if shortcutsOpen, event.window?.isWheelWindow == true, KeyCodes.specialName(event.keyCode) == "escape",
           event.modifierFlags.intersection(.deviceIndependentFlagsMask).isEmpty {
            closeShortcuts()
            return true
        }
        guard event.window?.isWheelWindow == true, let chord = KeyChord(event: event), let id = keymap.action(for: chord) else {
            return false
        }
        guard let entry = CommandActions.entry(id, self), entry.enabled(self) else {
            Log.info("key \(chord.text): \(id) cannot run now; the key goes on")
            return false
        }
        Log.info("key \(chord.text): \(id)")
        CommandActions.run(id, self)
        return true
    }
}
