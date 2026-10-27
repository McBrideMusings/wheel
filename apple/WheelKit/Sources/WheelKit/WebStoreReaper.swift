import Foundation

/// The browser storage of artifacts whose entries are gone, waiting to be deleted. A store goes
/// only once no pane shows it: a pane keeps showing a removed artifact until a person changes it,
/// and a store a live page is using cannot be deleted.
public struct WebStoreReaper: Sendable, Equatable {
    private var pending: Set<UUID> = []
    private var running: Set<UUID> = []
    private var failures: [UUID: Int] = [:]

    /// Failed deletions after which a store is left alone.
    public static let maxFailures = 5

    public init() {}

    /// The store of the artifact `ref` in the space at `cwd` is no longer wanted. `cwd` is
    /// ignored for a `machine` artifact, as `WebPanes.store` ignores it.
    @discardableResult
    public mutating func gone(scope: ArtifactScope, name: String, cwd: String?) -> UUID {
        let store = WebPanes.store(cwd: cwd ?? "", artifact: ArtifactRef(scope: scope, name: name))
        pending.insert(store)
        return store
    }

    /// A pane has just opened `store` afresh: the artifact has an entry again, so its storage is
    /// wanted after all.
    public mutating func wanted(_ store: UUID) {
        pending.remove(store)
        failures[store] = nil
    }

    /// The stores to delete now: wanted no more, shown by no pane, not already being deleted and
    /// not given up on. Each is marked as being deleted until `finished` says how it went. A store
    /// a pane shows has its failures forgotten, so it gets a fresh set of tries once that pane is gone.
    public mutating func start(shown: Set<UUID>) -> [UUID] {
        for store in shown { failures[store] = nil }
        let due = pending.subtracting(shown).subtracting(running)
            .filter { failures[$0, default: 0] < Self.maxFailures }
        running.formUnion(due)
        return due.sorted { $0.uuidString < $1.uuidString }
    }

    /// Deleting `store` ended: a deleted store is forgotten, one that failed stays to be tried again
    /// the next time `start` runs. Returns true when this failure is the last try: the store is
    /// then left alone until it is shown by a pane again.
    @discardableResult
    public mutating func finished(_ store: UUID, deleted: Bool) -> Bool {
        running.remove(store)
        if deleted {
            pending.remove(store)
            failures[store] = nil
            return false
        }
        failures[store, default: 0] += 1
        return failures[store] == Self.maxFailures
    }
}
