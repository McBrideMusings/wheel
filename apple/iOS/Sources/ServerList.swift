import Foundation
import Observation
import SwiftUI
import WheelKit

/// Every wheeld this phone is paired with, a `PhoneModel` and its connection each, and which one the
/// phone shows. No wheeld knows another: the phone merges what each reports (ADR-0057).
@MainActor @Observable
final class ServerList {
    /// One per Keychain pairing, in the order they were paired.
    private(set) var servers: [PhoneModel] = []
    /// The server whose session the phone shows.
    private(set) var shown: PhoneModel?
    /// The model a pairing code for a new server is tried on (first run, Add Server…); once a code
    /// connects it joins `servers` and a fresh one takes its place.
    private(set) var adding: PhoneModel!

    /// Debug: further pairing codes from `WHEEL_PAIR_JSON`, held while they connect.
    @ObservationIgnored private var trials: [PhoneModel] = []

    /// Apple's push token, given to every server: each wheeld sends its own pushes (ADR-0024).
    private var apnsToken: String?

    init() { adding = make() }

    /// More than one server: only then is a server's mark drawn (ADR-0057).
    var several: Bool { servers.count > 1 }

    func start() {
        #if DEBUG
        let env = ProcessInfo.processInfo.environment
        if let marks = env["WHEEL_LOG_MARKS"] { Log.file.writeMarks(marks) }
        // A UI test starts from no paired server: the simulator's Keychain outlives an uninstall.
        if env["WHEEL_FORGET_SERVERS"] != nil {
            for pairing in PairingStore.all() { PairingStore.remove(pairing) }
            Log.info("servers: forgot every pairing (WHEEL_FORGET_SERVERS)")
        }
        #endif
        for pairing in PairingStore.all() where !servers.contains(where: { $0.pairing?.url == pairing.url }) {
            let model = make()
            servers.append(model)
            model.resume(pairing)
        }
        let saved = UserDefaults.standard.string(forKey: Self.shownKey)
        show(servers.first { $0.pairing?.url.absoluteString == saved } ?? servers.first)
        Log.info("servers: \(servers.count) paired, showing \(shown?.pairing?.url.absoluteString ?? "none")")
        #if DEBUG
        // `WHEEL_PAIR_JSON` is pasted pairing codes: one code's JSON, or a JSON array of them, each
        // added as Add Server… adds it.
        if let json = env["WHEEL_PAIR_JSON"] {
            Log.info("pairing from WHEEL_PAIR_JSON")
            for code in Self.codes(in: json) {
                let model = trials.isEmpty && !adding.isTrying ? adding! : make()
                if model !== adding { trials.append(model) }
                model.pair(json: code)
            }
        }
        #endif
    }

    /// Add Server… closed: a code still being tried stops.
    func cancelAdding() {
        if adding.isTrying { adding.unpair() }
    }

    /// Forget `model`'s server: its pairing leaves the Keychain and the phone shows another server.
    func remove(_ model: PhoneModel) {
        Log.info("forget server \(model.serverName)")
        model.unpair()
        servers.removeAll { $0 === model }
        if shown === model { show(servers.first) }
    }

    /// Show `session` of `model`'s server; the server shown before stops streaming.
    func show(session: String, of model: PhoneModel) {
        show(model)
        model.selectSession(session)
    }

    /// A tapped notification's space and pane: shown on the server holding that space. At a cold
    /// start no server has listed its spaces yet, so the tap waits for the layout that names it.
    func focus(space: String, pane: String) {
        guard let model = servers.first(where: { $0.spaces.contains { $0.id == space } }) else {
            Log.info("notification for \(space)/\(pane): no server lists the space yet; waiting for a layout")
            pendingFocus = (space, pane)
            return
        }
        pendingFocus = nil
        show(model)
        model.focus(space: space, pane: pane)
    }

    /// A notification's space and pane whose server has not listed the space yet.
    @ObservationIgnored private var pendingFocus: (space: String, pane: String)?

    private func layoutArrived(on model: PhoneModel) {
        guard let pending = pendingFocus, model.spaces.contains(where: { $0.id == pending.space }) else { return }
        focus(space: pending.space, pane: pending.pane)
    }

    func pushToken(_ token: String) {
        apnsToken = token
        for model in servers { model.pushToken(token) }
    }

    /// Show `model`'s server; every other server stops streaming and drops its view.
    func show(_ model: PhoneModel?) {
        for other in servers where other !== model { other.setOnScreen(false) }
        model?.setOnScreen(true)
        guard shown !== model else { return }
        shown = model
        UserDefaults.standard.set(model?.pairing?.url.absoluteString, forKey: Self.shownKey)
        if let model { Log.info("showing server \(model.serverName)") }
    }

    private func make() -> PhoneModel {
        let model = PhoneModel()
        model.setOnScreen(false)
        if let apnsToken { model.pushToken(apnsToken) }
        model.onPaired = { [weak self] model in self?.paired(model) }
        model.onLayout = { [weak self, weak model] in if let model { self?.layoutArrived(on: model) } }
        return model
    }

    /// A pairing code connected and was kept: the server joins the list in place of any entry for
    /// the same wheeld, and is shown.
    private func paired(_ model: PhoneModel) {
        if adding === model { adding = make() }
        trials.removeAll { $0 === model }
        let same: (PhoneModel) -> Bool = { other in
            guard other !== model, let mine = model.pairing, let theirs = other.pairing else { return false }
            if let id = mine.server, let held = theirs.server { return id == held }
            return mine.url == theirs.url
        }
        if let i = servers.firstIndex(where: same) {
            // Paired again: the new model takes the old one's place, as its Keychain entry does.
            Log.info("server \(servers[i].serverName) paired again; the new pairing replaces it")
            servers[i].disconnect()
            servers[i] = model
            servers.removeAll { $0 !== model && same($0) }
        } else if !servers.contains(where: { $0 === model }) {
            servers.append(model)
        }
        show(model)
    }

    /// The pairing codes in `text`: one JSON object, or an array of them.
    static func codes(in text: String) -> [String] {
        guard let data = text.data(using: .utf8),
              let array = try? JSONSerialization.jsonObject(with: data) as? [Any] else { return [text] }
        return array.compactMap { item in
            (try? JSONSerialization.data(withJSONObject: item)).flatMap { String(data: $0, encoding: .utf8) }
        }
    }

    private static let shownKey = "shownServer"
}

/// A server is its model: the list and the session bar key it by the model's identity.
extension PhoneModel: Identifiable {}

/// A server's mark: its system colour on a server glyph (a Mac's `desktopcomputer`, a Linux
/// server's `server.rack`), beside its name when `named` (ADR-0057). Drawn only with several servers.
struct ServerMark: View {
    let model: PhoneModel
    var named = true

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: Self.symbol(platform: model.host?.platform))
                .foregroundStyle(Self.color(model.host?.color))
            if named { Text(model.serverName).lineLimit(1) }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Server \(model.serverName)")
    }

    static func symbol(platform: String?) -> String {
        platform?.hasPrefix("linux") == true ? "server.rack" : "desktopcomputer"
    }

    /// The system colour `server.color` names; the secondary colour for none or one this app does not know.
    static func color(_ name: String?) -> Color {
        switch name {
        case "red": .red
        case "orange": .orange
        case "yellow": .yellow
        case "green": .green
        case "mint": .mint
        case "teal": .teal
        case "cyan": .cyan
        case "blue": .blue
        case "indigo": .indigo
        case "purple": .purple
        case "pink": .pink
        case "brown": .brown
        case "gray": .gray
        default: .secondary
        }
    }
}
