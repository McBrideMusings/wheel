import Foundation
import Testing
import WheelKit
@testable import WheelUI

@MainActor @Suite struct EarlierConversationsTests {
    @Test func eachConversationIsReadOnceAndTitledByItsFirstPrompt() {
        let store = EarlierConversations()
        var asked: [String] = []
        store.load = { asked.append($0) }
        store.show(["a", "b"])
        store.request("a")
        store.request("a")
        #expect(asked == ["a"])
        #expect(store.list[0].title == "Earlier conversation")
        store.loaded("a", agent: "claude", events: [StampedEvent(.user(turn: 1, text: "fix  the\ntest")), StampedEvent(.text("done"))])
        #expect(store.list[0].title == "fix the test")
        store.request("a")
        #expect(asked == ["a"])
        store.gone("b")
        store.request("b")
        #expect(asked == ["a"])
        #expect(store.list[1].missing)
    }

    @Test func showingANewListKeepsWhatWasRead() {
        let store = EarlierConversations()
        store.show(["a", "b"])
        store.loaded("a", agent: "claude", events: [StampedEvent(.user(turn: 1, text: "first"))])
        store.show(["b", "c", "a"])
        #expect(store.list.map(\.id) == ["b", "c", "a"])
        #expect(store.list[2].title == "first")
        #expect(store.list[1].chat == nil)
    }
}
