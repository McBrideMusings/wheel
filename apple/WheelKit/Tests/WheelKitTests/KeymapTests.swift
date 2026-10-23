import Testing
@testable import WheelKit

struct KeymapTests {
    static let defaults: [(action: String, chord: KeyChord?)] = [
        ("split-right", KeyChord("cmd+d")),
        ("split-down", KeyChord("shift+cmd+d")),
        ("close-pane", KeyChord("cmd+w")),
        ("rename-window", nil),
    ]

    @Test func aChordReadsOnlyTheStoredSpellingAndPrintsAsAMenuDoes() {
        #expect(KeyChord("shift+cmd+d")?.symbol == "⇧⌘D")
        #expect(KeyChord("ctrl+opt+cmd+arrow_left")?.symbol == "⌃⌥⌘←")
        #expect(KeyChord("cmd+bracket_left")?.symbol == "⌘[")
        #expect(KeyChord("shift+cmd+enter")?.text == "shift+cmd+enter")
        #expect(KeyChord("cmd+f12")?.symbol == "⌘F12")
        for bad in ["cmd+shift+d", "cmd+cmd+d", "cmd+D", "cmd+", "super+d", "cmd+f13", "cmd+dd"] {
            #expect(KeyChord(bad) == nil, "\(bad)")
        }
    }

    @Test func theSettingAppliesOverTheDefaultsEntryByEntry() {
        let map = Keymap(defaults: Self.defaults, setting: "cmd+w=unbind,ctrl+cmd+w=close-pane,cmd+d=split-down,cmd+k=rename-window")
        #expect(map.chords["close-pane"]?.text == "ctrl+cmd+w")
        // A chord moved onto another action leaves both the old holder of the chord and the
        // action's own default.
        #expect(map.chords["split-down"]?.text == "cmd+d")
        #expect(map.chords["split-right"] == nil)
        #expect(map.chords["rename-window"]?.text == "cmd+k")
        #expect(map.action(for: KeyChord("cmd+w")!) == nil)
        #expect(map.problems.isEmpty)
    }

    @Test func clearDropsWhatCameBeforeItAndAnUnknownActionIsAProblem() {
        let map = Keymap(defaults: Self.defaults, setting: "clear,cmd+d=split-right,cmd+j=split-sideways")
        #expect(map.chords.mapValues(\.text) == ["split-right": "cmd+d"])
        #expect(map.problems == ["cmd+j=split-sideways: Wheel has no action split-sideways"])
    }
}
