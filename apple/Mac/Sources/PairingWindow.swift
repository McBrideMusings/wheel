import AppKit
import SwiftUI
import WheelKit

/// What the pairing window shows and does while a phone pairs. The code itself is the Devices
/// model's (a one-time token wheeld issued, and the address it names); this holds the window's own
/// state: the switch, the details, who connected, and the events the details list.
struct PairingState: Equatable {
    var open = false
    /// "Close when a device connects": on by default.
    var closeOnConnect = true
    var detailsOpen = false
    /// The device that connected while the window was open, once one has.
    var connected: String?
    /// The latest events, newest last, at most five.
    var events: [String] = []
}

/// The concealed-pasteboard marker clipboard managers honour: an item that carries it is not recorded.
private let concealedType = NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType")

/// How long a copied pairing code stays on the clipboard if nothing else is copied.
let pairingClipboardLife: Duration = .seconds(60)

extension AppModel {
    /// Why the pairing code cannot be copied or shown now; nil when it can.
    var pairingDisabledReason: String? {
        if serverStoppedByMenu || connection != .connected { return "The server is stopped." }
        guard let info = settings.devices.info else { return "Asking wheeld how phones reach it…" }
        if info.problem != nil { return "The remote listener failed. Open Wheel Server for the reason." }
        if info.listen == nil { return "Remote access is off. Turn it on in Settings, Devices." }
        return nil
    }

    /// The row's second line: the address, or why there is none.
    var pairingRowDetail: String { pairingDisabledReason ?? pairingAddressText }

    // MARK: The clipboard

    /// Copy a fresh pairing code as a concealed clipboard item, cleared after a minute if unchanged.
    func copyPairingCode() {
        guard pairingDisabledReason == nil else { return }
        copyPending = true
        requestPairingCode()
    }

    /// The code reached the app: whatever asked for it takes it now.
    func pairingCodeArrived() {
        if copyPending, let json = settings.devices.pairingJSON {
            copyPending = false
            writeConcealed(json)
        }
        if pairing.open { objectWillChange.send() }
    }

    func writeConcealed(_ text: String) {
        let board = NSPasteboard.general
        board.clearContents()
        board.declareTypes([.string, concealedType], owner: nil)
        board.setString(text, forType: .string)
        board.setData(Data(), forType: concealedType)
        Log.info("pairing code copied to the clipboard as a concealed item; clearing it in \(pairingClipboardLife)")
        clipboardClear?.cancel()
        // An app nobody is looking at is napped by the system, which would stretch the minute; the
        // clear is held to its time.
        let activity = ProcessInfo.processInfo.beginActivity(options: .userInitiatedAllowingIdleSystemSleep, reason: "clearing the pairing code from the clipboard")
        clipboardClear = Task { [weak self] in
            defer { ProcessInfo.processInfo.endActivity(activity) }
            try? await Task.sleep(for: pairingClipboardLife)
            guard !Task.isCancelled else { return }
            // Unchanged means the clipboard still holds the code. The change count is no test: a
            // clipboard manager or the system touches it after a write, whatever was copied.
            let board = NSPasteboard.general
            if board.string(forType: .string) == text {
                board.clearContents()
                Log.info("pairing code cleared from the clipboard after a minute")
            } else {
                let now = board.string(forType: .string)
                Log.info("clipboard holds something else now (\(now.map { "\($0.count) chars" } ?? "no text"), the code was \(text.count)); left alone")
            }
            self?.clipboardClear = nil
        }
    }

    func copyPairingImage() {
        guard let json = settings.devices.pairingJSON, let image = DevicesSettings.qrImage(json) else { return }
        let board = NSPasteboard.general
        board.clearContents()
        board.writeObjects([image])
        Log.info("pairing code image copied to the clipboard")
    }

    // MARK: The window

