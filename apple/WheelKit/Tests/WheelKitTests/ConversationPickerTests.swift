import Foundation
import Testing
@testable import WheelKit

struct ConversationPickerTests {
    private func conversations(_ n: Int) -> [ConversationInfo] {
        (0..<n).map { ConversationInfo(id: "c\($0)", cwd: "/", title: "Topic \($0) Notes", updatedAt: nil, current: false) }
    }

    /// The query keeps the conversations whose title holds it, ignoring case, in listing order;
    /// typing puts the selection back on the first.
    @Test func filtersByTitleAndKeepsListingOrder() {
        var picker = ConversationPicker()
        picker.listed(conversations(10_000))
        picker.move(3)
        picker.type("topic 99")
        #expect(picker.rows.count == 111)
        #expect(picker.all[picker.rows[0]].id == "c99")
        #expect(picker.selected == 0)
        picker.type("no such text")
        #expect(picker.rows.isEmpty)
        #expect(picker.selectedConversation == nil)
        #expect(picker.footer == "0 of 10000")
    }

    /// Arrow keys stop at the ends, and a fresh listing keeps the selected conversation selected.
    @Test func movesWithinTheRowsAndSurvivesARelisting() {
        var picker = ConversationPicker()
        picker.listed(conversations(3))
        picker.move(-1)
        #expect(picker.selected == 0)
        picker.move(5)
        #expect(picker.selectedConversation?.id == "c2")
        picker.listed(conversations(3).reversed())
        #expect(picker.selectedConversation?.id == "c2")
        #expect(picker.selected == 0)
    }

    /// A failed listing says why and keeps the rows it had.
    @Test func aFailedListingKeepsTheRows() {
        var picker = ConversationPicker()
        #expect(picker.isLoading)
        picker.listed(conversations(2))
        picker.relisting()
        picker.failed("Internal error")
        #expect(picker.error == "Internal error")
        #expect(picker.rows.count == 2)
        #expect(!picker.isLoading)
    }

    @Test func aRowTitleIsTheFirstLineCut() {
        let long = String(repeating: "x", count: 200)
        #expect(ConversationInfo(id: "a", cwd: "/", title: "one\ntwo", updatedAt: nil, current: false).rowTitle == "one")
        #expect(ConversationInfo(id: "a", cwd: "/", title: nil, updatedAt: nil, current: false).rowTitle == "Untitled conversation")
        #expect(ConversationInfo(id: "a", cwd: "/", title: long, updatedAt: nil, current: false).rowTitle.count == ConversationInfo.titleLimit)
    }
}
