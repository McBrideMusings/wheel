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
            guard case let .pane(id, .unknown(kind, _), _) = broken else { Issue.record("\(raw) decoded"); return }
            #expect((id, kind) == ("p5", "web"))
        }
    }

    @Test func opensThePageOnTheDaemonsHTTPSide() {
        let url = WebPanes.url(daemon: URL(string: "ws://127.0.0.1:7499/ws")!, pane: "p2", token: "a+b")
        #expect(url?.absoluteString == "http://127.0.0.1:7499/artifacts/p2/?token=a%2Bb")
        let secure = WebPanes.url(daemon: URL(string: "wss://mac.local:7420/ws")!, pane: "p2", token: "t")
        #expect(secure?.absoluteString == "https://mac.local:7420/artifacts/p2/?token=t")
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
}
