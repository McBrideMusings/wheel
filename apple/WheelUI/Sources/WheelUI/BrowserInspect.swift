#if os(macOS)
import AppKit
import AVFoundation
import WebKit

/// What `inspect_pane` runs in a browser pane's web view, for every Mac client that hosts one:
/// JavaScript (`wheel pane eval`), its HTML (`wheel pane dom`), one element drawn as a PNG
/// (`wheel pane shot --selector`) and a film of the page (`wheel pane record`). Each answers
/// with the text or bytes the daemon relays, or the message it refuses with. The web view's own
/// snapshots draw the page, so no Screen Recording permission is involved.
@MainActor
public enum BrowserInspect {
    /// What one `inspect_pane` produced: the reply it becomes, or why it was refused.
    public enum Outcome: Sendable {
        /// `inspected`: `eval` or `dom` text.
        case text(String)
        /// `rendered`: a `shot` of one element.
        case png(Data)
        /// `recorded`: a `record`.
        case film(Film)
        /// `render_refused`, with its message.
        case refused(String)
    }

    /// `action` (`eval`, `dom`, `shot` or `record`, see `pane_inspect`) with `arg`, run in `web`.
    public static func run(_ action: String, arg: String?, in web: WKWebView) async -> Outcome {
        switch action {
        case "eval":
            switch await eval(web, arg ?? "") {
            case let .success(text): return .text(text)
            case let .failure(refusal): return .refused(refusal.message)
            }
        case "dom":
            switch await dom(web, selector: arg) {
            case let .success(text): return .text(text)
            case let .failure(refusal): return .refused(refusal.message)
            }
        case "shot":
            switch await shotElement(web, selector: arg ?? "") {
            case let .success(png): return .png(png)
            case let .failure(refusal): return .refused(refusal.message)
            }
        case "record":
            guard let seconds = arg.flatMap(Double.init), seconds > 0 else { return .refused("record needs a number of seconds") }
            do { return .film(try await film(web, seconds: seconds)) } catch { return .refused("recording failed: \(error.localizedDescription)") }
        default:
            return .refused("unknown inspect op \(action)")
        }
    }

    /// `script` run in the page; the result as JSON text, `undefined` as `null`.
    public static func eval(_ web: WKWebView, _ script: String) async -> Result<String, Refusal> {
        let outcome = await evaluate(web, script)
        switch outcome {
        case let .failure(message): return .failure(message)
        case let .success(value):
            guard let text = json(value) else { return .failure("the result is not JSON") }
            return .success(text)
        }
    }

    /// The document's HTML, or the outer HTML of each element `selector` matches, one per line.
    public static func dom(_ web: WKWebView, selector: String?) async -> Result<String, Refusal> {
        let source = selector.map { literal($0) }
        let script = """
        (function (selector) {
          if (selector === null) return "ok:" + document.documentElement.outerHTML;
          var found = document.querySelectorAll(selector);
          if (!found.length) return "none";
          return "ok:" + Array.prototype.map.call(found, function (e) { return e.outerHTML; }).join("\\n");
        })(\(source ?? "null"))
        """
        switch await evaluate(web, script) {
        case let .failure(message): return .failure(message)
        case let .success(value):
            guard let reply = value as? String else { return .failure("the page returned no HTML") }
            if reply.hasPrefix("ok:") { return .success(String(reply.dropFirst(3))) }
            return .failure("no element matches \(selector ?? "")")
        }
    }

    /// The first element `selector` matches, scrolled into view and cropped out of the web view's
    /// own snapshot, as a PNG.
    public static func shotElement(_ web: WKWebView, selector: String) async -> Result<Data, Refusal> {
        let script = """
        (function (selector) {
          var el = document.querySelector(selector);
          if (!el) return "none";
          el.scrollIntoView({ block: "nearest", inline: "nearest" });
          var r = el.getBoundingClientRect();
          return JSON.stringify([r.x, r.y, r.width, r.height]);
        })(\(literal(selector)))
        """
        let value: Any?
        switch await evaluate(web, script) {
        case let .failure(message): return .failure(message)
        case let .success(found): value = found
        }
        guard let text = value as? String, text != "none" else { return .failure("no element matches \(selector)") }
        guard let box = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [Double], box.count == 4 else {
            return .failure("the page gave no box for \(selector): \(text)")
        }
        let rect = CGRect(x: box[0], y: box[1], width: box[2], height: box[3]).intersection(web.bounds)
        guard !rect.isNull, rect.width >= 1, rect.height >= 1 else { return .failure("\(selector) has no visible area in the pane") }
        let configuration = WKSnapshotConfiguration()
        configuration.rect = rect
        return await snapshotPNG(web, configuration)
    }

