import Foundation
import WheelKit

/// Timestamped lines in `Documents/phone.log`, which the simulator's container exposes to a check.
/// The daemon client sends them to wheeld, which ships them with its own; `PhoneModel` names the
/// client `ipad` on an iPad.
enum Log {
    static let file = AppLog(
        url: URL.documentsDirectory.appending(path: "phone.log"),
        client: "iphone")

    static func info(_ line: String) {
        file.write(line)
    }
}
