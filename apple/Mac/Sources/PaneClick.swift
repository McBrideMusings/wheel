import AppKit
import WheelKit

/// A press anywhere in a pane focuses it, whatever the pane shows: a page or a terminal takes the
/// press for itself before any SwiftUI gesture sees it, so the window's events are watched instead.
extension AppModel {
    func installPaneClickMonitor() {
        NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) { [weak self] event in
            MainActor.assumeIsolated { self?.paneClicked(event) }
            return event
        }
    }

    private func paneClicked(_ event: NSEvent) {
        guard let window = event.window, window.isWheelWindow, let point = event.topLeftLocation else { return }
        if window.attachedSheet != nil || commandPalette != nil || shortcutsOpen {
            Log.info("pane click ignored: \(window.attachedSheet != nil ? "a sheet" : commandPalette != nil ? "the command palette" : "the shortcut sheet") is open")
            return
        }
        guard let pane = pane(at: point, in: window) else { return }
        Log.info("pane click \(pane)")
        paneFocused(pane: pane)
    }

    /// The pane under `point` (top-left origin, SwiftUI's global space) in the space `window`
    /// shows; only the zoomed pane while one is, since the others sit hidden under it.
    func pane(at point: CGPoint, in window: NSWindow) -> String? {
        let shown: String?
        if let little = WindowManager.littleID(of: window) {
            shown = layout.little(little)?.space
        } else {
            shown = WindowManager.id(of: window).flatMap { layout.window($0)?.active }
        }
        guard let shown, let space = spaces.first(where: { $0.id == shown }) else { return nil }
        if let zoomed = zoomedPane(in: space) { return paneFrames[zoomed]?.contains(point) == true ? zoomed : nil }
        return space.root.panes.first { paneFrames[$0.id]?.contains(point) == true }?.id
    }
}

extension NSEvent {
    /// Where the event happened in its window's content, top-left origin: the space SwiftUI's
    /// `.global` frames are in. nil for an event with no window.
    var topLeftLocation: CGPoint? {
        guard let height = window?.contentView?.bounds.height else { return nil }
        return CGPoint(x: locationInWindow.x, y: height - locationInWindow.y)
    }
}
