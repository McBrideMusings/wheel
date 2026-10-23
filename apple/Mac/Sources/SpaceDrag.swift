import AppKit
import SwiftUI
import WheelKit

/// A space being dragged by its sidebar row, its title-bar chip or its row in the menu bar
/// popover. The drag is tracked here, not by AppKit's drag session, so it can tell a drop on the
/// desktop from a cancel, show a gap in a sidebar and a hint naming what releasing will do.
struct SpaceDrag: Equatable {
    let space: String
    /// Whether a held mouse button drives it; the debug endpoint's drags have none.
    var byPointer = false
    /// Where releasing would put it; nil until the pointer is over something.
    var target: SpaceDropTarget?
}

extension AppModel {
    /// The hint under a drag, or nil when it is over nothing yet.
    var spaceDragHint: String? {
        guard let drag = spaceDrag, let target = drag.target else { return nil }
        return SpaceDropping.hint(target, space: spaceName(drag.space) ?? drag.space) { id in
            layout.window(id).map { title(of: $0) } ?? id
        }
    }

    func beginSpaceDrag(_ space: String, byPointer: Bool = false) {
        guard spaceDrag == nil, spaces.contains(where: { $0.id == space }) else { return }
        spaceDrag = SpaceDrag(space: space, byPointer: byPointer)
        frozenRowFrames = rowFrames
        frozenSidebarFrames = sidebarFrames
        // The sidebar drops the dragged row from its list, which can remove the view whose gesture
        // would call `endSpaceDrag`, so the release is watched here as well.
        dragMonitorForSpaces = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .leftMouseUp]) { [weak self] event in
            guard let self, self.spaceDrag != nil else { return event }
            if event.type == .leftMouseUp {
                if self.spaceDrag?.byPointer == true { MainActor.assumeIsolated { self.endSpaceDrag() } }
                return event
            }
            guard event.keyCode == 53 else { return event }
            MainActor.assumeIsolated { self.cancelSpaceDrag(escape: true) }
            return nil
        }
        Log.info("space drag \(space): started")
    }

    /// The pointer moved (to `point` in screen coordinates; where the mouse is when nil): find what
    /// it is over.
    func moveSpaceDrag(at point: CGPoint? = nil) {
        guard var drag = spaceDrag else { return }
        let target = dropTarget(at: point ?? NSEvent.mouseLocation)
        setSpaceDragTarget(target, &drag)
    }

    /// Name the target directly, as the debug endpoint does without a pointer.
    func hoverSpaceDrag(_ target: SpaceDropTarget?) {
        guard var drag = spaceDrag else { return }
        setSpaceDragTarget(target, &drag)
    }

    private func setSpaceDragTarget(_ target: SpaceDropTarget?, _ drag: inout SpaceDrag) {
        guard drag.target != target else { return }
        drag.target = target
        withAnimation(.smooth(duration: 0.2)) { spaceDrag = drag }
        Log.info("space drag \(drag.space): over \(target.map(Self.describe) ?? "nothing")")
        if target == .desktop { showDesktopOverlay() } else { hideDesktopOverlay() }
    }

    private static func describe(_ target: SpaceDropTarget) -> String {
        switch target {
        case let .sidebar(window, index): "sidebar of \(window.prefix(8)) at \(index)"
        case let .window(window): "window \(window.prefix(8))"
        case let .card(window): "popover card \(window.map { String($0.prefix(8)) } ?? "background")"
        case .desktop: "the desktop"
        }
    }

    /// Release: put the space where the pointer is, or nowhere when it is over nothing.
    func endSpaceDrag() {
        guard let drag = spaceDrag else { return }
        removeSpaceDragMonitor()
        hideDesktopOverlay()
        withAnimation(.smooth(duration: 0.2)) { spaceDrag = nil }
        guard let target = drag.target else {
            Log.info("space drag \(drag.space): released over nothing")
            return
        }
        Log.info("space drag \(drag.space): dropped on \(Self.describe(target))")
        switch target {
        case let .sidebar(window, index): place(drag.space, in: window, at: index)
        case let .window(window): place(drag.space, in: window)
        case let .card(window): place(drag.space, in: window)
        case .desktop: tearOff(drag.space)
        }
    }

    func cancelSpaceDrag(escape: Bool = false) {
        guard let drag = spaceDrag else { return }
        removeSpaceDragMonitor()
        hideDesktopOverlay()
        withAnimation(.smooth(duration: 0.2)) { spaceDrag = nil }
        Log.info("space drag \(drag.space): cancelled\(escape ? " by escape" : "")")
    }

    private func removeSpaceDragMonitor() {
        frozenRowFrames = nil
        frozenSidebarFrames = nil
        if let dragMonitorForSpaces { NSEvent.removeMonitor(dragMonitorForSpaces) }
        dragMonitorForSpaces = nil
    }

    // MARK: Hit testing

    /// A rectangle in a window's SwiftUI space (top left origin) as screen coordinates.
    func screenRect(_ frame: CGRect, in window: NSWindow) -> CGRect {
        guard let content = window.contentView else { return .zero }
        let local = NSRect(x: frame.minX, y: content.bounds.height - frame.maxY, width: frame.width, height: frame.height)
        return window.convertToScreen(content.convert(local, to: nil))
    }

    /// What releasing at screen point `point` would do, measured against the windows as they are drawn.
    func dropTarget(at point: CGPoint) -> SpaceDropTarget {
        let ordered = NSApp.orderedWindows.filter { $0.isVisible && $0.isWheelWindow }
        var windows: [DropWindow] = []
        for nswindow in ordered {
            guard let raw = nswindow.identifier?.rawValue, raw.hasPrefix(WindowManager.idPrefix) else { continue }
            let id = String(raw.dropFirst(WindowManager.idPrefix.count))
            guard let record = layout.window(id) else { continue }
            let sidebar = record.sidebar ? (frozenSidebarFrames ?? sidebarFrames)[id].map { screenRect($0, in: nswindow) } : nil
            let rows = record.spaces.compactMap { (frozenRowFrames ?? rowFrames)[id]?[$0] }.map { screenRect($0, in: nswindow) }
            windows.append(DropWindow(id: id, frame: nswindow.frame, sidebar: sidebar, rows: rows))
        }
        var popover: DropPopover?
        if let pop = MenuBarWindow.current, pop.isVisible {
            let cards = cardFrames.map { (key, frame) in (window: key == MenuBarWindow.backgroundKey ? nil : key, frame: screenRect(frame, in: pop)) }
            popover = DropPopover(frame: pop.frame, cards: cards)
        }
        return SpaceDropping.target(at: point, windows: windows, popover: popover)
    }

    // MARK: The desktop outline

    private func showDesktopOverlay() {
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main
        guard let screen else { return }
        if desktopOverlay == nil { desktopOverlay = DesktopDropOverlay() }
        desktopOverlay?.show(on: screen, hint: spaceDragHint ?? "")
    }

    private func hideDesktopOverlay() { desktopOverlay?.hide() }
}

