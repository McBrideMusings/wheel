import AppKit
import SwiftUI

/// How the window splits between the lead sidebar and the space grid: both side by side, the
/// sidebar hidden, or the sidebar filling the window. The mode and the sidebar's width are
/// this Mac's own presentation, so they live in UserDefaults and survive a relaunch.
@MainActor
final class SidebarLayout: ObservableObject {
    enum Mode: String { case split, hidden, full }

    static let minWidth = 340.0
    static let maxWidth = 640.0
    static let defaultWidth = 360.0
    /// The grid keeps at least this much beside the sidebar, however wide the sidebar is set.
    static let minGrid = 300.0

    private static let modeKey = "leadSidebarMode"
    private static let widthKey = "leadSidebarWidth"

    @Published private(set) var mode: Mode
    /// The width the sidebar has in the split layout and returns to after hiding.
    @Published private(set) var width: Double

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        mode = defaults.string(forKey: Self.modeKey).flatMap(Mode.init) ?? .split
        let saved = defaults.object(forKey: Self.widthKey) as? Double
        width = Self.clamp(saved ?? Self.defaultWidth)
    }

    static func clamp(_ width: Double) -> Double { min(max(width, minWidth), maxWidth) }

    /// The width the split layout can draw in a window `total` points wide.
    func splitWidth(in total: Double) -> Double {
        max(Self.minWidth, min(width, total - Self.minGrid))
    }

    func setWidth(_ new: Double) {
        guard new.isFinite else { return }
        let clamped = Self.clamp(new)
        guard clamped != width else { return }
        width = clamped
        defaults.set(clamped, forKey: Self.widthKey)
        Log.info("sidebar: width \(Int(clamped))")
    }

    /// Hide or show the sidebar; from the full window it returns to the split.
    func toggleHidden(animated: Bool) {
        set(mode == .split ? .hidden : .split, animated: animated)
    }

    /// Fill the window with the sidebar, or return to the split.
    func toggleFull(animated: Bool) {
        set(mode == .full ? .split : .full, animated: animated)
    }

    func set(_ new: Mode, animated: Bool) {
        guard new != mode else { return }
        let change = { self.mode = new }
        if animated, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            withAnimation(.easeInOut(duration: 0.2), change)
        } else {
            change()
        }
        defaults.set(new.rawValue, forKey: Self.modeKey)
        Log.info("sidebar: mode \(new.rawValue), width \(Int(width)), animated \(animated)")
    }
}

/// The drag handle on the sidebar's inner edge.
struct SidebarHandle: View {
    @ObservedObject var sidebar: SidebarLayout
    @State private var start: Double?
    @State private var hovering = false

    var body: some View {
        Color.clear
            .frame(width: 10)
            .contentShape(Rectangle())
            .onHover { inside in
                if inside, !hovering { NSCursor.resizeLeftRight.push() }
                if !inside, hovering { NSCursor.pop() }
                hovering = inside
            }
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { drag in
                        let from = start ?? sidebar.width
                        start = from
                        sidebar.setWidth(from + drag.translation.width)
                    }
                    .onEnded { _ in start = nil }
            )
            .onDisappear { if hovering { NSCursor.pop(); hovering = false } }
            .accessibilityLabel("Resize lead sidebar")
    }
}
