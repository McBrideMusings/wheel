import Foundation
import Testing
@testable import WheelKit

@Suite struct UsageTests {
    static let board = #"""
    {"ev":"usage_changed","board":{"meters":[{"agent":"claude","window":"5h","level":"meter"},{"agent":"claude","window":"weekly-fable","level":"full"},{"agent":"codex","window":"5h","level":"meter"}],"labels":["folder","billing"],"agents":[
      {"id":"claude","name":"Claude","shown":true,"readable":true,"accounts":[
        {"id":"d5901d82","name":".claude","usage":{"kind":"stale","age_secs":240,"why":"HTTP 429","source":"s",
          "reading":{"windows":[{"id":"5h","label":"5-hour","pct":42,"resets_at":"2026-10-17T20:00:00Z","length_secs":18000},{"id":"weekly","label":"Weekly","pct":7.5,"length_secs":604800},{"id":"weekly-fable","label":"Weekly Fable","pct":30,"length_secs":604800}],"billing":"Max 20x","read_at":1}}},
        {"id":"a5045afa","name":".claude-work","usage":{"kind":"missing","why":"signed out: open Claude Code with /h/.claude-work to sign in again","source":"s"}}]},
      {"id":"codex","name":"Codex","shown":true,"readable":true,"accounts":[
        {"id":"c0dex001","name":".codex-work","usage":{"kind":"fresh","age_secs":1,"source":"wham/usage via Codex logins",
          "reading":{"windows":[{"id":"weekly","label":"Weekly","pct":92,"resets_at":"2026-11-13T18:09:15Z","length_secs":604800}],"billing":"Prolite","read_at":1}}}]}]}}
    """#

    private func decoded() throws -> UsageBoard {
        guard case let .usageChanged(board) = try JSONDecoder().decode(ServerMsg.self, from: Data(Self.board.utf8)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "not usage_changed"))
        }
        return board
    }

    @Test func eachAccountShowsOnlyTheMeteredWindowsItHasAndNothingStandsIn() throws {
        let board = try decoded()
        let claude = board.agents[0]
        let (home, work) = (claude.accounts[0], claude.accounts[1])
        #expect(board.stripRows(for: home, of: claude).map(\.id) == ["5h", "weekly-fable"])
        #expect(board.stripRows(for: home, of: claude).map(\.level) == [.meter, .full])
        #expect(board.stripRows(for: home, of: claude).map(\.window.pct) == [42, 30])
        #expect(home.isStale)
        #expect(work.missingNote == "signed out: open Claude Code with /h/.claude-work to sign in again")
        #expect(board.stripRows(for: work, of: claude).isEmpty)
        // A Pro Lite login has no 5-hour window: the codex:5h meter draws nothing for it.
        let codex = board.agents[1]
        #expect(board.stripRows(for: codex.accounts[0], of: codex).isEmpty)
        #expect(codex.accounts[0].usage.reading?.billing == "Prolite")
        #expect(codex.accounts[0].usage.source == "wham/usage via Codex logins")
    }

    @Test func anAgentWithoutAccountsShowsWhy() throws {
        let json = #"{"id":"codex","name":"Codex","shown":true,"readable":false,"why":"not detected: no Codex login on this Mac"}"#
        let codex = try JSONDecoder().decode(UsageAgent.self, from: Data(json.utf8))
        #expect(codex.missingNote == "not detected: no Codex login on this Mac")
        #expect(codex.accounts.isEmpty)
    }

    @Test func metersLabelsAndAgentsTurnIntoTheSettingValuesTheDaemonStores() throws {
        let board = try decoded()
        #expect(board.level(agent: "claude", window: "weekly-fable") == .full)
        #expect(board.level(agent: "claude", window: "weekly") == nil)
        // A changed entry keeps its place; a new one goes last; off removes it.
        #expect(board.metersSetting(agent: "claude", window: "5h", level: .full) == "claude:5h:full,claude:weekly-fable:full,codex:5h:meter")
        #expect(board.metersSetting(agent: "claude", window: "weekly", level: .meter) == "claude:5h:meter,claude:weekly-fable:full,codex:5h:meter,claude:weekly:meter")
        #expect(board.metersSetting(agent: "claude", window: "weekly-fable", level: nil) == "claude:5h:meter,codex:5h:meter")
        #expect(board.shows(.folder) && board.shows(.billing) && !board.shows(.email))
        #expect(board.labelsSetting(toggling: .email) == "folder,billing,email")
        #expect(board.labelsSetting(toggling: .folder) == "billing")
        #expect(board.printsFolder(of: board.agents[0]))
        #expect(!board.printsFolder(of: board.agents[1]))
        #expect(board.agentsSetting(toggling: "codex") == "claude")
    }

    @Test func paceIsTheShareOfTheWindowElapsedByItsReset() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let at = { (secs: Double) in ISO8601DateFormatter().string(from: now.addingTimeInterval(secs)) }
        // 18000 s window, 4500 s left: three quarters elapsed.
        #expect(UsageText.pace(resetsAt: at(4500), lengthSecs: 18000, now: now) == 0.75)
        // Past its reset, or before its start, clamps.
        #expect(UsageText.pace(resetsAt: at(-10), lengthSecs: 18000, now: now) == 1)
        #expect(UsageText.pace(resetsAt: at(20000), lengthSecs: 18000, now: now) == 0)
        #expect(UsageText.pace(resetsAt: nil, lengthSecs: 18000, now: now) == nil)
        #expect(UsageText.pace(resetsAt: at(10), lengthSecs: nil, now: now) == nil)
        let window = UsageWindow(id: "weekly", label: "Weekly", pct: 1, resetsAt: at(302_400), lengthSecs: 604_800)
        #expect(window.pace(now: now) == 0.5)
    }

    @Test func resetsReadAsTheShortestWholeUnitAndAsAClock() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let at = { (secs: Double) in ISO8601DateFormatter().string(from: now.addingTimeInterval(secs)) }
        #expect(UsageText.resets(at(4 * 3600 + 120), now: now) == "resets 4h")
        #expect(UsageText.resets(at(3 * 86_400 + 60), now: now) == "resets 3d")
        #expect(UsageText.resets(at(25 * 60), now: now) == "resets 25m")
        #expect(UsageText.resets(nil, now: now) == nil)
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        utc.locale = Locale(identifier: "en_US")
        // 1_000_000 s after the epoch is 1970-01-12 13:46:40 UTC.
        #expect(UsageText.resetClock(at(3600), now: now, calendar: utc) == "2:46 PM")
        #expect(UsageText.resetClock(at(2 * 86_400), now: now, calendar: utc) == "Wed 1:46 PM")
        #expect(UsageText.resetClock(nil, now: now) == nil)
    }
}
