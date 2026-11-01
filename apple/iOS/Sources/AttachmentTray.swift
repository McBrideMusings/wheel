import Foundation
import Observation
import UIKit
import UniformTypeIdentifiers
import WheelKit
import WheelUI

/// The files one agent pane's composer holds for its next message. Each is uploaded to the Mac
/// (`Uploads`) as soon as it is picked, one at a time so only one file's bytes are in flight; once
/// stored, its Mac path goes into the message at send. Removing one cancels its upload or deletes
/// the stored file.
@MainActor @Observable
final class AttachmentTray {
    struct Item {
        var chip: ComposerAttachment
        /// The bytes as they will be sent, kept until the Mac stores them so a failure can retry.
        var data: Data?
        var stored: UploadedFile?
    }

    let pane: String
    /// The session the pane showed when the tray was made; a new one under the pane's id gets a new tray.
    let session: String
    private(set) var items: [Item] = []
    /// The most the tray holds in bytes not yet stored on the Mac, so a large pick fails its chip
    /// rather than having iOS end the app for its memory.
    static let maxHeld = 200 << 20
    /// Where uploads go: set by the model at each connect, nil while there is no connection.
    @ObservationIgnored var uploads: (() -> Uploads?)?
    @ObservationIgnored private var running: (id: UUID, task: Task<Void, Never>)?

    init(pane: String, session: String) {
        self.pane = pane
        self.session = session
    }

    /// Bytes held for uploads not yet stored.
    private var held: Int { items.reduce(0) { $0 + ($1.data?.count ?? 0) } }

    var chips: [ComposerAttachment] { items.map(\.chip) }

    /// The stored files' paths, in the order they were picked; complete only when `ready`.
    var paths: [String] { items.compactMap(\.stored?.path) }

    var ready: Bool { items.allSatisfy { $0.chip.state == .ready } }

    /// Add a file the person picked, already in the form it is sent in.
    func add(_ data: Data, name: String, isImage: Bool) {
        let id = UUID()
        let thumbnail = isImage ? Self.thumbnail(data) : nil
        Log.info("attach \(name): \(data.count) bytes\(isImage ? ", image" : "")")
        if data.count > Uploads.maxBytes {
            items.append(Item(chip: ComposerAttachment(id: id, name: name, detail: Self.size(data.count), thumbnail: thumbnail,
                                                       state: .failed("over \(Uploads.maxBytes >> 20) MB")), data: nil))
            Log.info("upload \(name) refused here: \(data.count) bytes is over \(Uploads.maxBytes >> 20) MB")
            return
        }
        if held + data.count > Self.maxHeld {
            items.append(Item(chip: ComposerAttachment(id: id, name: name, detail: Self.size(data.count), thumbnail: thumbnail,
                                                       state: .failed("send or remove the others first")), data: nil))
            Log.info("upload \(name) held back: \(held) bytes already waiting")
            return
        }
        items.append(Item(chip: ComposerAttachment(id: id, name: name, detail: "waiting", thumbnail: thumbnail, state: .uploading(nil)), data: data))
        next()
    }

    /// A file too large to read in, shown failed without its bytes ever loaded.
    func addTooLarge(name: String, bytes: Int) {
        Log.info("upload \(name) refused here: \(bytes) bytes is over \(Uploads.maxBytes >> 20) MB")
        items.append(Item(chip: ComposerAttachment(id: UUID(), name: name, detail: Self.size(bytes), thumbnail: nil,
                                                   state: .failed("over \(Uploads.maxBytes >> 20) MB")), data: nil))
    }

    /// A pick that could not be read on the phone.
    func addUnreadable(name: String, why: String) {
        Log.info("attach \(name) failed: \(why)")
        items.append(Item(chip: ComposerAttachment(id: UUID(), name: name, detail: "", thumbnail: nil, state: .failed(why)), data: nil))
    }

    func remove(_ id: UUID) {
        guard let at = items.firstIndex(where: { $0.chip.id == id }) else { return }
        let item = items.remove(at: at)
        if running?.id == id {
            running?.task.cancel()
            running = nil
            Log.info("upload \(item.chip.name) cancelled")
        }
        if let stored = item.stored, let uploads = uploads?() {
            let pane = pane
            Task {
                do {
                    try await uploads.delete(name: stored.name, pane: pane)
                    Log.info("upload \(stored.name) deleted on the Mac")
                } catch {
                    Log.info("upload \(stored.name) not deleted on the Mac: \(error.localizedDescription)")
                }
            }
        }
        next()
    }

    func retry(_ id: UUID) {
        guard let at = items.firstIndex(where: { $0.chip.id == id }), items[at].data != nil else { return }
        items[at].chip.state = .uploading(nil)
        items[at].chip.detail = "waiting"
        next()
    }

    /// Empty the tray once its paths went out in a message; the files stay on the Mac for the agent.
    func sent() {
        running?.task.cancel()
        running = nil
        items = []
    }

