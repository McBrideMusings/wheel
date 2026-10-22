import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI
import WheelKit
import WheelUI

/// The Devices tab's state: how remote clients reach the daemon, which of its addresses the
/// pairing code names, the code itself (a one-time token the daemon issued) and the devices that
/// paired. Nothing here decides what an address is or what the code holds: `RemoteInfo` does, so
/// the debug endpoint and the window read one answer.
@MainActor
final class DevicesModel: ObservableObject {
    /// The daemon's last `remote` reply; nil until it has answered.
    @Published private(set) var info: RemoteInfo?
    /// The address a person picked for the code; nil keeps `RemoteInfo.defaultAddress`.
    @Published var picked: String?
    /// The devices that paired, as the daemon last listed them.
    @Published private(set) var paired: [DeviceInfo] = []
    /// The one-time token the code carries, and when it stops working.
    @Published private(set) var code: (token: String, expires: Date)?
    /// Why the daemon would not issue a code.
    @Published var codeProblem: String?

    func received(_ info: RemoteInfo) { self.info = info }
    func received(devices: [DeviceInfo]) { paired = devices }
    func received(token: String, expiresMs: UInt64) {
        let expires = Date(timeIntervalSince1970: Double(expiresMs) / 1000)
        code = (token, expires)
        codeProblem = nil
        expiry?.cancel()
        expiry = Task { [weak self] in
            try? await Task.sleep(for: .seconds(max(0, expires.timeIntervalSinceNow) + 0.05))
            guard !Task.isCancelled else { return }
            self?.objectWillChange.send()
        }
    }

    /// Redraws the tab when the code expires: `liveToken` reads the clock, and nothing else changes then.
    private var expiry: Task<Void, Never>?

    /// The code's token while it still works.
    var liveToken: String? {
        guard let code, code.expires > Date() else { return nil }
        return code.token
    }

    /// The address the code names: the pick while the daemon still lists it, else the default.
    var address: String? {
        guard let info else { return nil }
        if let picked, info.addresses.contains(picked) { return picked }
        return info.defaultAddress
    }

    /// Why there is no code to show.
    var unavailable: String {
        guard let info else { return "the daemon has not answered remote_info yet" }
        if let problem = info.problem { return "remote access did not start: \(problem)" }
        if info.listen == nil { return "remote access is off" }
        if address == nil { return "the daemon reports no address" }
        return codeProblem ?? "no code has been made, or the last one expired"
    }

    /// The JSON the code encodes, or nil when there is nothing to pair with.
    var pairingJSON: String? {
        guard let info, info.isOn, let address, let token = liveToken else { return nil }
        return info.pairing(address: address, token: token)
    }

    /// The tab as the debug state reports it.
    func stateJSON() -> [String: Any] {
        [
            "known": info != nil,
            "on": info?.isOn ?? false,
            "listen": info?.listen ?? NSNull(),
            "problem": info?.problem ?? NSNull(),
            "addresses": info?.addresses ?? [],
            "port": info.map { Int($0.port) } ?? NSNull(),
            "address": address ?? NSNull(),
            "fingerprint": info?.fingerprint ?? NSNull(),
            "code_expires_ms": code.map { Int($0.expires.timeIntervalSince1970 * 1000) } ?? NSNull(),
            "pairing": pairingJSON ?? NSNull(),
            "paired": paired.map { ["id": $0.id, "name": $0.name, "connected": $0.connected, "last_seen_ms": $0.lastSeenMs] as [String: Any] },
        ]
    }
}

/// Settings › Devices: the addresses another device can dial, the port, and a QR code holding
/// the URL, token and certificate fingerprint a phone needs to pair.
struct DevicesSettings: View {
    @ObservedObject var model: AppModel
    @ObservedObject var devices: DevicesModel

