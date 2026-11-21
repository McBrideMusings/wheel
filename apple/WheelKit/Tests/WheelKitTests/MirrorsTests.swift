import Foundation
import Testing
@testable import WheelKit

struct MirrorsTests {
    /// Two spaces as wheeld sends them: `work` holds a named terminal, an agent pane and a web pane;
    /// `orch` mirrors each, one mirror of a mirror and one of a pane that is gone.
    private func spaces() throws -> [SpaceInfo] {
        let json = """
        [{"id":"s1","name":"work","cwd":"/w","root":{"node":"split","id":"x1","axis":"horizontal","ratio":0.5,
            "a":{"node":"pane","id":"p1","kind":"terminal","term":"t1","pane_name":"build"},
            "b":{"node":"split","id":"x2","axis":"vertical","ratio":0.5,
                "a":{"node":"pane","id":"p2","kind":"agent","agent":"w1","harness":"claude","pane_title":"Fix the tests"},
                "b":{"node":"pane","id":"p3","kind":"web","scope":"project","name":"notes"}}}},
         {"id":"s2","name":"orch","cwd":"/o","root":{"node":"split","id":"x3","axis":"horizontal","ratio":0.5,
            "a":{"node":"pane","id":"m1","kind":"mirror","source":"p1"},
            "b":{"node":"split","id":"x4","axis":"vertical","ratio":0.5,
                "a":{"node":"pane","id":"m2","kind":"mirror","source":"p2"},
                "b":{"node":"pane","id":"m4","kind":"mirror","source":"m1"}}},
            "backgrounded":[{"node":"pane","id":"m3","kind":"mirror","source":"p3"},{"node":"pane","id":"m5","kind":"mirror","source":"p9"}]}]
        """
        return try JSONDecoder().decode([SpaceInfo].self, from: Data(json.utf8))
    }

    @Test func aMirrorShowsItsSourceAndAnyOtherPaneItself() throws {
        let spaces = try spaces()
        #expect(spaces.shownPane("m1")?.id == "p1")
        #expect(spaces.shownPane("p2")?.id == "p2")
        #expect(spaces.shownPane("m4") == nil)
        #expect(spaces.shownPane("m5") == nil)
        #expect(spaces.shownPane("p9") == nil)
    }

    @Test func aResolvedMirrorTakesItsSourcesKindUnderItsOwnId() throws {
        let orch = try spaces().resolvingMirrors()[1]
        let kinds = Dictionary(uniqueKeysWithValues: orch.allPanes.map { ($0.id, $0.kind) })
        #expect(kinds["m1"] == .terminal(term: "t1"))
        #expect(kinds["m2"] == .agent(agent: "w1", harness: "claude"))
        #expect(kinds["m3"] == .web(artifact: ArtifactRef(scope: .project, name: "notes")))
        #expect(orch.mirrors["m1"] == MirrorShown(source: "p1", title: "work · build"))
        #expect(orch.mirrors["m2"]?.title == "work · Fix the tests")
        #expect(orch.mirrors["m3"]?.title == "work · web")
    }

    @Test func aMirrorWithNoSourceToShowStaysUnknown() throws {
        let orch = try spaces().resolvingMirrors()[1]
        let kinds = Dictionary(uniqueKeysWithValues: orch.allPanes.map { ($0.id, $0.kind) })
        #expect(kinds["m4"]?.mirrorSource == "m1")
        #expect(kinds["m5"]?.mirrorSource == "p9")
        #expect(orch.mirrors["m4"] == nil && orch.mirrors["m5"] == nil)
    }
}

extension MirrorsTests {
    @Test func aLookupByAgentOrPageFindsTheSourceNotItsMirror() throws {
        // orch listed first, so a first match over every pane would find the mirror.
        let spaces = Array(try spaces().resolvingMirrors().reversed())
        #expect(ChatLinks.place(ofAgent: "w1", in: spaces)?.pane == "p2")
        #expect(AgentPanes.harness(ofAgent: "w1", in: spaces) == "claude")
        #expect(spaces[0].ownedPanes.map(\.id) == ["m4", "m5"])
    }
}
