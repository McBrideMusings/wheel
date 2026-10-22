import Foundation

/// An app's own log file, and the outbox that sends its lines to wheeld. Each line is
/// `<ISO 8601 UTC time> <message>`; past `cap` bytes the file rolls to `<file>.1`. A saved position
/// (`<file>.sent`: the file's inode and the byte offset wheeld accepted) marks what has gone, so
/// lines written while disconnected go out on the next connection, once each — unless the
/// connection drops between wheeld taking a batch and its `ok` arriving, when that batch goes again and
/// wheeld drops the lines it already has (each line carries the offset just past it, and the batch
/// the file's inode). Every file operation
/// runs on one serial queue, off the caller's thread. The lines leave the device, so every URL in
/// one is written without its user, password, query and fragment, whoever wrote it.
public final class AppLog: @unchecked Sendable {
    /// A place in the log: the file (by inode, since a roll renames it) and a byte offset in it.
    public struct Position: Codable, Equatable, Sendable {
        public var inode: UInt64
        public var offset: UInt64
    }

    /// Lines not yet accepted, and the position just past them.
    public struct Batch: Sendable {
        public let lines: [AppLogLine]
        public let end: Position
    }

    public let url: URL
    /// The app's name as wheeld labels its lines: `mac`, `iphone` or `ipad`.
    public var client: String {
        get { lock.withLock { name } }
        set { lock.withLock { name = newValue } }
    }
    private var name: String
    private let cap: UInt64
    private let positionURL: URL
    /// Guards `name` and `written`.
    private let lock = NSLock()
    private let io = DispatchQueue(label: "wheel.applog", qos: .utility)
    private var written: (@Sendable () -> Void)?

    nonisolated(unsafe) private static let stamp: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    public init(url: URL, client: String, cap: UInt64 = 10 << 20) {
        self.url = url
        name = client
        self.cap = cap
        positionURL = url.appendingPathExtension("sent")
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        // The first run sends from here on, not every line the app wrote before it had an outbox.
        // A position file that exists but does not read sends the whole file again instead.
        if !FileManager.default.fileExists(atPath: positionURL.path) {
            if !FileManager.default.fileExists(atPath: url.path) { FileManager.default.createFile(atPath: url.path, contents: nil) }
            if let inode = inode(url) { save(Position(inode: inode, offset: size(url))) }
        }
    }

    /// Runs, on the log's own queue, after each line is in the file.
    public var onWrite: (@Sendable () -> Void)? {
        get { lock.withLock { written } }
        set { lock.withLock { written = newValue } }
    }

    public func write(_ message: String, at date: Date = Date()) {
        let data = Data("\(Self.stamp.string(from: date)) \(Self.redactingURLs(message))\n".utf8)
        io.async { [self] in
            if size(url) + UInt64(data.count) > cap {
                let rolled = url.appendingPathExtension("1")
                try? FileManager.default.removeItem(at: rolled)
                try? FileManager.default.moveItem(at: url, to: rolled)
            }
            if let handle = try? FileHandle(forWritingTo: url) {
                defer { try? handle.close() }
                _ = try? handle.seekToEnd()
                try? handle.write(contentsOf: data)
            } else {
                try? data.write(to: url)
            }
            onWrite?()
        }
    }