/// The popover window, found by what it is.
enum MenuBarWindow {
    static let backgroundKey = "background"
    static var current: NSWindow? { NSApp.windows.first { String(describing: type(of: $0)).contains("MenuBarExtra") } }
}

/// The whole desktop outlined with the hint, while a space is held over no window: a window of its
/// own that covers the screen and lets every click through.
@MainActor
final class DesktopDropOverlay {
    private let window: NSWindow
    private let host = NSHostingView(rootView: DesktopDropView(hint: ""))

    init() {
        window = NSWindow(contentRect: .zero, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.backgroundColor = .clear
        window.isOpaque = false
        window.hasShadow = false
        window.ignoresMouseEvents = true
        window.level = .floating
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        window.contentView = host
    }

    func show(on screen: NSScreen, hint: String) {
        host.rootView = DesktopDropView(hint: hint)
        window.setFrame(screen.frame, display: true)
        window.orderFrontRegardless()
    }

    func hide() { window.orderOut(nil) }

    var isShown: Bool { window.isVisible }
    var nsWindow: NSWindow { window }
}

struct DesktopDropView: View {
    let hint: String

    var body: some View {
        ZStack(alignment: .bottom) {
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(Color.accentColor, lineWidth: 4)
                .padding(2)
            DropHintPill(text: hint).padding(.bottom, 60)
        }
        .accessibilityHidden(true)
    }
}

/// The words that name what releasing will do.
struct DropHintPill: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.callout.weight(.medium))
            .foregroundStyle(.white)
            .padding(.horizontal, 12).padding(.vertical, 5)
            .background(Color.accentColor, in: Capsule())
    }
}

extension View {
    /// Let the view be dragged as `space`: a press and a move starts the drag; releasing drops it, from the gesture or, when the row's view is gone by then, from the event monitor.
    func spaceDraggable(_ model: AppModel, space: String) -> some View {
        gesture(
            DragGesture(minimumDistance: 6, coordinateSpace: .global)
                .onChanged { _ in
                    model.beginSpaceDrag(space, byPointer: true)
                    model.moveSpaceDrag()
                }
                .onEnded { _ in model.endSpaceDrag() }
        )
    }
}

extension AppModel {
    /// The measurements a drag is aimed at, in screen coordinates: every open window, its sidebar
    /// and rows, and the popover's cards.
    func geometryJSON() -> [String: Any] {
        func rect(_ r: CGRect) -> [Double] { [r.minX, r.minY, r.width, r.height].map { Double($0) } }
        var windows: [String: Any] = [:]
        for nswindow in NSApp.windows where nswindow.isWheelWindow && nswindow.isVisible {
            guard let raw = nswindow.identifier?.rawValue else { continue }
            let id = String(raw.dropFirst(WindowManager.idPrefix.count))
            var entry: [String: Any] = ["frame": rect(nswindow.frame)]
            if let sidebar = sidebarFrames[id] { entry["sidebar"] = rect(screenRect(sidebar, in: nswindow)) }
            entry["rows"] = Dictionary(uniqueKeysWithValues: (rowFrames[id] ?? [:]).map { ($0.key, rect(screenRect($0.value, in: nswindow))) })
            windows[id] = entry
        }
        var out: [String: Any] = ["windows": windows]
        if let pop = MenuBarWindow.current, pop.isVisible {
            out["popover"] = ["frame": rect(pop.frame), "cards": Dictionary(uniqueKeysWithValues: cardFrames.map { ($0.key, rect(screenRect($0.value, in: pop))) })]
        }
        return out
    }
}
