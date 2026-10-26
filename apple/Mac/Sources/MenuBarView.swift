import AppKit
import SwiftUI
import WheelKit

/// The menu bar item's label: the Wheel mark, and the count of approval requests waiting in spaces
/// no window shows.
struct MenuBarLabel: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let waiting = model.unseenApprovals
        HStack(spacing: 3) {
            Image(systemName: "steeringwheel")
            if waiting > 0 { Text("\(waiting)") }
        }
        .accessibilityLabel(waiting > 0 ? "Wheel, \(waiting) waiting for approval" : "Wheel")
    }
}

/// What the menu bar item opens: the server, the pairing code and settings, a card per window
/// listing its spaces, a Background card for the spaces no window shows, and New Window.
struct MenuBarPopover: View {
    @ObservedObject var model: AppModel
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ServerHeader(model: model)
            Divider()
            VStack(alignment: .leading, spacing: 2) {
                PairingRow(model: model)
                Button {
                    NSApp.activate()
                    openSettings()
                } label: {
                    PopoverRow(symbol: "gearshape", title: "Settings…", trailing: "⌘,")
                }
                .buttonStyle(.plain)
            }
            ForEach(model.layout.windows) { window in
                WindowCard(model: model, window: window)
            }
            BackgroundCard(model: model)
            if let hint = model.spaceDragHint, case .card? = model.spaceDrag?.target {
                DropHintPill(text: hint).frame(maxWidth: .infinity)
            }
            Divider()
            Button { model.newWindow() } label: {
                PopoverRow(symbol: "macwindow", title: "New Window", trailing: "⌘N")
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .frame(width: 320)
    }
}

/// One line of the popover: an icon, a title, and optional trailing text.
struct PopoverRow: View {
    let symbol: String
    let title: String
    var detail: String?
    var trailing: String?

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: symbol).frame(width: 18).foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                if let detail { Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle) }
            }
            Spacer(minLength: 0)
            if let trailing { Text(trailing).foregroundStyle(.secondary) }
        }
        .padding(.vertical, 3)
        .contentShape(Rectangle())
    }
}

/// The server's line: the app icon, its name, and a coloured dot with a word, so the colour is
/// never the only signal. Pressing it opens the details and the Stop and Start buttons.
struct ServerHeader: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button { withAnimation(.smooth(duration: 0.2)) { model.serverExpanded.toggle() } } label: {
                HStack(spacing: 10) {
                    Image(nsImage: NSApp.applicationIconImage)
                        .resizable()
                        .frame(width: 36, height: 36)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Wheel Server").fontWeight(.semibold)
                        HStack(spacing: 5) {
                            Circle().fill(model.serverStatus.color).frame(width: 8, height: 8)
                            Text(model.serverStatus.word).font(.caption).foregroundStyle(.secondary)
                        }
                        .accessibilityElement(children: .combine)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: model.serverExpanded ? "chevron.up" : "chevron.down").foregroundStyle(.secondary)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Wheel Server, \(model.serverStatus.word)")
            .accessibilityHint(model.serverExpanded ? "Hides the details" : "Shows the details")
            if model.serverExpanded { ServerDetails(model: model) }
        }
    }
}

struct ServerDetails: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let stopped = model.serverStoppedByMenu
        VStack(alignment: .leading, spacing: 6) {
            ForEach(model.serverDetails, id: \.self) { line in
                Text(line).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if let problem = model.serverProblem {
                Label(problem, systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.orange)
            }
            if let why = model.serverControl?.unavailable {
                Text(why).font(.caption).foregroundStyle(.secondary)
            } else if stopped || model.connection != .connected {
                Button("Start Server") { model.startServer() }
                    .buttonStyle(.borderedProminent)
            } else if model.serverConfirming {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Stop Wheel Server?").fontWeight(.semibold)
                    Text("Terminals and running agents in \(model.spaces.count) \(model.spaces.count == 1 ? "space" : "spaces") stop.")
                        .font(.callout).foregroundStyle(.secondary)
                    HStack {
                        Spacer()
                        Button("Cancel") { model.cancelStopServer() }
                            .keyboardShortcut(.cancelAction)
                        Button("Stop Server", role: .destructive) { model.stopServer() }
                            .buttonStyle(.borderedProminent).tint(.red)
                    }
                }
                .padding(10)
                .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator))
            } else {
                Button("Stop Server", role: .destructive) { model.askToStopServer() }
            }
        }
        .padding(.leading, 46)
    }
}

struct PairingRow: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let reason = model.pairingDisabledReason
        HStack(spacing: 4) {
            PopoverRow(symbol: "iphone", title: "Pairing code", detail: model.pairingRowDetail)
            Button { model.copyPairingCode() } label: {
                Image(systemName: "doc.on.doc").frame(width: 16)
            }
            .buttonStyle(.borderless)
            .disabled(reason != nil)
            .help(reason ?? "Copy the pairing code; the clipboard clears after a minute")
            .accessibilityLabel("Copy pairing code")
            Button { model.showPairingWindow() } label: {
                Image(systemName: "qrcode").frame(width: 16)
            }
            .buttonStyle(.borderless)
            .disabled(reason != nil)
            .help(reason ?? "Show the pairing code as a QR code")
            .accessibilityLabel("Show QR code")
        }
    }
}

/// A window's card: its title and how many spaces it holds, then one row per space.
struct WindowCard: View {
    @ObservedObject var model: AppModel
    let window: WindowRecord