    var body: some View {
        Group {
            if model.connection != .connected {
                notice("wheeld unreachable", "Devices come from the daemon; they show here once it answers.")
            } else if let info = devices.info {
                if let problem = info.problem {
                    notice("Remote access did not start", problem)
                } else if info.listen == nil {
                    off
                } else {
                    on(info)
                }
            } else {
                notice("Asking wheeld how devices reach it…", nil)
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    private var off: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Remote access is off").font(ChatFont.body).fontWeight(.semibold)
            Text("Add this to this machine’s wheel.toml, or set remote.listen on the General tab, then restart wheeld:")
                .font(ChatFont.line).foregroundStyle(.secondary)
            Text("[remote]\nlisten = \"0.0.0.0:7430\"")
                .font(.system(.body, design: .monospaced))
                .textSelection(.enabled)
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
        }
    }

    private func on(_ info: RemoteInfo) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top, spacing: 24) {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Pair a device").font(ChatFont.body).fontWeight(.semibold)
                    Picker("Address in the code", selection: Binding(get: { devices.address ?? "" }, set: { devices.picked = $0 })) {
                        ForEach(info.addresses, id: \.self) { Text($0).tag($0) }
                    }
                    .pickerStyle(.menu)
                    row("Port", "\(info.port)")
                    row("Listening on", info.listen ?? "")
                    row("Fingerprint", info.fingerprint, mono: true)
                    HStack {
                        Button(devices.liveToken == nil ? "Make a pairing code" : "Make another code") { model.requestPairingCode() }
                        if let expires = devices.code?.expires, expires > Date() {
                            Text("Works once, until \(expires.formatted(date: .omitted, time: .shortened)).")
                                .font(ChatFont.line).foregroundStyle(.secondary)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                qr(devices.pairingJSON)
            }
            pairedList
        }
    }

    private var pairedList: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Paired devices").font(ChatFont.body).fontWeight(.semibold)
            if devices.paired.isEmpty {
                Text("None yet. Scan the code from the iPhone app.").font(ChatFont.line).foregroundStyle(.secondary)
            }
            ForEach(devices.paired) { device in
                HStack {
                    Image(systemName: "iphone")
                    VStack(alignment: .leading, spacing: 1) {
                        Text(device.name)
                        Text(device.connected > 0 ? "Connected" : "Last seen \(Date(timeIntervalSince1970: Double(device.lastSeenMs) / 1000).formatted(.relative(presentation: .named)))")
                            .font(ChatFont.line).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Revoke", role: .destructive) { model.revoke(device: device.id) }
                        .accessibilityLabel("Revoke \(device.name)")
                }
                .font(ChatFont.line)
                Divider()
            }
        }
    }

    private func row(_ label: String, _ value: String, mono: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label).foregroundStyle(.secondary).frame(width: 84, alignment: .leading)
            Text(value).font(mono ? .system(.caption, design: .monospaced) : ChatFont.line).textSelection(.enabled)
        }
        .font(ChatFont.line)
    }

    @ViewBuilder private func qr(_ payload: String?) -> some View {
        if let payload, let image = Self.qrImage(payload) {
            Image(nsImage: image)
                .interpolation(.none)
                .resizable()
                .frame(width: 220, height: 220)
                .padding(10)
                .background(.white, in: RoundedRectangle(cornerRadius: 8))
                .accessibilityLabel("Pairing code")
        } else {
            Text("No pairing code: \(devices.unavailable)").font(ChatFont.line).foregroundStyle(.secondary).frame(width: 220)
        }
    }

    private func notice(_ title: String, _ detail: String?) -> some View {
        VStack(spacing: 6) {
            Text(title).font(ChatFont.body)
            if let detail { Text(detail).font(ChatFont.line).foregroundStyle(.secondary) }
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 60)
    }

    /// `text` as a QR code, one pixel per module scaled up to 440 so it stays sharp.
    static func qrImage(_ text: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scale = (440 / output.extent.width).rounded(.down)
        let scaled = output.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        let rep = NSCIImageRep(ciImage: scaled)
        let image = NSImage(size: rep.size)
        image.addRepresentation(rep)
        return image
    }
}
