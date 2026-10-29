import SwiftUI
import WheelKit

/// The window: the active space's pane grid, with the connection and the usage strip under it.
/// Stock macOS styling.
struct ContentView: View {
    var model: AppModel
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
        .overlay {
            if model.shortcutsOpen, model.layout.focused == windowID {
                ShortcutSheet(model: model)
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
            if !model.sidebarOpen(in: windowID) {
                ToolbarItem(placement: .navigation) {
                    SpaceTabs(model: model, windowID: windowID)
                }
            }
            ToolbarItem(placement: .primaryAction) {
                if let space = model.layout.window(windowID)?.active {
                    ProcessesButton(model: model, space: space)
                }
            }
            ToolbarItem(placement: .primaryAction) {
                Button { model.openCommandPalette() } label: {
                    Text("⌘K").font(.system(size: 12, weight: .medium, design: .rounded))
                }
                .help("Command palette: spaces, panes, artifacts and actions")
                .accessibilityLabel("Command palette")
            }
            ToolbarItem(placement: .primaryAction) {
                let pinned = model.layout.window(windowID)?.pinned == true
                Button { model.setPinned(!pinned, window: windowID) } label: {
                    Image(systemName: pinned ? "pin.fill" : "pin").foregroundStyle(pinned ? Color.accentColor : Color.primary)
                }
                .help(pinned ? "Unpin" : "Pin on top")
                .accessibilityLabel(pinned ? "Unpin" : "Pin")
            }
            ToolbarItem(placement: .primaryAction) {
                Button { model.openNewSpacePalette() } label: { Image(systemName: "plus") }
                    .help("New space from a folder")
                    .accessibilityLabel("New space")
            }
        }
        // The debug endpoint's stand-in for the app menu's Settings…
        .onChange(of: settings.openRequest) { if model.layout.focused == windowID { openSettings() } }
        .sheet(isPresented: Binding(get: { model.closingWindow == windowID },
                                    set: { if !$0, model.closingWindow == windowID { model.answerWindowClose(.cancel) } })) {
            WindowCloseSheet(model: model, windowID: windowID)
        }
        // A close or a pane change that ends a shell or an agent, asked in this window. Return
        // answers yes and Esc no, so a key that asked can be answered from the keys.
        .alert(
            model.confirmation?.title ?? "",
            isPresented: Binding(get: { model.confirmation != nil && model.confirmation?.window == windowID },
                                 set: { if !$0 { model.answerConfirmation(false) } }),
            presenting: model.confirmation
        ) { pending in
            Button(pending.button, role: .destructive) { model.answerConfirmation(true) }
                .keyboardShortcut(.defaultAction)
            Button("Cancel", role: .cancel) { model.answerConfirmation(false) }
        } message: { pending in
            Text(pending.message)
        }
    }

    @Environment(\.openSettings) private var openSettings
}

/// The sheet a main window's red light opens when the window holds spaces.
struct WindowCloseSheet: View {
    var model: AppModel
    let windowID: String

    var body: some View {
        let record = model.layout.window(windowID)
        VStack(spacing: 6) {
            Text("Close the “\(record.map { model.title(of: $0) } ?? "")” window?")
                .font(.headline)
            Text(record.map { model.closeSummary(of: $0) } ?? "")
                .font(.subheadline).foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            VStack(spacing: 8) {
                Button { model.answerWindowClose(.background) } label: { Text("Background Spaces and Close").frame(maxWidth: .infinity) }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                Button(role: .destructive) { model.answerWindowClose(.stop) } label: { Text("Close All Processes").foregroundStyle(.red).frame(maxWidth: .infinity) }
                Button { model.answerWindowClose(.cancel) } label: { Text("Cancel").frame(maxWidth: .infinity) }
                    .keyboardShortcut(.cancelAction)
            }
            .controlSize(.regular)
            .padding(.top, 12)
        }
        .padding(20)
        .frame(width: 300)
    }
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
    var model: AppModel
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

/// While a space is dragged over this window: an accent outline when its body takes the space.
struct SpaceDropOutline: View {
    var model: AppModel
    let windowID: String

    var body: some View {
        let target = model.spaceDrag?.target
        ZStack {
            if target == .window(windowID) {
                RoundedRectangle(cornerRadius: 8).strokeBorder(Color.accentColor, lineWidth: 3).padding(2)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// The active space's name in the title bar of a window whose sidebar is hidden: plain text, not a
/// button. Drag it to move the space to another window; right-click it for the space's menu.
struct SpaceChip: View {
    var model: AppModel
    let windowID: String
    let space: String
    let name: String

    var body: some View {
        Text(name)
            .lineLimit(1)
            .opacity(model.isDragged(space) ? 0.4 : 1)
            .contentShape(Rectangle())
            .spaceDraggable(model, space: space)
            .contextMenu {
                SpaceMenuItems(model: model, space: space, window: windowID)
                Divider()
                Button("Rename Window…") { model.beginRename(window: windowID) }
            }
            .help("Drag to move this space to another window; right-click for its menu")
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
