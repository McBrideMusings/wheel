/// What a client does to its terminal surfaces when a `spaces` broadcast arrives: which to make
/// and attach, which to detach, which draw, and whether the keyboard moves. The client keeps one
/// surface per terminal pane, keyed by term id, and acts on this without deciding anything itself.
public struct SurfacePlan: Equatable, Sendable {
    /// Terms in the layout with no surface yet, sorted; the client makes and attaches these.
    public let added: [String]
    /// Terms with a surface whose pane left the layout, sorted; the client detaches these.
    public let removed: [String]
    /// Every term in the layout: true when its space is on screen. A hidden or background space's
    /// surfaces stay alive so their screens survive; they only stop drawing.
    public let visible: [String: Bool]
    /// The focus key to remember for the next broadcast.
    public let focusKey: String?
    /// The term to give the keyboard: set only when the active space's focus moved to a terminal.
    public let focus: String?

    /// `active` is the space the keyboard is in; `shown` is every space some window shows (the
    /// active one alone when nil). `existing` is the terms that have a surface; `focusKey` is the
    /// key the client last acted on.
    public init(_ spaces: [SpaceInfo], active: String?, shown: Set<String>? = nil, existing: Set<String>, focusKey last: String?) {
        let shown = shown ?? Set(active.map { [$0] } ?? [])
        var visible: [String: Bool] = [:]
        for space in spaces {
            for pane in space.root.panes {
                if case let .terminal(term, _) = pane.kind { visible[term] = shown.contains(space.id) }
            }
        }
        self.visible = visible
        added = visible.keys.filter { !existing.contains($0) }.sorted()
        removed = existing.filter { visible[$0] == nil }.sorted()
        guard let space = spaces.first(where: { $0.id == active }) else {
            focusKey = last
            focus = nil
            return
        }
        let key = Self.key(pane: space.focus, in: spaces)
        focusKey = key
        focus = key == last ? nil : spaces.term(of: space.focus)
    }

    /// The focus key of `pane`, `<pane>/<term>`: a pane that turns into a terminal takes the
    /// keyboard too, so the key holds its term as well as its id.
    public static func key(pane: String, in spaces: [SpaceInfo]) -> String {
        "\(pane)/\(spaces.term(of: pane) ?? "")"
    }
}

extension [SpaceInfo] {
    /// The term id a pane shows, if it is a terminal.
    func term(of pane: String) -> String? {
        for space in self {
            for p in space.root.panes where p.id == pane {
                if case let .terminal(term, _) = p.kind { return term }
            }
        }
        return nil
    }
}
