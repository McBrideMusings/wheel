import Foundation
import Testing
@testable import WheelKit

@Suite struct UsageTests {
    static let board = #"""
    {"ev":"usage_changed","board":{"pins":["claude:session","claude:opus","codex:session"],"agents":[
      {"id":"claude","name":"Claude","shown":true,"readable":true,"usage":{"kind":"stale","age_secs":240,"why":"HTTP 429","source":"s",
        "reading":{"session_pct":42,"session_resets_at":"2026-10-17T20:00:00Z","weekly_pct":7.5,"model_limits":[{"id":"opus","label":"Weekly Opus","pct":30}],"plan":"Max 20x","read_at":1}}},
      {"id":"codex","name":"Codex","shown":true,"readable":false,"why":"not detected: no `codex` on wheeld's PATH"}]}}
    """#

    private func decoded() throws -> UsageBoard {
        guard case let .usageChanged(board) = try JSONDecoder().decode(ServerMsg.self, from: Data(Self.board.utf8)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "not usage_changed"))
        }
        return board
    }

    @Test func theStripShowsOnlyPinnedRowsTheAgentHasAReadingFor() throws {
        let board = try decoded()
        let claude = board.agents[0]
        #expect(board.stripRows(for: claude).map(\.id) == ["claude:session", "claude:opus"])
        #expect(board.stripRows(for: claude).map(\.pct) == [42, 30])
        #expect(claude.isStale)
        #expect(claude.rows.map(\.id) == ["claude:session", "claude:weekly", "claude:opus"])
    }

    @Test func anAgentWithoutAReadingShowsWhy() throws {
        let codex = try decoded().agents[1]
        #expect(codex.missingNote == "not detected: no `codex` on wheeld's PATH")
        #expect(codex.rows.isEmpty)
    }

    @Test func pinsAndAgentsTurnIntoTheSettingValueTheDaemonStores() throws {
        let board = try decoded()
        #expect(board.pins(toggling: "claude:weekly") == "claude:session,claude:opus,codex:session,claude:weekly")
        #expect(board.pins(toggling: "claude:opus") == "claude:session,codex:session")
        #expect(board.agentsSetting(toggling: "codex") == "claude")
    }

    @Test func resetsReadAsTheShortestWholeUnit() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let at = { (secs: Double) in ISO8601DateFormatter().string(from: now.addingTimeInterval(secs)) }
        #expect(UsageText.resets(at(4 * 3600 + 120), now: now) == "resets 4h")
        #expect(UsageText.resets(at(3 * 86_400 + 60), now: now) == "resets 3d")
        #expect(UsageText.resets(at(25 * 60), now: now) == "resets 25m")
        #expect(UsageText.resets(nil, now: now) == nil)
    }
}
