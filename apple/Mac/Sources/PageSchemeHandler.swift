import Foundation
import WebKit
import WheelKit

/// Where the daemon is and the token it wants; set at each connect and shared by every web pane.
@MainActor
final class PageDaemon {
    var url: URL?
    var token: String?
}

/// Answers one web pane's requests under `WebPanes.scheme` by fetching them from the daemon with the
/// token in a header, for that pane's own folder only. The page's origin is the scheme and a fixed
/// host, so its `localStorage` stays with it when the daemon comes back on another port.
@MainActor
final class PageSchemeHandler: NSObject, WKURLSchemeHandler {
    private let pane: String
    private let daemon: PageDaemon
    /// Told the status of each page the pane's main frame loads.
    var onDocument: ((Int) -> Void)?

    init(pane: String, daemon: PageDaemon) {
        self.pane = pane
        self.daemon = daemon
    }

    /// No cookie jar and no cache: the token travels in a header, and a reload after a file
    /// changed must reach the daemon.
    private let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.httpCookieStorage = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: config)
    }()
    private var running: [ObjectIdentifier: (serial: Int, work: Task<Void, Never>)] = [:]
    private var serial = 0

    /// Headers the page's own request carries that the daemon must not see: any `Origin` makes it
    /// refuse the request as a browser's.
    private static let dropped: Set<String> = ["origin", "referer", "host", "cookie", "authorization"]

    /// WebKit hands a request's body over as a stream.
    private static func drain(_ stream: InputStream) -> Data {
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 16384)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count <= 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }

    func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        let page = task.request.url
        guard let page, let address = daemon.url, let token = daemon.token else {
            Log.info("web pane \(pane) request refused: no daemon address or token yet")
            task.didFailWithError(URLError(.cannotConnectToHost))
            return
        }
        // The page reaches its own folder and its pane's data directory, nothing else the token opens.
        guard let target = WebPanes.daemonURL(for: page, daemon: address, pane: pane) else {
            Log.info("web pane \(pane) request refused outside its folder: \(WebPane.withoutQuery(page))")
            task.didFailWithError(URLError(.noPermissionsToReadFile))
            return
        }
        var request = URLRequest(url: target)
        request.httpMethod = task.request.httpMethod
        request.httpBody = task.request.httpBody ?? task.request.httpBodyStream.map(Self.drain)
        for (name, value) in task.request.allHTTPHeaderFields ?? [:] where !Self.dropped.contains(name.lowercased()) {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let shown = WebPane.withoutQuery(page)
        let pane = pane
        let session = session
        let sent = request
        serial += 1
        let mine = serial
        let work = Task { @MainActor [weak self] in
            defer { if self?.running[key]?.serial == mine { self?.running.removeValue(forKey: key) } }
            do {
                let (body, response) = try await session.data(for: sent)
                guard !Task.isCancelled else { return }
                guard let http = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
                var headers: [String: String] = [:]
                for (name, value) in http.allHeaderFields {
                    if let name = name as? String, let value = value as? String { headers[name] = value }
                }
                // URLSession already inflated the body, so the encoded length no longer applies.
                headers["Content-Length"] = String(body.count)
                headers.removeValue(forKey: "Content-Encoding")
                guard let answer = HTTPURLResponse(url: page, statusCode: http.statusCode, httpVersion: "HTTP/1.1", headerFields: headers) else {
                    task.didFailWithError(URLError(.badServerResponse))
                    return
                }
                Log.info("web pane \(pane) page \(shown) -> \(http.statusCode), \(body.count) bytes")
                if task.request.mainDocumentURL == page { self?.onDocument?(http.statusCode) }
                task.didReceive(answer)
                task.didReceive(body)
                task.didFinish()
            } catch {
                guard !Task.isCancelled else { return }
                Log.info("web pane \(pane) page \(shown) failed: \(WebPane.describe(error))")
                task.didFailWithError(error)
            }
        }
        running[key] = (mine, work)
    }

    func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {
        running.removeValue(forKey: ObjectIdentifier(task))?.work.cancel()
    }
}
