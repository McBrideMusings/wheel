import AppKit
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
        // The window's title bar is transparent and its content runs under it, so the title row
        // is drawn here, as tall as the title bar the system gives the window.
        GeometryReader { outer in
            VStack(spacing: 0) {
                TitleRow(model: model, windowID: windowID)
                    // A full-screen window may report no title bar; the row keeps its own height.
                    .frame(height: max(outer.safeAreaInsets.top, TitleRow.minHeight))
                VStack(spacing: 6) {
                    if model.serverStoppedByMenu {
                        ServerStoppedNotice()
                    } else {
                        if model.mode(in: windowID) == WindowMode.desk {
                            DeskView()
                        } else {
                            SpacesView(model: model, windowID: windowID)
                        }
                    }
                    HStack(spacing: 12) {
                        ConnectionBadge(connection: model.connection)
                        UsageStrip(usage: model.usage)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding([.horizontal, .bottom], 10)
                .padding(.top, 2)
            }
            .ignoresSafeArea(.container, edges: .top)
        }
        .frame(minWidth: 900, minHeight: 560)
        .background(Color(nsColor: .windowBackgroundColor))
        .overlay { SpaceDropOutline(model: model, windowID: windowID) }
        .overlay { if model.layout.focused == windowID { ToastStack(model: model) } }
        .overlay {
            // A window with no spaces shows the palette in place (`NoSpaces`).
            if let palette = model.commandPalette, model.layout.focused == windowID, !model.spaces(in: windowID).isEmpty {
                CommandPaletteOverlay(model: model, palette: palette)
            }
        }
        .overlay {
            if model.shortcutsOpen, model.layout.focused == windowID {
                ShortcutSheet(model: model)
            }
        }
        // The debug endpoint's stand-in for the app menu's Settings…
        .onChange(of: settings.openRequest) { if model.layout.focused == windowID { openSettings() } }
        .sheet(isPresented: Binding(get: { model.closingWindow == windowID },
                                    set: { if !$0, model.closingWindow == windowID { model.answerWindowClose(.cancel) } })) {
            WindowCloseSheet(model: model, windowID: windowID)
        }
        // On a view of its own: a sheet shares its view with no other sheet.
        .background {
            Color.clear.sheet(isPresented: Binding(get: { model.diskAccessScreenShown && model.layout.focused == windowID },
                                                   set: { if !$0 { model.dismissDiskAccessScreen() } })) {
                DiskAccessScreen(model: model)
            }
        }
        .onAppear { model.startDiskAccessCheck() }
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

/// The sheet the red light of the last window showing a session opens: the session runs on in the
/// background, or closes with everything in it.
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
                Button { model.answerWindowClose(.background) } label: { Text("Background Session and Close").frame(maxWidth: .infinity) }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                Button(role: .destructive) { model.answerWindowClose(.stop) } label: { Text("Close Session and All Processes").foregroundStyle(.red).frame(maxWidth: .infinity) }
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
        case .unreachable:
            Label("Not connected to wheeld", systemImage: "exclamationmark.triangle")
                .font(.caption).foregroundStyle(.secondary)
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
                    if let space { Text(space).foregroundStyle(.secondary) }
                } else {
                    Text(space ?? "Wheel").fontWeight(.semibold)
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
