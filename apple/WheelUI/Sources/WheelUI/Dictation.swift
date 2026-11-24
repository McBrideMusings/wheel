#if os(iOS)
import AVFoundation
import Foundation
import Observation
internal import FluidAudio

/// Speech to text for the composer, on this device: the microphone records while the person
/// holds the turn, Parakeet TDT 0.6b v3 (FluidAudio, `docs/voice-mode/stt-engines.md`) turns the
/// recording into text, and the text goes into the draft of the conversation that asked. No audio
/// leaves the device and nothing here talks to wheeld; the person sends the draft as typed text.
/// One recorder for the app: starting in one conversation while another records stops nothing,
/// it is refused until that one ends.
@MainActor
@Observable
public final class Dictation {
    public static let shared = Dictation()

    public enum Phase: Equatable, Sendable {
        case idle
        /// The model is not on this device: the composer names what the download fetches and asks.
        case asking
        /// The model is downloading, `0...1`.
        case downloading(Double)
        case recording
        case transcribing
    }

    /// What one recording turned into, for the conversation that recorded it. `id` grows with each
    /// one, so the same words twice still reach the draft twice.
    public struct Heard: Equatable, Sendable {
        public let owner: String
        public let text: String
        public let id: Int
    }

    /// What the download fetches, named before it starts (ADR-0013).
    public static let modelName = "Parakeet TDT 0.6b v3"
    public static let modelSize = "469 MB"
    public static let modelSource = "huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml"

    public private(set) var phase: Phase = .idle
    /// The conversation recording, asked or downloading for; nil while idle.
    public private(set) var owner: String?
    /// Why the last attempt failed, and whose it was, until the next one starts.
    public private(set) var problem: String?
    public private(set) var problemOwner: String?
    public private(set) var heard: Heard?

    /// Where the model lives: FluidAudio's own cache (`~/Library/Application Support/FluidAudio/Models`
    /// on the Mac, the app container's on the phone), or `WHEEL_DICTATION_MODELS` for a check that
    /// must start without it.
    public nonisolated static var modelFolder: URL {
        let standard = AsrModels.defaultCacheDirectory(for: .v3)
        guard let models = ProcessInfo.processInfo.environment["WHEEL_DICTATION_MODELS"], !models.isEmpty else { return standard }
        return URL(fileURLWithPath: models).appendingPathComponent(standard.lastPathComponent)
    }

    public var modelOnDisk: Bool { AsrModels.modelsExist(at: Self.modelFolder, version: .v3) }

    @ObservationIgnored private var loading: Task<AsrManager, any Error>?
    @ObservationIgnored private var recorder: Recorder?
    @ObservationIgnored private var heardCount = 0

    private init() {}

    /// The composer's microphone for conversation `owner`: start recording, or stop and transcribe
    /// when `owner` is the one recording. With no model on the device it asks first.
    public func toggle(for owner: String) {
        switch phase {
        case .recording where self.owner == owner:
            finish()
        case .idle, .asking:
            guard modelOnDisk else {
                self.owner = owner
                phase = .asking
                problem = nil
                UILog.info("dictation: \(Self.modelName) is not on this device; asking to download it")
                return
            }
            start(for: owner)
        default:
            UILog.info("dictation: \(owner) asked while \(self.owner ?? "nobody") is \(phase)")
        }
    }

    /// Fetch the model the prompt named, then go back to idle: the person taps the microphone
    /// again once it is here.
    public func download() {
        guard phase == .asking else { return }
        phase = .downloading(0)
        UILog.info("dictation: downloading \(Self.modelName) from \(Self.modelSource) into \(Self.modelFolder.path)")
        Task {
            do {
                _ = try await AsrModels.download(to: Self.modelFolder, version: .v3) { progress in
                    Task { @MainActor in
                        if case .downloading = Dictation.shared.phase { Dictation.shared.phase = .downloading(progress.fractionCompleted) }
                    }
                }
                UILog.info("dictation: \(Self.modelName) downloaded")
                phase = .idle
                owner = nil
                prepare()
            } catch {
                fail("The download stopped: \(error.localizedDescription)")
            }
        }
    }

    /// Close the prompt or throw away the recording.
    public func cancel() {
        recorder?.stop()
        recorder = nil
        if phase == .asking || phase == .recording {
            phase = .idle
            owner = nil
        }
    }

    /// Load the model ahead of the first tap when it is on the device: loading takes from a fraction
    /// of a second to half a minute while Core ML compiles it.
    public func prepare() {
        guard loading == nil, modelOnDisk else { return }
        loading = Task.detached(priority: .utility) {
            let started = Date()
            let models = try await AsrModels.load(from: Dictation.modelFolder, version: .v3)
            let asr = AsrManager(config: .default)
            try await asr.loadModels(models)
            await UILog.info("dictation: model loaded in \(String(format: "%.1f", Date().timeIntervalSince(started))) s")
            return asr
        }
    }

