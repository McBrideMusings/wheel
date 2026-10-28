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
        let at = point ?? NSEvent.mouseLocation
        let target = dropTarget(at: at)
        setSpaceDragTarget(target, &drag)
        if target == .desktop { desktopOverlay?.move(to: at) }
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
        let card = desktopOverlay?.cardFrame
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
        case .desktop:
            // A space with no main window to belong to has nowhere to keep its tab: it opens a full window.
            if openLittle(for: drag.space, from: card) == nil { tearOff(drag.space) }
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
        guard let drag = spaceDrag, let space = spaces.first(where: { $0.id == drag.space }) else { return }
        if desktopOverlay == nil { desktopOverlay = DesktopDropOverlay() }
        desktopOverlay?.show(space: space, at: NSEvent.mouseLocation)
    }

    private func hideDesktopOverlay() { desktopOverlay?.hide() }
}

/// The popover window, found by what it is.
enum MenuBarWindow {
    static let backgroundKey = "background"
    static var current: NSWindow? { NSApp.windows.first { String(describing: type(of: $0)).contains("MenuBarExtra") } }
}

/// The drag's preview card while a space is held over no window: the space's name over a
/// miniature of its grid, in a window of its own that follows the pointer and lets every click
/// through. Releasing grows it into the little window.
@MainActor
final class DesktopDropOverlay {
    static let size = CGSize(width: 230, height: 150)
    private let window: NSWindow
    private let host = NSHostingView(rootView: SpaceCardView(space: nil))

    init() {
        window = NSWindow(contentRect: NSRect(origin: .zero, size: Self.size), styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.backgroundColor = .clear
        window.isOpaque = false
        window.hasShadow = true
        window.ignoresMouseEvents = true
        window.level = .floating
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        window.contentView = host
    }

    /// Shrink the held space into the card under `point`.
    func show(space: SpaceInfo, at point: CGPoint) {
        host.rootView = SpaceCardView(space: space)
        let target = Self.frame(centeredOn: point)
        if window.isVisible {
            window.setFrame(target, display: true)
            return
        }
        let reduced = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        // The held space shrinks into the card.
        window.setFrame(reduced ? target : target.insetBy(dx: -target.width * 0.3, dy: -target.height * 0.3), display: true)
        window.orderFrontRegardless()
        if !reduced { window.setFrame(target, display: true, animate: true) }
    }

    func move(to point: CGPoint) {
        guard window.isVisible else { return }
        window.setFrame(Self.frame(centeredOn: point), display: true)
    }

    private static func frame(centeredOn point: CGPoint) -> NSRect {
        NSRect(x: point.x - size.width / 2, y: point.y - size.height / 2, width: size.width, height: size.height)
    }

    func hide() { window.orderOut(nil) }

    var isShown: Bool { window.isVisible }
    var nsWindow: NSWindow { window }
    /// Where the card is on screen (AppKit's coordinates).
    var cardFrame: CGRect { window.frame }
}

/// A space as a card: its name over a miniature of its pane grid.
struct SpaceCardView: View {
    let space: SpaceInfo?

    var body: some View {
        VStack(spacing: 0) {
            Text(space?.name ?? "")
                .font(.callout.weight(.semibold))
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 10)
                .frame(height: 28)
            Divider()
            GeometryReader { geo in
                if let space {
                    let grid = WheelKit.GridLayout(space.root, in: CGRect(origin: .zero, size: geo.size).insetBy(dx: 8, dy: 8))
                    ForEach(grid.panes) { pane in
                        RoundedRectangle(cornerRadius: 3)
                            .fill(.quaternary)
                            .overlay(RoundedRectangle(cornerRadius: 3).strokeBorder(.separator))
                            .frame(width: pane.frame.width, height: pane.frame.height)
                            .position(x: pane.frame.midX, y: pane.frame.midY)
                    }
                }
            }
        }
        .frame(width: DesktopDropOverlay.size.width, height: DesktopDropOverlay.size.height)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
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
    /// Let the view be dragged as `space` (the title chip and the popover's rows; a tab or sidebar row uses `spacePressTarget`): a press and a move starts the drag; releasing drops it, from the gesture or, when the row's view is gone by then, from the event monitor.
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
            guard let raw = nswindow.identifier?.rawValue, !raw.hasPrefix(WindowManager.littlePrefix) else { continue }
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
