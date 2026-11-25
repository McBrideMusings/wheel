#if os(iOS)
import AVFoundation
import Foundation
import Observation
internal import FluidAudio

/// One model Settings › Speech lists: what it is, how big its download is, and where it comes from
/// (ADR-0013). The sizes are the files FluidAudio fetches, summed from each Hugging Face repository.
public struct SpeechModel: Identifiable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let size: String
    public let source: String
    public let role: String
}

/// Settings › Speech on the phone (wheel-7w13): which Parakeet `Dictation` and `Voice` use, kept
/// across launches, and each model's download, redownload and removal. FluidAudio fetches and lays
/// out the files; a model dictation or voice mode is using stays while it runs.
@MainActor
@Observable
public final class SpeechModels {
    public static let shared = SpeechModels()

    public nonisolated static let v3 = SpeechModel(id: "v3", name: "Parakeet TDT 0.6b v3", size: "483.1 MB",
                                       source: "huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml", role: "Speech to text, multilingual")
    public nonisolated static let v2 = SpeechModel(id: "v2", name: "Parakeet TDT 0.6b v2", size: "464.4 MB",
                                       source: "huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml", role: "Speech to text, English only")
    public nonisolated static let vad = SpeechModel(id: "vad", name: "Silero VAD v6", size: "1.1 MB",
                                        source: "huggingface.co/FluidInference/silero-vad-coreml", role: "Hears when you stop speaking, for voice mode")
    public nonisolated static let speechToText = [v3, v2]
    public nonisolated static let all = [v3, v2, vad]

    /// The `UserDefaults` key holding the chosen speech-to-text model's id.
    nonisolated static let choiceKey = "speech.stt"

    /// The speech-to-text model chosen, read where `Dictation` resolves its folder off the main actor.
    nonisolated static var chosenModel: SpeechModel {
        UserDefaults.standard.string(forKey: choiceKey) == v2.id ? v2 : v3
    }

    nonisolated static func version(of model: SpeechModel) -> AsrModelVersion { model.id == v2.id ? .v2 : .v3 }

    public private(set) var chosen: SpeechModel = SpeechModels.chosenModel
    public private(set) var downloading: SpeechModel?
    public private(set) var progress: Double = 0
    /// A model picked while not on the phone: its download is offered, and it becomes the choice
    /// once that lands.
    public private(set) var offered: SpeechModel?
    public private(set) var problem: String?
    /// Grows when a model lands or goes, so a view showing what is on disk reads it again.
    public private(set) var revision = 0
    @ObservationIgnored private var task: Task<Void, Never>?
    @ObservationIgnored private let preview = AVSpeechSynthesizer()

    /// The reply Settings speaks so the person hears voice mode's voice before using it.
    public nonisolated static let sampleReply = "I ran the tests in the daemon. Two failed: the lease test timed out, and the voice line test expected a newline. Want me to fix them?"

    private init() {}

    public nonisolated static func folder(of model: SpeechModel) -> URL {
        model == vad ? Voice.vadFolder : Dictation.modelFolder(for: version(of: model))
    }

    public func onDisk(_ model: SpeechModel) -> Bool {
        _ = revision
        if model == Self.vad { return Voice.shared.vadOnDisk }
        let version = Self.version(of: model)
        return AsrModels.modelsExist(at: Dictation.modelFolder(for: version), version: version)
    }

    /// Why `model` cannot be removed or replaced now, or nil: dictation recording or transcribing holds
    /// the chosen model, voice mode holds it and the VAD.
    public func inUse(_ model: SpeechModel) -> String? {
        let voicing = ![.off, .asking, .downloading].contains(Voice.shared.phase)
        if voicing && (model == chosen || model == Self.vad) { return "Voice mode is using it." }
        let dictating = Dictation.shared.phase == .recording || Dictation.shared.phase == .transcribing
        if dictating && model == chosen { return "Dictation is using it." }
        return nil
    }

    /// A download runs, here or from the composer or voice mode: Settings starts none beside it.
    public var busy: Bool {
        if downloading != nil || Voice.shared.phase == .downloading { return true }
        if case .downloading = Dictation.shared.phase { return true }
        return false
    }

