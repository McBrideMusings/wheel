import Foundation
import os

/// Timestamped app log at ~/Library/Logs/wheel/WheelMac.log, also mirrored to the unified log.
enum Log {
    static let path: URL = {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Logs/wheel")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appending(path: "WheelMac.log")
    }()

    private static let logger = Logger(subsystem: "com.piercemakes.wheel", category: "app")
    private static let queue = DispatchQueue(label: "wheel.log")
    nonisolated(unsafe) private static let formatter: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    static func info(_ message: String) {
        logger.info("\(message, privacy: .public)")
        let line = "\(formatter.string(from: Date())) \(message)\n"
        queue.async {
            if let handle = try? FileHandle(forWritingTo: path) {
                handle.seekToEndOfFile()
                handle.write(Data(line.utf8))
                try? handle.close()
            } else {
                try? Data(line.utf8).write(to: path)
            }
        }
    }
}
