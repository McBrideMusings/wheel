#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import CoreImage.CIFilterBuiltins

/// What a pairing code becomes on the Mac's clipboard: a concealed text item cleared after a
/// minute, or an image of the QR code. Shared by the Mac app and the GPUI client's bridge.
public enum PairingClipboard {
    /// The concealed-pasteboard marker clipboard managers honour: an item that carries it is not recorded.
    public static let concealedType = NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType")

    /// `text` as a QR code, one pixel per module scaled up to 440 so it stays sharp.
    public static func qrImage(_ text: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scale = (440 / output.extent.width).rounded(.down)
        let scaled = output.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        let rep = NSCIImageRep(ciImage: scaled)
        let image = NSImage(size: rep.size)
        image.addRepresentation(rep)
        return image
    }

    /// Put the QR code for `text` on the clipboard. Returns whether it drew.
    @discardableResult
    public static func copyImage(_ text: String) -> Bool {
        guard let image = qrImage(text) else { return false }
        let board = NSPasteboard.general
        board.clearContents()
        board.writeObjects([image])
        return true
    }

    /// Write `text` as a concealed item and clear it after `life` if nothing else was copied.
    /// `done(cleared)` runs on the main actor when the minute is up: true when the clipboard
    /// still held the code and was cleared, false when it held something else and was left alone.
    /// Cancel the returned task to drop the clear (a new copy replaces the old one).
    @discardableResult
    public static func writeConcealed(_ text: String, life: Duration, done: @escaping @MainActor (Bool) -> Void) -> Task<Void, Never> {
        let board = NSPasteboard.general
        board.clearContents()
        board.declareTypes([.string, concealedType], owner: nil)
        board.setString(text, forType: .string)
        board.setData(Data(), forType: concealedType)
        // An app nobody is looking at is napped by the system, which would stretch the minute; the
        // clear is held to its time.
        let activity = ProcessInfo.processInfo.beginActivity(options: .userInitiatedAllowingIdleSystemSleep, reason: "clearing the pairing code from the clipboard")
        return Task { @MainActor in
            defer { ProcessInfo.processInfo.endActivity(activity) }
            try? await Task.sleep(for: life)
            guard !Task.isCancelled else { return }
            // Unchanged means the clipboard still holds the code. The change count is no test: a
            // clipboard manager or the system touches it after a write, whatever was copied.
            if board.string(forType: .string) == text {
                board.clearContents()
                done(true)
            } else {
                done(false)
            }
        }
    }
}
#endif
