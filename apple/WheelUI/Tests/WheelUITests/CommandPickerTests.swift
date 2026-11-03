import Foundation
import Testing
import WheelKit
@testable import WheelUI

@MainActor @Suite struct CommandPickerTests {
    private let scanned = [
        SkillInfo(name: "wheel-artifact", description: "Make an artifact"),
        SkillInfo(name: "other-projects-skill", description: "Not this project's"),
    ]

    @Test func claudeOffersExactlyWhatItReportedWithDescriptionsJoined() {
        let chat = AgentChat(name: "claude")
        chat.commands = ["loop", "wheel-artifact"]
        let offered = chat.accepted(scanned, agent: "claude")
        #expect(offered.map(\.name) == ["loop", "wheel-artifact"])
        #expect(offered.map(\.description) == ["", "Make an artifact"])
    }

    @Test func claudeOffersNothingUntilItHasReported() {
        let chat = AgentChat(name: "claude")
        #expect(chat.accepted(scanned, agent: "claude").isEmpty)
    }

    @Test func anAgentThatReportsNoCommandsKeepsTheScan() {
        let chat = AgentChat(name: "codex")
        #expect(chat.accepted(scanned, agent: "codex") == scanned)
    }
}
