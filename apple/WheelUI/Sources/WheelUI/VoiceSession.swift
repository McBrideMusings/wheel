#if os(iOS)
import Foundation
import Observation

/// What voice mode's session needs of the audio: `Voice` on a device, a fake in a test.
@MainActor
public protocol VoiceEngine: AnyObject {
    func start(for owner: String)
    func stop(for owner: String)
    func sent()
    func speak(_ reply: String)
}

extension Voice: VoiceEngine {}

/// The wheeld calls voice mode makes, set by whoever owns the connection.
public struct VoiceTransport {
    /// `lease_claim` key `voice`; the answer comes back as `claimed` or `refused`.
    public var claim: () -> Void
    /// `lease_release` key `voice`.
    public var release: () -> Void
    /// `agent_voice`: the agent whose turns carry the voice line, or nil to clear it.
    public var name: (String?) -> Void
    /// The person's spoken turn as a message to `agent` (`agent_send`).
    public var send: (_ agent: String, _ text: String) -> Void

    public init(claim: @escaping () -> Void, release: @escaping () -> Void, name: @escaping (String?) -> Void,
                send: @escaping (String, String) -> Void) {
        self.claim = claim
        self.release = release
        self.name = name
        self.send = send
    }
}

/// Voice mode over one agent view (`docs/voice-mode/index.md` §3, §5.9): wheeld's `voice` lease, the agent
/// it names, the turns the person speaks and the replies spoken back. The layer and the pill only draw it.
/// What it decides is here, apart from the audio and the socket, so a test drives it with fakes.
@MainActor
@Observable
public final class VoiceSession {
    public enum Stage: Equatable, Sendable {
        case off
        /// `lease_claim` is out; nothing listens yet.
        case claiming
        case on
    }

    /// The full layer, or the pill that floats over the panes.
    public enum Layout: Equatable, Sendable { case layer, pill }

    public static let leaseKey = "voice"
    public static let movedAway = "Voice mode moved to another device."

    public private(set) var stage: Stage = .off
    public private(set) var layout: Layout = .layer
    /// The conversation (agent name) voice mode talks to.
    public private(set) var agent: String?
    /// The pane's label and its "space · harness" line, for the header.
    public private(set) var title = ""
    public private(set) var subtitle = ""
    /// Why voice mode closed on its own, or was refused; shown by the composer until the next open.
    public private(set) var notice: String?
    /// The card the agent showed since voice opened, as the chip above the orb.
    public private(set) var card: String?

    @ObservationIgnored public var transport: VoiceTransport?
    @ObservationIgnored private let engine: any VoiceEngine
    /// This connection's id from `lease_claimed`; a `lease` event naming another owner means the lease left.
    @ObservationIgnored private var mine: String?
    @ObservationIgnored private var lastHeard = 0

    public init(engine: any VoiceEngine) { self.engine = engine }

    public convenience init() { self.init(engine: Voice.shared) }

    public var isOn: Bool { stage != .off }

    /// Whether voice mode may open: wheeld takes the lease and the agent line.
    public static func offered(by ops: Set<String>) -> Bool { ops.contains("lease_claim") && ops.contains("agent_voice") }

    /// The button on the composer: claim the lease, then listen.
    public func open(agent: String, title: String, subtitle: String, heard: Int) {
        guard stage == .off, transport != nil else { return }
        stage = .claiming
        layout = .layer
        self.agent = agent
        self.title = title
        self.subtitle = subtitle
        notice = nil
        card = nil
        mine = nil
        lastHeard = heard
        transport?.claim()
    }

    /// `lease_claimed`: this connection holds the lease. Name the agent, then start the microphone.
    public func claimed(owner: String) {
        guard stage != .off, let agent else {
            // Closed while the claim was out: give the lease straight back.
            transport?.release()
            return
        }
        guard stage == .claiming else { return }
        mine = owner
        stage = .on
        transport?.name(agent)
        engine.start(for: agent)
    }

    /// A refused claim closes voice mode and says why.
    public func refused(_ why: String) {
        guard stage == .claiming else { return }
        finish(notice: why, releasing: false)
    }

    /// `lease` for any key: another owner for ours ends voice mode here at once.
    public func lease(key: String, owner: String?) {
        guard key == Self.leaseKey, stage == .on, let mine, owner != mine else { return }
        finish(notice: Self.movedAway, releasing: false)
    }

