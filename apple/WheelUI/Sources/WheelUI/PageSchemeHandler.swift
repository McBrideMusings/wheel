import Foundation
import WebKit
import WheelKit

/// Where the daemon is and the token it wants; set at each connect and shared by every web pane.
/// `pairing` is set when the daemon is a remote listener with a self-signed certificate, so the
/// handler's requests pin it.
@MainActor
public final class PageDaemon {
    public var url: URL?
    public var token: String?
    public var pairing: Pairing?

    public init() {}
}

/// Answers one web pane's requests under `WebPanes.scheme` by fetching them from the daemon with the
/// token in a header, for that pane's own folder only. The page's origin is the scheme and a fixed
/// host, so its `localStorage` stays with it when the daemon comes back on another port.
@MainActor
public final class PageSchemeHandler: NSObject, WKURLSchemeHandler {
    private let pane: String
    private let daemon: PageDaemon
    /// Told the status of each page the pane's main frame loads.
    public var onDocument: ((Int) -> Void)?

    public init(pane: String, daemon: PageDaemon) {
        self.pane = pane
        self.daemon = daemon
    }

    private var pinned: (pairing: Pairing, session: URLSession)?
    private lazy var plain: URLSession = Self.makeSession(pairing: nil)

    /// No cookie jar and no cache: the token travels in a header, and a reload after a file
    /// changed must reach the daemon. With a pairing, the connection pins its certificate.
    private static func makeSession(pairing: Pairing?) -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.httpCookieStorage = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        guard let pairing else { return URLSession(configuration: config) }
        return PinnedSessionDelegate.session(for: pairing, configuration: config) {
            Task { @MainActor in UILog.info("web page request refused: certificate does not match the pinned fingerprint") }
        }
    }

    /// The session for the daemon's current pairing: pinned to its certificate for a remote
    /// listener, plain for a loopback daemon.
    private func currentSession() -> URLSession {
        guard let pairing = daemon.pairing else { return plain }
        if let pinned, pinned.pairing == pairing { return pinned.session }
        pinned?.session.invalidateAndCancel()
        let session = Self.makeSession(pairing: pairing)
        pinned = (pairing, session)
        return session
    }
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

    public func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        let page = task.request.url
        guard let page, let address = daemon.url, let token = daemon.token else {
            UILog.info("web pane \(pane) request refused: no daemon address or token yet")
            task.didFailWithError(URLError(.cannotConnectToHost))
            return
        }
        // The page reaches its own folder and its pane's data directory, nothing else the token opens.
        guard let target = WebPanes.daemonURL(for: page, daemon: address, pane: pane) else {
            UILog.info("web pane \(pane) request refused outside its folder: \(WebPanes.withoutQuery(page))")
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
        let shown = WebPanes.withoutQuery(page)
        let pane = pane
        let session = currentSession()
        let sent = request
        serial += 1
        let mine = serial
        let work = Task { @MainActor [weak self] in
            defer { if self?.running[key]?.serial == mine { self?.running.removeValue(forKey: key) } }
            do {
                // The page's event stream never completes, so it is handed over as it arrives;
                // every other response is held whole so its length is known.
                let streaming = target.path.hasSuffix("/.wheel/events")
                var held = Data()
                var events: URLSession.AsyncBytes?
                let response: URLResponse
                if streaming {
                    let (bytes, answered) = try await session.bytes(for: sent)
                    events = bytes
                    response = answered
                } else {
                    let (body, answered) = try await session.data(for: sent)
                    held = body
                    response = answered
                }
                guard !Task.isCancelled else { return }
                guard let http = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
                var headers: [String: String] = [:]
                for (name, value) in http.allHeaderFields {
                    if let name = name as? String, let value = value as? String { headers[name] = value }
                }
                // URLSession already inflated the body, so the encoded length no longer applies.
                headers.removeValue(forKey: "Content-Encoding")
                if streaming {
                    headers.removeValue(forKey: "Content-Length")
                } else {
                    headers["Content-Length"] = String(held.count)
                }
                guard let answer = HTTPURLResponse(url: page, statusCode: http.statusCode, httpVersion: "HTTP/1.1", headerFields: headers) else {
                    task.didFailWithError(URLError(.badServerResponse))
                    return
                }
                UILog.info("web pane \(pane) page \(shown) -> \(http.statusCode), \(streaming ? "streaming" : "\(held.count) bytes")")
                if task.request.mainDocumentURL == page { self?.onDocument?(http.statusCode) }
                task.didReceive(answer)
                if let events {
                    // Flushing at every newline sends whole lines, so a page never sees half of one.
                    var chunk = Data()
                    for try await byte in events {
                        guard !Task.isCancelled else { return }
                        chunk.append(byte)
                        if byte == 0x0A {
                            task.didReceive(chunk)
                            chunk.removeAll(keepingCapacity: true)
                        }
                    }
                    guard !Task.isCancelled else { return }
                    if !chunk.isEmpty { task.didReceive(chunk) }
                } else {
                    task.didReceive(held)
                }
                task.didFinish()
            } catch {
                guard !Task.isCancelled else { return }
                UILog.info("web pane \(pane) page \(shown) failed: \(WebPanes.describe(error))")
                task.didFailWithError(error)
            }
        }
        running[key] = (mine, work)
    }

    public func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {
        running.removeValue(forKey: ObjectIdentifier(task))?.work.cancel()
    }
}
