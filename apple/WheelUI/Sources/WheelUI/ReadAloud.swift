import AVFoundation
import WheelKit

/// Speaks an agent's finished reply with the system voice. One synthesizer for the app: a new
/// reply interrupts the one still being read, and turning read-aloud off stops it at once.
@MainActor
public final class ReadAloud {
    public static let shared = ReadAloud()
    private let synthesizer = AVSpeechSynthesizer()
    /// The conversation the synthesizer is reading for.
    private var owner: String?
    /// What was last handed to the synthesizer, for the state query.
    public private(set) var lastSpoken = ""
    /// How many replies have been spoken, for the state query.
    public private(set) var spokenCount = 0

    public var isSpeaking: Bool { synthesizer.isSpeaking }

    /// Say `reply` (markdown) aloud for conversation `owner`, replacing anything being spoken. A
    /// reply with no prose says nothing.
    public func speak(_ reply: String, for owner: String) {
        let text = SpokenText.from(reply)
        guard !text.isEmpty else { return }
        synthesizer.stopSpeaking(at: .immediate)
        self.owner = owner
        lastSpoken = text
        spokenCount += 1
        UILog.info("read aloud: \(text.count) character(s)")
        synthesizer.speak(AVSpeechUtterance(string: text))
    }

    /// Stop speech, but only when it is `owner`'s: turning one conversation's read-aloud off never
    /// cuts off another's.
    public func stop(for owner: String) {
        guard self.owner == owner else { return }
        synthesizer.stopSpeaking(at: .immediate)
    }
}