    /// X: clear the agent, give the lease back, stop the audio.
    public func close() {
        guard stage != .off else { return }
        // A claim still out is given back when its answer arrives (`claimed`).
        finish(notice: nil, releasing: stage == .on)
    }

    /// The connection, the pane or the app view went away: stop the audio; wheeld frees a lease its connection held.
    public func end(_ notice: String? = nil) {
        guard stage != .off else { return }
        finish(notice: notice, releasing: stage == .on && transport != nil)
    }

    private func finish(notice: String?, releasing: Bool) {
        if stage == .on, let agent { engine.stop(for: agent) }
        if releasing {
            transport?.name(nil)
            transport?.release()
        }
        stage = .off
        layout = .layer
        agent = nil
        mine = nil
        card = nil
        self.notice = notice
    }

    /// `Voice.said` changed: a new turn of this conversation goes to the agent as the person's message.
    public func heard(_ said: Voice.Said?) {
        guard stage == .on, let said, said.owner == agent, said.id > lastHeard else { return }
        lastHeard = said.id
        transport?.send(said.owner, said.text)
        engine.sent()
    }

    /// An agent's turn ended: its last reply is spoken when voice mode talks to that agent.
    public func turnEnded(agent name: String, reply: String) {
        guard stage == .on, name == agent else { return }
        engine.speak(reply)
    }

    /// The agent showed a card after voice opened.
    public func cardShown(agent name: String, title: String) {
        guard stage != .off, name == agent else { return }
        card = title
    }

    public func clearCard() { card = nil }

    /// The audio stopped by itself (no microphone, a model that failed): voice mode closes with the reason.
    public func audioStopped(phase: Voice.Phase, problem: String?) {
        guard stage == .on, phase == .off else { return }
        finish(notice: problem ?? "Voice mode stopped.", releasing: true)
    }

    public func shrink() { if stage != .off { layout = .pill } }
    public func restore() { if stage != .off { layout = .layer } }
}

/// How the orb looks for what the audio is doing (`docs/voice-mode/index.md` Orb states).
public enum VoiceFace: Equatable, Sendable {
    case listening, thinking, speaking, muted

    public static func of(phase: Voice.Phase, muted: Bool) -> VoiceFace {
        if muted { return .muted }
        switch phase {
        case .speaking: return .speaking
        case .thinking, .transcribing: return .thinking
        default: return .listening
        }
    }

    /// The word under the orb.
    public func word(phase: Voice.Phase) -> String {
        switch (self, phase) {
        case (.muted, _): return "Muted"
        case (_, .starting), (_, .downloading): return "Starting"
        case (_, .hearing): return "Hearing you"
        case (.listening, _): return "Listening"
        case (.thinking, .transcribing): return "Transcribing"
        case (.thinking, _): return "Thinking"
        case (.speaking, _): return "Speaking"
        }
    }
}

/// One model the first download of voice mode fetches, named before it does (ADR-0013).
public struct VoiceModelNote: Equatable, Sendable {
    public let name: String
    public let size: String
    public let source: String
    public let folder: String

    public init(name: String, size: String, source: String, folder: String) {
        self.name = name
        self.size = size
        self.source = source
        self.folder = folder
    }
}

extension Voice {
    /// The models voice mode still lacks on this device, each with its size, source and folder.
    public var modelsMissing: [VoiceModelNote] {
        var notes: [VoiceModelNote] = []
        if !Dictation.shared.modelOnDisk {
            notes.append(VoiceModelNote(name: Dictation.modelName, size: Dictation.modelSize, source: Dictation.modelSource,
                                        folder: Dictation.modelFolder.path))
        }
        if !vadOnDisk {
            notes.append(VoiceModelNote(name: Self.vadName, size: Self.vadSize, source: Self.vadSource, folder: Self.vadFolder.path))
        }
        return notes
    }
}

#if DEBUG
extension VoiceSession {
    /// A check's stand-in for the lease: voice mode on over `agent` with nothing claimed, so a screenshot
    /// reaches the layer and the pill.
    public func demoOn(agent: String, title: String, subtitle: String, card: String?, pill: Bool) {
        self.agent = agent
        self.title = title
        self.subtitle = subtitle
        self.card = card
        stage = .on
        layout = pill ? .pill : .layer
    }
}
#endif
#endif
