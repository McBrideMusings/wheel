import AVFoundation
import Foundation
import Observation
import WheelKit
internal import FluidAudio

/// Voice mode's audio on this device (`docs/voice-mode/index.md` §3): the microphone stays open,
/// Silero VAD (FluidAudio) decides when the person has stopped speaking, Parakeet (`Dictation`'s
/// model) turns that turn into text, and the agent's reply is spoken with the system voice through
/// the same audio engine, so the engine's echo cancellation hears what it plays and the person can
/// talk over it. No audio leaves the device; the caller sends the text with `agent_send`.
@MainActor
@Observable
public final class Voice {
    public static let shared = Voice()

    public enum Phase: String, Sendable {
        case off
        /// A model is missing: the layer names what the download fetches and asks.
        case asking
        case downloading
        /// The models load and the microphone starts.
        case starting
        case listening
        /// Speech is under way.
        case hearing
        case transcribing
        /// The turn went to the agent; its reply has not come.
        case thinking
        case speaking
    }

    /// One turn the person spoke, for the conversation voice mode talks to. `id` grows with each.
    public struct Said: Equatable, Sendable {
        public let owner: String
        public let text: String
        public let id: Int
    }

    /// The turn-taking model, named before it downloads (ADR-0013); Parakeet is `Dictation`'s.
    public static let vadName = "Silero VAD v6"
    public static let vadSize = "1.1 MB"
    public static let vadSource = "huggingface.co/FluidInference/silero-vad-coreml"

    public private(set) var phase: Phase = .off
    public private(set) var owner: String?
    public private(set) var muted = false
    /// The microphone's level while listening, the speech's while speaking, `0...1`.
    public private(set) var level: Float = 0
    public private(set) var said: Said?
    /// What is being spoken, for the caption.
    public private(set) var caption: String?
    public private(set) var problem: String?

    /// Where the VAD model lives: beside Parakeet in FluidAudio's `Models` folder, as
    /// FluidAudio's `ModelHub` lays it out (`<base>/Models/silero-vad-coreml`).
    public nonisolated static var vadFolder: URL {
        Dictation.modelFolder.deletingLastPathComponent().appendingPathComponent("silero-vad-coreml")
    }
    /// The `<base>` `VadManager(modelDirectory:)` takes.
    nonisolated static var vadBase: URL {
        Dictation.modelFolder.deletingLastPathComponent().deletingLastPathComponent()
    }

    public var modelsOnDisk: Bool {
        Dictation.shared.modelOnDisk && FileManager.default.fileExists(atPath: Self.vadFolder.appendingPathComponent(ModelNames.VAD.sileroVadFile).path)
    }

    @ObservationIgnored private var audio: VoiceAudio?
    @ObservationIgnored private var vad: VadManager?
    @ObservationIgnored private var saidCount = 0
    @ObservationIgnored private var levelAt = Date.distantPast
    /// A reply that came while the person was mid-turn, spoken once that turn is done.
    @ObservationIgnored private var waiting: String?

    private init() {}

    /// Open voice mode for conversation `owner`: ask for the models when they are missing, else
    /// load them and start listening. A second start for another conversation moves voice there.
    public func start(for owner: String) {
        stopAudio()
        self.owner = owner
        problem = nil
        muted = false
        caption = nil
        guard modelsOnDisk else {
            phase = .asking
            UILog.info("voice: models not on this device; asking to download them")
            return
        }
        phase = .starting
        Task { await begin(owner) }
    }

    /// Fetch what the prompt named, then start.
    public func download() {
        guard phase == .asking, let owner else { return }
        phase = .downloading
        UILog.info("voice: downloading \(Dictation.modelName) and \(Self.vadName) into \(Self.vadFolder.deletingLastPathComponent().path)")
        Task {
            do {
                if !Dictation.shared.modelOnDisk {
                    _ = try await AsrModels.download(to: Dictation.modelFolder, version: .v3)
                }
                vad = try await VadManager(modelDirectory: Self.vadBase)
                guard self.owner == owner, phase == .downloading else { return }
                phase = .starting
                await begin(owner)
            } catch {
                fail("The download stopped: \(error.localizedDescription)")
            }
        }
    }

