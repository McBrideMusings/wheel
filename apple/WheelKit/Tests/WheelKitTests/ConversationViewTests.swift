import Testing
@testable import WheelKit

@Suite struct ConversationViewTests {
    private func terminal(running name: String?) -> PaneKind {
        .terminal(term: "t1", agent: name.map { TermAgent(name: $0, conversation: "c1") })
    }

    @Test func claudeAndCodexTerminalsOfferTheChat() {
        #expect(terminal(running: "claude").otherView { _ in true } == .agent)
        #expect(terminal(running: "codex").otherView { _ in true } == .agent)
    }

    @Test func claudeAndCodexAgentPanesOfferTheTerminal() {
        #expect(PaneKind.agent(agent: "w1", harness: "claude").otherView { _ in true } == .terminal)
        #expect(PaneKind.agent(agent: "w1", harness: "codex").otherView { _ in true } == .terminal)
    }

    @Test func aTerminalWhoseProgramIsNotInstalledOffersNothing() {
        #expect(terminal(running: "codex").otherView { $0 == "claude" } == nil)
    }

    @Test func otherPanesOfferNothing() {
        #expect(terminal(running: nil).otherView { _ in true } == nil)
        #expect(terminal(running: "pi").otherView { _ in true } == nil)
        #expect(PaneKind.agent(agent: "w1", harness: "pi").otherView { _ in true } == nil)
        #expect(PaneKind.empty.otherView { _ in true } == nil)
    }
}
