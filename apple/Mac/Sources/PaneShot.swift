import AppKit
import WebKit
import WheelKit

/// `render_pane`: the daemon asks this window to draw one pane as a PNG (`wheel pane shot`). A web
/// or browser pane is WebKit's own render of the page under the header the window draws for it,
/// unless a palette or a refresh request covers it; every other kind is the pane's rectangle of
/// the window's content view, drawn by the view itself, so no screen-recording permission is
/// involved and no window is brought forward.
extension AppModel {
    /// Draw `pane` and answer request `id`. A terminal or agent pane in a space no window shows is
    /// not shown: its views are mounted but drawn at zero opacity. A web or browser pane in such a
    /// space still has its WebKit view mounted, and WebKit renders the page itself, so it is drawn.
    func renderPane(id: UInt64, pane: String) {
        guard let space = spaces.first(where: { $0.root.panes.contains { $0.id == pane } }),
              let kind = space.root.panes.first(where: { $0.id == pane })?.kind else {
            Log.info("render \(id): pane \(pane) is not on screen")
            reply(.renderRefused(id: id, message: RenderRefusal.notShown))
            return
        }
        // A palette or a refresh request open over the pane is drawn by the window, not by WebKit's
        // snapshot of the page.
        let covered = palettes[pane] != nil || perms.contains { $0.session == pane }
        let web = covered ? nil : webPanes[pane]?.view ?? browserPanes[pane]?.view
        guard web != nil || shownSpaces.contains(space.id) else {
            Log.info("render \(id): pane \(pane) is not on screen")
            reply(.renderRefused(id: id, message: RenderRefusal.notShown))
            return
        }
        Log.info("render \(id): drawing \(pane) (\(kind.name))")
        if let web {
            // The header strip is the window's drawing of the pane above its body; WebKit's
            // snapshot is the page alone, so the strip goes on top of it.
            let header = headerStrip(pane: pane, space: space.id, web: web)
            if header == nil { Log.info("render \(id): \(pane) has no header to draw (no window shows it); the shot is the page alone") }
            web.takeSnapshot(with: nil) { [weak self] image, error in
                MainActor.assumeIsolated {
                    let whole = image.map { page in header.map { Self.stack($0, over: page) } ?? page }
                    guard let png = whole.flatMap(Self.png) else {
                        let drew = "WebKit drew no image: \(error?.localizedDescription ?? "no error")"
                        let hidden = self.map { !$0.shownSpaces.contains(space.id) } ?? false
                        self?.refuseRender(id, pane, hidden
                            ? "pane \(pane) is in background space \(space.id), which no window shows, so it cannot be drawn; show the space first with `wheel space move \(space.id) <window>` (\(drew))"
                            : drew)
                        return
                    }
                    self?.answerRender(id, pane, png)
                }
            }
            return
        }
        guard let frame = paneFrames[pane], frame.width > 0, frame.height > 0,
              let window = layout.window(holding: space.id).flatMap({ windowHost?.window($0.id) }) ?? NSApp.wheelWindow,
              let content = window.contentView else {
            refuseRender(id, pane, "the pane has no size in the window")
            return
        }
        let paneRect = Self.contentRect(frame, in: content)
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

    /// The pane's header, drawn by the window: the part of the pane's frame above its body. Nil when
    /// no window shows the pane (a background space), where the shot is the page alone.
    private func headerStrip(pane: String, space: String, web: NSView) -> NSImage? {
        guard let frame = paneFrames[pane], let body = paneBodyFrames[pane], body.minY > frame.minY,
              shownSpaces.contains(space),
              let window = layout.window(holding: space).flatMap({ windowHost?.window($0.id) }) ?? NSApp.wheelWindow,
              let content = window.contentView else { return nil }
        let strip = Self.contentRect(CGRect(x: frame.minX, y: frame.minY, width: frame.width, height: body.minY - frame.minY), in: content)
        let rect = content.backingAlignedRect(strip, options: .alignAllEdgesInward)
        guard rect.width > 0, rect.height > 0, let rep = content.bitmapImageRepForCachingDisplay(in: rect) else { return nil }
        // cacheDisplay paints a WKWebView's page unclipped, over the header, so the web view is
        // hidden for the capture.
        let wasHidden = web.isHidden
        web.isHidden = true
        content.cacheDisplay(in: rect, to: rep)
        web.isHidden = wasHidden
        let image = NSImage(size: rect.size)
        image.addRepresentation(rep)
        return image
    }

    /// A rect in SwiftUI's global space (top left origin, the whole window including its title bar)
    /// as a rect of the content view, which starts below the title bar and may be unflipped.
    private static func contentRect(_ global: CGRect, in content: NSView) -> NSRect {
        let titleBar = (content.superview?.bounds.height ?? content.bounds.height) - content.bounds.height
        let top = global.minY - titleBar
        return NSRect(x: global.minX, y: content.isFlipped ? top : content.bounds.height - top - global.height,
                      width: global.width, height: global.height)
    }

    /// `top` above `page`, as wide as the page, in a bitmap with as many pixels per point as the
    /// sharper of the two, so a Retina snapshot keeps its pixels.
    private static func stack(_ top: NSImage, over page: NSImage) -> NSImage {
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
