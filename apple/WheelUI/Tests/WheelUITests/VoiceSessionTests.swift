#if os(iOS)
import Foundation
import Testing
@testable import WheelUI

@MainActor private final class FakeEngine: VoiceEngine {
    var log: [String] = []
    func start(for owner: String) { log.append("start \(owner)") }
    func stop(for owner: String) { log.append("stop \(owner)") }
    func sent() { log.append("sent") }
    func speak(_ reply: String) { log.append("speak \(reply)") }
}

@MainActor private final class Wire {
    var log: [String] = []
    var transport: VoiceTransport {
        VoiceTransport(
            claim: { self.log.append("claim") },
            release: { self.log.append("release") },
            name: { self.log.append("name \($0 ?? "-")") },
            send: { self.log.append("send \($0): \($1)") }
        )
    }
}

@MainActor @Suite struct VoiceSessionTests {
    private func opened() -> (VoiceSession, FakeEngine, Wire) {
        let engine = FakeEngine()
        let wire = Wire()
        let session = VoiceSession(engine: engine)
        session.transport = wire.transport
        session.open(agent: "w5", title: "Claude", subtitle: "wheel · claude", heard: 0)
        return (session, engine, wire)
    }

    @Test func theClaimComesFirstThenTheAgentIsNamedAndTheMicrophoneStarts() {
        let (session, engine, wire) = opened()
        #expect(session.stage == .claiming && wire.log == ["claim"] && engine.log.isEmpty)
        session.claimed(owner: "c1")
        #expect(session.stage == .on && wire.log == ["claim", "name w5"] && engine.log == ["start w5"])
    }

    @Test func xNamesNoAgentThenReleasesTheLease() {
        let (session, engine, wire) = opened()
        session.claimed(owner: "c1")
        session.close()
        #expect(session.stage == .off && wire.log == ["claim", "name w5", "name -", "release"] && engine.log == ["start w5", "stop w5"])
    }

    @Test func anotherOwnerOfTheLeaseEndsVoiceAtOnceWithoutReleasing() {
        let (session, engine, wire) = opened()
        session.claimed(owner: "c1")
        session.lease(key: "voice", owner: "c1")
        #expect(session.stage == .on, "our own claim announced back changes nothing")
        session.lease(key: "other", owner: "c9")
        #expect(session.stage == .on, "another lease is not ours")
        session.lease(key: "voice", owner: "c9")
        #expect(session.stage == .off && session.notice == "Voice mode moved to another device.")
        #expect(wire.log == ["claim", "name w5"] && engine.log == ["start w5", "stop w5"])
    }

    @Test func aRefusedClaimClosesAndSaysWhy() {
        let (session, engine, _) = opened()
        session.refused("this agent has no voice line")
        #expect(session.stage == .off && session.notice == "this agent has no voice line" && engine.log.isEmpty)
    }

    @Test func aClaimAnsweredAfterTheCloseIsGivenBack() {
        let (session, _, wire) = opened()
        session.close()
        session.claimed(owner: "c1")
        #expect(session.stage == .off && wire.log == ["claim", "release"])
    }

    @Test func aNewTurnOfThisConversationIsSentOnceAndOthersAreNot() {
        let (session, engine, wire) = opened()
        session.claimed(owner: "c1")
        session.heard(Voice.Said(owner: "w5", text: "run the tests", id: 1))
        session.heard(Voice.Said(owner: "w5", text: "run the tests", id: 1))
        session.heard(Voice.Said(owner: "w9", text: "elsewhere", id: 2))
        #expect(wire.log == ["claim", "name w5", "send w5: run the tests"] && engine.log == ["start w5", "sent"])
    }

    @Test func aTurnHeardBeforeOpeningIsNotSentAgain() {
        let engine = FakeEngine()
        let wire = Wire()
        let session = VoiceSession(engine: engine)
        session.transport = wire.transport
        session.open(agent: "w5", title: "", subtitle: "", heard: 3)
        session.claimed(owner: "c1")
        session.heard(Voice.Said(owner: "w5", text: "old", id: 3))
        #expect(!wire.log.contains { $0.hasPrefix("send") })
    }

    @Test func aFinishedReplyIsSpokenOnlyForTheConversationVoiceTalksTo() {
        let (session, engine, _) = opened()
        session.claimed(owner: "c1")
        session.turnEnded(agent: "w9", reply: "not mine")
        session.turnEnded(agent: "w5", reply: "Done.")
        #expect(engine.log == ["start w5", "speak Done."])
    }

    @Test func aCardShownDuringVoiceIsTheChipAndClearsWithTheClose() {
        let (session, _, _) = opened()
        session.claimed(owner: "c1")
        session.cardShown(agent: "w5", title: "Diff")
        #expect(session.card == "Diff")
        session.close()
        #expect(session.card == nil)
    }

    @Test func theAudioStoppingByItselfClosesVoiceWithItsReason() {
        let (session, _, wire) = opened()
        session.claimed(owner: "c1")
        session.audioStopped(phase: .off, problem: "No microphone is available.")
        #expect(session.stage == .off && session.notice == "No microphone is available.")
        #expect(wire.log.suffix(2) == ["name -", "release"])
    }

    @Test func theLayerShrinksToAPillAndComesBack() {
        let (session, _, _) = opened()
        session.claimed(owner: "c1")
        session.shrink()
        #expect(session.layout == .pill)
        session.restore()
        #expect(session.layout == .layer)
    }

    @Test func voiceIsOfferedOnlyWithBothOps() {
        #expect(VoiceSession.offered(by: ["lease_claim", "agent_voice"]))
        #expect(!VoiceSession.offered(by: ["lease_claim"]))
        #expect(!VoiceSession.offered(by: ["agent_voice"]))
    }

    @Test func theRoundButtonIsStopOrSend() {
        #expect(ComposerRoundButton.of(busy: true, hasContent: false) == .stop)
        #expect(ComposerRoundButton.of(busy: true, hasContent: true) == .send)
        #expect(ComposerRoundButton.of(busy: false, hasContent: true) == .send)
        #expect(ComposerRoundButton.of(busy: false, hasContent: false) == .send, "a dimmed Send")
    }

    @Test func theOrbFollowsTheAudio() {
        #expect(VoiceFace.of(phase: .listening, muted: false) == .listening)
        #expect(VoiceFace.of(phase: .hearing, muted: false) == .listening)
        #expect(VoiceFace.of(phase: .transcribing, muted: false) == .thinking)
        #expect(VoiceFace.of(phase: .thinking, muted: false) == .thinking)
        #expect(VoiceFace.of(phase: .speaking, muted: false) == .speaking)
        #expect(VoiceFace.of(phase: .speaking, muted: true) == .muted)
        #expect(VoiceFace.muted.word(phase: .listening) == "Muted")
    }
}
#endif
