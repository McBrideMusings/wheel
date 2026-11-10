import AppKit
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
        let space = model.layout.window(windowID)?.active
        let plugins = space.map { model.titleButtons[$0] ?? [] } ?? []
        let hidden = Set(plugins.filter(\.hidden).map(\.id))
        let shown = plugins.filter { !$0.hidden }
        HStack(spacing: 6) {
            Color.clear.frame(width: Self.lightsWidth, height: 1)
            if !hidden.contains("sidebar") {
                Button { model.setSidebar(open: !open, in: windowID) } label: {
                    Image(systemName: "sidebar.left")
                }
                .help(open ? "Hide the sidebar; spaces become tabs" : "Show the sidebar")
                .accessibilityLabel(open ? "Hide sidebar" : "Show sidebar")
                .titleButtonFrame(model, window: windowID, id: "sidebar")
            }
            SessionMenu(model: model, windowID: windowID)
            if !hidden.contains("mode") {
                ModeMenu(model: model, windowID: windowID)
                    .titleButtonFrame(model, window: windowID, id: "mode")
            }
            if let space {
                ForEach(shown.filter { $0.side == .left }) { button in
                    PluginButton(model: model, window: windowID, space: space, button: button)
                }
            }
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
                if let space {
                    ForEach(shown.filter { $0.side == .right }) { button in
                        PluginButton(model: model, window: windowID, space: space, button: button)
                    }
                    if !hidden.contains("processes") {
                        ProcessesButton(model: model, space: space)
                            .titleButtonFrame(model, window: windowID, id: "processes")
                    }
                }
                if !hidden.contains("palette") {
                    Button { model.openCommandPalette() } label: {
                        Text("⌘K").font(.system(size: 12, weight: .medium, design: .rounded))
                    }
                    .help("Command palette: spaces, panes, artifacts and actions")
                    .accessibilityLabel("Command palette")
                    .titleButtonFrame(model, window: windowID, id: "palette")
                }
                let pinned = model.layout.window(windowID)?.pinned == true
                if !hidden.contains("pin") {
                    Button { model.setPinned(!pinned, window: windowID) } label: {
                        Image(systemName: pinned ? "pin.fill" : "pin").foregroundStyle(pinned ? Color.accentColor : Color.primary)
                    }
                    .help(pinned ? "Unpin" : "Pin on top")
                    .accessibilityLabel(pinned ? "Unpin" : "Pin")
                    .titleButtonFrame(model, window: windowID, id: "pin")
                }
                if !hidden.contains("new-space") {
                    Button { model.openNewSpacePalette() } label: { Image(systemName: "plus") }
                        .help("New space from a folder")
                        .accessibilityLabel("New space")
                        .titleButtonFrame(model, window: windowID, id: "new-space")
                }
            }
        }
        .buttonStyle(TitleButtonStyle())
        .padding(.trailing, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(TitleBarGround())
    }
}

/// The mode button: one icon at the title row's left, whose menu picks what the window's body shows.
/// The icon is a fixed box, so a mode with a wider symbol never moves a neighbour.
struct ModeMenu: View {
    var model: AppModel
    let windowID: String

    var body: some View {
        let current = model.mode(in: windowID)
        Menu {
            ForEach(Self.modes, id: \.id) { mode in
                Button { model.setMode(mode.id, in: windowID) } label: {
                    Label(mode.title, systemImage: current == mode.id ? "checkmark" : mode.icon)
                }
            }
        } label: {
            Image(systemName: Self.modes.first { $0.id == current }?.icon ?? "square.grid.2x2")
                .frame(width: 16, height: 16)
        }
        .menuIndicator(.hidden)
        .menuStyle(.button)
        .buttonStyle(TitleButtonStyle())
        .fixedSize()
        .help("Mode: what this window shows")
        .accessibilityLabel("Mode")
    }

    static let modes: [(id: String, title: String, icon: String)] = [
        (WindowMode.spaces, "Spaces", "square.grid.2x2"),
        (WindowMode.desk, "Desk", "rectangle.3.group"),
    ]
}

/// A title row button: a bare symbol that shows a rounded fill under the pointer and while pressed.
/// A button a `[[title.button]]` entry defines (ADR-0037): an SF Symbol in the title row's own
/// button style, tinted and badged by what its `state` command printed. A press goes to wheeld,
/// which runs the entry's `command`; an entry with no command is an indicator and draws the same
/// slot without being a button. Icon-only and the badge an overlay, so a state change never moves
/// a neighbour.
struct PluginButton: View {
    var model: AppModel
    let window: String
    let space: String
    let button: TitleButtonInfo

    var body: some View {
        // A fixed icon box: a state that swaps in a wider symbol never grows the slot.
        let symbol = Image(systemName: button.icon)
            .frame(width: 16, height: 16)
            .foregroundStyle(Self.color(button.tint) ?? Color.primary)
            .overlay(alignment: .topTrailing) {
                if let badge = button.badge {
                    Text(badge)
                        .font(.system(size: 8, weight: .bold, design: .rounded).monospacedDigit())
                        .foregroundStyle(.white)
                        .padding(.horizontal, 3)
                        .background(Capsule().fill(Color.red))
                        .offset(x: 8, y: -6)
                }
            }
        Group {
            if button.hasCommand {
                Button { model.pressTitleButton(space: space, id: button.id) } label: { symbol }
                    .buttonStyle(TitleButtonStyle())
            } else {
                symbol.font(.system(size: 13)).frame(minWidth: 28, minHeight: 26)
            }
        }
        .disabled(!button.enabled)
        .help(button.tip)
        .accessibilityLabel(button.tip.isEmpty ? button.id : button.tip)
        .titleButtonFrame(model, window: window, id: "plugin-\(button.id)")
    }

    /// The named colours a `state` command's `tint` may print; anything else draws in the primary colour.
    static func color(_ name: String?) -> Color? {
        switch name {
        case "red": .red
        case "orange": .orange
        case "yellow": .yellow
        case "green": .green
        case "blue": .blue
        case "purple": .purple
        case "pink": .pink
        case "gray", "grey": .gray
        default: nil
        }
    }
}

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
        // A button a `[[title.button]]` entry hides, or a space switch removes, leaves no frame behind.
        .onDisappear { model.titleButtonFrames[window]?[id] = nil }
    }
}

/// The main window's hosting view. The title row sits in the title bar's strip, where AppKit takes a
/// press on a view that reports `mouseDownCanMoveWindow` for a window drag (the window is not
/// movable, so it only swallows the press); a tab's own gesture would never see it. The view reports
/// false, so a press anywhere in the window reaches SwiftUI.
final class TitleRowHostingView<Content: View>: NSHostingView<Content> {
    override var mouseDownCanMoveWindow: Bool { false }
}

final class TitleRowHostingController<Content: View>: NSHostingController<Content> {
    override func loadView() {
        view = TitleRowHostingView(rootView: rootView)
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
