/// The protocol version a server reports in `status`, and what a client needs it to be for each
/// feature. A client connected to an older server hides what that server lacks and says a restart
/// would bring it (ADR-0020). `PROTOCOL_VERSION` in `proto/src/lib.rs` is the server's side.
public enum ServerProtocol {
    /// The version this client was built against.
    public static let current: UInt32 = 2

    public enum Feature: String, CaseIterable, Sendable {
        /// `session_unsend` and `session_clear_queue`: Edit, Remove and Clear queue on a sent message.
        case takeBack = "take_back"
        /// The permission request's why fields: the reason, decision reason and path on the card.
        case permissionWhy = "permission_why"

        /// The first protocol version that has it.
        public var since: UInt32 {
            switch self {
            case .takeBack, .permissionWhy: 2
            }
        }

        public var title: String {
            switch self {
            case .takeBack: "Edit and Remove on sent messages"
            case .permissionWhy: "the reason on permission requests"
            }
        }
    }

    /// What a server speaking `version` lacks, in declaration order.
    public static func lacking(at version: UInt32) -> [Feature] {
        Feature.allCases.filter { $0.since > version }
    }

    /// The line a client shows in place of `feature` when a server speaking `version` lacks it;
    /// nil when it has it.
    public static func unavailable(_ feature: Feature, at version: UInt32) -> String? {
        guard feature.since > version else { return nil }
        return "\(feature.title.prefix(1).uppercased() + feature.title.dropFirst()) need a newer server; restart the server to get them."
    }
}
