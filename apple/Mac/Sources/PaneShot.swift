import AppKit
import SwiftUI
import WebKit
import WheelUI
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
    func renderPane(id: UInt64, pane: String, full: Bool = false) {
        drawPane(pane, full: full) { [weak self] result in
            switch result {
            case let .success(png): self?.answerRender(id, pane, png)
            case let .failure(refusal): self?.refuseRender(id, pane, refusal.message)
            }
        }
    }

    /// Why a pane could not be drawn.
    struct RenderFailure: Error { let message: String }

    /// Draw `pane` and hand the PNG, or the reason it could not be drawn, to `done`: the answer to
    /// `wheel pane shot` and the header's Copy Pane Image button share this one drawing.
    func drawPane(_ pane: String, full: Bool = false, done: @escaping (Result<Data, RenderFailure>) -> Void) {
        func refuse(_ message: String) { done(.failure(RenderFailure(message: message))) }
        func answer(_ png: Data) { done(.success(png)) }
        guard let space = spaces.first(where: { $0.allPanes.contains { $0.id == pane } }),
              let kind = space.allPanes.first(where: { $0.id == pane })?.kind else {
            refuse(RenderRefusal.notShown)
            return
        }
        // A palette or a refresh request open over the pane is drawn by the window, not by WebKit's
        // snapshot of the page.
        let covered = palettes[pane] != nil || perms.contains { $0.agent == pane }
        // A web pane on an A2UI server draws native controls, so its shot is the window's, not WebKit's.
        let native = a2uiPanes[pane]?.isNative == true
        let web = covered || native ? nil : webPanes[pane]?.view ?? browserPanes[pane]?.view
        // A native page in a space no window shows has no mounted view to copy from, so its own view is
        // hosted off screen at the size the pane last had and drawn from there.
        if web == nil, native, !shownSpaces.contains(space.id), let a2ui = a2uiPanes[pane] {
            let size = paneFrames[pane].map { CGSize(width: max($0.width, 320), height: max($0.height, 240)) } ?? CGSize(width: 480, height: 660)
            let host = NSHostingView(rootView: A2UIPaneView(pane: a2ui).frame(width: size.width, height: size.height).background(Color(nsColor: .windowBackgroundColor)))
            host.frame = CGRect(origin: .zero, size: size)
            host.layoutSubtreeIfNeeded()
            guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { refuse("the pane could not be drawn off screen"); return }
            host.cacheDisplay(in: host.bounds, to: rep)
            guard let png = rep.representation(using: .png, properties: [:]) else { refuse("the drawing could not be encoded as a PNG"); return }
            Log.info("render: drew \(pane) off screen, \(Int(size.width))x\(Int(size.height)) pt, its space \(space.id) is shown by no window")
            answer(png)
            return
        }
        guard web != nil || shownSpaces.contains(space.id) else {
            refuse(RenderRefusal.notShown)
            return
        }
        Log.info("render: drawing \(pane) (\(kind.name))")
        if let web {
            // The header strip is the window's drawing of the pane above its body; WebKit's
            // snapshot is the page alone, so the strip goes on top of it.
            let header = headerStrip(pane: pane, space: space.id, web: web)
            if header == nil { Log.info("render \(pane) \(pane) has no header to draw (no window shows it); the shot is the page alone") }
            // A page in a hidden space is hidden (no cursor rects); WebKit draws it only when shown.
            web.isHidden = false
            let take: (WKSnapshotConfiguration?) -> Void = { configuration in
              web.takeSnapshot(with: configuration) { [weak self] image, error in
                MainActor.assumeIsolated {
                    self?.updatePageVisibility()
                    let whole = image.map { page in header.map { Self.stack($0, over: page) } ?? page }
                    guard let png = whole.flatMap(Self.png) else {
                        let drew = "WebKit drew no image: \(error?.localizedDescription ?? "no error")"
                        let hidden = self.map { !$0.shownSpaces.contains(space.id) } ?? false
                        refuse(hidden
                            ? "pane \(pane) is in background space \(space.id), which no window shows, so it cannot be drawn; show the space first with `wheel space move \(space.id) <window>` (\(drew))"
                            : drew)
                        return
                    }
                    answer(png)
                }
              }
            }
            guard full else { take(nil); return }
            // The page's own size, as the document lays it out: the snapshot's rect covers all of it.
            web.evaluateJavaScript("[Math.max(document.documentElement.scrollWidth, document.body ? document.body.scrollWidth : 0), Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0)]") { value, _ in
                MainActor.assumeIsolated {
                    let size = (value as? [Double]).flatMap { $0.count == 2 ? CGSize(width: $0[0], height: $0[1]) : nil }
                    guard let size, size.width > 0, size.height > 0 else { take(nil); return }
                    let configuration = WKSnapshotConfiguration()
                    configuration.rect = CGRect(origin: .zero, size: size)
                    configuration.snapshotWidth = NSNumber(value: Double(size.width))
                    Log.info("render \(pane) \(pane) page is \(Int(size.width))x\(Int(size.height)) pt; drawing all of it")
                    take(configuration)
                }
            }
            return
        }
        guard let frame = paneFrames[pane], frame.width > 0, frame.height > 0,
              let window = layout.window(holding: space.id).flatMap({ windowHost?.window($0.id) }) ?? NSApp.wheelWindow,
              let content = window.contentView else {
            refuse("the pane has no size in the window")
            return
        }
        let paneRect = Self.contentRect(frame, in: content)
        if full, let scroll = Self.scrollView(under: paneRect, in: content),
           let document = scroll.documentView, document.frame.height > scroll.contentView.bounds.height + 1 {
            Task { @MainActor in
                guard let png = await Self.stitched(scroll, in: content, within: paneRect) else {
                    refuse("the window could not draw the pane's whole content")
                    return
                }
                answer(png)
            }
            return
        }
        if full, kind.name == "terminal" {
            refuse("the terminal's scrollback is not a scroll view the window can draw whole; read it with `wheel term read`")
            return
        }
        // A pane's frame is rarely whole device pixels (a 1090.5 pt pane on a 1x display). The bitmap
        // rounds its size up to whole pixels, so an unaligned rect draws a partial pixel row of the
        // neighbour beyond the pane's edge; snapping every edge inward keeps the image inside the frame.
        let rect = content.backingAlignedRect(paneRect, options: .alignAllEdgesInward)
        guard rect.width > 0, rect.height > 0, let rep = content.bitmapImageRepForCachingDisplay(in: rect) else {
            refuse("the window could not make a bitmap for \(Int(paneRect.width))x\(Int(paneRect.height))")
            return
        }
        content.cacheDisplay(in: rect, to: rep)
        Log.info("render \(pane) \(pane) frame \(frame) drawn from \(rect) into \(rep.pixelsWide)x\(rep.pixelsHigh) px")
        guard let png = rep.representation(using: .png, properties: [:]) else {
            refuse("the drawing could not be encoded as a PNG")
            return
        }
        answer(png)
    }

    /// The smallest scroll view whose frame holds the middle of `rect` (a rect of `content`).
    private static func scrollView(under rect: NSRect, in content: NSView) -> NSScrollView? {
        let point = NSPoint(x: rect.midX, y: rect.midY)
        func all(in view: NSView) -> [NSScrollView] {
            view.subviews.flatMap { ($0 as? NSScrollView).map { [$0] } ?? [] + all(in: $0) }
        }
        return all(in: content)
            .filter { $0.convert($0.bounds, to: content).contains(point) }
            .min { $0.bounds.width * $0.bounds.height < $1.bounds.width * $1.bounds.height }
    }

    /// The scroll view's whole document as one PNG: the scroll view's area of the window drawn at
    /// each scroll position from the top to the end, each slice placed at its offset in the
    /// document. The scroll position is put back afterwards.
    private static func stitched(_ scroll: NSScrollView, in content: NSView, within pane: NSRect) async -> Data? {
        guard let document = scroll.documentView else { return nil }
        let clip = scroll.contentView
        let origin = clip.bounds.origin
        defer {
            clip.scroll(to: origin)
            scroll.reflectScrolledClipView(clip)
        }
        let view = clip.bounds.height
        let total = document.frame.height
        let area = scroll.convert(scroll.bounds, to: content).intersection(pane)
        guard view > 0, area.width > 0, area.height > 0 else { return nil }
        let rect = content.backingAlignedRect(area, options: .alignAllEdgesInward)
        var slices: [(top: CGFloat, rep: NSBitmapImageRep)] = []
        var top: CGFloat = 0
        while true {
            let at = min(top, total - view)
            let y = document.isFlipped ? at : total - view - at
            clip.scroll(to: NSPoint(x: origin.x, y: y))
            scroll.reflectScrolledClipView(clip)
            // A table builds the rows of a new position on the next turn of the run loop.
            try? await Task.sleep(for: .milliseconds(80))
            content.layoutSubtreeIfNeeded()
            guard let rep = content.bitmapImageRepForCachingDisplay(in: rect) else { return nil }
            content.cacheDisplay(in: rect, to: rep)
            slices.append((at, rep))
            if at >= total - view - 0.5 { break }
            top += view
        }
        guard let first = slices.first else { return nil }
        let pixels = CGFloat(first.rep.pixelsHigh) / rect.height
        let width = first.rep.pixelsWide
        let height = Int((total * pixels).rounded())
        guard let out = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0),
              let context = NSGraphicsContext(bitmapImageRep: out) else { return nil }
        // A later slice repeats rows an earlier one drew, so drawing each at its offset joins them.
        for slice in slices {
            guard let image = slice.rep.cgImage else { continue }
            let h = CGFloat(slice.rep.pixelsHigh)
            context.cgContext.draw(image, in: CGRect(x: 0, y: CGFloat(height) - slice.top * pixels - h, width: CGFloat(width), height: h))
        }
        return out.representation(using: .png, properties: [:])
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

    static func png(_ image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation else { return nil }
        return NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
    }

    func answerRender(_ id: UInt64, _ pane: String, _ png: Data) {
        Log.info("render \(id): \(pane) drawn, \(png.count) bytes")
        reply(.rendered(id: id, png: png))
    }

    func refuseRender(_ id: UInt64, _ pane: String, _ message: String) {
        Log.info("render \(id): \(pane) not drawn: \(message)")
        reply(.renderRefused(id: id, message: message))
    }
}
