import Foundation
import Testing
@testable import WheelKit

struct WebPanesTests {
    @Test func decodesAWebPaneAndListsItsFolder() throws {
        let text = #"{"ev":"spaces","active":"s1","spaces":[{"id":"s1","name":"tmp","cwd":"/tmp","focus":"p2","root":{"node":"split","id":"x3","axis":"horizontal","ratio":0.5,"a":{"node":"pane","id":"p2","kind":"web","dir":"/tmp/site"},"b":{"node":"pane","id":"p4","kind":"terminal","term":"t1"}}}]}"#
        guard case let .spaces(spaces, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(spaces.first?.root.panes.first?.kind == .web(dir: "/tmp/site"))
        #expect(WebPanes.folders(spaces) == ["p2": "/tmp/site"])
        // A web pane missing its folder is unsupported, not a reason to drop the frame.
        let broken = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p5","kind":"web"}"#.utf8))
        #expect(broken == .pane(id: "p5", kind: .unknown(kind: "web", raw: ["kind": .string("web")])))
    }

    @Test func opensThePageOnTheDaemonsHTTPSide() {
        let url = WebPanes.url(daemon: URL(string: "ws://127.0.0.1:7499/ws")!, pane: "p2", token: "a+b")
        #expect(url?.absoluteString == "http://127.0.0.1:7499/artifacts/p2/index.html?token=a%2Bb")
        let secure = WebPanes.url(daemon: URL(string: "wss://mac.local:7420/ws")!, pane: "p2", token: "t")
        #expect(secure?.absoluteString == "https://mac.local:7420/artifacts/p2/index.html?token=t")
    }

    @Test func encodesAWebKindWithItsFolderAndDecodesAReload() throws {
        #expect(try ClientMsg.paneSetKind(req: 4, pane: "p2", kind: .web, dir: "/tmp/site").json()
            == #"{"dir":"\/tmp\/site","kind":"web","op":"pane_set_kind","pane":"p2","req":4}"#)
        guard case let .paneReload(pane) = try ServerMsg.decode(#"{"ev":"pane_reload","pane":"p2"}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(pane == "p2")
    }
}
