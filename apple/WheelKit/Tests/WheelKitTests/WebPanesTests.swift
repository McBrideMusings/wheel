import Foundation
import Testing
@testable import WheelKit

struct WebPanesTests {
    @Test func decodesAWebPaneAndListsItsArtifact() throws {
        let text = #"{"ev":"spaces","active":"s1","spaces":[{"id":"s1","name":"tmp","cwd":"/tmp","focus":"p2","root":{"node":"split","id":"x3","axis":"horizontal","ratio":0.5,"a":{"node":"pane","id":"p2","kind":"web","scope":"project","name":"site"},"b":{"node":"pane","id":"p4","kind":"terminal","term":"t1"}}}]}"#
        let site = ArtifactRef(scope: .project, name: "site")
        guard case let .spaces(spaces, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(spaces.first?.root.panes.first?.kind == .web(artifact: site))
        #expect(WebPanes.artifacts(spaces) == ["p2": site])
        // A web pane missing its name, carrying a scope this build does not know, or naming an
        // artifact the daemon would refuse, is unsupported, not a reason to drop the frame.
        for raw in [#"{"node":"pane","id":"p5","kind":"web"}"#,
                    #"{"node":"pane","id":"p5","kind":"web","name":"site"}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"hologram","name":"site"}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"repo","name":"site"}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"project","name":"/etc"}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"project","name":".."}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"project","name":"-rf"}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"project","name":""}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"project","name":"a/b"}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"project","name":".hidden"}"#,
                    #"{"node":"pane","id":"p5","kind":"web","scope":"project","name":"-́x"}"#] {
            let broken = try JSONDecoder().decode(Node.self, from: Data(raw.utf8))
            guard case let .pane(id, .unknown(kind, _)) = broken else { Issue.record("\(raw) decoded"); return }
            #expect((id, kind) == ("p5", "web"))
        }
    }

    @Test func keepsOneStoragePerArtifact() {
        let site = ArtifactRef(scope: .project, name: "site")
        let shared = ArtifactRef(scope: .machine, name: "site")
        let here = WebPanes.store(cwd: "/work/a", artifact: site)
        // Pinned: a change to how the id is made would open every artifact's storage empty after
        // the update. (SHA-256 of "project\n/work/a\nsite", computed outside Swift.)
        #expect(here.uuidString == "40CDFA28-991D-8A34-BF20-267AE15CA56D")
        #expect(here != WebPanes.store(cwd: "/work/b", artifact: site), "another project's site")
        #expect(here != WebPanes.store(cwd: "/work/a", artifact: ArtifactRef(scope: .project, name: "docs")))
        #expect(here != WebPanes.store(cwd: "/work/a", artifact: shared), "a machine artifact of the same name")
        #expect(WebPanes.store(cwd: "/work/a", artifact: shared) == WebPanes.store(cwd: "/work/b", artifact: shared))

        let text = #"{"ev":"spaces","active":"s1","spaces":[{"id":"s1","name":"a","cwd":"/work/a","focus":"p2","root":{"node":"split","id":"x3","axis":"horizontal","ratio":0.5,"a":{"node":"pane","id":"p2","kind":"web","scope":"project","name":"site"},"b":{"node":"pane","id":"p4","kind":"web","scope":"project","name":"site"}}},{"id":"s5","name":"b","cwd":"/work/b","focus":"p6","root":{"node":"pane","id":"p6","kind":"web","scope":"project","name":"site"}}]}"#
        guard case let .spaces(spaces, _) = try? ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        let stores = WebPanes.stores(spaces)
        #expect(stores["p2"] == here && stores["p4"] == here, "two panes on one artifact share it")
        #expect(stores["p6"] == WebPanes.store(cwd: "/work/b", artifact: site))
    }

    @Test func opensThePageUnderOneOriginWhateverTheDaemonsPort() {
        let page = WebPanes.pageURL(pane: "p2")
        #expect(page?.absoluteString == "wheel-page://pages/artifacts/p2/")
        let sub = URL(string: "wheel-page://pages/artifacts/p2/data/rows.json?a=b%2Bc")!
        #expect(WebPanes.daemonURL(for: page!, daemon: URL(string: "ws://127.0.0.1:7499/ws")!, pane: "p2")?.absoluteString
            == "http://127.0.0.1:7499/artifacts/p2/")
        #expect(WebPanes.daemonURL(for: sub, daemon: URL(string: "ws://127.0.0.1:7500/ws")!, pane: "p2")?.absoluteString
            == "http://127.0.0.1:7500/artifacts/p2/data/rows.json?a=b%2Bc")
        #expect(WebPanes.daemonURL(for: sub, daemon: URL(string: "wss://mac.local:7420/ws")!, pane: "p2")?.absoluteString
            == "https://mac.local:7420/artifacts/p2/data/rows.json?a=b%2Bc")
        #expect(WebPanes.daemonURL(for: URL(string: "http://evil.example/")!, daemon: URL(string: "ws://127.0.0.1:7499/ws")!, pane: "p2") == nil)
        // Another pane's folder and the daemon's own routes are not the page's to read.
        for path in ["wheel-page://pages/artifacts/p3/", "wheel-page://pages/artifacts/p2", "wheel-page://pages/ws", "wheel-page://pages/artifacts/p22/"] {
            #expect(WebPanes.daemonURL(for: URL(string: path)!, daemon: URL(string: "ws://127.0.0.1:7499/ws")!, pane: "p2") == nil, "\(path)")
        }
    }

    @Test func encodesAWebKindWithItsArtifactAndDecodesAReload() throws {
        #expect(try ClientMsg.paneSetKind(req: 4, pane: "p2", kind: .web, artifact: "site", scope: .machine).json()
            == #"{"artifact":"site","kind":"web","op":"pane_set_kind","pane":"p2","req":4,"scope":"machine"}"#)
        // No scope: the daemon resolves project, then machine.
        #expect(try ClientMsg.paneSetKind(req: 5, pane: "p2", kind: .web, artifact: "site").json()
            == #"{"artifact":"site","kind":"web","op":"pane_set_kind","pane":"p2","req":5}"#)
        guard case let .paneReload(pane) = try ServerMsg.decode(#"{"ev":"pane_reload","pane":"p2"}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(pane == "p2")
    }

    @Test func aGoneArtifactsStoreWaitsUntilNoPaneShowsItAndRetriesAFailure() throws {
        guard case let .artifactGone(scope, name, cwd) = try ServerMsg.decode(#"{"ev":"artifact_gone","scope":"project","name":"site","cwd":"/work/a"}"#),
              case let .artifactGone(machineScope, machineName, machineCwd) = try ServerMsg.decode(#"{"ev":"artifact_gone","scope":"machine","name":"viewer"}"#) else {
            Issue.record("wrong case"); return
        }
        #expect((scope, name, cwd) == (.project, "site", "/work/a"))
        #expect((machineScope, machineName, machineCwd) == (.machine, "viewer", nil))

        var reaper = WebStoreReaper()
        let store = reaper.gone(scope: scope, name: name, cwd: cwd)
        #expect(store == WebPanes.store(cwd: "/work/a", artifact: ArtifactRef(scope: .project, name: "site")))
        #expect(reaper.gone(scope: .machine, name: "viewer", cwd: nil) == WebPanes.store(cwd: "/anywhere", artifact: ArtifactRef(scope: .machine, name: "viewer")))
        #expect(reaper.start(shown: [store]).contains(store) == false, "a pane still shows it")
        #expect(reaper.start(shown: []).contains(store))
        #expect(reaper.start(shown: []).isEmpty, "already being deleted")
        reaper.finished(store, deleted: false)
        #expect(reaper.start(shown: []) == [store], "a failure is tried again")
        reaper.finished(store, deleted: true)
        #expect(reaper.start(shown: []).isEmpty)
        // An artifact made again under the same name opens a pane on the same store: keep it.
        let again = reaper.gone(scope: .project, name: "site", cwd: "/work/a")
        reaper.wanted(again)
        #expect(reaper.start(shown: []).contains(again) == false)
    }
}
