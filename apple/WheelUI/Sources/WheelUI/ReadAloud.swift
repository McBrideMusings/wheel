import AVFoundation
import Observation
import WheelKit

/// Speaks an agent's replies with the system voice. One synthesizer for the app: a new reply
/// interrupts the one still being read, and turning read-aloud off stops it at once. A caller names
/// what it speaks with `key`, so a button can show whether its own reply is the one being read.
@MainActor
@Observable
public final class ReadAloud {
    public static let shared = ReadAloud()
    @ObservationIgnored private let synthesizer = AVSpeechSynthesizer()
    @ObservationIgnored private let finish = Finish()
    /// The conversation the synthesizer is reading for.
    @ObservationIgnored private var owner: String?
    @ObservationIgnored private var current: AVSpeechUtterance?
    /// The key of the reply being read, nil when nothing is. Views read this to draw play or stop.
    public private(set) var playing: String?
    /// What was last handed to the synthesizer, for the state query.
    @ObservationIgnored public private(set) var lastSpoken = ""
    /// How many replies have been spoken, for the state query.
    @ObservationIgnored public private(set) var spokenCount = 0

    public var isSpeaking: Bool { synthesizer.isSpeaking }

    private init() {
        finish.onEnd = { [weak self] utterance in
            guard let self, self.current.map(ObjectIdentifier.init) == utterance else { return }
            self.current = nil
            self.playing = nil
        }
        synthesizer.delegate = finish
    }

    /// Say `reply` (markdown) aloud for conversation `owner`, replacing anything being spoken. A
    /// reply with no prose says nothing.
    public func speak(_ reply: String, for owner: String, key: String? = nil) {
        let text = SpokenText.from(reply)
        guard !text.isEmpty else { return }
        synthesizer.stopSpeaking(at: .immediate)
        self.owner = owner
        lastSpoken = text
        spokenCount += 1
        UILog.info("read aloud: \(text.count) character(s)")
        let utterance = AVSpeechUtterance(string: text)
        current = utterance
        playing = key
        synthesizer.speak(utterance)
    }

    /// The play button on a reply: speak it, or stop it when it is the one being read.
    public func toggle(_ reply: String, for owner: String, key: String) {
        if playing == key {
            synthesizer.stopSpeaking(at: .immediate)
            current = nil
            playing = nil
        } else {
            speak(reply, for: owner, key: key)
        }
    }

    /// Stop speech, but only when it is `owner`'s: turning one conversation's read-aloud off never
    /// cuts off another's.
    public func stop(for owner: String) {
        guard self.owner == owner else { return }
        synthesizer.stopSpeaking(at: .immediate)
        current = nil
        playing = nil
    }

    /// Tells the synthesizer's owner when an utterance ends or is cut off.
    private final class Finish: NSObject, AVSpeechSynthesizerDelegate, @unchecked Sendable {
        var onEnd: (@MainActor (ObjectIdentifier) -> Void)?
        func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
            let id = ObjectIdentifier(utterance)
            Task { @MainActor in self.onEnd?(id) }
        }
        func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
            let id = ObjectIdentifier(utterance)
            Task { @MainActor in self.onEnd?(id) }
        }
    }
}
