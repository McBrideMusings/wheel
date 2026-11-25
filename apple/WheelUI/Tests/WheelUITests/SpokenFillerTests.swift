import Testing
@testable import WheelUI

@Suite struct SpokenFillerTests {
    @Test func soundsAreFiller() {
        for text in ["", "Hmm.", "Mm-hmm.", "Uh, um...", "Oh.", "  "] {
            #expect(SpokenFiller.isFiller(text, whileSpeaking: false), "\(text)")
        }
    }

    @Test func wordsAreNot() {
        #expect(!SpokenFiller.isFiller("Wait, stop.", whileSpeaking: true))
        #expect(!SpokenFiller.isFiller("Hmm, try the other one.", whileSpeaking: false))
    }

    @Test func nodsOnlyFillWhileSpeaking() {
        #expect(SpokenFiller.isFiller("Yeah, okay.", whileSpeaking: true))
        #expect(!SpokenFiller.isFiller("Yeah.", whileSpeaking: false))
    }
}
