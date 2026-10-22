import Foundation
import os
import WheelKit

/// Timestamped app log, also mirrored to the unified log. The daemon client sends its lines to
/// wheeld, which ships them with its own. Each app process writes the log of the daemon it uses,
/// so no two ship one file: `~/Library/Logs/<instance>/WheelMac.log` beside the bundled server's
/// `wheeld.log`, or `WheelMac.log` in `WHEEL_TOKEN_FILE`'s folder for an app pointed at another
/// daemon with `WHEEL_URL`.
enum Log {
    static let file = AppLog(url: location(ProcessInfo.processInfo.environment), client: "mac")
    static var path: URL { file.url }

    private static let logger = Logger(subsystem: "com.piercemakes.wheel", category: "app")

    static func info(_ message: String) {
        logger.info("\(message, privacy: .public)")
        file.write(message)
    }

    private static func location(_ env: [String: String]) -> URL {
        if env["WHEEL_URL"] != nil, let token = env["WHEEL_TOKEN_FILE"] {
            return URL(filePath: token).deletingLastPathComponent().appending(path: "WheelMac.log")
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appending(path: "Library/Logs/\(DaemonService.bundledInstance ?? "wheel")/WheelMac.log")
    }
}
