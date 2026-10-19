import AppKit
import WebKit
import WheelKit

/// `render_pane`: the daemon asks this window to draw one pane as a PNG (`wheel pane shot`). A web
/// or browser pane is WebKit's own render of the page; every other kind is the pane's rectangle of
/// the window's content view, drawn by the view itself, so no screen-recording permission is
/// involved and no window is brought forward.
extension AppModel {
    /// Draw `pane` and answer request `id`. A pane in a space that is not the active one is not
    /// shown: its views are mounted but drawn at zero opacity.
    func renderPane(id: UInt64, pane: String) {
        guard let space = activeSpace, let kind = space.root.panes.first(where: { $0.id == pane })?.kind else {
            Log.info("render \(id): pane \(pane) is not on screen")
            reply(.renderRefused(id: id, message: RenderRefusal.notShown))
            return
        }
        Log.info("render \(id): drawing \(pane) (\(kind.name))")
        // A palette open over the pane is drawn by the window, not by WebKit's snapshot of the page.
        let web = palettes[pane] != nil ? nil : webPanes[pane]?.view ?? browserPanes[pane]?.view
        if let web {
            web.takeSnapshot(with: nil) { [weak self] image, error in
                MainActor.assumeIsolated {
                    guard let png = image.flatMap(Self.png) else {
                        self?.refuseRender(id, pane, "WebKit drew no image: \(error?.localizedDescription ?? "no error")")
                        return
                    }
                    self?.answerRender(id, pane, png)
                }
            }
            return
        }
        guard let frame = paneFrames[pane], frame.width > 0, frame.height > 0,
              let window = NSApp.windows.first(where: { $0.title == "Wheel" }), let content = window.contentView else {
            refuseRender(id, pane, "the pane has no size in the window")
            return
        }
        // SwiftUI's global space has its origin at the top left; an unflipped view's has it at the bottom left.
        let paneRect = NSRect(x: frame.minX, y: content.isFlipped ? frame.minY : content.bounds.height - frame.maxY,
                              width: frame.width, height: frame.height)
        // A pane's frame is rarely whole device pixels (a 1090.5 pt pane on a 1x display). The bitmap
        // rounds its size up to whole pixels, so an unaligned rect draws a partial pixel row of the
        // neighbour beyond the pane's edge; snapping every edge inward keeps the image inside the frame.
        let rect = content.backingAlignedRect(paneRect, options: .alignAllEdgesInward)
        guard rect.width > 0, rect.height > 0, let rep = content.bitmapImageRepForCachingDisplay(in: rect) else {
            refuseRender(id, pane, "the window could not make a bitmap for \(Int(paneRect.width))x\(Int(paneRect.height))")
            return
        }
        content.cacheDisplay(in: rect, to: rep)
        Log.info("render \(id): \(pane) frame \(frame) drawn from \(rect) into \(rep.pixelsWide)x\(rep.pixelsHigh) px")
        guard let png = rep.representation(using: .png, properties: [:]) else {
            refuseRender(id, pane, "the drawing could not be encoded as a PNG")
            return
        }
        answerRender(id, pane, png)
    }

    private static func png(_ image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation else { return nil }
        return NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
    }

    private func answerRender(_ id: UInt64, _ pane: String, _ png: Data) {
        Log.info("render \(id): \(pane) drawn, \(png.count) bytes")
        reply(.rendered(id: id, png: png))
    }

    private func refuseRender(_ id: UInt64, _ pane: String, _ message: String) {
        Log.info("render \(id): \(pane) not drawn: \(message)")
        reply(.renderRefused(id: id, message: message))
    }
}
