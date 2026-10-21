import AppKit
import SwiftUI
import WheelUI

/// The pointer and key state behind one transcript's selection, held on `TranscriptSelection.input`.
@MainActor
final class SelectionInput {
    /// The mouse and key monitor, held while the transcript is on screen.
    var monitor: Any?
    /// The last press landed in this transcript, so the keyboard shortcuts are meant for it.
    var active = false
    var dragging = false
    var lastPoint = CGPoint.zero
    var scrollTimer: Timer?
}

extension ProseStyle {
    var textStyle: NSFont.TextStyle {
        switch self {
        case .body: .body
        case .callout: .callout
        }
    }
}

extension TranscriptSelection {
    private var state: SelectionInput {
        if let state = input as? SelectionInput { return state }
        let state = SelectionInput()
        input = state
        return state
    }

    // MARK: Copy

    @discardableResult
    func copy(in chat: SessionChat) -> Bool {
        let text = text(in: chat)
        guard !text.isEmpty else { return false }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        Log.info("transcript \(chat.name): copied \(text.utf16.count) characters from rows \(rows(in: chat).map(\.id))")
        return true
    }

    // MARK: Mouse

    /// The prose block under a point, if one is.
    private func target(at point: CGPoint) -> Int? {
        targets.first { $0.value.frame.contains(point) }?.key
    }

    /// The text position nearest a point: the closest built row's prose, clamped to its ends.
    func resolve(_ point: CGPoint, in chat: SessionChat) -> Point? {
        func distance(_ rect: CGRect) -> CGFloat {
            let dx = max(rect.minX - point.x, 0, point.x - rect.maxX)
            let dy = max(rect.minY - point.y, 0, point.y - rect.maxY)
            return dx * dx + dy * dy
        }
        guard let (id, target) = targets.min(by: { distance($0.value.frame) < distance($1.value.frame) }),
              let item = chat.items.first(where: { $0.id == id }) else { return nil }
        let geometry = TextGeometry(item.plainText(expanded: chat.expanded), style: target.style, width: target.frame.width)
        return Point(row: id, offset: geometry.offset(at: CGPoint(x: point.x - target.frame.minX, y: point.y - target.frame.minY)))
    }

    func mouseDown(at point: CGPoint, extend: Bool, in chat: SessionChat) {
        state.active = true
        guard target(at: point) != nil, let position = resolve(point, in: chat) else {
            state.dragging = false
            clear()
            return
        }
        state.dragging = true
        state.lastPoint = point
        if extend, anchor != nil { head = position } else { select(from: position, to: position) }
    }

    func mouseDragged(to point: CGPoint, in chat: SessionChat, window: NSWindow) {
        guard state.dragging else { return }
        state.lastPoint = point
        if let position = resolve(point, in: chat) { head = position }
        autoscroll(chat, window: window)
    }

    func mouseUp() {
        guard state.dragging else { return }
        state.dragging = false
        stopScrolling()
        if anchor == head { clear() }
    }

    /// While the pointer is held past the top or bottom of the transcript, scroll toward it and
    /// keep extending the selection.
    private func autoscroll(_ chat: SessionChat, window: NSWindow) {
        guard overshoot(chat) != 0 else { return stopScrolling() }
        guard state.scrollTimer == nil else { return }
        state.scrollTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self, weak chat, weak window] _ in
            MainActor.assumeIsolated {
                guard let self, let chat, let window else { return }
                self.tick(chat, window: window)
            }
        }
    }

    private func overshoot(_ chat: SessionChat) -> CGFloat {
        let frame = chat.frameInWindow
        if state.lastPoint.y < frame.minY { return state.lastPoint.y - frame.minY }
        if state.lastPoint.y > frame.maxY { return state.lastPoint.y - frame.maxY }
        return 0
    }

    private func tick(_ chat: SessionChat, window: NSWindow) {
        let over = overshoot(chat)
        guard state.dragging, over != 0, let scroll = chat.scrollView(in: window), let document = scroll.documentView else {
            return stopScrolling()
        }
        let clip = scroll.contentView
        let step = min(max(abs(over) * 0.5, 4), 30) * (over < 0 ? -1 : 1)
        let bottom = max(0, document.frame.height - clip.bounds.height)
        let y = min(max(0, clip.bounds.origin.y + step), bottom)
        chat.userScrolling = true
        clip.scroll(to: NSPoint(x: clip.bounds.origin.x, y: y))
        scroll.reflectScrolledClipView(clip)
        DispatchQueue.main.async { chat.userScrolling = false }
        if let position = resolve(state.lastPoint, in: chat) { head = position }
    }

    private func stopScrolling() {
        state.scrollTimer?.invalidate()
        state.scrollTimer = nil
    }

    // MARK: Events

    func watch(_ chat: SessionChat) {
        stop()
        state.monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .leftMouseDragged, .leftMouseUp, .keyDown]) { [weak self, weak chat] event in
            guard let self, let chat else { return event }
            return self.handle(event, chat: chat)
        }
    }

    func stop() {
        if let monitor = state.monitor { NSEvent.removeMonitor(monitor) }
        state.monitor = nil
        stopScrolling()
        state.dragging = false
    }

    private func handle(_ event: NSEvent, chat: SessionChat) -> NSEvent? {
        guard let window = event.window, window.title == "Wheel", let height = window.contentView?.bounds.height else { return event }
        let point = CGPoint(x: event.locationInWindow.x, y: height - event.locationInWindow.y)
        switch event.type {
        case .leftMouseDown:
            if chat.frameInWindow.contains(point) {
                mouseDown(at: point, extend: event.modifierFlags.contains(.shift), in: chat)
            } else {
                state.active = false
                clear()
            }
        case .leftMouseDragged:
            mouseDragged(to: point, in: chat, window: window)
        case .leftMouseUp:
            mouseUp()
        case .keyDown:
            return key(event, chat: chat, window: window) ? nil : event
        default:
            break
        }
        return event
    }

    /// ⌘C copies the selection, ⌘A selects every row, Esc clears. They apply while the last press
    /// landed in this transcript, though the composer stays first responder after it; ⌘C yields to
    /// an editor that holds a selection of its own.
    private func key(_ event: NSEvent, chat: SessionChat, window: NSWindow) -> Bool {
        guard state.active, window.isKeyWindow else { return false }
        let editor = window.firstResponder as? NSText
        let command = event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command
        switch event.charactersIgnoringModifiers {
        case "c" where command && !isEmpty:
            if let editor, editor.selectedRange.length > 0 { return false }
            return copy(in: chat)
        case "a" where command:
            selectAll(in: chat)
            return true
        case "\u{1B}" where !isEmpty:
            clear()
            return true
        default:
            return false
        }
    }
}

