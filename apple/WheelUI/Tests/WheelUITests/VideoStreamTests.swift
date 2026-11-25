// VideoStreams.swift is macOS-only.
#if os(macOS)
import AVFoundation
import CoreMedia
import Foundation
import Testing
@testable import WheelUI

/// The gallery's fixture clip: 2 s of 320x180 H.264 at 30 fps, Annex-B.
private let clip = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    .deletingLastPathComponent().appendingPathComponent("plugins/gallery/demo.h264")

struct VideoStreamTests {
    @Test func volumeIsTheGainClampedAndMutedIsSilence() {
        #expect(PaneVideoStreamView.gain(volume: nil, muted: nil) == 1)
        #expect(PaneVideoStreamView.gain(volume: 0.25, muted: false) == 0.25)
        #expect(PaneVideoStreamView.gain(volume: 0.25, muted: true) == 0)
        #expect(PaneVideoStreamView.gain(volume: 3, muted: nil) == 1)
        #expect(PaneVideoStreamView.gain(volume: -1, muted: nil) == 0)
    }

    @Test func aFittedPictureLetterboxesAndAFillingOneCrops() {
        let video = CGSize(width: 1920, height: 1080)
        let wide = CGRect(x: 0, y: 0, width: 400, height: 100)
        let r = { (rect: CGRect) in [rect.minX, rect.minY, rect.width, rect.height].map { ($0 * 10).rounded() / 10 } }
        #expect(r(StreamView.videoRect(video, in: wide, fills: false)) == [111.1, 0, 177.8, 100])
        #expect(r(StreamView.videoRect(video, in: wide, fills: true)) == [0, -62.5, 400, 225])
    }

