import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI
import WheelKit

/// The Devices tab's state: how remote clients reach the daemon, which of its addresses the
/// pairing code names, and whether the token shows. Nothing here decides what an address is or
/// what the code holds: `RemoteInfo` does, so the debug endpoint and the window read one answer.
@MainActor
final class DevicesModel: ObservableObject {
    /// The daemon's last `remote` reply; nil until it has answered.
    @Published private(set) var info: RemoteInfo?
    /// The address a person picked for the code; nil keeps `RemoteInfo.defaultAddress`.
    @Published var picked: String?
    @Published var tokenShown = false
    /// The outstanding `remote_info`.
    var askedReq: UInt64?

    func received(_ info: RemoteInfo) { self.info = info }

    /// The address the code names: the pick while the daemon still lists it, else the default.
    var address: String? {
        guard let info else { return nil }
        if let picked, info.addresses.contains(picked) { return picked }
        return info.defaultAddress
    }

    /// The daemon's token, read from its file; nil when the file cannot be read.
    func token(_ file: URL) -> String? {
        let text = (try? String(contentsOf: file, encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines)
        return text?.isEmpty == false ? text : nil
    }

    /// Why there is no code to show.
    var unavailable: String {
        guard let info else { return "the daemon has not answered remote_info yet" }
        if let problem = info.problem { return "remote access did not start: \(problem)" }
        if info.listen == nil { return "remote access is off" }
        return address == nil ? "the daemon reports no address" : "no token"
    }

    /// The JSON the code encodes, or nil when there is nothing to pair with.
    func pairingJSON(token: String?) -> String? {
        guard let info, info.isOn, let address, let token else { return nil }
        return info.pairing(address: address, token: token)
    }

    /// The tab as the debug state reports it.
    func stateJSON(token: String?) -> [String: Any] {
        [
            "known": info != nil,
            "on": info?.isOn ?? false,
            "listen": info?.listen ?? NSNull(),
            "problem": info?.problem ?? NSNull(),
            "addresses": info?.addresses ?? [],
            "port": info.map { Int($0.port) } ?? NSNull(),
            "address": address ?? NSNull(),
            "fingerprint": info?.fingerprint ?? NSNull(),
            "token_shown": tokenShown,
            "pairing": pairingJSON(token: token) ?? NSNull(),
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
            Text("Add this to this machine’s wheel.toml, or set remote.listen on the Global tab, then restart wheeld:")
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
        let token = devices.token(model.tokenFile)
        return HStack(alignment: .top, spacing: 24) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Pair a device").font(ChatFont.body).fontWeight(.semibold)
                Picker("Address in the code", selection: Binding(get: { devices.address ?? "" }, set: { devices.picked = $0 })) {
                    ForEach(info.addresses, id: \.self) { Text($0).tag($0) }
                }
                .pickerStyle(.menu)
                row("Port", "\(info.port)")
                row("Listening on", info.listen ?? "")
                row("Fingerprint", info.fingerprint, mono: true)
                HStack(alignment: .firstTextBaseline) {
                    Text("Token").foregroundStyle(.secondary).frame(width: 84, alignment: .leading)
                    if devices.tokenShown {
                        Text(token ?? "unreadable").font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    } else {
                        Text("••••••••").foregroundStyle(.secondary)
                    }
                    Button(devices.tokenShown ? "Hide token" : "Show token") { devices.tokenShown.toggle() }
                        .buttonStyle(.link)
                }
                .font(ChatFont.line)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            qr(devices.pairingJSON(token: token))
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
