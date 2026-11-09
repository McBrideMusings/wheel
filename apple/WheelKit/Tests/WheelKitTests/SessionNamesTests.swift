import Testing
@testable import WheelKit

@Suite struct SessionNamesTests {
    @Test func aNameIsAnAdjectiveAndANoun() {
        let parts = SessionNames.next(avoiding: []).split(separator: "-").map(String.init)
        #expect(parts.count == 2)
        #expect(SessionNames.adjectives.contains(parts[0]))
        #expect(SessionNames.nouns.contains(parts[1]))
    }

    @Test func aTakenNameIsNeverReturned() {
        let all = SessionNames.adjectives.flatMap { a in SessionNames.nouns.map { "\(a)-\($0)" } }
        let free = all.last!
        #expect(SessionNames.next(avoiding: all.dropLast()) == free)
        #expect(!all.contains(SessionNames.next(avoiding: all)))
    }
}
