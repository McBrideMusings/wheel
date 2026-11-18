/// The protocol version this client was built against. A server reporting a lower one in `status`
/// is older than this app: the app hides what needs the newer server and asks for a restart
/// (ADR-0020). `PROTOCOL_VERSION` in `proto/src/lib.rs` is the server's side.
public enum ServerProtocol {
    public static let current: UInt32 = 52  // 52: pane_reload set and unset; 51: pane_reload reloads any pane; 50: Codex reviewer persistence; 49: where, a browser answers its page on demand; 48: a browser pane's home
}
