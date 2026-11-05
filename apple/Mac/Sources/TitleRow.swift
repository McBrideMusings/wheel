import SwiftUI
import WheelKit

/// A main window's title row, drawn under its transparent title bar: the sidebar button beside the
/// traffic lights, the session menu, the space tabs while the sidebar is closed (the window's title while it is open),
/// and the processes, command palette, pin and new-space buttons at the right end. The tabs take
/// only the width left between the two button groups, so the buttons never leave the row.
struct TitleRow: View {
    var model: AppModel
    let windowID: String

    /// The width the traffic lights take at the row's left end.
    static let lightsWidth: CGFloat = 72
    /// The row's height where the window has no title bar to match.
    static let minHeight: CGFloat = 38

    var body: some View {
        let open = model.sidebarOpen(in: windowID)
        HStack(spacing: 6) {
            Color.clear.frame(width: Self.lightsWidth, height: 1)
            Button { model.setSidebar(open: !open, in: windowID) } label: {
                Image(systemName: "sidebar.left")
            }
            .help(open ? "Hide the sidebar; spaces become tabs" : "Show the sidebar")
            .accessibilityLabel(open ? "Hide sidebar" : "Show sidebar")
            .titleButtonFrame(model, window: windowID, id: "sidebar")
            SessionMenu(model: model, windowID: windowID)
            if open || model.renamingWindow == windowID {
                WindowTitle(model: model, windowID: windowID)
                    .padding(.leading, 4)
            } else {
                SpaceTabs(model: model, windowID: windowID)
                    .layoutPriority(1)
                    .transition(.opacity)
            }
            Spacer(minLength: 8)
            HStack(spacing: 2) {
                if let space = model.layout.window(windowID)?.active {
                    ProcessesButton(model: model, space: space)
                        .titleButtonFrame(model, window: windowID, id: "processes")
                }
                Button { model.openCommandPalette() } label: {
                    Text("⌘K").font(.system(size: 12, weight: .medium, design: .rounded))
                }
                .help("Command palette: spaces, panes, artifacts and actions")
                .accessibilityLabel("Command palette")
                .titleButtonFrame(model, window: windowID, id: "palette")
                let pinned = model.layout.window(windowID)?.pinned == true
                Button { model.setPinned(!pinned, window: windowID) } label: {
                    Image(systemName: pinned ? "pin.fill" : "pin").foregroundStyle(pinned ? Color.accentColor : Color.primary)
                }
                .help(pinned ? "Unpin" : "Pin on top")
                .accessibilityLabel(pinned ? "Unpin" : "Pin")
                .titleButtonFrame(model, window: windowID, id: "pin")
                Button { model.openNewSpacePalette() } label: { Image(systemName: "plus") }
                    .help("New space from a folder")
                    .accessibilityLabel("New space")
                    .titleButtonFrame(model, window: windowID, id: "new-space")
            }
        }
        .buttonStyle(TitleButtonStyle())
        .padding(.trailing, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(TitleBarGround())
    }
}

/// A title row button: a bare symbol that shows a rounded fill under the pointer and while pressed.
struct TitleButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        TitleButtonBody(configuration: configuration)
    }

    private struct TitleButtonBody: View {
        let configuration: Configuration
        @State private var hovering = false
        @Environment(\.isEnabled) private var enabled

        var body: some View {
            configuration.label
                .font(.system(size: 13))
                .foregroundStyle(enabled ? .secondary : .tertiary)
                .frame(minWidth: 28, minHeight: 26)
                .background(
                    RoundedRectangle(cornerRadius: 6)
                        .fill(Color.primary.opacity(configuration.isPressed ? 0.14 : hovering ? 0.08 : 0))
                )
                .contentShape(Rectangle())
                .onHover { hovering = $0 }
        }
    }
}

extension View {
    /// Records the button's frame in its window, for `GET /geometry`'s `title_buttons`.
    func titleButtonFrame(_ model: AppModel, window: String, id: String) -> some View {
        onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
            model.titleButtonFrames[window, default: [:]][id] = $0
        }
    }
}

/// The title row's empty ground. The window is not movable by its title bar (a tab's drag would
/// move it), so this view does what the title bar would: a drag moves the window, and a
/// double-click does what System Settings › Desktop & Dock › "Double-click a window's title bar"
/// says (`AppleActionOnDoubleClick`: zoom, the default; minimize; or nothing).
struct TitleBarGround: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { GroundView() }
    func updateNSView(_ view: NSView, context: Context) {}

    final class GroundView: NSView {
        override var mouseDownCanMoveWindow: Bool { false }

        override func mouseDown(with event: NSEvent) {
            guard let window else { return }
            if event.clickCount == 2 {
                let action = UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") ?? "Maximize"
                Log.info("title row double-click: \(action)")
                switch action {
                case "Minimize": window.performMiniaturize(nil)
                case "None": break
                default: window.performZoom(nil)
                }
            } else {
                Log.info("title row drag: moving the window")
                window.performDrag(with: event)
            }
        }
    }
}
