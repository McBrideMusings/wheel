import Testing
@testable import WheelKit

@Suite struct SkillSearchTests {
    let skills = [
        SkillInfo(name: "wrap-up", description: "Commit and land finished work"),
        SkillInfo(name: "review", description: "Review the changes"),
        SkillInfo(name: "code-review", description: "Review a diff"),
        SkillInfo(name: "implement", description: "Plan and edit"),
    ]

    @Test func prefixesBelongToTheirAgent() {
        #expect(SkillSearch.prefix(ofAgent: "claude") == "/")
        #expect(SkillSearch.prefix(ofAgent: "codex") == "$")
        #expect(SkillSearch.prefix(ofAgent: "pi") == nil)
    }

    @Test func aQueryIsTheTextAfterThePrefixUntilWhitespace() {
        #expect(SkillSearch.query(in: "/rev", prefix: "/") == "rev")
        #expect(SkillSearch.query(in: "/", prefix: "/") == "")
        #expect(SkillSearch.query(in: "/review ", prefix: "/") == nil)
        #expect(SkillSearch.query(in: "hi /rev", prefix: "/") == nil)
        #expect(SkillSearch.query(in: "/rev", prefix: "$") == nil)
    }

    @Test func rankingPutsPrefixThenContainsThenFuzzyThenDescription() {
        #expect(SkillSearch.rank("rev", in: skills).map(\.name) == ["review", "code-review"])
        #expect(SkillSearch.rank("wu", in: skills).map(\.name) == ["wrap-up"])
        #expect(SkillSearch.rank("land", in: skills).map(\.name) == ["wrap-up"])
        #expect(SkillSearch.rank("zzz", in: skills).isEmpty)
        #expect(SkillSearch.rank("", in: skills).map(\.name) == ["code-review", "implement", "review", "wrap-up"])
    }
}
