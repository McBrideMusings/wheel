import SwiftUI
import Textual
import WheelKit

#if os(macOS)
import AppKit
typealias PlatformImage = NSImage
#else
import UIKit
typealias PlatformImage = UIImage
#endif

extension Image {
    init(platform: PlatformImage) {
        #if os(macOS)
        self.init(nsImage: platform)
        #else
        self.init(uiImage: platform)
        #endif
    }
}

/// The pixel size of encoded image data, read without drawing it.
func imagePixelSize(_ data: Data) -> CGSize? {
    guard let image = PlatformImage(data: data) else { return nil }
    #if os(macOS)
    if let rep = image.representations.first, rep.pixelsWide > 0 { return CGSize(width: rep.pixelsWide, height: rep.pixelsHigh) }
    return image.size
    #else
    return CGSize(width: image.size.width * image.scale, height: image.size.height * image.scale)
    #endif
}

/// One image a reply asked for, as `/state` lists it: where it came from, whether it loaded, and
/// the size it loaded at.
public struct ChatImageRecord: Equatable, Sendable {
    public var row: Int
    public var source: String
    public var path: String?
    public var loaded: Bool
    public var width: Int
    public var height: Int
    public var bytes: Int
    public var error: String?
    /// What a click on the drawn image does; nil for an image that did not load.
    var open: ChatImageOpen?
}

private extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}

/// Every image the chats drew or failed to draw, by session, so a check reads back what a reply
/// showed without looking at the screen.
@MainActor
public final class ChatImageRegistry {
    public static let shared = ChatImageRegistry()
    private var bySession: [String: [ChatImageRecord]] = [:]

    func record(_ record: ChatImageRecord, session: String) {
        var list = bySession[session] ?? []
        if let at = list.firstIndex(where: { $0.row == record.row && $0.source == record.source }) {
            list[at] = record
        } else {
            list.append(record)
        }
        bySession[session] = list
    }

    public func images(session: String) -> [ChatImageRecord] { bySession[session] ?? [] }

    /// Do what a click on the `index`-th image of `session` does; false when there is none to click.
    public func click(session: String, index: Int) -> Bool {
        guard let open = images(session: session)[safe: index]?.open else { return false }
        open.run()
        return true
    }
}

/// What a click on a drawn image asks for; two are equal whatever they hold, so a fresh closure
/// on every redraw does not make the attachment look new.
final class ChatImageOpen: @unchecked Sendable, Hashable {
    let run: @Sendable () -> Void
    init(_ run: @escaping @Sendable () -> Void) { self.run = run }
    static func == (a: ChatImageOpen, b: ChatImageOpen) -> Bool { true }
    func hash(into hasher: inout Hasher) {}
}

/// A reply's image inside its text: the picture at up to `maxHeight` points tall, opening full
/// size on a click, or a line saying why there is none.
struct ChatImageAttachment: Attachment {
    enum Content: Hashable, Sendable {
        case image(Data, width: Int, height: Int)
        case failed(String)
    }

    let id: String
    let content: Content
    let maxHeight: Double
    let open: ChatImageOpen

    var description: String { id }

    @MainActor var body: some View {
        switch content {
        case let .image(data, _, _):
            if let image = PlatformImage(data: data) {
                Image(platform: image)
                    .resizable()
                    .scaledToFit()
                    .frame(maxHeight: maxHeight)
                    .onTapGesture { open.run() }
                    .accessibilityIdentifier("chat-image")
            }
        case let .failed(message):
            Label(message, systemImage: "photo")
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("chat-image-failed")
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, in _: TextEnvironmentValues) -> CGSize {
        switch content {
        case let .image(_, w, h):
            let natural = CGSize(width: Double(w), height: Double(h))
            guard natural.width > 0, natural.height > 0 else { return .zero }
            let width = min(proposal.width ?? natural.width, natural.width)
            let scale = min(width / natural.width, maxHeight / natural.height, 1)
            return CGSize(width: natural.width * scale, height: natural.height * scale)
        case .failed:
            return CGSize(width: min(proposal.width ?? 320, 320), height: 20)
        }
    }

    func pngData() -> Data? {
        if case let .image(data, _, _) = content { return data }
        return nil
    }
}

/// Loads the images of one reply: a web address from the web, any other path through the daemon, so
/// a phone draws a file that lives on the Mac. Each result is recorded for `/state`.
struct ChatImageLoader: AttachmentLoader {
    let session: String
    let row: Int
    let maxHeight: Double
    let resolver: ChatLinkResolver?

    func attachment(for url: URL, text: String, environment: ColorEnvironmentValues) async throws -> ChatImageAttachment {
        let source = url.scheme == nil ? url.absoluteString.removingPercentEncoding ?? url.absoluteString : url.absoluteString
        let web = url.scheme == "http" || url.scheme == "https"
        let session = session, row = row, resolver = resolver
        var resolved: String?
        var data: Data?
        var failure: String?
        if web {
            do {
                let (body, response) = try await URLSession.shared.data(from: url)
                if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                    failure = "HTTP \(http.statusCode)"
                } else {
                    data = body
                }
            } catch {
                failure = error.localizedDescription
            }
        } else {
            let path = url.scheme == "file" ? url.path : source
            let image = await resolver?.fetchImage(path: path, session: session)
                ?? FileImage(path: path, found: false, error: "not connected")
            resolved = image.path
            data = image.data
            if !image.found { failure = image.error.map { $0.prefix(1).uppercased() + $0.dropFirst() } ?? "No image at \(path)" }
        }
        var content = ChatImageAttachment.Content.failed(failure ?? "Cannot read the image")
        if failure == nil, let data, let size = imagePixelSize(data) {
            content = .image(data, width: Int(size.width), height: Int(size.height))
        } else if failure == nil {
            failure = "Cannot read the image"
        }
        let shownData: Data? = data
        let shownPath = resolved ?? source
        let open = ChatImageOpen { [shownData] in
            guard let shownData else { return }
            Task { @MainActor in resolver?.openImage(session: session, row: row, path: shownPath, data: shownData) }
        }
        let record: ChatImageRecord
        if case let .image(bytes, w, h) = content {
            record = ChatImageRecord(row: row, source: source, path: resolved, loaded: true, width: w, height: h, bytes: bytes.count, error: nil, open: open)
        } else {
            record = ChatImageRecord(row: row, source: source, path: resolved, loaded: false, width: 0, height: 0, bytes: 0, error: failure, open: nil)
        }
        await MainActor.run { ChatImageRegistry.shared.record(record, session: session) }
        return ChatImageAttachment(id: "\(source)#\(shownData?.count ?? 0)", content: content, maxHeight: maxHeight, open: open)
    }
}

/// The popover over a reply that shows an image at the size it has, scrolling when it is larger
/// than the popover.
struct ChatImageFull: View {
    let image: ChatLinkResolver.FullImage

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(image.path)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.head)
            if let platform = PlatformImage(data: image.data) {
                ScrollView([.horizontal, .vertical]) {
                    Image(platform: platform)
                        .accessibilityIdentifier("chat-image-full")
                }
            }
        }
        .padding(12)
        .frame(minWidth: 320, idealWidth: 720, maxWidth: 900, minHeight: 200, idealHeight: 520, maxHeight: 700)
    }
}
