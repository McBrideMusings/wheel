import AppKit
import SwiftUI
import WheelKit

/// One entry of a space's context menu: its tab, its sidebar row and its title-bar chip.
struct SpaceMenuItem: Equatable {
    let id: String
    let title: String
    let enabled: Bool
    /// The block of the menu it is drawn in; a divider separates two blocks.
    let section: Int
    var destructive = false
}

extension AppModel {
    /// What right-clicking `space` in `window` offers; the views draw it and the debug state reports it.
    /// `session:<id>` items and `new-session` move the space: one per other session, then a new one. A new window is ⌘N's and the menu bar's, never a space's.
    /// `window` is nil for a space in a session no window shows (a popover row); it only matters to
    /// Close Other Spaces in Window.
    func spaceMenu(for space: String, in window: String?) -> [SpaceMenuItem] {
        let home = layout.sessions.session(of: space)?.id
        let held = layout.sessions.session(of: space)?.spaces ?? []
        let index = held.firstIndex(of: space)
        var items: [SpaceMenuItem] = []
        for other in layout.sessions where other.id != home {
            items.append(SpaceMenuItem(id: "session:\(other.id)", title: "Move to \(other.name)", enabled: true, section: 0))
        }
        items.append(SpaceMenuItem(id: "new-session", title: "Move to New Session", enabled: true, section: 0))
        items += [
            SpaceMenuItem(id: "little", title: layout.little(showing: space) == nil ? "Open in Little Window" : "Go to Little Window", enabled: true, section: 0),
            SpaceMenuItem(id: "left", title: "Move Left", enabled: index.map { $0 > 0 } ?? false, section: 1),
            SpaceMenuItem(id: "right", title: "Move Right", enabled: index.map { $0 < held.count - 1 } ?? false, section: 1),
            SpaceMenuItem(id: "rename", title: "Rename Space…", enabled: true, section: 2),
            SpaceMenuItem(id: "copy-path", title: "Copy Path", enabled: true, section: 2),
            SpaceMenuItem(id: "reveal", title: "Reveal in Finder", enabled: true, section: 2),
            SpaceMenuItem(id: "new-pane", title: "New Pane", enabled: true, section: 3),
            SpaceMenuItem(id: "close-others", title: "Close Other Spaces in Window…", enabled: window != nil && held.count > 1, section: 4, destructive: true),
            SpaceMenuItem(id: "close", title: "Close Space", enabled: true, section: 4, destructive: true),
        ]
        return items
    }

    /// Run a space menu item, as the menu, the debug endpoint and a check do.
    func runSpaceMenu(_ id: String, space: String, in window: String?) {
        guard let info = spaces.first(where: { $0.id == space }) else { return }
        Log.info("space menu \(space): \(id) in \(window.map { "window \($0.prefix(8))" } ?? "no window")")
        let held = layout.sessions.session(of: space)?.spaces ?? []
        switch id {
        case "new-session": newSession(holding: space)
        case "little": openLittle(for: space)
        case "left", "right":
            // A reorder is wheeld's; every window showing the session lists the new order and
            // each keeps showing the space it showed.
            guard let session = layout.sessions.session(of: space)?.id, let i = held.firstIndex(of: space),
                  id == "left" ? i > 0 : i < held.count - 1 else { return }
            moveSpace(space, toSession: session, at: id == "left" ? i - 1 : i + 1)
        case "rename": beginRename(space: space)
        case "copy-path":
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(info.cwd, forType: .string)
        case "reveal": NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: info.cwd)])
        case "new-pane": split(info.focus, nil)
        case "close-others":
            if let window { askCloseOtherSpaces(keeping: space, in: window) }
        case "close": askCloseSpace(space)
        default:
            if id.hasPrefix("session:") { moveSpace(space, toSession: String(id.dropFirst("session:".count))) }
        }
    }

    /// Close every space `window` holds but `space`, after asking.
    func askCloseOtherSpaces(keeping space: String, in window: String) {
        let others = (layout.window(window).map(layout.spaces(of:)) ?? []).filter { $0 != space }
        guard !others.isEmpty else { return }
        let names = others.compactMap(spaceName).map { "“\($0)”" }.joined(separator: ", ")
        let count = others.count == 1 ? "1 other space" : "\(others.count) other spaces"
        confirm("Close \(count) in this window?", "Every terminal and agent in \(names) stops.", button: others.count == 1 ? "Close Other Space" : "Close Other Spaces") { [weak self] in
            for other in others { self?.closeSpace(other) }
        }
    }
}

/// A space's context menu, drawn from `spaceMenu(for:in:)`. Moving to a session is one flat item
/// per session ("Move to <name>", then "Move to New Session"): a submenu in a context menu did not open.
struct SpaceMenuItems: View {
    var model: AppModel
    let space: String
    let window: String?