    /// End voice mode for `owner`: the microphone closes and speech stops. A stop for a
    /// conversation voice has since moved to changes nothing.
    public func stop(for owner: String) {
        guard self.owner == owner else { return }
        stop()
    }

    /// End voice mode: the microphone closes and speech stops.
    public func stop() {
        stopAudio()
        phase = .off
        owner = nil
        level = 0
        caption = nil
        muted = false
    }

    public func setMuted(_ on: Bool) {
        muted = on
        audio?.muted = on
        if on, phase == .hearing { phase = .listening }
        level = 0
    }

    /// The caller sent the turn: the orb thinks until the reply is spoken.
    public func sent() {
        if phase == .listening || phase == .transcribing { phase = .thinking }
    }

    /// Say the agent's reply (markdown); speaking over it interrupts it. A reply that comes while
    /// the person is mid-turn waits until that turn is sent, so neither is lost.
    public func speak(_ reply: String) {
        guard let audio, phase != .off else { return }
        if phase == .hearing || phase == .transcribing {
            waiting = reply
            return
        }
        let text = SpokenText.from(reply)
        guard !text.isEmpty else {
            if phase == .thinking { phase = .listening }
            return
        }
        caption = text
        phase = .speaking
        UILog.info("voice: speaking \(text.count) character(s)")
        audio.speak(text) { [weak self] in
            guard let self, self.phase == .speaking else { return }
            self.phase = .listening
            self.level = 0
        }
    }

    /// Tap on the orb, or speech while it speaks: silence at once, and listen. The unspoken rest is
    /// dropped from audio only; the chat keeps the whole reply.
    public func interrupt() {
        guard phase == .speaking else { return }
        audio?.silence()
        phase = .listening
        level = 0
        UILog.info("voice: interrupted")
    }

    private func begin(_ owner: String) async {
        do {
            let asr = try await Dictation.shared.loadedModel()
            if vad == nil { vad = try await VadManager(modelDirectory: Self.vadBase) }
            guard let vad, self.owner == owner, phase == .starting else { return }
            let file = ProcessInfo.processInfo.environment["WHEEL_VOICE_FILE"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) }
            let audio = VoiceAudio(file: file, silent: ProcessInfo.processInfo.environment["WHEEL_VOICE_SILENT"] == "1")
            audio.onChunk = { [weak self] chunk, rms in self?.heard(chunk, rms: rms, vad: vad, asr: asr) }
            audio.onOutputLevel = { [weak self] rms in
                guard let self, self.phase == .speaking else { return }
                self.publish(level: rms)
            }
            try await audio.start()
            guard self.owner == owner, phase == .starting else { audio.stop(); return }
            self.audio = audio
            phase = .listening
            UILog.info("voice: listening for \(owner)\(file.map { " from \($0.lastPathComponent)" } ?? "")")
        } catch {
            fail(error.localizedDescription)
        }
    }

    /// The turn being heard, and the VAD's state across chunks.
    @ObservationIgnored private var turn: [Float] = []
    @ObservationIgnored private var stream: VadStreamState = .initial()
    @ObservationIgnored private var judging = false
    @ObservationIgnored private var pending: [[Float]] = []

    /// One `VadManager.chunkSize` chunk of 16 kHz audio from the microphone.
    private func heard(_ chunk: [Float], rms: Float, vad: VadManager, asr: AsrManager) {
        guard !muted, phase != .off else { return }
        if phase == .listening || phase == .hearing { publish(level: rms) }
        pending.append(chunk)
        guard !judging else { return }
        judging = true
        Task {
            while !pending.isEmpty {
                let next = pending.removeFirst()
                do {
                    let result = try await vad.processStreamingChunk(next, state: stream)
                    stream = result.state
                    await judge(next, result.event, asr: asr)
                } catch {
                    UILog.info("voice: VAD: \(error.localizedDescription)")
                }
            }
            judging = false
        }
    }

    private func judge(_ chunk: [Float], _ event: VadStreamEvent?, asr: AsrManager) async {
        if event?.isStart == true {
            if phase == .speaking { interrupt() }
            if phase == .listening || phase == .thinking {
                phase = .hearing
                // The chunk before the start holds the first syllable.
                turn = []
            }
        }
        guard phase == .hearing else { return }
        turn += chunk
        guard event?.isEnd == true, let owner else { return }
        let samples = turn
        turn = []
        phase = .transcribing
        do {
            let padded = samples.count < 16_000 ? samples + [Float](repeating: 0, count: 16_000 - samples.count) : samples
            var state = TdtDecoderState.make(decoderLayers: await asr.decoderLayerCount)
            let text = try await asr.transcribe(padded, decoderState: &state).text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard self.owner == owner, phase == .transcribing else { return }
            UILog.info("voice: heard \(text.count) character(s) from \(String(format: "%.1f", Double(samples.count) / 16_000)) s")
            if text.isEmpty {
                phase = .listening
            } else {
                saidCount += 1
                said = Said(owner: owner, text: text, id: saidCount)
                phase = .thinking
            }
        } catch {
            UILog.info("voice: could not transcribe: \(error.localizedDescription)")
            phase = .listening
        }
        if let reply = waiting {
            waiting = nil
            speak(reply)
        }
    }

    /// At most fifteen level changes a second, so the orb moves without flooding every window.
    private func publish(level rms: Float) {
        let now = Date()
        guard now.timeIntervalSince(levelAt) > 1.0 / 15 else { return }
        levelAt = now
        level = min(1, rms * 8)
    }

    private func stopAudio() {
        waiting = nil
        audio?.stop()
        audio = nil
        turn = []
        pending = []
        stream = .initial()
    }

    private func fail(_ why: String) {
        UILog.info("voice: \(why)")
        stopAudio()
        problem = why
        phase = .off
    }
}

