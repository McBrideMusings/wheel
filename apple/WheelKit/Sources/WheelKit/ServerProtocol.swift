/// The protocol version this client was built against. A server reporting a lower one in `status`
/// is older than this app: the app hides what needs the newer server and asks for a restart
/// (ADR-0020). `PROTOCOL_VERSION` in `proto/src/lib.rs` is the server's side.
public enum ServerProtocol {
    public static let current: UInt32 = 44  // 44: a browser pane's session follows it (pane_session_get/put, pane_share_session)
}
