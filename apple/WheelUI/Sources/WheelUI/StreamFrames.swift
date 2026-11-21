import Foundation

/// The codec a stream's parameter sets name.
public enum StreamCodec: UInt8, Sendable { case h264 = 1, hevc = 2 }

/// One frame on a stream socket (ADR-0053, `docs/plugin-runtime.md` "Video streams"). On the wire each is a type byte, a
/// 4-byte big-endian length and the payload.
public enum StreamFrame: Equatable, Sendable {
    /// Type 1: the codec byte, then the codec's parameter sets (SPS and PPS, with VPS first for HEVC) in Annex-B.
    case parameterSets(codec: StreamCodec, units: [Data])
    /// Type 2: an 8-byte big-endian time in microseconds, then one access unit in Annex-B.
    case video(time: UInt64, units: [Data])
    /// Type 3: a 4-byte big-endian sample rate, a channel count byte, then interleaved s16 little-endian PCM.
    case audio(rate: UInt32, channels: UInt8, pcm: Data)
    /// A type this client does not know, skipped.
    case unknown(type: UInt8)

    /// What a client sends: key (16), pointer (17) and controller (18) events, each a UTF-8 JSON object.
    public enum Input: UInt8, Sendable { case key = 16, pointer = 17, controller = 18 }

    /// The bytes of one input frame.
    public static func input(_ kind: Input, _ event: [String: Any]) -> Data {
        let body = (try? JSONSerialization.data(withJSONObject: event, options: [.sortedKeys])) ?? Data("{}".utf8)
        return frame(type: kind.rawValue, payload: body)
    }

    /// One frame's bytes: type, big-endian length, payload.
    public static func frame(type: UInt8, payload: Data) -> Data {
        var out = Data([type])
        withUnsafeBytes(of: UInt32(payload.count).bigEndian) { out.append(contentsOf: $0) }
        out.append(payload)
        return out
    }

    /// The NAL units of an Annex-B byte run, start codes (3 or 4 bytes) removed.
    public static func nalUnits(_ annexB: Data) -> [Data] {
        let b = [UInt8](annexB)
        var starts: [Int] = []
        var i = 0
        while i + 2 < b.count {
            if b[i] == 0, b[i + 1] == 0, b[i + 2] == 1 { starts.append(i + 3); i += 3 } else { i += 1 }
        }
        return starts.enumerated().compactMap { k, start in
            var end = k + 1 < starts.count ? starts[k + 1] - 3 : b.count
            // The zero before a 4-byte start code belongs to the code, not to this unit.
            while k + 1 < starts.count, end > start, b[end - 1] == 0 { end -= 1 }
            return end > start ? Data(b[start..<end]) : nil
        }
    }
}

/// Why a stream's bytes could not be read.
public struct StreamFrameError: Error, Equatable, CustomStringConvertible {
    public let description: String
}

/// Splits what a stream socket delivers into frames, keeping a cut frame until the rest of it arrives.
public struct StreamFrameReader {
    /// The longest payload taken; a longer length is a broken stream, not a frame to wait for.
    public static let maxPayload = 32 << 20
    private var buffer: [UInt8] = []

    public init() {}

    /// The frames the bytes read so far complete.
    public mutating func push(_ bytes: Data) throws -> [StreamFrame] {
        buffer.append(contentsOf: bytes)
        var frames: [StreamFrame] = []
        var at = 0
        while buffer.count - at >= 5 {
            let length = buffer[(at + 1)..<(at + 5)].reduce(0) { $0 << 8 | Int($1) }
            guard length <= Self.maxPayload else {
                throw StreamFrameError(description: "a frame of \(length) bytes is longer than \(Self.maxPayload)")
            }
            guard buffer.count - at - 5 >= length else { break }
            frames.append(try Self.decode(type: buffer[at], Data(buffer[(at + 5)..<(at + 5 + length)])))
            at += 5 + length
        }
        buffer.removeFirst(at)
        return frames
    }

    static func decode(type: UInt8, _ p: Data) throws -> StreamFrame {
        let b = [UInt8](p)
        switch type {
        case 1:
            guard let first = b.first, let codec = StreamCodec(rawValue: first) else {
                throw StreamFrameError(description: "parameter sets name no codec this client decodes (\(b.first.map(String.init) ?? "none"))")
            }
            return .parameterSets(codec: codec, units: StreamFrame.nalUnits(Data(b.dropFirst())))
        case 2:
            guard b.count >= 8 else { throw StreamFrameError(description: "a video frame of \(b.count) bytes has no time") }
            let time = b[0..<8].reduce(UInt64(0)) { $0 << 8 | UInt64($1) }
            return .video(time: time, units: StreamFrame.nalUnits(Data(b[8...])))
        case 3:
            guard b.count >= 5 else { throw StreamFrameError(description: "an audio frame of \(b.count) bytes has no format") }
            let rate = b[0..<4].reduce(UInt32(0)) { $0 << 8 | UInt32($1) }
            guard rate > 0, b[4] > 0 else { throw StreamFrameError(description: "audio at \(rate) Hz with \(b[4]) channels") }
            return .audio(rate: rate, channels: b[4], pcm: Data(b[5...]))
        default:
            return .unknown(type: type)
        }
    }
}