/// The microphone (or a file standing in for it, paced as if spoken) and the speaker of voice mode,
/// on one `AVAudioEngine` with voice processing on, so what it plays is cancelled from what it hears.
/// Made and stopped on the main actor; the audio thread only calls `onChunk` through the main actor.
private final class VoiceAudio: @unchecked Sendable {
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let synthesizer = AVSpeechSynthesizer()
    private let file: URL?
    private let silent: Bool
    private var feeding: Task<Void, Never>?
    private let gather = Samples()
    /// Bumped by every `speak` and `silence`, so a finished buffer of older speech changes nothing.
    private var generation = 0
    var muted = false
    var onChunk: (@MainActor ([Float], Float) -> Void)?
    var onOutputLevel: (@MainActor (Float) -> Void)?
    private let speechFormat = AVAudioFormat(standardFormatWithSampleRate: 22_050, channels: 1)!

    init(file: URL?, silent: Bool) {
        self.file = file
        self.silent = silent
    }

    @MainActor
    func start() async throws {
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: speechFormat)
        if silent { player.volume = 0 }
        player.installTap(onBus: 0, bufferSize: 2048, format: speechFormat) { [weak self] buffer, _ in
            let rms = Self.rms(buffer)
            Task { @MainActor in self?.onOutputLevel?(rms) }
        }
        if let file {
            let samples = try AudioConverter(sampleRate: 16_000).resampleAudioFile(file)
            try engine.start()
            feed(samples)
            return
        }
        guard await AVAudioApplication.requestRecordPermission() else {
            throw DictationError("Wheel may not use the microphone. Allow it in System Settings › Privacy & Security › Microphone.")
        }
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .allowBluetoothHFP])
        try session.setActive(true)
        #endif
        let input = engine.inputNode
        try input.setVoiceProcessingEnabled(true)
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0 else { throw DictationError("No microphone is available.") }
        let converter = AudioConverter(sampleRate: 16_000)
        input.installTap(onBus: 0, bufferSize: 4096, format: format) { [weak self] buffer, _ in
            guard let self, !self.muted, let converted = try? converter.resampleBuffer(buffer) else { return }
            self.collect(converted)
        }
        engine.prepare()
        try engine.start()
    }

    /// A file's samples, a chunk at a time at the pace they would be spoken, then silence, so the
    /// VAD sees the turn end.
    private func feed(_ samples: [Float]) {
        let all = samples + [Float](repeating: 0, count: 16_000 * 2)
        feeding = Task { [weak self] in
            var at = 0
            while at < all.count, !Task.isCancelled {
                let end = min(at + VadManager.chunkSize, all.count)
                self?.collect(Array(all[at..<end]))
                at = end
                try? await Task.sleep(for: .milliseconds(256))
            }
            // Then quiet, chunk by chunk, until voice mode ends.
            while !Task.isCancelled {
                self?.collect([Float](repeating: 0, count: VadManager.chunkSize))
                try? await Task.sleep(for: .milliseconds(256))
            }
        }
    }

    /// Cut the stream into the VAD's chunks and hand each to the main actor with its level.
    private func collect(_ more: [Float]) {
        gather.append(more)
        while let chunk = gather.take(VadManager.chunkSize) {
            let rms = sqrt(chunk.reduce(0) { $0 + $1 * $1 } / Float(chunk.count))
            Task { @MainActor in self.onChunk?(chunk, rms) }
        }
    }

    /// How far one reply's speech has got: buffers still to play, and whether the synthesizer is done.
    @MainActor private final class Playback {
        var queued = 0
        var finished = false
        let done: @MainActor () -> Void
        init(done: @escaping @MainActor () -> Void) { self.done = done }
        func check() { if finished, queued == 0 { done() } }
    }

    @MainActor
    func speak(_ text: String, done: @escaping @MainActor () -> Void) {
        silence()
        let mine = generation
        let playback = Playback(done: done)
        synthesizer.write(AVSpeechUtterance(string: text)) { [weak self] buffer in
            guard let pcm = buffer as? AVAudioPCMBuffer else { return }
            Task { @MainActor in
                guard let self, mine == self.generation else { return }
                if pcm.frameLength == 0 {
                    playback.finished = true
                    playback.check()
                    return
                }
                guard let converted = self.convert(pcm) else { return }
                playback.queued += 1
                self.player.scheduleBuffer(converted) {
                    Task { @MainActor in
                        guard mine == self.generation else { return }
                        playback.queued -= 1
                        playback.check()
                    }
                }
                if !self.player.isPlaying { self.player.play() }
            }
        }
    }

    @MainActor
    func silence() {
        generation += 1
        synthesizer.stopSpeaking(at: .immediate)
        player.stop()
    }

    private func convert(_ buffer: AVAudioPCMBuffer) -> AVAudioPCMBuffer? {
        if buffer.format == speechFormat { return buffer }
        guard let converter = AVAudioConverter(from: buffer.format, to: speechFormat) else { return nil }
        let ratio = speechFormat.sampleRate / buffer.format.sampleRate
        guard let out = AVAudioPCMBuffer(pcmFormat: speechFormat, frameCapacity: AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 1) else { return nil }
        var given = false
        converter.convert(to: out, error: nil) { _, status in
            if given {
                status.pointee = .noDataNow
                return nil
            }
            given = true
            status.pointee = .haveData
            return buffer
        }
        return out
    }

    private static func rms(_ buffer: AVAudioPCMBuffer) -> Float {
        guard let data = buffer.floatChannelData?[0], buffer.frameLength > 0 else { return 0 }
        var sum: Float = 0
        for i in 0..<Int(buffer.frameLength) { sum += data[i] * data[i] }
        return sqrt(sum / Float(buffer.frameLength))
    }

    @MainActor
    func stop() {
        feeding?.cancel()
        silence()
        player.removeTap(onBus: 0)
        if file == nil { engine.inputNode.removeTap(onBus: 0) }
        if engine.isRunning { engine.stop() }
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        #endif
    }

    /// Samples the audio thread appends, taken a chunk at a time.
    private final class Samples: @unchecked Sendable {
        private let lock = NSLock()
        private var floats: [Float] = []
        func append(_ more: [Float]) { lock.withLock { floats += more } }
        func take(_ n: Int) -> [Float]? {
            lock.withLock {
                guard floats.count >= n else { return nil }
                defer { floats.removeFirst(n) }
                return Array(floats[0..<n])
            }
        }
    }
}
