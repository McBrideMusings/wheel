import SwiftUI
import WheelKit

/// The window: the active space's pane grid, with the connection and the usage strip under it.
/// Stock macOS styling.
struct ContentView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel
    /// The window this view fills (its record in `model.layout`).
    let windowID: String

    var body: some View {
        VStack(spacing: 6) {
            if model.serverStoppedByMenu {
                ServerStoppedNotice()
            } else {
                SpacesView(model: model, windowID: windowID)
            }
            HStack(spacing: 12) {
                ConnectionBadge(connection: model.connection)
                UsageStrip(usage: model.usage)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(10)
        .frame(minWidth: 900, minHeight: 560)
        .background(Color(nsColor: .windowBackgroundColor))
        .overlay { SpaceDropOutline(model: model, windowID: windowID) }
        .overlay {
            if let palette = model.commandPalette, model.layout.focused == windowID {
                CommandPaletteOverlay(model: model, palette: palette)
            }
        }
        .toolbar {
            ToolbarItem(placement: .navigation) {
                let open = model.sidebarOpen(in: windowID)
                Button { model.setSidebar(open: !open, in: windowID) } label: {
                    Image(systemName: "sidebar.left")
                }
                .help(open ? "Hide the sidebar; spaces become tabs" : "Show the sidebar")
                .accessibilityLabel(open ? "Hide sidebar" : "Show sidebar")
            }
            ToolbarItem(placement: .navigation) {
                WindowTitle(model: model, windowID: windowID)
            }
        }
        // The debug endpoint's stand-in for the app menu's Settings…
        .onChange(of: settings.openRequest) { if model.layout.focused == windowID { openSettings() } }
    }

    @Environment(\.openSettings) private var openSettings
}

struct ConnectionBadge: View {
    let connection: DaemonClient.Connection

    var body: some View {
        switch connection {
        case .connected:
            Label("Connected to wheeld", systemImage: "checkmark.circle")
                .font(.caption).foregroundStyle(.secondary)
        case .connecting:
            Label("Connecting to wheeld…", systemImage: "arrow.triangle.2.circlepath")
                .font(.caption).foregroundStyle(.secondary)
        case let .unreachable(reason):
            Label("wheeld unreachable: \(reason). Retrying every 2 seconds.", systemImage: "exclamationmark.triangle")
                .font(.caption).foregroundStyle(.orange)
        }
    }
}

/// The window's title in the title bar: its name in bold with the active space beside it, or the
/// space's name alone when the window has none. Double-clicking edits the name; an empty name
/// clears it.
struct WindowTitle: View {
    @ObservedObject var model: AppModel
    let windowID: String

    var body: some View {
        let record = model.layout.window(windowID)
        let space = record?.active.flatMap(model.spaceName)
        if model.renamingWindow == windowID {
            InlineName(
                text: record?.name ?? "", placeholder: space ?? "Window name", error: nil,
                commit: { model.commitRename(window: windowID, to: $0) }, cancel: { model.endRenaming() }
            )
            .frame(width: 220)
        } else {
            HStack(spacing: 6) {
                if let name = record?.name {
                    Text(name).fontWeight(.bold)
                    if let space, record?.sidebar != false { Text(space).foregroundStyle(.secondary) }
                } else if record?.sidebar != false {
                    Text(space ?? "Wheel").fontWeight(.semibold)
                }
                // With the sidebar hidden the active space is a chip: drag it like a sidebar row,
                // click it for the window's spaces.
                if record?.sidebar == false, let active = record?.active, let space {
                    SpaceChip(model: model, windowID: windowID, space: active, name: space)
                }
            }
            .lineLimit(1)
            .contentShape(Rectangle())
            .onTapGesture(count: 2) { model.beginRename(window: windowID) }
            .contextMenu {
                if let active = record?.active {
                    Button("Rename Space…") { model.beginRename(space: active) }
                }
                Button("Rename Window…") { model.beginRename(window: windowID) }
            }
            .help("Right-click to rename the space or the window")
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Window \(model.layout.window(windowID).map { model.title(of: $0) } ?? "")")
            .accessibilityAction(named: "Rename Window…") { model.beginRename(window: windowID) }
        }
    }
}

/// While a space is dragged over this window: an accent outline when its body takes the space, and
/// the hint naming what releasing will do.
struct SpaceDropOutline: View {
    @ObservedObject var model: AppModel
    let windowID: String

    var body: some View {
        let target = model.spaceDrag?.target
        let mine: Bool = switch target {
        case let .window(id)?: id == windowID
        case let .sidebar(id, _)?: id == windowID
        default: false
        }
        ZStack(alignment: .top) {
            if target == .window(windowID) {
                RoundedRectangle(cornerRadius: 8).strokeBorder(Color.accentColor, lineWidth: 3).padding(2)
            }
            if mine, let hint = model.spaceDragHint {
                DropHintPill(text: hint).padding(.top, 8).transition(.opacity)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// The active space's name in the title bar of a window whose sidebar is hidden: plain text, not a
/// button. Drag it to move the space to another window; right-click it to rename the space.
struct SpaceChip: View {
    @ObservedObject var model: AppModel
    let windowID: String
    let space: String
    let name: String

    var body: some View {
        Text(name)
            .lineLimit(1)
            .contentShape(Rectangle())
            .spaceDraggable(model, space: space)
            .contextMenu {
                Button("Rename Space…") { model.beginRename(space: space) }
                Button("Rename Window…") { model.beginRename(window: windowID) }
            }
            .help("Drag to move this space to another window; right-click to rename it")
            .accessibilityLabel("Space \(name)")
            .accessibilityAction(named: "Rename Space…") { model.beginRename(space: space) }
    }
}

/// What every window says while the server is stopped from the menu bar item.
struct ServerStoppedNotice: View {
    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: "powersleep").font(.largeTitle).foregroundStyle(.secondary)
            Text("Wheel Server is stopped").font(.headline)
            Text("Start it from the Wheel item in the menu bar.").foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
        .accessibilityElement(children: .combine)
    }
}
