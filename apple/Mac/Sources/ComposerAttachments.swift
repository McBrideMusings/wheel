import AppKit
import Foundation
import ImageIO
import Observation
import WheelKit
import WheelUI

/// The files one agent's composer holds for its next message. The agent runs on this Mac, so a
/// file is attached where it is: its path goes into the message at send and nothing is copied.
/// An image with no file (a screenshot on the clipboard) is written once to a folder of its own
/// so it has a path to send; removing its chip deletes that copy.
@MainActor @Observable
final class ComposerAttachments {
    struct Item {
        var chip: ComposerAttachment
        let path: String
        /// A copy this tray wrote, deleted when the chip is removed.
        let owned: Bool
    }

    private(set) var items: [Item] = []

    var chips: [ComposerAttachment] { items.map(\.chip) }
    var paths: [String] { items.map(\.path) }
    var ready: Bool { items.allSatisfy { $0.chip.state == .ready } }

    /// Where bytes with no file are written, one folder per image so the file keeps a readable name.
    private static var folder: URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("wheel-attachments", isDirectory: true)
    }

    func add(_ drops: [ComposerDrop]) {
        for drop in drops {
            switch drop {
            case let .file(url): addFile(url)
            case let .image(data): addImage(data)
            }
        }
    }

    func remove(_ id: UUID) {
        guard let at = items.firstIndex(where: { $0.chip.id == id }) else { return }
        let item = items.remove(at: at)
        if item.owned { try? FileManager.default.removeItem(at: URL(fileURLWithPath: item.path).deletingLastPathComponent()) }
    }

    /// Empty the tray once its paths went out; the files stay where they are for the agent.
    func sent() { items = [] }

    private func addFile(_ url: URL) {
        let name = url.lastPathComponent
        let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .fileSizeKey, .contentTypeKey])
        let isImage = values?.contentType?.conforms(to: .image) == true
        let detail = values?.isDirectory == true ? "Folder" : values?.fileSize.map { ByteCountFormatter.string(fromByteCount: Int64($0), countStyle: .file) } ?? ""
        let thumbnail = isImage ? Self.thumbnail(of: url) : nil
        items.append(Item(chip: ComposerAttachment(id: UUID(), name: name, detail: detail, thumbnail: thumbnail, state: .ready),
                          path: url.path, owned: false))
        Log.info("attach \(name)\(isImage ? ", image" : "")")
    }

    private func addImage(_ data: Data) {
        let stamp = DateFormatter()
        stamp.dateFormat = "yyyyMMdd-HHmmss"
        stamp.locale = Locale(identifier: "en_US_POSIX")
        let name = "Image-\(stamp.string(from: Date())).png"
        let dir = Self.folder.appendingPathComponent(UUID().uuidString, isDirectory: true)
        let file = dir.appendingPathComponent(name)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            guard let png = Self.png(data) else { throw CocoaError(.fileReadCorruptFile) }
            try png.write(to: file)
            items.append(Item(chip: ComposerAttachment(id: UUID(), name: name,
                                                       detail: ByteCountFormatter.string(fromByteCount: Int64(png.count), countStyle: .file),
                                                       thumbnail: Self.thumbnail(of: file), state: .ready),
                              path: file.path, owned: true))
            Log.info("attach \(name): \(png.count) bytes, pasted image")
        } catch {
            Log.info("attach pasted image failed: \(error.localizedDescription)")
            items.append(Item(chip: ComposerAttachment(id: UUID(), name: name, detail: "", thumbnail: nil,
                                                       state: .failed("cannot save it: \(error.localizedDescription)")),
                              path: file.path, owned: false))
        }
    }

    /// PNG bytes for image data in any type an agent might not read (TIFF from a screenshot).
    private static func png(_ data: Data) -> Data? {
        guard let rep = NSBitmapImageRep(data: data) else { return nil }
        return rep.representation(using: .png, properties: [:])
    }

    private static func thumbnail(of url: URL) -> Data? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 96,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        return NSBitmapImageRep(cgImage: image).representation(using: .jpeg, properties: [.compressionFactor: 0.7])
    }
}
