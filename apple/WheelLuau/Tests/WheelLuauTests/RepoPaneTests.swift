import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// The wheelhouse Repo pane (`plugins/wheelhouse/panes/repo.luau`) against a fixture `repo` feed: each
/// branch's Upstream, Merged, PR and Delete cells in git's and GitHub's words.
@Suite struct RepoPaneTests {
    typealias W = WheelhousePaneTests

    static func branch(_ name: String, upstream: String?, track: String = "", merged: String? = nil,
                       pr: String? = nil, dead: String? = nil, blocked: String? = nil, localOnly: Int = 0) -> String {
        func q(_ s: String?) -> String { s.map { "\"\($0)\"" } ?? "null" }
        return """
        {"branch":"\(name)","ts":100,"subject":"s","author":"a","ahead":1,"behind":0,"upstream":\(q(upstream)),
         "track":"\(track)","upstream_gone":\(track == "gone"),"remote":null,"unpushed":null,"worktree":null,
         "pr":\(pr ?? "null"),"pr_merged":false,"merged":\(q(merged)),"local_only":\(localOnly),
         "dead":\(q(dead)),"blocked":\(q(blocked)),"finished":\(["merged", "squash-merged", "gone"].contains(dead ?? ""))}
        """
    }

    static func feed() throws -> JSONValue {
        let pr = { (n: Int, state: String, draft: Bool) in
            #"{"number":\#(n),"title":"t","state":"\#(state)","draft":\#(draft),"url":"https://x/\#(n)","review":null,"mergeable":"","ci":null}"#
        }
        let rows = [
            branch("open-pr", upstream: "origin/open-pr", pr: pr(19, "OPEN", false), dead: "pushed"),
            branch("draft-pr", upstream: "origin/draft-pr", pr: pr(20, "OPEN", true), dead: "pushed"),
            branch("squashed", upstream: "origin/squashed", merged: "squash-merged", pr: pr(10, "MERGED", false), dead: "squash-merged"),
            branch("closed-pr", upstream: "origin/closed-pr", pr: pr(7, "CLOSED", false), dead: "pushed"),
            branch("deleted", upstream: "origin/deleted", track: "gone", merged: "merged", dead: "gone"),
            branch("dirty", upstream: "origin/main", track: "behind 118", merged: "merged", dead: "merged", blocked: "uncommitted changes"),
            branch("other-upstream", upstream: "origin/base", track: "ahead 5", localOnly: 5),
            branch("never-pushed", upstream: nil, localOnly: 6),
        ]
        return try W.json("""
        {"now":400,"name":"p","top":"/p","default":"origin/main","head":"main","branches":[\(rows.joined(separator: ","))],
         "stashes":[],"commits":[],"fetched":280,"pr_error":null,"error":null,"scripts":[],
         "cleanup":{"delete":["squashed","deleted"],"keep":[{"branch":"dirty","why":"blocked: uncommitted changes"}]}}
        """)
    }

    /// Each row's cells by key.
    static func cells(_ tree: PaneNode) throws -> [String: [String: String]] {
        var out: [String: [String: String]] = [:]
        for r in try W.rows(tree) {
            guard case let .array(cells)? = r.row["cells"] else { continue }
            var m: [String: String] = [:]
            for case let .object(c) in cells { if let k = W.str(c["key"]) { m[k] = W.str(c["text"]) ?? "" } }
            out[r.id] = m
        }
        return out
    }

    @Test func eachBranchIsDescribedInGitsWords() throws {
        let tree = try W.pane("repo", feed: "repo", value: Self.feed()).view(W.data)
        let c = try Self.cells(tree)
        func row(_ b: String) -> [String] { ["upstream", "merged", "pr", "delete"].map { c[b]?[$0] ?? "?" } }
        #expect(row("open-pr") == ["up to date", "", "#19 open", "safe"])
        #expect(row("draft-pr") == ["up to date", "", "#20 draft", "safe"])
        #expect(row("squashed") == ["up to date", "squash-merged", "#10 merged", "safe"])
        #expect(row("closed-pr") == ["up to date", "", "#7 closed", "safe"])
        #expect(row("deleted") == ["gone", "merged", "", "safe"])
        #expect(row("dirty") == ["origin/main: behind 118", "merged", "", "blocked: uncommitted changes"])
        #expect(row("other-upstream") == ["origin/base: ahead 5", "", "", "5 unpushed"])
        #expect(row("never-pushed") == ["no upstream", "", "", "6 unpushed"])
    }

    /// Each row's cell tones by key.
    static func tones(_ tree: PaneNode) throws -> [String: [String: String]] {
        var out: [String: [String: String]] = [:]
        for r in try W.rows(tree) {
            guard case let .array(cells)? = r.row["cells"] else { continue }
            var m: [String: String] = [:]
            for case let .object(c) in cells { if let k = W.str(c["key"]) { m[k] = W.str(c["tone"]) ?? "" } }
            out[r.id] = m
        }
        return out
    }

    @Test func aFinishedBranchIsFaintInEveryCellButDelete() throws {
        let t = try Self.tones(W.pane("repo", feed: "repo", value: Self.feed()).view(W.data))
        for b in ["squashed", "deleted", "dirty"] {
            let row = try #require(t[b])
            #expect(row.filter { $0.key != "delete" }.allSatisfy { $0.value == "faint" }, "\(b): \(row)")
        }
        #expect(t["squashed"]?["delete"] == "ok")
        #expect(t["deleted"]?["delete"] == "ok")
        #expect(t["dirty"]?["delete"] == "warn")
        for b in ["open-pr", "closed-pr", "other-upstream", "never-pushed"] {
            #expect(t[b]?.values.contains("faint") == false, "\(b) is not finished")
        }
    }

    @Test func theActionsButtonIsAtTheRightAndCleanUpComesFirst() throws {
        let runtime = try W.pane("repo", feed: "repo", value: Self.feed())
        let tree = try runtime.view(W.data)
        let texts = W.texts(W.jsonOf(tree))
        #expect(texts.contains("Actions ⌄"))
        #expect(!texts.contains { $0.contains("listed") })
        let buttons = W.buttons(tree)
        let clean = try #require(buttons.firstIndex { $0.text == "Clean up merged  2" })
        let copy = try #require(buttons.firstIndex { $0.text == "Copy IDs" })
        #expect(clean < copy)
        #expect(!buttons[clean].disabled)
        #expect(buttons[clean].help.contains("squashed, deleted"))
        #expect(buttons[clean].help.contains("dirty (blocked: uncommitted changes)"))
        #expect(W.actionType(buttons[clean]) == "cleanup")
        try runtime.send(W.msg("list.select", ["ids": .array([.string("deleted")])]))
        #expect(W.texts(W.jsonOf(try runtime.view(W.data))).contains("1 selected · Actions ⌄"))
        // The bar pins the actions to the far right: its two groups are spread apart.
        let bars = W.all(try runtime.view(W.data), "Row").filter { W.str($0["justify"]) == "spaceBetween" }
        #expect(bars.count == 1)
    }

    @Test func theUpstreamHeaderSaysWhenOriginWasFetched() throws {
        let tree = try W.pane("repo", feed: "repo", value: Self.feed()).view(W.data)
        guard let table = W.all(tree, "Table").first, case let .array(cols)? = table["columns"] else { throw W.Failure("no table") }
        let labels = cols.compactMap { col -> String? in if case let .object(o) = col { return W.str(o["label"]) }; return nil }
        #expect(labels.contains { $0.hasPrefix("Upstream · fetched ") })
        #expect(!labels.contains("State"))
    }
}
