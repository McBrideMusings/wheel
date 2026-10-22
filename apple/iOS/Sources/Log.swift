import Foundation

/// Timestamped lines in `Documents/phone.log`, which the simulator's container exposes to a check.
enum Log {
    private static let url = URL.documentsDirectory.appending(path: "phone.log")
    private static let lock = NSLock()

    static func info(_ line: String) {
        let stamp = Date().formatted(.iso8601.year().month().day().time(includingFractionalSeconds: true))
        let data = Data("\(stamp) \(line)\n".utf8)
        lock.lock()
        defer { lock.unlock() }
        if let handle = try? FileHandle(forWritingTo: url) {
            defer { try? handle.close() }
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: data)
        } else {
            try? data.write(to: url)
        }
    }
}
