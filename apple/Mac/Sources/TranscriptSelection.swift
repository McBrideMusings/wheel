import AppKit
import SwiftUI

/// One text selection across a whole transcript, held over the model's text and not over the row
/// views: a List builds only the rows near its viewport, and a view's own selection cannot span
/// sibling views. Offsets count UTF-16 units into a row's plain text.
@MainActor
final class TranscriptSelection: ObservableObject {
    struct Point: Equatable {
        var row: Int
        var offset: Int
    }

    /// Where a built row's prose sits in the window (top-left origin), and the font it is drawn in.
    struct Target {
        var frame: CGRect
        var style: NSFont.TextStyle
    }

    @Published private(set) var anchor: Point?
    @Published private(set) var head: Point?
    /// The prose each built row draws, by row id. Not published: rows report it as they lay out.
    var targets: [Int: Target] = [:]
    /// The mouse and key monitor, held while the transcript is on screen.
    private var monitor: Any?
    /// The last press landed in this transcript, so the keyboard shortcuts are meant for it.
    private var active = false
    private var dragging = false
    private var lastPoint = CGPoint.zero
    private var scrollTimer: Timer?

    var isEmpty: Bool { anchor == nil || anchor == head }

    func clear() {
        guard anchor != nil || head != nil else { return }
        anchor = nil
        head = nil
    }

    func select(from: Point, to: Point) {
        anchor = from
        head = to
    }

    // MARK: The selected text

    /// The ordered ends of the selection, with each one's index among `items`.
    private func ends(in items: [ChatItem]) -> (lo: Point, loIndex: Int, hi: Point, hiIndex: Int)? {
        guard let anchor, let head,
              let a = items.firstIndex(where: { $0.id == anchor.row }),
              let h = items.firstIndex(where: { $0.id == head.row }) else { return nil }
        return (a, anchor.offset) <= (h, head.offset) ? (anchor, a, head, h) : (head, h, anchor, a)
    }

    private func span(index: Int, length: Int, ends: (lo: Point, loIndex: Int, hi: Point, hiIndex: Int)) -> Range<Int>? {
        guard index >= ends.loIndex, index <= ends.hiIndex else { return nil }
        let start = index == ends.loIndex ? min(max(ends.lo.offset, 0), length) : 0
        let end = index == ends.hiIndex ? min(max(ends.hi.offset, 0), length) : length
        return start < end ? start..<end : nil
    }

    /// The selected slice of each row, in transcript order. Rows that are not built count, since
    /// the order and the text come from the items, not from the views.
    func rows(in chat: SessionChat) -> [(id: Int, range: Range<Int>)] {
        let items = chat.items
        guard let ends = ends(in: items) else { return [] }
        var found: [(id: Int, range: Range<Int>)] = []
        for index in ends.loIndex...ends.hiIndex {
            let length = items[index].plainText(expanded: chat.expanded).utf16.count
            if let range = span(index: index, length: length, ends: ends) { found.append((items[index].id, range)) }
        }
        return found
    }

    /// The selected rows' slices joined by blank lines.
    func text(in chat: SessionChat) -> String {
        let items = chat.items
        return rows(in: chat).compactMap { row in
            guard let item = items.first(where: { $0.id == row.id }) else { return nil }
            return (item.plainText(expanded: chat.expanded) as NSString).substring(with: NSRange(location: row.range.lowerBound, length: row.range.count))
        }.joined(separator: "\n\n")
    }

    /// The selected range of one row's text of `length`, for a row to draw its highlight.
    func range(of id: Int, in chat: SessionChat, length: Int) -> Range<Int>? {
        let items = chat.items
        guard let ends = ends(in: items), let index = items.firstIndex(where: { $0.id == id }) else { return nil }
        return span(index: index, length: length, ends: ends)
    }

    @discardableResult
    func copy(in chat: SessionChat) -> Bool {
        let text = text(in: chat)
        guard !text.isEmpty else { return false }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        Log.info("transcript \(chat.name): copied \(text.utf16.count) characters from rows \(rows(in: chat).map(\.id))")
        return true
    }

    private func selectAll(in chat: SessionChat) {
        let withText = chat.items.filter { !$0.plainText(expanded: chat.expanded).isEmpty }
        guard let first = withText.first, let last = withText.last else { return }
        select(from: Point(row: first.id, offset: 0),
               to: Point(row: last.id, offset: last.plainText(expanded: chat.expanded).utf16.count))
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
        active = true
        guard target(at: point) != nil, let position = resolve(point, in: chat) else {
            dragging = false
            clear()
            return
        }
        dragging = true
        lastPoint = point
        if extend, anchor != nil { head = position } else { select(from: position, to: position) }
    }

