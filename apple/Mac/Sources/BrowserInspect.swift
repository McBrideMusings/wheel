import AppKit
import WebKit
import WheelKit

/// `inspect_pane`: the daemon asks this window to run JavaScript in a browser pane's page
/// (`wheel pane eval`), read its HTML (`wheel pane dom`) or draw one element of it as a PNG
/// (`wheel pane shot --selector`). Only a browser pane's web view answers; an artifact pane is not
/// one, and the daemon refuses it before it asks.
extension AppModel {
    func inspectPane(id: UInt64, pane: String, op: String, arg: String?) {
        guard let web = browserPanes[pane]?.view else {
            Log.info("inspect \(id): pane \(pane) is not on screen")
            reply(.renderRefused(id: id, message: RenderRefusal.notShown))
            return
        }
        Log.info("inspect \(id): \(op) on \(pane) arg=\(arg ?? "-")")
        switch op {
        case "eval":
            web.evaluateJavaScript(arg ?? "") { [weak self] value, error in
                MainActor.assumeIsolated {
                    if let error { self?.refuseRender(id, pane, Self.scriptMessage(error)); return }
                    guard let text = Self.json(value) else {
                        self?.refuseRender(id, pane, "the result is not JSON")
                        return
                    }
                    Log.info("inspect \(id): \(pane) eval result \(text.count) chars")
                    self?.reply(.inspected(id: id, text: text))
                }
            }
        case "dom":
            let source = arg.map { Self.literal($0) }
            let script = """
            (function (selector) {
              if (selector === null) return "ok:" + document.documentElement.outerHTML;
              var found = document.querySelectorAll(selector);
              if (!found.length) return "none";
              return "ok:" + Array.prototype.map.call(found, function (e) { return e.outerHTML; }).join("\\n");
            })(\(source ?? "null"))
            """
            web.evaluateJavaScript(script) { [weak self] value, error in
                MainActor.assumeIsolated {
                    if let error { self?.refuseRender(id, pane, Self.scriptMessage(error)); return }
                    guard let reply = value as? String else { self?.refuseRender(id, pane, "the page returned no HTML"); return }
                    if reply.hasPrefix("ok:") {
                        Log.info("inspect \(id): \(pane) dom \(reply.count - 3) chars")
                        self?.reply(.inspected(id: id, text: String(reply.dropFirst(3))))
                    } else {
                        self?.refuseRender(id, pane, "no element matches \(arg ?? "")")
                    }
                }
            }
        case "shot":
            shotElement(id: id, pane: pane, web: web, selector: arg ?? "")
        default:
            refuseRender(id, pane, "unknown inspect op \(op)")
        }
    }

    /// The first element `selector` matches, scrolled into view and cropped out of the web view's own snapshot.
    private func shotElement(id: UInt64, pane: String, web: WKWebView, selector: String) {
        let script = """
        (function (selector) {
          var el = document.querySelector(selector);
          if (!el) return "none";
          el.scrollIntoView({ block: "nearest", inline: "nearest" });
          var r = el.getBoundingClientRect();
          return JSON.stringify([r.x, r.y, r.width, r.height]);
        })(\(Self.literal(selector)))
        """
        web.evaluateJavaScript(script) { [weak self] value, error in
            MainActor.assumeIsolated {
                guard let self else { return }
                if let error { self.refuseRender(id, pane, Self.scriptMessage(error)); return }
                guard let text = value as? String, text != "none" else {
                    self.refuseRender(id, pane, "no element matches \(selector)")
                    return
                }
                guard let box = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [Double], box.count == 4 else {
                    self.refuseRender(id, pane, "the page gave no box for \(selector): \(text)")
                    return
                }
                let rect = CGRect(x: box[0], y: box[1], width: box[2], height: box[3]).intersection(web.bounds)
                guard !rect.isNull, rect.width >= 1, rect.height >= 1 else {
                    self.refuseRender(id, pane, "\(selector) has no visible area in the pane")
                    return
                }
                let configuration = WKSnapshotConfiguration()
                configuration.rect = rect
                Log.info("inspect \(id): \(pane) shot \(selector) rect \(rect)")
                web.takeSnapshot(with: configuration) { [weak self] image, error in
                    MainActor.assumeIsolated {
                        guard let png = image.flatMap(Self.png) else {
                            self?.refuseRender(id, pane, "WebKit drew no image: \(error?.localizedDescription ?? "no error")")
                            return
                        }
                        self?.answerRender(id, pane, png)
                    }
                }
            }
        }
    }

    /// `text` as a JavaScript string literal.
    private static func literal(_ text: String) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: [text]), let json = String(data: data, encoding: .utf8) else { return "\"\"" }
        return String(json.dropFirst().dropLast())
    }

    /// A script result as JSON text; `undefined` is `null`.
    private static func json(_ value: Any?) -> String? {
        guard let value, !(value is NSNull) else { return "null" }
        guard JSONSerialization.isValidJSONObject([value]),
              let data = try? JSONSerialization.data(withJSONObject: [value], options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8) else { return nil }
        return String(text.dropFirst().dropLast())
    }

    /// What the page threw, as the page worded it.
    private static func scriptMessage(_ error: Error) -> String {
        let info = (error as NSError).userInfo
        return (info["WKJavaScriptExceptionMessage"] as? String) ?? error.localizedDescription
    }
}