    /// Download `model`, or download it again over what is there.
    public func download(_ model: SpeechModel) {
        problem = nil
        guard !busy else { problem = "Another download is running."; return }
        if let why = inUse(model) { problem = "\(model.name) cannot be replaced now: \(why)"; return }
        let again = onDisk(model)
        downloading = model
        progress = 0
        let folder = Self.folder(of: model)
        UILog.info("speech settings: downloading \(model.name) from \(model.source) into \(folder.path)\(again ? " again" : "")")
        // A staging folder beside the models, swapped in only once whole: a stopped download or
        // redownload never touches the model already on the phone.
        let staging = folder.deletingLastPathComponent().appendingPathComponent(".staging-\(model.id)")
        task = Task {
            let fm = FileManager.default
            do {
                try? fm.removeItem(at: staging)
                if model == Self.vad {
                    _ = try await VadManager(modelDirectory: staging)
                } else {
                    _ = try await AsrModels.download(to: staging.appendingPathComponent(folder.lastPathComponent), version: Self.version(of: model)) { p in
                        Task { @MainActor in
                            if SpeechModels.shared.downloading == model { SpeechModels.shared.progress = p.fractionCompleted }
                        }
                    }
                }
                try Task.checkCancellation()
                // FluidAudio picks the layout under the folder it is given; the model is the folder named as ours.
                guard let fetched = Self.find(folder.lastPathComponent, in: staging) else {
                    let held = (FileManager.default.subpaths(atPath: staging.path) ?? []).prefix(12).joined(separator: ", ")
                    throw DictationError("\(model.name) downloaded, but no \(folder.lastPathComponent) folder came with it; \(staging.path) holds [\(held)].")
                }
                forget(model)
                try fm.createDirectory(at: folder.deletingLastPathComponent(), withIntermediateDirectories: true)
                if fm.fileExists(atPath: folder.path) { try fm.removeItem(at: folder) }
                try fm.moveItem(at: fetched, to: folder)
                try? fm.removeItem(at: staging)
                landed(model)
            } catch {
                try? fm.removeItem(at: staging)
                stopped(model, error is CancellationError || Task.isCancelled ? nil : error.localizedDescription)
            }
        }
    }

    /// The first folder named `name` under `root`.
    nonisolated static func find(_ name: String, in root: URL) -> URL? {
        let all = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isDirectoryKey])
        while let url = all?.nextObject() as? URL {
            if url.lastPathComponent == name, (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true { return url }
        }
        return nil
    }

    /// Speak the sample reply with the synthesizer and default utterance voice mode uses.
    public func hearVoice() {
        guard Voice.shared.phase == .off else { return }
        UILog.info("speech settings: speaking the sample reply")
        preview.stopSpeaking(at: .immediate)
        preview.speak(AVSpeechUtterance(string: Self.sampleReply))
    }

    public func cancel() {
        offered = nil
        guard let downloading else { return }
        UILog.info("speech settings: cancelling the download of \(downloading.name)")
        task?.cancel()
    }

    public func remove(_ model: SpeechModel) {
        problem = nil
        if downloading == model { problem = "\(model.name) is downloading."; return }
        if let why = inUse(model) { problem = "\(model.name) cannot be removed now: \(why)"; return }
        forget(model)
        let folder = Self.folder(of: model)
        do {
            if FileManager.default.fileExists(atPath: folder.path) { try FileManager.default.removeItem(at: folder) }
            UILog.info("speech settings: removed \(model.name) from \(folder.path)")
        } catch {
            problem = "Could not remove \(model.name): \(error.localizedDescription)"
            UILog.info("speech settings: \(problem ?? "")")
        }
        revision += 1
    }

    /// Pick the speech-to-text model: one on the phone becomes the choice; one that is not is offered
    /// for download first.
    public func choose(_ model: SpeechModel) {
        problem = nil
        guard model != chosen, model != Self.vad, downloading == nil else { return }
        if let why = inUse(chosen) { problem = "The model cannot change now: \(why)"; return }
        if onDisk(model) {
            offered = nil
            save(model)
        } else {
            offered = model
        }
    }

    private func landed(_ model: SpeechModel) {
        UILog.info("speech settings: \(model.name) downloaded")
        downloading = nil
        task = nil
        revision += 1
        if offered == model {
            offered = nil
            // Dictation or voice mode started on the chosen model meanwhile: it keeps it.
            if let why = inUse(chosen) { problem = "\(model.name) downloaded; pick it again once it is free. \(why)"; return }
            save(model)
        }
    }

    private func stopped(_ model: SpeechModel, _ why: String?) {
        UILog.info("speech settings: the download of \(model.name) stopped: \(why ?? "cancelled")")
        downloading = nil
        task = nil
        offered = nil
        revision += 1
        if let why { problem = "\(model.name) did not download: \(why)" }
    }

    private func save(_ model: SpeechModel) {
        forget(chosen)
        chosen = model
        UserDefaults.standard.set(model.id, forKey: Self.choiceKey)
        UILog.info("speech settings: speech to text is \(model.name)")
    }

    /// Let go of `model` loaded in memory: the next use loads what is chosen then.
    private func forget(_ model: SpeechModel) {
        if model == Self.vad { Voice.shared.forgetVad() } else if model == chosen { Dictation.shared.forgetModel() }
    }
}
#endif
