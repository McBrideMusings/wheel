import AppKit
import SwiftUI
import WheelKit
import WheelUI

/// The menu bar item's label: the Wheel mark as a template image, with the number of spaces
/// running in the background in a badge, and none at zero. Approvals waiting in spaces no window
/// shows are in the accessibility label, on the Dock icon and on the rows.
struct MenuBarLabel: View {
    var model: AppModel

    var body: some View {
        let background = model.backgroundRows.count
        let waiting = model.unseenApprovals
        let update = model.serverRestartWaiting
        Image(nsImage: MenuBarIcon.image(background: background, update: update))
            .accessibilityLabel(Self.accessibility(background: background, waiting: waiting, update: update))
    }

    static func accessibility(background: Int, waiting: Int, update: Bool = false) -> String {
        var parts = ["Wheel"]
        if update { parts.append("a new server waits for a restart") }
        if background > 0 { parts.append("\(background) \(background == 1 ? "space" : "spaces") in the background") }
        if waiting > 0 { parts.append("\(waiting) waiting for approval") }
        return parts.joined(separator: ", ")
    }
}

/// What the menu bar item opens: the server, the pairing code and settings, a card per window
/// listing its spaces, a Background card for the spaces no window shows, and New Window.
struct MenuBarPopover: View {
    var model: AppModel
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
            Divider()
            Button { model.newWindow() } label: {
                PopoverRow(symbol: "macwindow", title: "New Window", trailing: "⌘N")
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .frame(width: 320)
        // Closing the popover under the pointer sends no exit event.
        .onDisappear {
            model.popoverHover = nil
            model.closePreview()
        }
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
    var model: AppModel

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
    var model: AppModel

    var body: some View {
        let stopped = model.serverStoppedByMenu
        VStack(alignment: .leading, spacing: 6) {
            ForEach(model.serverDetails, id: \.self) { line in
                Text(line).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if let problem = model.serverProblem {
                Label(problem, systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.orange)
            }
            if model.serverRestartWaiting, model.serverControl?.unavailable == nil { ServerRestartControl(model: model) }
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

/// The window's banner while a newer server waits for a restart: what it brings, Restart Server…
/// and a close button. Closing it hides it until a different server is installed; the menu bar
/// item keeps its dot and its header keeps the notice.
struct ServerUpdateBanner: View {
    var model: AppModel

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: "arrow.triangle.2.circlepath").foregroundStyle(.orange)
            Text(AppModel.serverUpdatedText)
                .font(.callout)
                .frame(maxWidth: .infinity, alignment: .leading)
            if model.serverControl?.unavailable == nil { ServerRestartControl(model: model) }
            Button { model.dismissServerBanner() } label: {
                Image(systemName: "xmark").frame(width: 16, height: 16)
            }
            .buttonStyle(.borderless)
            .help("Hide this until a different server is installed")
            .accessibilityLabel("Dismiss")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color.orange.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.orange.opacity(0.35)))
    }
}

/// Restart Server: one click restarts the server from the bundle. In the window's banner, the menu
/// bar header and Settings › General while a restart waits.
struct ServerRestartControl: View {
    var model: AppModel

    var body: some View {
        Button("Restart Server") { model.restartServer() }
            .buttonStyle(.borderedProminent)
            .help("Restart the server; every terminal and agent session ends")
    }
}

struct PairingRow: View {
    var model: AppModel

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

/// A window's group: its numbered icon, title and how many spaces it holds, then one row per space.
struct WindowCard: View {
    var model: AppModel
    let window: WindowRecord

    var body: some View {
        let spaces = model.spaces(in: window.id)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                PlaceGlyph(kind: .window(model.windowNumber(window.id) ?? 0)).foregroundStyle(.secondary)
                Text(model.title(of: window)).fontWeight(.semibold).lineLimit(1)
                Spacer(minLength: 8)
                Text("\(spaces.count) \(spaces.count == 1 ? "space" : "spaces")")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(.bottom, 4)
            if spaces.isEmpty {
                Text("No spaces. Drop one here.").font(.caption).foregroundStyle(.secondary).padding(.vertical, 4)
            }
            ForEach(Array(spaces.enumerated()), id: \.element.id) { offset, space in
                SpaceRow(model: model, space: space, in: window.id, shown: window.active == space.id, offset: offset, count: spaces.count)
            }
        }
        .padding(10)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(model.isCardTarget(window.id) ? Color.accentColor : Color.secondary.opacity(0.35), lineWidth: model.isCardTarget(window.id) ? 2 : 1))
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.cardFrames[window.id] = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(model.title(of: window)) window")
    }
}

