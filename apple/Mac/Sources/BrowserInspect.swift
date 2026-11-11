import AppKit
import WebKit
import WheelKit
import WheelUI

/// `inspect_pane`: the daemon asks this window to run JavaScript in a browser pane's page
/// (`wheel pane eval`), read its HTML (`wheel pane dom`), draw one element of it as a PNG
/// (`wheel pane shot --selector`) or film it (`wheel pane record`). Only a browser pane's web view
/// answers; an artifact pane is not one, and the daemon refuses it before it asks. The work is
/// `BrowserInspect` in WheelUI, shared with the GPUI client's bridge.
extension AppModel {
    func inspectPane(id: UInt64, pane: String, op: String, arg: String?) {
        guard let web = browserPanes[pane]?.view else {
            Log.info("inspect \(id): pane \(pane) is not on screen")
            reply(.renderRefused(id: id, message: RenderRefusal.notShown))
            return
        }
        Log.info("inspect \(id): \(op) on \(pane) arg=\(arg ?? "-")")
        Task { @MainActor [weak self] in
            switch await BrowserInspect.run(op, arg: arg, in: web) {
            case let .text(text):
                Log.info("inspect \(id): \(pane) \(op) \(text.count) chars")
                self?.reply(.inspected(id: id, text: text))
            case let .png(png): self?.answerRender(id, pane, png)
            case let .film(film):
                Log.info("record \(id): \(pane) \(film.frames) frames in \(film.millis) ms, \(film.mov.count) bytes")
                self?.reply(.recorded(id: id, mov: film.mov, frames: film.frames, millis: film.millis))
            case let .refused(message): self?.refuseRender(id, pane, message)
            }
        }
    }
}