    @Test func aReaderKeepsACutFrameUntilTheRestArrives() throws {
        let video = StreamFrame.frame(type: 2, payload: Data([0, 0, 0, 0, 0, 0, 0x01, 0x02, 0, 0, 0, 1, 0x65, 0xAA, 0, 0, 1, 0x41, 0xBB]))
        let audio = StreamFrame.frame(type: 3, payload: Data([0, 0, 0xBB, 0x80, 2, 1, 0, 2, 0]))
        let bytes = video + audio + StreamFrame.frame(type: 99, payload: Data([7]))
        var reader = StreamFrameReader()
        var got: [StreamFrame] = []
        for i in stride(from: 0, to: bytes.count, by: 3) {
            got += try reader.push(bytes.subdata(in: i..<min(i + 3, bytes.count)))
        }
        #expect(got == [
            .video(time: 0x0102, units: [Data([0x65, 0xAA]), Data([0x41, 0xBB])]),
            .audio(rate: 48000, channels: 2, pcm: Data([1, 0, 2, 0])),
            .unknown(type: 99),
        ])
    }

    @Test func aBrokenFrameIsRefused() {
        var reader = StreamFrameReader()
        #expect(throws: StreamFrameError.self) { try reader.push(Data([2, 0xFF, 0xFF, 0xFF, 0xFF])) }
        var other = StreamFrameReader()
        #expect(throws: StreamFrameError.self) { try other.push(StreamFrame.frame(type: 1, payload: Data([9, 0, 0, 1, 0x67]))) }
        var short = StreamFrameReader()
        #expect(throws: StreamFrameError.self) { try short.push(StreamFrame.frame(type: 2, payload: Data([0, 1]))) }
    }

    @Test func aFourByteStartCodeLeavesNoZeroOnTheUnitBefore() {
        let units = StreamFrame.nalUnits(Data([0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 1, 0x65]))
        #expect(units == [Data([0x67, 0x42]), Data([0x68, 0xCE]), Data([0x65])])
    }

    @Test func anInputFrameIsItsTypeLengthAndJSON() throws {
        let bytes = StreamFrame.input(.key, ["kind": "down", "code": 0])
        #expect(bytes.first == 16)
        let length = bytes[1..<5].reduce(0) { $0 << 8 | Int($1) }
        #expect(length == bytes.count - 5)
        let body = try JSONSerialization.jsonObject(with: bytes.subdata(in: 5..<bytes.count)) as? [String: Any]
        #expect(body?["kind"] as? String == "down")
    }

    #if os(macOS)
    @Test func theAudioRingKeepsTheNewestAndPlaysSilenceWhenEmpty() {
        let ring = StreamAudioRing()
        ring.push([1, -1, 2, -2, 3, -3], keep: 4)
        let left = UnsafeMutablePointer<Float>.allocate(capacity: 3), right = UnsafeMutablePointer<Float>.allocate(capacity: 3)
        defer { left.deallocate(); right.deallocate() }
        let list = AudioBufferList.allocate(maximumBuffers: 2)
        defer { free(list.unsafeMutablePointer) }
        list[0] = AudioBuffer(mNumberChannels: 1, mDataByteSize: 12, mData: left)
        list[1] = AudioBuffer(mNumberChannels: 1, mDataByteSize: 12, mData: right)
        ring.pull(into: list, frames: 3, channels: 2)
        #expect(Array(UnsafeBufferPointer(start: left, count: 3)) == [2, 3, 0])
        #expect(Array(UnsafeBufferPointer(start: right, count: 3)) == [-2, -3, 0])
    }
    #endif

    @Test func videoToolboxDecodesEveryFrameOfTheClip() async throws {
        let units = StreamFrame.nalUnits(try Data(contentsOf: clip))
        let params = units.filter { [7, 8].contains($0[0] & 0x1F) }
        var frames: [[Data]] = []
        var pending: [Data] = []
        for unit in units where ![7, 8].contains(unit[0] & 0x1F) {
            pending.append(unit)
            if [1, 5].contains(unit[0] & 0x1F) { frames.append(pending); pending = [] }
        }
        #expect(frames.count == 60)
        let decoded = StreamStats()
        let decoder = StreamDecoder { sample in
            if CMSampleBufferGetImageBuffer(sample) != nil { decoded.add("frames") }
        }
        #expect(decoder.parameterSets(codec: .h264, units: Array(params.prefix(2))))
        #expect(decoder.dimensions.map { [$0.width, $0.height] } == [320, 180])
        for (i, units) in frames.enumerated() { decoder.decode(time: UInt64(i) * 33_333, units: units) }
        for _ in 0..<100 where decoded["frames"] < frames.count { try await Task.sleep(for: .milliseconds(20)) }
        #expect(decoder.lastError == nil)
        #expect(decoded["frames"] == frames.count)
    }

    @Test func videoToolboxDecodesAnHEVCClip() async throws {
        let units = StreamFrame.nalUnits(try Data(contentsOf: hevcClip))
        let kind = { (u: Data) in Int((u[0] >> 1) & 0x3F) }
        // VPS, SPS and PPS, the first of each.
        let params = [32, 33, 34].compactMap { t in units.first { kind($0) == t } }
        var frames: [[Data]] = []
        var pending: [Data] = []
        for unit in units where !(32...34).contains(kind(unit)) {
            pending.append(unit)
            if kind(unit) < 32 { frames.append(pending); pending = [] }
        }
        #expect(params.count == 3 && frames.count == 30)
        let decoded = StreamStats()
        let decoder = StreamDecoder { sample in
            if CMSampleBufferGetImageBuffer(sample) != nil { decoded.add("frames") }
        }
        #expect(decoder.parameterSets(codec: .hevc, units: params))
        #expect(decoder.dimensions.map { [$0.width, $0.height] } == [320, 180])
        for (i, units) in frames.enumerated() { decoder.decode(time: UInt64(i) * 33_333, units: units) }
        for _ in 0..<100 where decoded["frames"] < frames.count { try await Task.sleep(for: .milliseconds(20)) }
        #expect(decoder.lastError == nil)
        #expect(decoded["frames"] == frames.count)
    }
}

/// 1 s of 320x180 HEVC at 30 fps, Annex-B.
private let hevcClip = clip.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    .appendingPathComponent("tools/fixtures/streams/demo.hevc")
#endif
