import AVFoundation
import FluidAudio
import Foundation
import WhisperKit
#if canImport(whisper)
import whisper
#endif

public protocol Engine {
    var name: String { get }
    mutating func load() async throws
    func transcribe(_ samples: [Float]) async throws -> String
}

public struct WhisperKitEngine: Engine {
    let model: String
    let base: URL
    var pipe: WhisperKit?
    public init(model: String, base: URL) { self.model = model; self.base = base }
    public var name: String { "whisperkit/\(model)" }
    public mutating func load() async throws {
        pipe = try await WhisperKit(WhisperKitConfig(model: model, downloadBase: base, verbose: false, logLevel: .none, prewarm: true))
    }
    public func transcribe(_ samples: [Float]) async throws -> String {
        let opts = DecodingOptions(language: "en", temperature: 0, withoutTimestamps: true)
        let r = try await pipe!.transcribe(audioArray: samples, decodeOptions: opts)
        return r.map(\.text).joined(separator: " ")
    }
}

public struct FluidEngine: Engine {
    let version: AsrModelVersion
    let label: String
    var asr: AsrManager?
    public init(version: AsrModelVersion, label: String) { self.version = version; self.label = label }
    public var name: String { "fluidaudio/\(label)" }
    public mutating func load() async throws {
        let models = try await AsrModels.downloadAndLoad(version: version)
        let m = AsrManager(config: .default)
        try await m.loadModels(models)
        asr = m
    }
    public func transcribe(_ samples: [Float]) async throws -> String {
        var state = TdtDecoderState.make(decoderLayers: await asr!.decoderLayerCount)
        return try await asr!.transcribe(samples, decoderState: &state).text
    }
}

#if canImport(whisper)
public final class WhisperCppEngine: Engine, @unchecked Sendable {
    let path: String
    var ctx: OpaquePointer?
    public init(path: String) { self.path = path }
    public var name: String { "whisper.cpp/\((path as NSString).lastPathComponent)" }
    public func load() async throws {
        var p = whisper_context_default_params()
        p.use_gpu = true
        p.flash_attn = true
        guard let c = whisper_init_from_file_with_params(path, p) else { throw BenchError.load(path) }
        ctx = c
    }
    public func transcribe(_ samples: [Float]) async throws -> String {
        var p = whisper_full_default_params(WHISPER_SAMPLING_GREEDY)
        p.print_progress = false; p.print_realtime = false; p.print_timestamps = false; p.no_timestamps = true
        p.n_threads = Int32(max(1, min(8, ProcessInfo.processInfo.activeProcessorCount - 2)))
        let lang = strdup("en"); defer { free(lang) }
        p.language = UnsafePointer(lang)
        let rc = samples.withUnsafeBufferPointer { whisper_full(ctx, p, $0.baseAddress, Int32($0.count)) }
        guard rc == 0 else { throw BenchError.run(Int(rc)) }
        return (0..<whisper_full_n_segments(ctx)).map { String(cString: whisper_full_get_segment_text(ctx, $0)) }.joined()
    }
}

#endif

public enum BenchError: Error { case load(String), run(Int), badEngine(String) }

public func makeEngine(_ spec: String, modelsDir: URL, cacheDir: URL) throws -> any Engine {
    let parts = spec.split(separator: ":", maxSplits: 1).map(String.init)
    switch (parts.first, parts.count > 1 ? parts[1] : "") {
    case ("whisperkit", let m): return WhisperKitEngine(model: m, base: cacheDir.appendingPathComponent("whisperkit"))
    case ("fluid", "v2"): return FluidEngine(version: .v2, label: "parakeet-tdt-0.6b-v2")
    case ("fluid", "v3"): return FluidEngine(version: .v3, label: "parakeet-tdt-0.6b-v3")
    #if canImport(whisper)
    case ("whispercpp", let f): return WhisperCppEngine(path: modelsDir.appendingPathComponent(f).path)
    #endif
    default: throw BenchError.badEngine(spec)
    }
}

/// Reads any audio file as 16 kHz mono Float32.
public func loadSamples(_ url: URL) throws -> [Float] {
    let file = try AVAudioFile(forReading: url)
    let fmt = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false)!
    let conv = AVAudioConverter(from: file.processingFormat, to: fmt)!
    let inBuf = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length))!
    try file.read(into: inBuf)
    let outBuf = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: AVAudioFrameCount(Double(file.length) * 16000 / file.processingFormat.sampleRate) + 1024)!
    var fed = false
    var err: NSError?
    conv.convert(to: outBuf, error: &err) { _, status in
        if fed { status.pointee = .endOfStream; return nil }
        fed = true; status.pointee = .haveData; return inBuf
    }
    if let err { throw err }
    return Array(UnsafeBufferPointer(start: outBuf.floatChannelData![0], count: Int(outBuf.frameLength)))
}

/// Current and peak physical footprint of this process, in MB.
public func footprintMB() -> (now: Double, peak: Double) {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
    let kr = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count) }
    }
    guard kr == KERN_SUCCESS else { return (0, 0) }
    return (Double(info.phys_footprint) / 1_048_576, Double(info.ledger_phys_footprint_peak) / 1_048_576)
}

/// Loads one engine, warms it on one clip, then transcribes every clip; writes one JSON object.
public func run(spec: String, clipsDir: URL, modelsDir: URL, cacheDir: URL? = nil, out: URL, log: (String) -> Void) async throws {
    var engine = try makeEngine(spec, modelsDir: modelsDir, cacheDir: cacheDir ?? modelsDir)
    let base = footprintMB().now
    let t0 = Date()
    try await engine.load()
    let loadS = Date().timeIntervalSince(t0)
    log("\(engine.name) loaded in \(String(format: "%.2f", loadS))s")
    var clips: [[String: Any]] = []
    let sets = ["clean", "noisy"]
    let first = clipsDir.appendingPathComponent("clean/u01.wav")
    _ = try await engine.transcribe(try loadSamples(first))
    for set in sets {
        let dir = clipsDir.appendingPathComponent(set)
        let files = try FileManager.default.contentsOfDirectory(atPath: dir.path).filter { $0.hasSuffix(".wav") }.sorted()
        for f in files {
            let s = try loadSamples(dir.appendingPathComponent(f))
            let t = Date()
            let text = try await engine.transcribe(s)
            let dt = Date().timeIntervalSince(t)
            clips.append(["set": set, "id": String(f.dropLast(4)), "audio_s": Double(s.count) / 16000, "latency_s": dt, "text": text])
            log("\(set)/\(f) \(String(format: "%.3f", dt))s \(text)")
        }
    }
    let fp = footprintMB()
    #if os(iOS)
    let platform = "iPhone"
    #else
    let platform = "Mac"
    #endif
    let obj: [String: Any] = ["platform": platform, "engine": engine.name, "spec": spec, "load_s": loadS, "base_mb": base, "end_mb": fp.now, "peak_mb": fp.peak, "clips": clips]
    try JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted, .sortedKeys]).write(to: out)
    log("wrote \(out.path) peak \(Int(fp.peak)) MB")
}
