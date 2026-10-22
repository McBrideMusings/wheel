import Foundation
import os
import WheelKit

/// Timestamped app log, also mirrored to the unified log. The daemon client sends its lines to
/// wheeld, which ships them with its own. Each app process writes the log of the daemon it uses,
/// so no two ship one file: `~/Library/Logs/<instance>/WheelMac.log` beside the bundled server's
/// `wheeld.log`, or `WheelMac.log` in `WHEEL_TOKEN_FILE`'s folder for an app pointed at another
/// daemon with `WHEEL_URL`. An app with neither a bundled server nor a token folder names the
/// folder after the daemon's host and port, so it never shares the installed app's file.
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
        let logs = FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Logs")
        if env["WHEEL_URL"] == nil, let instance = DaemonService.bundledInstance {
            return logs.appending(path: "\(instance)/WheelMac.log")
        }
        let url = env["WHEEL_URL"].flatMap(URL.init(string:)) ?? URL(string: "ws://\(wheelDefaultListen)/ws")!
        let daemon = [url.host, url.port.map(String.init)].compactMap { $0 }.joined(separator: "-")
        return logs.appending(path: "wheel-\(daemon)/WheelMac.log")
    }
}
