/// Where a tapped notification goes among the paired servers: ids are per server, so a push names
/// the wheeld that sent it (`server` in its payload) and only that server opens it.
public enum PushRoute: Equatable, Sendable {
    /// The server at this index in the list sent it.
    case open(Int)
    /// No server has that id, but one has not said its id yet (a pairing from before server ids,
    /// not connected): wait for it.
    case wait
    /// No paired server sent it: it opens nothing.
    case none

    /// The route for a push from `server`, given each paired server's id (nil when not known yet).
    public static func route(server: String, among ids: [String?]) -> PushRoute {
        if let i = ids.firstIndex(of: server) { return .open(i) }
        return ids.contains(nil) ? .wait : .none
    }
}