    /// Start the oldest waiting upload when none runs.
    private func next() {
        guard running == nil,
              let item = items.first(where: { $0.chip.state == .uploading(nil) && $0.stored == nil && $0.data != nil }),
              let data = item.data
        else { return }
        let id = item.chip.id
        let name = item.chip.name
        guard let uploads = uploads?() else {
            update(id) { $0.chip.state = .failed("not connected to the Mac; tap to try again") }
            return
        }
        let pane = pane
        update(id) { $0.chip.detail = "0 of \(Self.size(data.count))" }
        // The task holds the tray until the upload ends; removing the chip or sending cancels it.
        let task = Task { [self] in
            let started = Date()
            do {
                let stored = try await uploads.upload(data, name: name, pane: pane) { [tray = self] fraction in
                    Task { @MainActor in
                        tray.update(id) {
                            // A redraw per two percent, not per slice.
                            guard case let .uploading(was) = $0.chip.state, fraction - (was ?? 0) >= 0.02 || fraction >= 1 else { return }
                            $0.chip.state = .uploading(fraction)
                            $0.chip.detail = "\(Self.size(Int(Double(data.count) * fraction))) of \(Self.size(data.count))"
                        }
                    }
                }
                Log.info("upload \(name): stored at \(stored.path), \(stored.bytes) bytes in \(Int(Date().timeIntervalSince(started) * 1000)) ms")
                finish(id) {
                    $0.stored = stored
                    $0.data = nil
                    $0.chip.state = .ready
                    $0.chip.detail = Self.size(stored.bytes)
                }
            } catch is CancellationError {
                return
            } catch {
                let why = (error as? UploadFailed)?.reason ?? error.localizedDescription
                Log.info("upload \(name) failed: \(why)")
                finish(id) { $0.chip.state = .failed(why) }
            }
        }
        running = (id, task)
    }

    private func finish(_ id: UUID, _ change: (inout Item) -> Void) {
        update(id, change)
        if running?.id == id { running = nil }
        next()
    }

    private func update(_ id: UUID, _ change: (inout Item) -> Void) {
        guard let at = items.firstIndex(where: { $0.chip.id == id }) else { return }
        change(&items[at])
    }

    static func size(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }

    private static func thumbnail(_ data: Data) -> Data? {
        UIImage(data: data)?.preparingThumbnail(of: CGSize(width: 96, height: 96))?.jpegData(compressionQuality: 0.7)
    }
}

/// Turning what a picker hands over into what is sent.
@MainActor
enum AttachmentMedia {
    /// The image types an agent reads as they are; any other image is sent as JPEG.
    private static let kept: [UTType] = [.png, .jpeg, .gif, .webP]

    /// `data` as it should go to the Mac, with its name: an image in a type an agent cannot show
    /// (HEIC, TIFF, …) becomes a JPEG and its name ends `.jpg`; everything else is unchanged.
    static func prepared(_ data: Data, name: String, type: UTType?) -> (data: Data, name: String, isImage: Bool) {
        guard let type, type.conforms(to: .image) else { return (data, name, false) }
        if kept.contains(where: { type.conforms(to: $0) }) { return (data, name, true) }
        guard let jpeg = UIImage(data: data)?.jpegData(compressionQuality: 0.9) else { return (data, name, false) }
        let base = (name as NSString).deletingPathExtension
        Log.info("converted \(name) (\(type.identifier), \(data.count) bytes) to JPEG, \(jpeg.count) bytes")
        return (jpeg, (base.isEmpty ? "photo" : base) + ".jpg", true)
    }

    /// Read a file the document picker (or a check) named and add it to `tray`, refusing one over
    /// the limit before reading it.
    static func addFile(_ url: URL, to tray: AttachmentTray) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let name = url.lastPathComponent
        let values = try? url.resourceValues(forKeys: [.fileSizeKey, .contentTypeKey])
        if let bytes = values?.fileSize, bytes > Uploads.maxBytes {
            tray.addTooLarge(name: name, bytes: bytes)
            return
        }
        do {
            let data = try Data(contentsOf: url)
            let type = values?.contentType ?? UTType(filenameExtension: url.pathExtension)
            let ready = prepared(data, name: name, type: type)
            tray.add(ready.data, name: ready.name, isImage: ready.isImage)
        } catch {
            tray.addUnreadable(name: name, why: "cannot read it: \(error.localizedDescription)")
        }
    }

    /// A camera photo, sent as JPEG.
    static func addPhoto(_ image: UIImage, to tray: AttachmentTray) {
        guard let jpeg = image.jpegData(compressionQuality: 0.9) else {
            tray.addUnreadable(name: "photo.jpg", why: "the camera gave no image")
            return
        }
        tray.add(jpeg, name: "Photo-\(stamp()).jpg", isImage: true)
    }

    /// A name for a picked photo, which carries none of its own.
    static func stamp() -> String {
        let format = DateFormatter()
        format.dateFormat = "yyyyMMdd-HHmmss"
        format.locale = Locale(identifier: "en_US_POSIX")
        return format.string(from: Date())
    }
}
