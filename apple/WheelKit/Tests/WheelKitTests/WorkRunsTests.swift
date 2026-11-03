import Testing
@testable import WheelKit

struct WorkRunsTests {
    private func tool(_ id: Int, _ kind: String) -> TranscriptRow {
        TranscriptRow(id: id, kind: .tool(title: "t", kind: kind, status: "completed", content: [], detail: nil))
    }

    private func thought(_ id: Int) -> TranscriptRow { TranscriptRow(id: id, kind: .thought("x")) }

    @Test func consecutiveCallsOfOneKindFold() {
        let rows = [thought(0), tool(1, "read"), tool(2, "read"), tool(3, "read"), tool(4, "edit"), tool(5, "search"), tool(6, "search")]
        #expect(WorkRuns.entries(rows) == [.row(0), .run(kind: "read", [1, 2, 3]), .row(4), .run(kind: "search", [5, 6])])
    }

    @Test func aThoughtEndsTheStretch() {
        let rows = [tool(0, "read"), thought(1), tool(2, "read")]
        #expect(WorkRuns.entries(rows) == [.row(0), .row(1), .row(2)])
    }

    @Test func labelsReadAsWords() {
        #expect(WorkRuns.label(kind: "read") == "Read files")
        #expect(WorkRuns.label(kind: "edit") == "Edited files")
        #expect(WorkRuns.label(kind: "execute") == "Ran commands")
    }

    @Test func summaryFollowsTheSetting() {
        let rows = [tool(0, "execute"), tool(1, "execute"), tool(2, "read")]
        #expect(WorkRuns.summary(.time, time: "Worked for 5s", rows: rows) == "Worked for 5s")
        #expect(WorkRuns.summary(.counts, time: "Worked for 5s", rows: rows) == "Ran 2 commands, read 1 file")
        #expect(WorkRuns.summary(.both, time: "Worked for 5s", rows: rows) == "Worked for 5s · Ran 2 commands, read 1 file")
        #expect(WorkRuns.summary(.counts, time: "Worked for 5s", rows: [thought(0)]) == "Worked for 5s")
    }

    @Test func settingReachesTheLayout() {
        let set = SettingInfo(key: "chat.work.summary", value: "both", origin: .machine, default: "time", help: "")
        #expect(ChatAppearance(settings: [set]).layout.workSummary == .both)
        #expect(ChatAppearance.standard.layout.workSummary == .time)
    }
}
