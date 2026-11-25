import Foundation
import WheelKit

/// The values pushed into the pages a remote client shows (`artifact_data`), carried on its one connection to wheeld
/// (`pageWatch`, `pageData`). A page's `.wheel/events` stream is answered from here, so a client that keeps a page for
/// every web pane holds no connection per page: wheeld's remote listener refuses a peer's connections past its 16th.
@MainActor
public final class PageFeed {
    private let send: (ClientMsg) -> Void
    private var listeners: [String: [UUID: AsyncThrowingStream<Data, Error>.Continuation]] = [:]
    /// The last value each watched pane was sent, for a second page of that pane that opens later.
    private var latest: [String: String] = [:]
    /// Panes watched again on a new connection, whose first value is the one their pages already have.
    private var rewatched: Set<String> = []

    public init(send: @escaping (ClientMsg) -> Void) {
        self.send = send
    }

    /// The event stream of `pane`'s page, as `text/event-stream` bytes: the latest value first, then each one pushed. The
    /// first stream of a pane watches it; the last one to end stops the watch.
    public func events(_ pane: String) -> AsyncThrowingStream<Data, Error> {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        let id = UUID()
        if listeners[pane]?.isEmpty ?? true {
            send(.pageWatch(req: nil, pane: pane))
        } else if let data = latest[pane] {
            continuation.yield(Self.frame(data))
        }
        listeners[pane, default: [:]][id] = continuation
        continuation.onTermination = { [weak self] _ in
            Task { @MainActor in self?.ended(pane, id) }
        }
        return stream
    }

    /// Take one wheeld message; true when it was a page's value.
    @discardableResult
    public func handle(_ msg: ServerMsg) -> Bool {
        guard case let .pageData(pane, data) = msg else { return false }
        guard let held = listeners[pane], !held.isEmpty else { return true }
        if rewatched.remove(pane) != nil, latest[pane] == data { return true }
        latest[pane] = data
        for continuation in held.values { continuation.yield(Self.frame(data)) }
        return true
    }

    /// A new connection watches nothing yet: watch every pane a page still streams.
    public func connected() {
        for (pane, held) in listeners where !held.isEmpty {
            if latest[pane] != nil { rewatched.insert(pane) }
            send(.pageWatch(req: nil, pane: pane))
        }
    }

    /// The connected wheeld has no `page_watch`: every stream ends, and each page's `EventSource` asks again, over HTTP.
    public func end() {
        let held = listeners.values.flatMap(\.values)
        listeners = [:]
        latest = [:]
        rewatched = []
        held.forEach { $0.finish(throwing: URLError(.networkConnectionLost)) }
    }

    /// Streams open on `pane`, for tests and the debug state.
    func streams(_ pane: String) -> Int { listeners[pane]?.count ?? 0 }

    private func ended(_ pane: String, _ id: UUID) {
        guard listeners[pane]?.removeValue(forKey: id) != nil else { return }
        if listeners[pane]?.isEmpty == true {
            listeners[pane] = nil
            latest[pane] = nil
            rewatched.remove(pane)
            send(.pageUnwatch(req: nil, pane: pane))
        }
    }

    private static func frame(_ data: String) -> Data { Data("data: \(data)\n\n".utf8) }
}