    var body: some View {
        let items = model.spaceMenu(for: space, in: window)
        ForEach(Array(items.enumerated()), id: \.element.id) { offset, item in
            if offset > 0, items[offset - 1].section != item.section { Divider() }
            Button(item.title, role: item.destructive ? .destructive : nil) {
                model.runSpaceMenu(item.id, space: space, in: window)
            }
            .disabled(!item.enabled)
        }
    }
}

extension View {
    /// A space tab in the title row: the same press as `spacePressTarget`, taken by an AppKit view
    /// over the tab. A SwiftUI `DragGesture` in the title bar's strip never saw the press (AppKit
    /// takes it for a window drag); a view that reports `mouseDownCanMoveWindow` false gets it, as
    /// the title row's own ground does (`TitleBarGround`).
    func tabPressTarget(_ model: AppModel, windowID: String, space: String) -> some View {
        overlay { TabPressTarget(model: model, windowID: windowID, space: space, active: model.renamingSpace != space) }
    }
}

private struct TabPressTarget: NSViewRepresentable {
    var model: AppModel
    let windowID: String
    let space: String
    /// False while the tab's rename field is up: the field takes its own clicks.
    let active: Bool

    func makeNSView(context: Context) -> TabPressView { TabPressView() }

    func updateNSView(_ view: TabPressView, context: Context) {
        view.model = model
        view.windowID = windowID
        view.space = space
        view.takesPresses = active
    }
}

private final class TabPressView: NSView {
    var model: AppModel?
    var windowID = ""
    var space = ""
    var takesPresses = true
    private var downAt: NSPoint?
    private var dragged = false

    override var mouseDownCanMoveWindow: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    /// A right press (or control-click) is not this view's: it falls through to the tab's context menu.
    override func hitTest(_ point: NSPoint) -> NSView? {
        guard takesPresses else { return nil }
        // The tab's close and back buttons sit in its last 28 points and take their own presses.
        if convert(point, from: superview).x > bounds.width - 28 { return nil }
        if let event = NSApp.currentEvent, event.type == .rightMouseDown || event.modifierFlags.contains(.control) { return nil }
        return super.hitTest(point)
    }

    override func mouseDown(with event: NSEvent) {
        guard let model, model.spaceDrag == nil else { return }
        downAt = event.locationInWindow
        dragged = false
        model.notePress(space: space)
        model.tapSpace(space, in: windowID)
    }

    override func mouseDragged(with event: NSEvent) {
        guard let model, let downAt else { return }
        if !dragged, hypot(event.locationInWindow.x - downAt.x, event.locationInWindow.y - downAt.y) >= 6 {
            dragged = true
            model.beginSpaceDrag(space, byPointer: true)
        }
        if dragged { model.moveSpaceDrag() }
    }

    override func mouseUp(with event: NSEvent) {
        if dragged { model?.endSpaceDrag() }
        downAt = nil
        dragged = false
    }
}

extension View {
    /// A space's tab or sidebar row: one gesture owns the press. The mouse-down shows the space; a
    /// press that then moves 6 points drags the space, released over a sidebar, a
    /// window, the popover or the desktop. Releasing ends the drag here or, when the row's view is
    /// gone by then, from the event monitor `beginSpaceDrag` installs.
    func spacePressTarget(_ model: AppModel, windowID: String, space: String) -> some View {
        modifier(SpacePress(model: model, windowID: windowID, space: space))
    }
}

private struct SpacePress: ViewModifier {
    var model: AppModel
    let windowID: String
    let space: String
    /// This press has started a drag: its release is never a click, and a drag Escape cancelled
    /// does not start again while the button is still held.
    @State private var dragged = false
    /// This press has shown its space: a space switches on the mouse-down, as a sidebar selects.
    @State private var pressed = false

    func body(content: Content) -> some View {
        content
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .global)
                    .onChanged { value in
                        if !pressed, !dragged, model.spaceDrag == nil {
                            pressed = true
                            model.notePress(space: space)
                            model.tapSpace(space, in: windowID)
                        }
                        if !dragged, hypot(value.translation.width, value.translation.height) >= 6 {
                            dragged = true
                            model.beginSpaceDrag(space, byPointer: true)
                        }
                        if dragged { model.moveSpaceDrag() }
                    }
                    .onEnded { _ in
                        pressed = false
                        if dragged {
                            dragged = false
                            model.endSpaceDrag()
                        }
                    },
                // The rename field takes its own clicks.
                including: model.renamingSpace == space ? .subviews : .all
            )
    }
}