/// The spaces that run with no window, and those in a little window that belongs to none: a
/// dashed, tinted card, so it reads apart from a window without colour.
struct BackgroundCard: View {
    var model: AppModel

    var body: some View {
        let spaces = model.backgroundRows
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                PlaceGlyph(kind: .background).foregroundStyle(.secondary)
                Text("Background").fontWeight(.semibold)
                Spacer(minLength: 8)
                Text("\(spaces.count)").font(.caption).foregroundStyle(.secondary)
            }
            .padding(.bottom, 4)
            if spaces.isEmpty {
                Text("Nothing here. Drop a space to keep it running without a window.")
                    .font(.caption).foregroundStyle(.secondary).padding(.vertical, 4)
            }
            ForEach(Array(spaces.enumerated()), id: \.element.id) { offset, space in
                SpaceRow(model: model, space: space, in: nil, shown: false, offset: offset, count: spaces.count)
            }
        }
        .padding(10)
        .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(model.isCardTarget(nil) ? Color.accentColor : Color.secondary.opacity(0.6), style: StrokeStyle(lineWidth: model.isCardTarget(nil) ? 2 : 1, dash: [4, 3])))
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.cardFrames[MenuBarWindow.backgroundKey] = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Background spaces")
    }
}

/// A space in a group: its name, one dot per agent (worst first, `+N` past four), a dot when its window shows it, a badge when approvals wait in
/// it, a little-window mark when it is in one that belongs to no window, and, under the pointer,
/// the buttons that move it. Clicking the name shows it; right-click opens the space's menu.
struct SpaceRow: View {
    var model: AppModel
    let space: SpaceInfo
    /// The window holding it; nil in the background.
    let window: String?
    let shown: Bool
    /// Its place in the group and the group's size: where the insertion bar draws.
    let offset: Int
    let count: Int

    init(model: AppModel, space: SpaceInfo, in window: String?, shown: Bool, offset: Int, count: Int) {
        self.model = model
        self.space = space
        self.window = window
        self.shown = shown
        self.offset = offset
        self.count = count
    }

    private var cardKey: String { window ?? MenuBarWindow.backgroundKey }

    /// Where the dragged space would land in this row's group: the number of rows above the bar.
    private var insertion: Int? {
        if let bar = model.spaceDragInsertion, bar.window == AppModel.popoverInsertionPrefix + cardKey { bar.index } else { nil }
    }

    var body: some View {
        let hovering = model.popoverHover == space.id
        let ownerlessLittle = window == nil && model.layout.little(showing: space.id) != nil
        HStack(spacing: 8) {
            PaneStack(space: space).foregroundStyle(.secondary)
            // A Text with a tap, as the sidebar's rows are: the drag gesture sat on a Button here, and a row dragged by hand did not move.
            Text(space.name)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .onTapGesture {
                    NSApp.activate()
                    model.focus(space: space.id)
                }
                .opacity(model.isDragged(space.id) ? 0.4 : 1)
                .spaceDraggable(model, space: space.id)
                .accessibilityAddTraits(.isButton)
                .accessibilityHint("Shows the space in its window")
            if ownerlessLittle {
                PlaceGlyph(kind: .little, size: 13).foregroundStyle(.secondary)
                    .help("In a little window")
                    .accessibilityLabel("In a little window")
            }
            SpaceDots(space: space, shown: model.statusDots)
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
            if hovering { PlaceButtons(model: model, space: space) }
        }
        .padding(.vertical, 3)
        .opacity(model.isDimmed(space: space.id) && !ownerlessLittle ? 0.5 : 1)
        .contentShape(Rectangle())
        .background { if model.paneIsOver(space: space.id) { PaneDropOutline(cornerRadius: 6) } }
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.popoverRowFrames[cardKey, default: [:]][space.id] = $0 }
        .overlay(alignment: .top) { if insertion == offset { InsertionBar().offset(y: -1) } }
        .overlay(alignment: .bottom) { if offset == count - 1, insertion == count { InsertionBar().offset(y: 1) } }
        .onHover { inside in model.rowHover(space.id, inside: inside) }
        .contextMenu { SpaceMenuItems(model: model, space: space.id, window: window) }
    }
}

extension AppModel {
    /// Whether the drag is over the popover group of `window` (the Background group when nil).
    func isCardTarget(_ window: String?) -> Bool {
        if case let .card(held, _)? = spaceDrag?.target { return held == window }
        return false
    }

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
