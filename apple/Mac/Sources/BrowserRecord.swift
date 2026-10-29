import AppKit
import AVFoundation
import WebKit
import WheelKit

/// `inspect_pane` `record`: `wheel pane record` films a browser pane. The web view's own snapshots
/// (WebKit draws the page, as for `pane shot`) are taken back to back for the requested seconds and
/// written to an H.264 QuickTime movie with AVFoundation, each frame stamped with the time it was
/// taken, so no Screen Recording permission is involved and the movie plays at the real speed.
extension AppModel {
    func recordPane(id: UInt64, pane: String, web: WKWebView, seconds: Double) {
        Log.info("record \(id): \(pane) for \(seconds)s")
        Task { @MainActor [weak self] in
            do {
                let film = try await Self.film(web, seconds: seconds)
                Log.info("record \(id): \(pane) \(film.frames) frames in \(film.millis) ms, \(film.mov.count) bytes")
                self?.reply(.recorded(id: id, mov: film.mov, frames: film.frames, millis: film.millis))
            } catch {
                self?.refuseRender(id, pane, "recording failed: \(error.localizedDescription)")
            }
        }
    }

    private struct Film {
        var mov: Data
        var frames: UInt64
        var millis: UInt64
    }

    private struct RecordError: LocalizedError {
        var errorDescription: String?
    }

    private static func snapshot(_ web: WKWebView) async throws -> CGImage {
        let configuration = WKSnapshotConfiguration()
        configuration.snapshotWidth = NSNumber(value: Double(web.bounds.width))
        let image: NSImage = try await withCheckedThrowingContinuation { continuation in
            web.takeSnapshot(with: configuration) { image, error in
                if let image { continuation.resume(returning: image) }
                else { continuation.resume(throwing: RecordError(errorDescription: "WebKit drew no image: \(error?.localizedDescription ?? "no error")")) }
            }
        }
        guard let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            throw RecordError(errorDescription: "the snapshot has no bitmap")
        }
        return cg
    }

    private static func film(_ web: WKWebView, seconds: Double) async throws -> Film {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("wheel-record-\(UUID().uuidString).mov")
        defer { try? FileManager.default.removeItem(at: url) }
        let first = try await snapshot(web)
        let width = first.width & ~1, height = first.height & ~1
        guard width > 0, height > 0 else { throw RecordError(errorDescription: "the pane has no size") }
        let writer = try AVAssetWriter(outputURL: url, fileType: .mov)
        let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: width, AVVideoHeightKey: height,
        ])
        input.expectsMediaDataInRealTime = true
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input, sourcePixelBufferAttributes: [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: width, kCVPixelBufferHeightKey as String: height,
        ])
        writer.add(input)
        guard writer.startWriting() else { throw RecordError(errorDescription: writer.error?.localizedDescription ?? "the movie file could not be started") }
        writer.startSession(atSourceTime: .zero)

        // A throw below leaves the writer mid-file; cancelling stops it before the file goes.
        var finished = false
        defer { if !finished { writer.cancelWriting() } }
        let timescale: Int32 = 1000
        let start = DispatchTime.now().uptimeNanoseconds
        var frames: UInt64 = 0
        var lastMillis: Int64 = -1
        var image = first
        while true {
            let millis = Int64((DispatchTime.now().uptimeNanoseconds - start) / 1_000_000)
            if Double(millis) >= seconds * 1000 { break }
            // A frame that arrives in the same millisecond as the last would not move the clock.
            if millis > lastMillis, input.isReadyForMoreMediaData, let buffer = pixelBuffer(image, width: width, height: height, pool: adaptor.pixelBufferPool) {
                guard adaptor.append(buffer, withPresentationTime: CMTime(value: millis, timescale: timescale)) else {
                    throw RecordError(errorDescription: writer.error?.localizedDescription ?? "a frame could not be written")
                }
                frames += 1
                lastMillis = millis
            }
            image = try await snapshot(web)
        }
        let total = Int64((DispatchTime.now().uptimeNanoseconds - start) / 1_000_000)
        input.markAsFinished()
        writer.endSession(atSourceTime: CMTime(value: max(total, lastMillis + 1), timescale: timescale))
        await writer.finishWriting()
        finished = true
        guard writer.status == .completed else { throw RecordError(errorDescription: writer.error?.localizedDescription ?? "the movie file could not be finished") }
        return Film(mov: try Data(contentsOf: url), frames: frames, millis: UInt64(total))
    }

    private static func pixelBuffer(_ image: CGImage, width: Int, height: Int, pool: CVPixelBufferPool?) -> CVPixelBuffer? {
        var buffer: CVPixelBuffer?
        if let pool { CVPixelBufferPoolCreatePixelBuffer(nil, pool, &buffer) }
        else { CVPixelBufferCreate(nil, width, height, kCVPixelFormatType_32BGRA, nil, &buffer) }
        guard let buffer else { return nil }
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        guard let context = CGContext(
            data: CVPixelBufferGetBaseAddress(buffer), width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: CVPixelBufferGetBytesPerRow(buffer), space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        ) else { return nil }
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return buffer
    }
}