    func mouseDragged(to point: CGPoint, in chat: SessionChat, window: NSWindow) {
        guard dragging else { return }
        lastPoint = point
        if let position = resolve(point, in: chat) { head = position }
        autoscroll(chat, window: window)
    }

    func mouseUp() {
        guard dragging else { return }
        dragging = false
        stopScrolling()
        if anchor == head { clear() }
    }

    /// While the pointer is held past the top or bottom of the transcript, scroll toward it and
    /// keep extending the selection.
    private func autoscroll(_ chat: SessionChat, window: NSWindow) {
        guard overshoot(chat) != 0 else { return stopScrolling() }
        guard scrollTimer == nil else { return }
        scrollTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self, weak chat, weak window] _ in
            MainActor.assumeIsolated {
                guard let self, let chat, let window else { return }
                self.tick(chat, window: window)
            }
        }
    }

    private func overshoot(_ chat: SessionChat) -> CGFloat {
        let frame = chat.frameInWindow
        if lastPoint.y < frame.minY { return lastPoint.y - frame.minY }
        if lastPoint.y > frame.maxY { return lastPoint.y - frame.maxY }
        return 0
    }

    private func tick(_ chat: SessionChat, window: NSWindow) {
        let over = overshoot(chat)
        guard dragging, over != 0, let scroll = chat.scrollView(in: window), let document = scroll.documentView else {
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
        if let position = resolve(lastPoint, in: chat) { head = position }
    }

    private func stopScrolling() {
        scrollTimer?.invalidate()
        scrollTimer = nil
    }

    // MARK: Events

    func watch(_ chat: SessionChat) {
        stop()
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .leftMouseDragged, .leftMouseUp, .keyDown]) { [weak self, weak chat] event in
            guard let self, let chat else { return event }
            return self.handle(event, chat: chat)
        }
    }

    func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        stopScrolling()
        dragging = false
    }

    private func handle(_ event: NSEvent, chat: SessionChat) -> NSEvent? {
        guard let window = event.window, window.title == "Wheel", let height = window.contentView?.bounds.height else { return event }
        let point = CGPoint(x: event.locationInWindow.x, y: height - event.locationInWindow.y)
        switch event.type {
        case .leftMouseDown:
            if chat.frameInWindow.contains(point) {
                mouseDown(at: point, extend: event.modifierFlags.contains(.shift), in: chat)
            } else {
                active = false
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
        guard active, window.isKeyWindow else { return false }
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

extension ChatItem {
    /// What a row contributes to a copy: reply and message text, a tool line's title, a thought's
    /// text while the person has it opened. Nothing for a row that shows no text of its own.
    @MainActor func plainText(expanded: Set<Int>) -> String {
        switch self {
        case let .row(row):
            switch row.row.kind.displayed {
            case let .user(text, _): return text
            case let .agent(text): return text
            case let .thought(text): return expanded.contains(row.id) ? text : ""
            case let .tool(title, _, _, _): return title
            case let .plan(entries): return entries.map(\.content).joined(separator: "\n")
            case let .notice(text), let .error(text): return text
            case let .mode(mode): return "Mode: \(mode)"
            case .block: return ""
            }
        case let .tools(group):
            return group.rows.compactMap { row in
                if case let .tool(title, _, _, _) = row.row.kind { title } else { nil }
            }.joined(separator: "\n")
        }
    }

    /// Prose a row draws as text of its own, whose highlight follows the characters; any other
    /// row is highlighted whole.
    @MainActor var isProse: Bool {
        guard case let .row(row) = self else { return false }
        switch row.row.kind {
        case .user, .agent, .thought: return true
        default: return false
        }
    }
}

/// Where the characters of a text fall when laid out at a width, so a point finds an offset and
/// an offset finds its rectangles. Exact for plain prose; Markdown spacing is approximated.
struct TextGeometry {
    private let storage: NSTextStorage
    private let manager = NSLayoutManager()
    private let container: NSTextContainer
    let length: Int

    init(_ text: String, style: NSFont.TextStyle, width: CGFloat) {
        storage = NSTextStorage(string: text, attributes: [.font: NSFont.preferredFont(forTextStyle: style)])
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

extension View {
    /// Marks a view as a row's prose for the transcript selection: reports where it sits, and
    /// draws the selected characters behind it.
    func selectableText(_ text: String, id: Int, chat: SessionChat, style: NSFont.TextStyle) -> some View {
        self
            .background { ProseHighlight(selection: chat.selection, chat: chat, id: id, text: text, style: style) }
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
                chat.selection.targets[id] = TranscriptSelection.Target(frame: $0, style: style)
            }
            .onDisappear { chat.selection.targets[id] = nil }
    }
}

private struct ProseHighlight: View {
    @ObservedObject var selection: TranscriptSelection
    let chat: SessionChat
    let id: Int
    let text: String
    let style: NSFont.TextStyle

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