    /// The page as the web view draws it, the part in view, or with `full` the whole document as
    /// one tall image.
    public static func shotPage(_ web: WKWebView, full: Bool) async -> Result<NSImage, Refusal> {
        var configuration: WKSnapshotConfiguration?
        if full, case let .success(value) = await evaluate(web, "[Math.max(document.documentElement.scrollWidth, document.body ? document.body.scrollWidth : 0), Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0)]"),
           let size = (value as? [Double]).flatMap({ $0.count == 2 ? CGSize(width: $0[0], height: $0[1]) : nil }), size.width > 0, size.height > 0 {
            let whole = WKSnapshotConfiguration()
            whole.rect = CGRect(origin: .zero, size: size)
            whole.snapshotWidth = NSNumber(value: Double(size.width))
            configuration = whole
        }
        return await snapshotImage(web, configuration)
    }

    public struct Film: Sendable {
        public var mov: Data
        public var frames: UInt64
        public var millis: UInt64
    }

    /// The web view's own snapshots taken back to back for `seconds`, written to an H.264
    /// QuickTime movie, each frame stamped with the time it was taken, so the movie plays at the
    /// real speed.
    public static func film(_ web: WKWebView, seconds: Double) async throws -> Film {
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

    /// Why a request could not be served, as the daemon relays it.
    public struct Refusal: Error, ExpressibleByStringInterpolation, Sendable {
        public let message: String
        public init(_ message: String) { self.message = message }
        public init(stringLiteral value: String) { message = value }
    }

    // MARK: pieces

    private struct RecordError: LocalizedError {
        var errorDescription: String?
    }

    /// A script's result crossing the continuation: WebKit hands it over on the main thread.
    private struct Evaluated: @unchecked Sendable { let value: Any? }

    /// `script` evaluated; a thrown error as the page worded it.
    private static func evaluate(_ web: WKWebView, _ script: String) async -> Result<Any?, Refusal> {
        let outcome: Result<Evaluated, Refusal> = await withCheckedContinuation { continuation in
            web.evaluateJavaScript(script) { value, error in
                if let error { continuation.resume(returning: .failure(Refusal(scriptMessage(error)))) } else { continuation.resume(returning: .success(Evaluated(value: value))) }
            }
        }
        return outcome.map(\.value)
    }

    private static func snapshotImage(_ web: WKWebView, _ configuration: WKSnapshotConfiguration?) async -> Result<NSImage, Refusal> {
        await withCheckedContinuation { continuation in
            web.takeSnapshot(with: configuration) { image, error in
                if let image { continuation.resume(returning: .success(image)) }
                else { continuation.resume(returning: .failure("WebKit drew no image: \(error?.localizedDescription ?? "no error")")) }
            }
        }
    }

    private static func snapshotPNG(_ web: WKWebView, _ configuration: WKSnapshotConfiguration?) async -> Result<Data, Refusal> {
        await snapshotImage(web, configuration).flatMap { png($0).map(Result.success) ?? .failure("the drawing could not be encoded as a PNG") }
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

    /// `text` as a JavaScript string literal.
    private static func literal(_ text: String) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: [text]), let json = String(data: data, encoding: .utf8) else { return "\"\"" }
        return String(json.dropFirst().dropLast())
    }

    /// A script result as JSON text; `undefined` is `null`.
    private static func json(_ value: Any?) -> String? {
        guard let value, !(value is NSNull) else { return "null" }
        guard JSONSerialization.isValidJSONObject([value]),
              let data = try? JSONSerialization.data(withJSONObject: [value], options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8) else { return nil }
        return String(text.dropFirst().dropLast())
    }

    /// What the page threw, as the page worded it.
    private static func scriptMessage(_ error: Error) -> String {
        let info = (error as NSError).userInfo
        return (info["WKJavaScriptExceptionMessage"] as? String) ?? error.localizedDescription
    }

    public static func png(_ image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation else { return nil }
        return NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
    }

    /// `top` above `page`, as wide as the page, in a bitmap with as many pixels per point as the
    /// sharper of the two, so a Retina snapshot keeps its pixels.
    public static func stack(_ top: NSImage, over page: NSImage) -> NSImage {
        func scale(_ image: NSImage) -> CGFloat {
            let wide = image.representations.map { CGFloat($0.pixelsWide) }.max() ?? 0
            return image.size.width > 0 ? wide / image.size.width : 1
        }
        let pixels = max(1, scale(top), scale(page))
        let size = NSSize(width: page.size.width, height: top.size.height + page.size.height)
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int((size.width * pixels).rounded()),
                                         pixelsHigh: Int((size.height * pixels).rounded()), bitsPerSample: 8, samplesPerPixel: 4,
                                         hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
              let context = NSGraphicsContext(bitmapImageRep: rep) else { return page }
        rep.size = size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        // A bitmap context counts pixels; the drawing below counts points.
        context.cgContext.scaleBy(x: pixels, y: pixels)
        page.draw(in: NSRect(origin: .zero, size: page.size))
        top.draw(in: NSRect(x: 0, y: page.size.height, width: page.size.width, height: top.size.height))
        NSGraphicsContext.restoreGraphicsState()
        let image = NSImage(size: size)
        image.addRepresentation(rep)
        return image
    }
}
#endif