extension SessionChat {
    /// The scroll view behind this transcript. SwiftUI content is not made of views, so a hit test
    /// stops at the hosting view: look through the window's scroll views for the smallest one
    /// under the transcript's middle.
    func scrollView(in window: NSWindow) -> NSScrollView? {
        guard let content = window.contentView, frameInWindow != .zero else { return nil }
        let point = NSPoint(x: frameInWindow.midX, y: content.isFlipped ? frameInWindow.midY : content.bounds.height - frameInWindow.midY)
        func scrollViews(in view: NSView) -> [NSScrollView] {
            view.subviews.flatMap { ($0 as? NSScrollView).map { [$0] } ?? [] + scrollViews(in: $0) }
                + view.subviews.compactMap { $0 as? NSScrollView }.flatMap { scrollViews(in: $0) }
        }
        return scrollViews(in: content)
            .filter { $0.convert($0.bounds, to: content).contains(point) }
            .min { $0.bounds.width * $0.bounds.height < $1.bounds.width * $1.bounds.height }
    }
}

/// Where the characters of a text fall when laid out at a width, so a point finds an offset and
/// an offset finds its rectangles. Exact for plain prose; Markdown spacing is approximated.
struct TextGeometry {
    private let storage: NSTextStorage
    private let manager = NSLayoutManager()
    private let container: NSTextContainer
    let length: Int

    init(_ text: String, style: ProseStyle, width: CGFloat) {
        storage = NSTextStorage(string: text, attributes: [.font: NSFont.preferredFont(forTextStyle: style.textStyle)])
        container = NSTextContainer(size: NSSize(width: max(width, 1), height: .greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        manager.addTextContainer(container)
        storage.addLayoutManager(manager)
        manager.ensureLayout(for: container)
        length = (text as NSString).length
    }

    /// The offset nearest a point in the text's own top-left coordinates, clamped to the text.
    func offset(at point: CGPoint) -> Int {
        guard length > 0 else { return 0 }
        let used = manager.usedRect(for: container)
        if point.y < used.minY { return 0 }
        if point.y > used.maxY { return length }
        var fraction: CGFloat = 0
        let index = manager.characterIndex(for: point, in: container, fractionOfDistanceBetweenInsertionPoints: &fraction)
        return min(length, index + (fraction > 0.5 ? 1 : 0))
    }

    /// The point of the insertion position before `offset`, in the text's top-left coordinates.
    func point(of offset: Int) -> CGPoint {
        guard length > 0 else { return CGPoint(x: 0, y: 5) }
        let index = min(max(offset, 0), length)
        if index == length {
            let last = manager.boundingRect(forGlyphRange: NSRange(location: manager.numberOfGlyphs - 1, length: 1), in: container)
            return CGPoint(x: last.maxX - 1, y: last.midY)
        }
        let box = manager.boundingRect(forGlyphRange: NSRange(location: manager.glyphIndexForCharacter(at: index), length: 1), in: container)
        return CGPoint(x: box.minX + 1, y: box.midY)
    }

    /// One rectangle per line the range covers.
    func rects(for range: Range<Int>) -> [CGRect] {
        guard length > 0 else { return [] }
        let glyphs = manager.glyphRange(forCharacterRange: NSRange(location: range.lowerBound, length: range.count), actualCharacterRange: nil)
        var found: [CGRect] = []
        manager.enumerateEnclosingRects(forGlyphRange: glyphs, withinSelectedGlyphRange: NSRange(location: NSNotFound, length: 0), in: container) { rect, _ in
            found.append(rect)
        }
        return found
    }
}

private let selectionFill = Color(nsColor: .selectedTextBackgroundColor)

struct ProseHighlight: View {
    @ObservedObject var selection: TranscriptSelection
    let chat: SessionChat
    let id: Int
    let text: String
    let style: ProseStyle

    var body: some View {
        GeometryReader { proxy in
            if let range = selection.range(of: id, in: chat, length: text.utf16.count) {
                Path { path in
                    for rect in TextGeometry(text, style: style, width: proxy.size.width).rects(for: range) { path.addRect(rect) }
                }
                .fill(selectionFill)
            }
        }
        .allowsHitTesting(false)
    }
}

/// The highlight of a row that is not prose: a tool line, a group, a plan, a notice.
struct ItemHighlight: View {
    @ObservedObject var selection: TranscriptSelection
    let chat: SessionChat
    let item: ChatItem

    var body: some View {
        let length = item.plainText(expanded: chat.expanded).utf16.count
        if length > 0, selection.range(of: item.id, in: chat, length: length) != nil {
            selectionFill.clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
        }
    }
}
