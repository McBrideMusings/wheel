import Foundation
import Testing
@testable import WheelKit

struct ArtifactPaletteTests {
    @Test func listsProjectThenMachineAndMatchesSubstringsBeforeScatteredLetters() {
        var p = ArtifactPalette()
        p.listed([
            ArtifactInfo(scope: .machine, name: "disk", title: "Disk and memory"),
            ArtifactInfo(scope: .project, name: "status", title: "Swarm status"),
            ArtifactInfo(scope: .machine, name: "print-queue", title: "Print queue"),
            ArtifactInfo(scope: .project, name: "merge-queue", title: "Merge queue depth"),
        ])
        #expect(p.rows.map(\.id) == ["project/merge-queue", "project/status", "machine/disk", "machine/print-queue"])
        p.type("QUEUE")
        #expect(p.rows.map(\.id) == ["project/merge-queue", "machine/print-queue"])
        #expect(p.rows[0].titleMatch == [6, 7, 8, 9, 10])
        #expect(p.footer == "2 of 4")
        // No row holds "mrgq" as a substring, so scattered letters match.
        p.type("mrgq")
        #expect(p.rows.map(\.id) == ["project/merge-queue"])
        // "disk" has a substring hit, so the scattered "dk" of other rows stays out.
        p.type("disk")
        #expect(p.rows.map(\.id) == ["machine/disk"])
        p.type("kanban")
        #expect(p.rows.isEmpty)
    }

    @Test func encodesTheListingRequestAndDecodesItsReply() throws {
        #expect(try ClientMsg.artifactLs(req: 3, space: "s1", paneOnly: true).json()
            == #"{"op":"artifact_ls","pane":true,"req":3,"space":"s1"}"#)
        let text = #"{"ev":"artifacts","req":3,"resolves":{"a":"project"},"artifacts":[{"scope":"project","name":"a","entry":"/p/wheel.toml","dir":"/p/a","page":"index.html","title":"A page","pane":true,"problems":[]},{"scope":"machine","name":"b","entry":"/m/wheel.toml","dir":"/m/b","title":"b","pane":true,"problems":["no index.html in /m/b"]}]}"#
        guard case let .artifacts(req, artifacts) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == 3)
        #expect(artifacts.map(\.id) == ["project/a", "machine/b"])
        #expect(artifacts[1].problems == ["no index.html in /m/b"])
    }
}
