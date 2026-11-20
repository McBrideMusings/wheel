import CoreMedia
import Foundation
import VideoToolbox

/// Decodes one stream's access units with VideoToolbox (ADR-0053) and hands each decoded frame on as a sample buffer
/// ready to display at once. Used from the stream's read queue only.
final class StreamDecoder {
    /// Takes each decoded frame, on VideoToolbox's thread.
    private let onFrame: @Sendable (CMSampleBuffer) -> Void
    private var format: CMVideoFormatDescription?
    private var codec: StreamCodec = .h264
    private var session: VTDecompressionSession?
    private(set) var lastError: String?

    init(onFrame: @escaping @Sendable (CMSampleBuffer) -> Void) { self.onFrame = onFrame }

    deinit { if let session { VTDecompressionSessionInvalidate(session) } }

    /// The video's size in pixels, once parameter sets came.
    var dimensions: CMVideoDimensions? { format.map(CMVideoFormatDescriptionGetDimensions) }

    /// New parameter sets: the next frames decode with them. Returns false, with `lastError` set, when they make no format.
    @discardableResult
    func parameterSets(codec: StreamCodec, units: [Data]) -> Bool {
        var made: CMFormatDescription?
        let status = Self.withPointers(units) { pointers, sizes in
            switch codec {
            case .h264:
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: nil, parameterSetCount: units.count, parameterSetPointers: pointers, parameterSetSizes: sizes,
                    nalUnitHeaderLength: 4, formatDescriptionOut: &made)
            case .hevc:
                CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    allocator: nil, parameterSetCount: units.count, parameterSetPointers: pointers, parameterSetSizes: sizes,
                    nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &made)
            }
        }
        guard status == noErr, let made else {
            lastError = "the \(codec == .h264 ? "H.264" : "HEVC") parameter sets make no format (\(status))"
            return false
        }
        // The same sets again (a backend repeats them before each key frame) keep the decoder.
        if session != nil, let format, CMFormatDescriptionEqual(format, otherFormatDescription: made) { return true }
        self.codec = codec
        format = made
        if let session { VTDecompressionSessionInvalidate(session) }
        session = nil
        return true
    }

    /// Decode one access unit shown at `time` microseconds. Parameter set units inside it are skipped: the stream's own
    /// parameter set frames carry them.
    func decode(time: UInt64, units: [Data]) {
        guard let format else { lastError = "a video frame came before any parameter sets"; return }
        if session == nil, !openSession(format) { return }
        guard let session else { return }
        var avcc = Data()
        for unit in units where !isParameterSet(unit) {
            withUnsafeBytes(of: UInt32(unit.count).bigEndian) { avcc.append(contentsOf: $0) }
            avcc.append(unit)
        }
        guard !avcc.isEmpty, let sample = Self.sample(avcc, format: format, time: time) else { return }
        let status = VTDecompressionSessionDecodeFrame(session, sampleBuffer: sample, flags: [._1xRealTimePlayback], infoFlagsOut: nil) {
            [onFrame] status, _, image, pts, _ in
            guard status == noErr, let image, let shown = Self.shown(image, at: pts) else { return }
            onFrame(shown)
        }
        if status == kVTInvalidSessionErr {
            VTDecompressionSessionInvalidate(session)
            self.session = nil
        }
        if status != noErr { lastError = "VideoToolbox refused a frame (\(status))" }
    }

    private func isParameterSet(_ unit: Data) -> Bool {
        guard let first = unit.first else { return true }
        switch codec {
        case .h264: return [7, 8, 9].contains(first & 0x1F)
        case .hevc: return (32...35).contains((first >> 1) & 0x3F)
        }
    }

    private func openSession(_ format: CMVideoFormatDescription) -> Bool {
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferIOSurfacePropertiesKey: [CFString: Any](),
        ]
        var made: VTDecompressionSession?
        let status = VTDecompressionSessionCreate(
            allocator: nil, formatDescription: format, decoderSpecification: nil,
            imageBufferAttributes: attributes as CFDictionary, outputCallback: nil, decompressionSessionOut: &made)
        guard status == noErr, let made else {
            lastError = "VideoToolbox opened no decoder (\(status))"
            return false
        }
        VTSessionSetProperty(made, key: kVTDecompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        session = made
        return true
    }

    private static func withPointers<R>(_ units: [Data], _ body: (UnsafePointer<UnsafePointer<UInt8>>, UnsafePointer<Int>) -> R) -> R {
        let copies = units.map { unit -> UnsafeMutablePointer<UInt8> in
            let p = UnsafeMutablePointer<UInt8>.allocate(capacity: max(unit.count, 1))
            unit.copyBytes(to: p, count: unit.count)
            return p
        }
        defer { copies.forEach { $0.deallocate() } }
        let pointers = copies.map { UnsafePointer($0) }
        let sizes = units.map(\.count)
        return pointers.withUnsafeBufferPointer { p in sizes.withUnsafeBufferPointer { s in body(p.baseAddress!, s.baseAddress!) } }
    }

    /// A compressed sample of `avcc` (length-prefixed units) at `time` microseconds.
    private static func sample(_ avcc: Data, format: CMVideoFormatDescription, time: UInt64) -> CMSampleBuffer? {
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: nil, memoryBlock: nil, blockLength: avcc.count, blockAllocator: nil, customBlockSource: nil,
            offsetToData: 0, dataLength: avcc.count, flags: kCMBlockBufferAssureMemoryNowFlag, blockBufferOut: &block) == noErr,
            let block else { return nil }
        let copied = avcc.withUnsafeBytes { CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: avcc.count) }
        guard copied == noErr else { return nil }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: CMTime(value: CMTimeValue(time), timescale: 1_000_000),
                                        decodeTimeStamp: .invalid)
        var size = avcc.count
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReady(allocator: nil, dataBuffer: block, formatDescription: format, sampleCount: 1,
                                        sampleTimingEntryCount: 1, sampleTimingArray: &timing, sampleSizeEntryCount: 1,
                                        sampleSizeArray: &size, sampleBufferOut: &sample) == noErr else { return nil }
        return sample
    }

    /// A decoded image as a sample buffer marked to show at once, since a live stream has no clock to wait for.
    private static func shown(_ image: CVImageBuffer, at pts: CMTime) -> CMSampleBuffer? {
        var format: CMVideoFormatDescription?
        guard CMVideoFormatDescriptionCreateForImageBuffer(allocator: nil, imageBuffer: image, formatDescriptionOut: &format) == noErr,
              let format else { return nil }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: pts, decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(allocator: nil, imageBuffer: image, formatDescription: format,
                                                       sampleTiming: &timing, sampleBufferOut: &sample) == noErr, let sample else { return nil }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true), CFArrayGetCount(attachments) > 0 {
            let first = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(first, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        return sample
    }
}