    func showPairingWindow() {
        guard pairingDisabledReason == nil else { return }
        pairing = PairingState(open: true, closeOnConnect: pairing.closeOnConnect, detailsOpen: pairing.detailsOpen)
        pairingEvent("opened the pairing window")
        requestPairingCode()
        // A code stops working after five minutes; the open window keeps one that works.
        pairingRefresh?.invalidate()
        pairingRefresh = Timer.scheduledTimer(withTimeInterval: 240, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.requestPairingCode() }
        }
        if pairingPanel == nil { pairingPanel = PairingPanel(model: self) }
        pairingPanel?.show()
    }

    func closePairingWindow() {
        pairing.open = false
        pairingRefresh?.invalidate()
        pairingRefresh = nil
        pairingPanel?.hide()
    }

    func pairingEvent(_ text: String) {
        let stamp = Date().formatted(.dateTime.hour(.twoDigits(amPM: .omitted)).minute().second())
        pairing.events = Array((pairing.events + ["\(stamp) \(text)"]).suffix(5))
    }

    /// wheeld says a remote client connected: name it, and close the window unless the switch is off.
    func deviceConnected(name: String, address: String, paired: Bool) {
        pairingEvent("client “\(name)” connected from \(address)\(paired ? " and paired" : "")")
        Log.info("device \(name) connected from \(address), paired=\(paired)")
        // A phone that was paired already and reconnected is not the one this window waits for.
        guard pairing.open, paired else { return }
        pairing.connected = name
        guard pairing.closeOnConnect else { return }
        Task { [weak self] in
            // Long enough to read who it was.
            try? await Task.sleep(for: .seconds(1.5))
            guard let self, self.pairing.open, self.pairing.connected == name else { return }
            self.closePairingWindow()
        }
    }
}

/// The pairing window: a small panel above the app's windows with only a close button.
@MainActor
final class PairingPanel: NSObject, NSWindowDelegate {
    private unowned let model: AppModel
    let panel: NSPanel

    init(model: AppModel) {
        self.model = model
        panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 340, height: 120), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        super.init()
        panel.title = "Pair a device"
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.becomesKeyOnlyIfNeeded = true
        panel.collectionBehavior = [.fullScreenAuxiliary, .moveToActiveSpace]
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true
        panel.delegate = self
        let host = NSHostingController(rootView: PairingView(model: model))
        host.sizingOptions = [.preferredContentSize]
        panel.contentViewController = host
    }

    func show() {
        if !panel.isVisible { panel.center() }
        panel.orderFrontRegardless()
    }

    func hide() { panel.orderOut(nil) }

    func windowWillClose(_ notification: Notification) {
        if model.pairing.open { model.closePairingWindow() }
    }
}

struct PairingView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var devices: DevicesModel

    init(model: AppModel) {
        self.model = model
        devices = model.settings.devices
    }

    private var connected: String? { model.pairing.connected }

    var body: some View {
        VStack(spacing: 10) {
            Group {
                if let json = devices.pairingJSON, let image = DevicesSettings.qrImage(json) {
                    Image(nsImage: image)
                        .interpolation(.none)
                        .resizable()
                        .frame(width: 190, height: 190)
                        .padding(8)
                        .background(.white, in: RoundedRectangle(cornerRadius: 6))
                        .accessibilityLabel("Pairing code")
                } else {
                    ProgressView().frame(width: 190, height: 190)
                }
            }
            HStack(spacing: 6) {
                Circle().fill(connected == nil ? Color.orange : Color.green).frame(width: 8, height: 8)
                Text(connected.map { "\($0) connected" } ?? "Waiting for a device to scan…").font(.callout)
            }
            .accessibilityElement(children: .combine)
            HStack(spacing: 14) {
                Button("Copy code") { model.copyPairingCode() }
                Button("Copy image") { model.copyPairingImage() }
            }
            .buttonStyle(.link)
            Divider()
            Toggle("Close when a device connects", isOn: Binding(get: { model.pairing.closeOnConnect }, set: { model.pairing.closeOnConnect = $0 }))
                .toggleStyle(.switch)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button(model.pairing.detailsOpen ? "Hide details" : "Details") {
                withAnimation(.smooth(duration: 0.2)) { model.pairing.detailsOpen.toggle() }
            }
            .buttonStyle(.link)
            if model.pairing.detailsOpen { PairingDetails(model: model, devices: devices) }
        }
        .padding(16)
        .frame(width: 320)
    }
}

struct PairingDetails: View {
    @ObservedObject var model: AppModel
    @ObservedObject var devices: DevicesModel

    private func row(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(label).foregroundStyle(.secondary).frame(width: 88, alignment: .leading)
            Text(value).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    var body: some View {
        let info = devices.info
        VStack(alignment: .leading, spacing: 5) {
            row("Listening", info?.listen ?? "off")
            row("Addresses", info?.addresses.joined(separator: ", ") ?? "unknown")
            row("Certificate", info.map { "sha256 \($0.fingerprint.prefix(4))…\($0.fingerprint.suffix(4))" } ?? "unknown")
            row("Problem", info?.problem ?? "none")
            Divider()
            ForEach(Array(model.pairing.events.enumerated()), id: \.offset) { _, line in
                Text(line).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .font(.system(size: 11, design: .monospaced))
        .padding(10)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.separator))
    }
}
