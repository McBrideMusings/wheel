/// What the connected wheeld offers, from its `status` (ADR-0050): its compatibility level and
/// the op of every client message it handles. A client connects only at its own level and sends
/// only ops the server listed; a feature that needs an op asks `has` and hides or disables itself
/// when the server lacks it. `proto/src/ops.rs` is the server's side.
public struct ServerOffer: Equatable, Sendable {
    /// This app's compatibility level: `COMPAT` in `proto/src/ops.rs`, which a proto test holds
    /// this line to.
    public static let clientCompat: UInt32 = 4

    /// 0 from a server that predates levels.
    public var compat: UInt32
    /// The server's `PROTOCOL_VERSION`, for logs; nothing is decided from it.
    public var protocolVersion: UInt32
    public var ops: Set<String>

    public init(compat: UInt32, protocolVersion: UInt32, ops: Set<String>) {
        self.compat = compat
        self.protocolVersion = protocolVersion
        self.ops = ops
    }

    /// Nothing offered: before a server answered.
    public static let none = ServerOffer(compat: 0, protocolVersion: 0, ops: [])

    public func has(_ op: String) -> Bool { ops.contains(op) }

    /// Why this app cannot talk to the server at all, naming which side is behind; nil when the
    /// levels match.
    public var mismatch: String? {
        if compat == Self.clientCompat { return nil }
        if compat < Self.clientCompat {
            return "This Wheel needs a newer wheeld: wheeld is at compatibility level \(compat), this Wheel needs \(Self.clientCompat). Restart wheeld."
        }
        return "This wheeld is newer than this Wheel: wheeld is at compatibility level \(compat), this Wheel at \(Self.clientCompat). Install the Wheel that matches it."
    }
}
