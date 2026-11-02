import Foundation
import Testing
@testable import WheelUI

@MainActor @Suite struct MessageMenuTests {
    @Test func theMenuHasThreeItemsWithTheSeparatorBeforeAttach() {
        #expect(MessageAction.allCases.map(\.title) == ["Copy message", "Copy message as Markdown", "Attach message as context"])
        #expect(MessageAction.allCases.filter(\.separatedAbove) == [.attach])
    }

    @Test func attachingQuotesTheMessageAtTheTopOfTheDraft() {
        let chat = AgentChat(name: "claude")
        chat.attachAsContext("one\n\ntwo")
        #expect(chat.draft == "> one\n>\n> two\n\n")
        chat.draft += "why?"
        chat.attachAsContext("three")
        #expect(chat.draft == "> three\n\n> one\n>\n> two\n\nwhy?")
    }
}
