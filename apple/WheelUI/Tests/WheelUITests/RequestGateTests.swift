import Testing
@testable import WheelUI

@MainActor
private final class Tally {
    var running = 0
    var peak = 0
    var finished = 0
}

@MainActor @Suite struct RequestGateTests {
    @Test func atMostTheLimitRunsAndEveryRequestFinishes() async {
        let gate = RequestGate(limit: 2)
        let tally = Tally()
        let tasks = (0 ..< 6).map { _ in
            Task { @MainActor in
                await gate.enter()
                tally.running += 1
                tally.peak = max(tally.peak, tally.running)
                await Task.yield()
                await Task.yield()
                tally.running -= 1
                tally.finished += 1
                gate.leave()
            }
        }
        for task in tasks { await task.value }
        #expect(tally.peak == 2)
        #expect(tally.finished == 6)
    }
}