    nonisolated(unsafe) private static let urlPattern = try! NSRegularExpression(pattern: #"[A-Za-z][A-Za-z0-9+.-]*://[^\s"'<>]+"#)

    /// `message` with each URL in it cut to its scheme, host, port and path: a setting or an address
    /// can carry a credential in its user, password or query.
    static func redactingURLs(_ message: String) -> String {
        let matches = urlPattern.matches(in: message, range: NSRange(message.startIndex..., in: message))
        guard !matches.isEmpty else { return message }
        var out = message
        for match in matches.reversed() {
            guard let range = Range(match.range, in: out), var parts = URLComponents(string: String(out[range])) else { continue }
            guard parts.user != nil || parts.password != nil || parts.query != nil || parts.fragment != nil else { continue }
            parts.user = nil
            parts.password = nil
            parts.query = nil
            parts.fragment = nil
            if let cut = parts.string { out.replaceSubrange(range, with: cut) }
        }
        return out
    }

    /// For a check: `<n>:<tag>` (the debug builds' `WHEEL_LOG_MARKS`) writes n numbered lines
    /// carrying the tag, so the check can find each one in Loki.
    public func writeMarks(_ spec: String) {
        guard let colon = spec.firstIndex(of: ":"), let n = Int(spec[..<colon]), n > 0 else { return }
        for i in 1...n { write("log mark \(i) of \(n) \(spec[spec.index(after: colon)...])") }
    }

    /// The oldest lines wheeld has not accepted, up to about `maxBytes`, handed to `done` on the
    /// log's queue; nil when there are none.
    public func pending(maxBytes: Int = 256 << 10, _ done: @escaping @Sendable (Batch?) -> Void) {
        io.async { [self] in done(read(maxBytes: maxBytes)) }
    }

    /// `pending`, waiting for the answer.
    public func pending(maxBytes: Int = 256 << 10) -> Batch? {
        io.sync { read(maxBytes: maxBytes) }
    }

    private func read(maxBytes: Int) -> Batch? {
        let current = inode(url)
        let rolled = url.appendingPathExtension("1")
        var from = read(positionURL) ?? Position(inode: current ?? 0, offset: 0)
        var file = url
        var gap: String?
        if from.inode != current {
            // The saved file rolled: finish it at `.1`, unless it is done or rolled away too.
            if inode(rolled) == from.inode {
                if from.offset < size(rolled) {
                    file = rolled
                } else {
                    from = Position(inode: current ?? 0, offset: 0)
                }
            } else {
                // Two rolls since the last acknowledgement: the saved file is gone.
                if current != nil, read(positionURL) != nil {
                    gap = "app log gap: the file saved at byte \(from.offset) rolled away before wheeld took it; every line after that point and before this one was dropped"
                }
                from = Position(inode: current ?? 0, offset: 0)
            }
        }
        if from.offset > size(file) { from.offset = 0 }
        guard let handle = try? FileHandle(forReadingFrom: file) else { return nil }
        defer { try? handle.close() }
        try? handle.seek(toOffset: from.offset)
        guard let data = try? handle.read(upToCount: maxBytes), !data.isEmpty else { return nil }
        var (lines, used) = Self.parse(data, whole: data.count < maxBytes)
        guard used > 0 else { return nil }
        for i in lines.indices { lines[i].end = (lines[i].end ?? 0) + from.offset }
        if let gap, let first = lines.first { lines.insert(AppLogLine(tsMs: first.tsMs, msg: gap), at: 0) }
        return Batch(lines: lines, end: Position(inode: from.inode, offset: from.offset + UInt64(used)))
    }

    /// wheeld accepted everything before `end`.
    public func acknowledge(_ end: Position) {
        io.async { [self] in save(end) }
    }

    /// The records in `data` and the bytes they span. A record is a stamped line and any unstamped
    /// lines after it (a message with a newline in it). Unless `whole`, the last record may be cut
    /// short, so it waits for the next batch.
    static func parse(_ data: Data, whole: Bool) -> ([AppLogLine], Int) {
        var lines: [AppLogLine] = []
        var used = 0
        var recordStart = 0
        var start = data.startIndex
        while let nl = data[start...].firstIndex(of: UInt8(ascii: "\n")) {
            let text = String(decoding: data[start..<nl], as: UTF8.self)
            let next = nl + 1
            if let space = text.firstIndex(of: " "), let date = stamp.date(from: String(text[..<space])) {
                recordStart = start - data.startIndex
                lines.append(AppLogLine(tsMs: UInt64(date.timeIntervalSince1970 * 1000), msg: String(text[text.index(after: space)...])))
            } else if let last = lines.indices.last {
                lines[last].msg += "\n" + text
            } else {
                lines.append(AppLogLine(tsMs: UInt64(Date().timeIntervalSince1970 * 1000), msg: text))
            }
            used = next - data.startIndex
            lines[lines.count - 1].end = UInt64(used)
            start = next
        }
        if !whole, lines.count > 1 {
            lines.removeLast()
            used = recordStart
        }
        return (lines, used)
    }

    private func read(_ file: URL) -> Position? {
        guard let data = try? Data(contentsOf: file) else { return nil }
        return try? JSONDecoder().decode(Position.self, from: data)
    }

    /// Temp file, sync, rename: a crash leaves the old position or the new one, never half of one.
    private func save(_ at: Position) {
        guard let data = try? JSONEncoder().encode(at) else { return }
        let temp = positionURL.appendingPathExtension("tmp")
        guard FileManager.default.createFile(atPath: temp.path, contents: data),
              let handle = try? FileHandle(forWritingTo: temp) else { return }
        try? handle.synchronize()
        try? handle.close()
        _ = rename(temp.path, positionURL.path)
    }

    private func inode(_ file: URL) -> UInt64? {
        (try? FileManager.default.attributesOfItem(atPath: file.path)[.systemFileNumber] as? NSNumber)?.uint64Value
    }

    private func size(_ file: URL) -> UInt64 {
        (try? FileManager.default.attributesOfItem(atPath: file.path)[.size] as? NSNumber)?.uint64Value ?? 0
    }
}
