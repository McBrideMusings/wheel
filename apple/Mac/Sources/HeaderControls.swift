import AppKit
import SwiftUI
import WheelKit

/// A control a pane's header can draw as an icon button.
enum HeaderControl: String, CaseIterable, Identifiable, Codable {
    case browser, switchView, info, splitRight, splitDown, pop, background, zoom, screenshot, close

    var id: String { rawValue }

    var title: String {
        switch self {
        case .browser: "Browser controls"
        case .switchView: "Chat / terminal switch"
        case .info: "Pane info"
        case .splitRight: "Split right"
        case .splitDown: "Split down"
        case .pop: "Pop out"
        case .background: "Background"
        case .zoom: "Zoom"
        case .screenshot: "Copy pane image"
        case .close: "Close"
        }
    }

    var symbol: String {
        switch self {
        case .browser: "ellipsis.circle"
        case .switchView: "bubble.left.and.text.bubble.right"
        case .info: "info.circle"
        case .splitRight: "square.split.2x1"
        case .splitDown: "square.split.1x2"
        case .pop: "rectangle.on.rectangle"
        case .background: "tray.and.arrow.down"
        case .zoom: "arrow.up.left.and.arrow.down.right"
        case .screenshot: "camera.viewfinder"
        case .close: "xmark"
        }
    }

    /// The header level (`PaneHeader.levels`) at which a narrow header stops drawing this control.
    /// A control the header drops lands in the ⋯ menu instead.
    var dropLevel: Int {
        switch self {
        case .browser, .info, .pop, .background, .screenshot: 1
        case .switchView: 2
        case .splitRight, .splitDown: 3
        case .zoom: 4
        case .close: Int.max
        }
    }

    /// Whether panes of kind `kind` (`PaneKind.name`) have this control at all.
    func applies(to kind: String) -> Bool {
        switch self {
        case .browser: kind == "browser"
        case .switchView: kind == "terminal" || kind == "agent"
        case .info, .splitRight, .splitDown: kind != "unknown"
        case .pop, .background, .zoom, .screenshot, .close: true
        }
    }
}

/// Where a control is offered: as an icon in the header, in the ⋯ menu, or nowhere.
enum HeaderPlacement: String, CaseIterable, Identifiable, Codable {
    case bar, menu, off

    var id: String { rawValue }

    var title: String {
        switch self {
        case .bar: "Top bar"
        case .menu: "⋯ menu"
        case .off: "Off"
        }
    }
}

/// The pane kinds that have a header of their own to set up, in the order Settings lists them.
enum HeaderKind: String, CaseIterable, Identifiable {
    case terminal, agent, web, browser, empty

    var id: String { rawValue }
    var title: String { rawValue.capitalized }
}

/// Each pane kind's choices, stored in this Mac's preferences (the header is this client's own
/// drawing, so no server holds it). A control with no choice for its kind is in the top bar.
struct HeaderLayout: Codable, Equatable {
    /// Placement by pane kind name, then control.
    var choices: [String: [String: HeaderPlacement]] = [:]

    static let key = "paneHeaderControls"

    static func saved() -> HeaderLayout {
        guard let data = Prefs.store.data(forKey: key),
              let layout = try? JSONDecoder().decode(HeaderLayout.self, from: data) else { return HeaderLayout() }
        return layout
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) { Prefs.store.set(data, forKey: Self.key) }
    }

    func placement(_ control: HeaderControl, kind: String) -> HeaderPlacement {
        choices[kind]?[control.rawValue] ?? .bar
    }

    mutating func set(_ control: HeaderControl, kind: String, to placement: HeaderPlacement) {
        if placement == .bar {
            choices[kind]?[control.rawValue] = nil
            if choices[kind]?.isEmpty == true { choices[kind] = nil }
        } else {
            choices[kind, default: [:]][control.rawValue] = placement
        }
    }

    /// Drawn as an icon in a header of `level`.
    func inBar(_ control: HeaderControl, kind: String, level: Int) -> Bool {
        control.applies(to: kind) && placement(control, kind: kind) == .bar && level < control.dropLevel
    }

    /// Offered in the ⋯ menu of a header of `level`: chosen for the menu, or dropped from the bar
    /// by a narrow header.
    func inMenu(_ control: HeaderControl, kind: String, level: Int) -> Bool {
        guard control.applies(to: kind) else { return false }
        return switch placement(control, kind: kind) {
        case .menu: true
        case .bar: level >= control.dropLevel
        case .off: false
        }
    }

    func isOff(_ control: HeaderControl, kind: String) -> Bool {
        placement(control, kind: kind) == .off
    }
}

extension AppModel {
    func setHeaderPlacement(_ control: HeaderControl, kind: String, to placement: HeaderPlacement) {
        headerLayout.set(control, kind: kind, to: placement)
        headerLayout.save()
        Log.info("pane header: \(kind) \(control.rawValue) -> \(placement.rawValue)")
    }

    func resetHeaderLayout(kind: String) {
        headerLayout.choices[kind] = nil
        headerLayout.save()
        Log.info("pane header: \(kind) reset")
    }

    /// The header's Copy Pane Image button: draw the pane (what `wheel pane shot` draws) and put
    /// the PNG on the clipboard, with a notice saying which. A later copy replaces the notice and
    /// restarts its 3 s timer.
    func copyPaneImage(_ pane: String) {
        drawPane(pane) { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success(png):
                let board = NSPasteboard.general
                board.clearContents()
                board.setData(png, forType: .png)
                Log.info("pane \(pane): image copied (\(png.count) bytes)")
                post(Toast(id: "pane-image", level: .notice, text: "Copied \(pane) as an image"))
            case let .failure(failure):
                post(Toast(id: "pane-image", level: .error, text: "Could not copy \(pane): \(failure.message)"))
            }
            paneImageTimer?.cancel()
            paneImageTimer = Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(3))
                guard !Task.isCancelled else { return }
                self?.removeToast("pane-image")
            }
        }
    }
}