    var body: some View {
        let spaces = model.spaces(in: window.id)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "macwindow").foregroundStyle(.secondary).frame(width: 18)
                Text(model.title(of: window)).fontWeight(.semibold).lineLimit(1)
                Spacer(minLength: 8)
                Text("window · \(spaces.count) \(spaces.count == 1 ? "space" : "spaces")")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(.bottom, 4)
            if spaces.isEmpty {
                Text("No spaces. Drop one here.").font(.caption).foregroundStyle(.secondary).padding(.vertical, 4)
            }
            ForEach(spaces) { space in
                SpaceRow(model: model, space: space, in: window.id, shown: window.active == space.id)
            }
        }
        .padding(10)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(model.spaceDrag?.target == .card(window: window.id) ? Color.accentColor : Color.secondary.opacity(0.35), lineWidth: model.spaceDrag?.target == .card(window: window.id) ? 2 : 1))
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.cardFrames[window.id] = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(model.title(of: window)) window")
    }
}

/// The spaces that run with no window: a dashed, tinted card, so it reads apart from a window
/// without colour.
struct BackgroundCard: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let spaces = model.backgroundSpaces
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "tray").foregroundStyle(.secondary).frame(width: 18)
                Text("Background").fontWeight(.semibold)
                Spacer(minLength: 8)
                Text("running, no window · \(spaces.count)").font(.caption).foregroundStyle(.secondary)
            }
            .padding(.bottom, 4)
            if spaces.isEmpty {
                Text("Nothing here. Drop a space to keep it running without a window.")
                    .font(.caption).foregroundStyle(.secondary).padding(.vertical, 4)
            }
            ForEach(spaces) { space in
                SpaceRow(model: model, space: space, in: nil, shown: false)
            }
        }
        .padding(10)
        .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(model.spaceDrag?.target == .card(window: nil) ? Color.accentColor : Color.secondary.opacity(0.6), style: StrokeStyle(lineWidth: model.spaceDrag?.target == .card(window: nil) ? 2 : 1, dash: [4, 3])))
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.cardFrames[MenuBarWindow.backgroundKey] = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Background spaces")
    }
}

/// A space in a card: its name, a dot when its window shows it, a badge when approvals wait in it,
/// and the buttons that send it to a window or the background. Clicking the name shows it.
struct SpaceRow: View {
    @ObservedObject var model: AppModel
    let space: SpaceInfo
    /// The window holding it; nil in the background.
    let window: String?
    let shown: Bool

    init(model: AppModel, space: SpaceInfo, in window: String?, shown: Bool) {
        self.model = model
        self.space = space
        self.window = window
        self.shown = shown
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                // A Text with a tap, as the sidebar's rows are: the drag gesture sat on a Button here, and a row dragged by hand did not move.
                Text(space.name)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    .onTapGesture {
                        NSApp.activate()
                        model.focus(space: space.id)
                    }
                    .spaceDraggable(model, space: space.id)
                    .accessibilityAddTraits(.isButton)
                    .accessibilityHint("Shows the space in its window")
                if let waiting = model.waitingApprovals[space.id] {
                    Text("\(waiting)")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.white)
                        .frame(minWidth: 16, minHeight: 16)
                        .background(Color.orange, in: Capsule())
                        .accessibilityLabel("\(waiting) waiting for approval")
                } else if shown {
                    Circle().fill(.green).frame(width: 7, height: 7).accessibilityLabel("Shown")
                }
                Button { model.windowChooser = model.windowChooser == space.id ? nil : space.id } label: {
                    Image(systemName: "macwindow").frame(width: 16)
                }
                .buttonStyle(.borderless)
                .help("Send \(space.name) to a window")
                .accessibilityLabel("Send \(space.name) to a window")
                Button { model.place(space.id, in: nil) } label: {
                    Image(systemName: "tray").frame(width: 16)
                }
                .buttonStyle(.borderless)
                .disabled(window == nil)
                .help("Send \(space.name) to the background")
                .accessibilityLabel("Send \(space.name) to the background")
            }
            .padding(.vertical, 3)
            if model.windowChooser == space.id {
                WindowChooser(model: model, space: space.id, current: window) { model.windowChooser = nil }
            }
        }
    }
}

/// The windows a space can go to, and a new one.
struct WindowChooser: View {
    @ObservedObject var model: AppModel
    let space: String
    let current: String?
    let done: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(model.layout.windows.filter { $0.id != current }) { window in
                Button {
                    model.place(space, in: window.id)
                    done()
                } label: {
                    Label(model.title(of: window), systemImage: "macwindow").frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.vertical, 3)
            }
            Button {
                model.tearOff(space)
                done()
            } label: {
                Label("New window", systemImage: "plus.rectangle").frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.vertical, 3)
        }
        .font(.callout)
        .padding(8)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
        .accessibilityLabel("Windows")
    }
}

extension AppModel {
    /// Approval requests waiting in each space, by space id.
    var waitingApprovals: [String: Int] { AgentPanes.waiting(perms, in: spaces) }

    /// Approval requests waiting in spaces no window shows: what the menu bar item and the Dock
    /// icon count.
    var unseenApprovals: Int {
        let shown = shownSpaces
        return waitingApprovals.filter { !shown.contains($0.key) }.values.reduce(0, +)
    }
}

extension AppModel {
    /// The address the pairing code names, or why there is none.
    var pairingAddressText: String {
        let devices = settings.devices
        guard let info = devices.info else { return "Asking wheeld…" }
        if info.listen == nil { return "Remote access is off" }
        if let problem = info.problem { return "Listener failed: \(problem)" }
        guard let address = devices.address else { return "No address" }
        return "wss://\(address):\(info.port)"
    }
}
