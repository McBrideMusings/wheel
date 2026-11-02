import Testing
@testable import WheelKit

struct ToolRowTests {
    private func info(_ title: String, _ kind: String, _ status: String = "completed", _ content: [ToolContent] = [], detail: String? = nil) -> ToolRowInfo {
        ToolRowInfo(title: title, kind: kind, status: status, content: content, detail: detail)
    }

    @Test func eachKindHasItsIconAndSentence() {
        #expect(info("Read /a/b/model.go", "read").symbol == "doc.text")
        #expect(info("Read /a/b/model.go", "read").label == "Read model.go")
        let search = info("grep addSuggestions in /x/model_test.go", "search")
        #expect(search.symbol == "magnifyingglass")
        #expect(search.label == "Searched for addSuggestions in model_test.go")
        #expect(info("grep foo", "search").label == "Searched for foo")
        #expect(info("Find **/*.go", "search").label == "Searched for files matching **/*.go")
        #expect(info("Edit /a/styles.go", "edit").label == "Edited styles.go")
        #expect(info("Edit /a/styles.go", "edit").symbol == "pencil")
        let create = info("Write /a/new.go", "edit", "completed", [.diff(path: "/a/new.go", oldText: nil, newText: "x\n")])
        #expect(create.label == "Created new.go")
        #expect(create.symbol == "doc.badge.plus")
        #expect(info("Write /a/new.go", "edit", "completed", [.diff(path: "/a/new.go", oldText: "y\n", newText: "x\n")]).label == "Edited new.go")
        #expect(info("rm", "delete").symbol == "trash")
        #expect(info("make build", "execute").label == "Ran make build")
        #expect(info("Run the tests", "execute", detail: "cargo test").label == "Run the tests")
        #expect(info("Skill implement", "skill").label == "Used the implement skill")
        #expect(info("Survey", "subagent").symbol == "person.2")
        #expect(info("Fetch https://x.dev", "fetch").label == "Fetched https://x.dev")
        #expect(info("Search swift actors", "fetch").label == "Searched the web for swift actors")
        #expect(info("mcp__github__list_issues", "mcp").label == "github: list_issues")
        #expect(info("x", "think").symbol == "brain")
    }

    @Test func editsCountTheirLines() {
        let edit = info("Edit /a/s.go", "edit", "completed", [.diff(path: "/a/s.go", oldText: "red\nblue\n", newText: "green\nblue\nplum\n")])
        #expect(edit.added == 2)
        #expect(edit.removed == 1)
        let write = info("Write /a/n.go", "edit", "completed", [.diff(path: "/a/n.go", oldText: nil, newText: "a\nb\n")])
        #expect(write.added == 2)
        #expect(write.removed == 0)
        #expect(info("Read x", "read").added == nil)
        #expect(info("Edit /a/s.go", "edit", "failed", [.diff(path: "/a/s.go", oldText: "a", newText: "b")]).added == nil)
    }

    @Test func aFailedCallKeepsItsIconAndShowsItsFirstLine() {
        let failed = info("make build", "execute", "failed", [.text("\nmake: *** [build] Error 2\nsecond")])
        #expect(failed.symbol == "terminal")
        #expect(failed.reason == "make: *** [build] Error 2")
        #expect(info("make build", "execute").reason == nil)
        #expect(info("x", "execute", "failed", [.text(String(repeating: "a", count: 200))]).reason?.count == 80)
    }

    @Test func aTurnTotalsItsLines() {
        var t = Transcript()
        t.apply(.toolCall(id: "1", title: "Edit a", kind: "edit", status: "completed", content: [.diff(path: "a", oldText: "x\n", newText: "y\nz\n")]))
        t.apply(.toolCall(id: "2", title: "Edit b", kind: "edit", status: "failed", content: [.diff(path: "b", oldText: "x\n", newText: "y\n")]))
        let total = Edits.totals(in: Array(t.rows))
        #expect(total.added == 2)
        #expect(total.removed == 1)
    }
}