    /// The loaded model, for voice mode, which transcribes each turn with the same one.
    func loadedModel() async throws -> AsrManager {
        prepare()
        guard let loading else { throw DictationError("\(Self.modelName) is not on this device.") }
        do {
            return try await loading.value
        } catch {
            self.loading = nil
            throw error
        }
    }

    private func start(for owner: String) {
        // A recording on disk standing in for the microphone, for a check that has no one to speak.
        let file = ProcessInfo.processInfo.environment["WHEEL_DICTATION_FILE"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) }
        problem = nil
        self.owner = owner
        phase = .recording
        prepare()
        Task {
            do {
                let recorder = file.map(Recorder.init(file:)) ?? Recorder()
                do {
                    try await recorder.start()
                } catch {
                    // A tap installed or a session activated before the failure is undone here.
                    recorder.stop()
                    throw error
                }
                // Stopped or cancelled while the microphone was being granted.
                guard phase == .recording, self.owner == owner else { recorder.stop(); return }
                self.recorder = recorder
                UILog.info("dictation: recording for \(owner)\(file.map { " from \($0.lastPathComponent)" } ?? "")")
            } catch {
                fail(error.localizedDescription)
            }
        }
    }

    private func finish() {
        guard let recorder, let owner else { return }
        self.recorder = nil
        let samples = recorder.stop()
        phase = .transcribing
        UILog.info("dictation: \(samples.count) samples (\(String(format: "%.1f", Double(samples.count) / 16_000)) s) to transcribe")
        Task {
            do {
                guard let loading else { throw DictationError("The speech model is not loaded.") }
                let asr = try await loading.value
                // Parakeet refuses less than a second of audio; silence after the words changes nothing.
                let padded = samples.count < 16_000 ? samples + [Float](repeating: 0, count: 16_000 - samples.count) : samples
                var state = TdtDecoderState.make(decoderLayers: await asr.decoderLayerCount)
                let started = Date()
                let text = try await asr.transcribe(padded, decoderState: &state).text.trimmingCharacters(in: .whitespacesAndNewlines)
                UILog.info("dictation: \(text.count) character(s) in \(Int(Date().timeIntervalSince(started) * 1000)) ms")
                phase = .idle
                self.owner = nil
                if text.isEmpty {
                    problem = "Heard nothing."
                    problemOwner = owner
                } else {
                    heardCount += 1
                    heard = Heard(owner: owner, text: text, id: heardCount)
                }
            } catch {
                // A model that failed to load is loaded again on the next tap.
                self.loading = nil
                fail("Could not transcribe: \(error.localizedDescription)")
            }
        }
    }

    private func fail(_ why: String) {
        UILog.info("dictation: \(why)")
        recorder?.stop()
        recorder = nil
        problem = why
        problemOwner = owner
        phase = .idle
        owner = nil
    }
}

struct DictationError: LocalizedError {
    let errorDescription: String?
    init(_ text: String) { errorDescription = text }
}

/// The microphone, or a file standing in for it, as 16 kHz mono samples.
/// Made and stopped on the main actor; only `Samples` is touched from the audio thread.
private final class Recorder: @unchecked Sendable {
    private let engine = AVAudioEngine()
    private let file: URL?
    private let samples = Samples()

    init() { file = nil }
    init(file: URL) { self.file = file }

    func start() async throws {
        if let file {
            samples.append(try AudioConverter(sampleRate: 16_000).resampleAudioFile(file))
            return
        }
        guard await AVAudioApplication.requestRecordPermission() else {
            throw DictationError("Wheel may not use the microphone. Allow it in System Settings › Privacy & Security › Microphone.")
        }
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.record, mode: .measurement)
        try session.setActive(true)
        #endif
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0 else { throw DictationError("No microphone is available.") }
        let converter = AudioConverter(sampleRate: 16_000)
        let samples = samples
        input.installTap(onBus: 0, bufferSize: 4096, format: format) { buffer, _ in
            if let converted = try? converter.resampleBuffer(buffer) { samples.append(converted) }
        }
        engine.prepare()
        try engine.start()
    }

    /// Stop the microphone and hand back everything it heard.
    @discardableResult
    func stop() -> [Float] {
        if file == nil {
            // Also after a start that failed part way: the tap and the session may be set up with the engine stopped.
            engine.inputNode.removeTap(onBus: 0)
            if engine.isRunning { engine.stop() }
            #if os(iOS)
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
            #endif
        }
        return samples.take()
    }
}

/// Samples the audio thread appends and the main actor takes.
private final class Samples: @unchecked Sendable {
    private let lock = NSLock()
    private var floats: [Float] = []
    func append(_ more: [Float]) { lock.withLock { floats += more } }
    func take() -> [Float] { lock.withLock { defer { floats = [] }; return floats } }
}

#if DEBUG
extension Dictation {
    /// A check's stand-in for the microphone: show that `owner` is recording, so a screenshot reaches the state.
    public func demoRecording(for owner: String) {
        self.owner = owner
        phase = .recording
    }
}
#endif
#endif
