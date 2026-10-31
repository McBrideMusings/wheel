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
    /// The font a block in this style is drawn in.
    var nsFont: NSFont {
        let size = CGFloat(size)
        switch family {
        case .system: return NSFont.systemFont(ofSize: size)
        case .mono: return NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
        case .serif, .rounded:
            let base = NSFont.systemFont(ofSize: size)
            let design: NSFontDescriptor.SystemDesign = family == .serif ? .serif : .rounded
            let descriptor = base.fontDescriptor.withDesign(design) ?? base.fontDescriptor
            return NSFont(descriptor: descriptor, size: size) ?? base
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

    /// The layout a row's text is drawn in: the one SwiftUI reported for a reply, or the text laid
    /// out again in its style for a row drawn as one piece of plain text.
    func layout(of item: ChatItem, target: Target, in chat: SessionChat) -> any ProseLayout {
        if let drawn = drawn[item.id], !drawn.isEmpty { return drawn }
        return TextGeometry(item.prose(expanded: chat.expanded), style: target.style, width: target.frame.width)
    }

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
        let geometry = layout(of: item, target: target, in: chat)
        return Point(row: id, offset: geometry.offset(at: CGPoint(x: point.x - target.frame.minX, y: point.y - target.frame.minY)))
    }

    /// A press at `point`. `clicks` is the click count: two selects the word, three the paragraph,
    /// and neither starts a drag.
    func mouseDown(at point: CGPoint, extend: Bool, clicks: Int = 1, in chat: SessionChat) {
        state.active = true
        guard target(at: point) != nil, let position = resolve(point, in: chat) else {
            state.dragging = false
            clear()
            return
        }
        if clicks >= 2 {
            state.dragging = false
            let done = clicks == 2 ? selectWord(at: position, in: chat) : selectParagraph(at: position, in: chat)
            Log.info("transcript \(chat.name): \(clicks == 2 ? "word" : "paragraph") selected at row \(position.row) offset \(position.offset): \(done)")
            return
        }
        state.dragging = true
        state.lastPoint = point
        if extend, anchor != nil { head = position } else { select(from: position, to: position) }
    }

    // MARK: Keyboard extension

    /// Shift-arrows move the selection's free end: Left and Right by a character (Option: a
    /// word), Up and Down by a line.
    func extend(keyCode: UInt16, byWord: Bool, in chat: SessionChat) -> Bool {
        switch keyCode {
        case 123: extendHead(forward: false, by: byWord ? .word : .character, in: chat)
        case 124: extendHead(forward: true, by: byWord ? .word : .character, in: chat)
        case 125: extendHeadByLine(down: true, in: chat)
        case 126: extendHeadByLine(down: false, in: chat)
        default: return false
        }
        return true
    }

    private func extendHeadByLine(down: Bool, in chat: SessionChat) {
        guard let head, let target = targets[head.row],
              let item = chat.items.first(where: { $0.id == head.row }) else { return }
        let geometry = layout(of: item, target: target, in: chat)
        let here = geometry.point(of: head.offset)
        let line = geometry.lineHeight(at: head.offset)
        let y = here.y + (down ? line : -line)
        if y >= 0, y < geometry.height {
            self.head = Point(row: head.row, offset: geometry.offset(at: CGPoint(x: here.x, y: y)))
        } else if let next = neighbour(of: head.row, forward: down, in: chat) {
            var offset = down ? 0 : next.length
            if let nextTarget = targets[next.id], let nextItem = chat.items.first(where: { $0.id == next.id }) {
                let nextGeometry = layout(of: nextItem, target: nextTarget, in: chat)
                offset = nextGeometry.offset(at: CGPoint(x: here.x, y: down ? 1 : nextGeometry.height - 1))
            }
            self.head = Point(row: next.id, offset: offset)
        }
    }

    // MARK: Context menu

    /// Right-click on prose: with nothing selected the word under the pointer is, then a menu
    /// offers Copy and Select All.
    private func showMenu(for event: NSEvent, at point: CGPoint, chat: SessionChat, window: NSWindow) {
        state.active = true
        if isEmpty, target(at: point) != nil, let position = resolve(point, in: chat) { selectWord(at: position, in: chat) }
        let responder = MenuResponder(copy: { [weak self, weak chat] in
            if let self, let chat { self.copy(in: chat) }
        }, selectAll: { [weak self, weak chat] in
            if let self, let chat { self.selectAll(in: chat) }
        })
        let menu = NSMenu()
        let copyItem = NSMenuItem(title: "Copy", action: #selector(MenuResponder.copyItem), keyEquivalent: "c")
        copyItem.target = responder
        copyItem.isEnabled = !isEmpty
        let allItem = NSMenuItem(title: "Select All", action: #selector(MenuResponder.selectAllItem), keyEquivalent: "a")
        allItem.target = responder
        menu.addItem(copyItem)
        menu.addItem(allItem)
        Log.info("transcript \(chat.name): context menu at \(Int(point.x)),\(Int(point.y)), copy \(isEmpty ? "disabled" : "enabled")")
        if let view = window.contentView {
            withExtendedLifetime(responder) { NSMenu.popUpContextMenu(menu, with: event, for: view) }
        }
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
        state.monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .leftMouseDragged, .leftMouseUp, .rightMouseDown, .keyDown]) { [weak self, weak chat] event in
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
        guard let window = event.window, window.isWheelWindow, let point = event.topLeftLocation else { return event }
        switch event.type {
        case .leftMouseDown:
            if chat.frameInWindow.contains(point) {
                mouseDown(at: point, extend: event.modifierFlags.contains(.shift), clicks: event.clickCount, in: chat)
            } else {
                state.active = false
                clear()
            }
        case .leftMouseDragged:
            mouseDragged(to: point, in: chat, window: window)
        case .leftMouseUp:
            mouseUp()
        case .rightMouseDown:
            guard chat.frameInWindow.contains(point), target(at: point) != nil else { return event }
            showMenu(for: event, at: point, chat: chat, window: window)
            return nil
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
        guard state.active else { return false }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let typing = (window.firstResponder as? NSText).map { !$0.string.isEmpty } ?? false
        if flags.contains(.shift), flags.isDisjoint(with: [.command, .control]), !isEmpty, !typing,
           extend(keyCode: event.keyCode, byWord: flags.contains(.option), in: chat) {
            return true
        }
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

/// The target of the context menu's items, which AppKit calls by selector.
@MainActor
private final class MenuResponder: NSObject {
    let copy: () -> Void
    let selectAll: () -> Void

    init(copy: @escaping () -> Void, selectAll: @escaping () -> Void) {
        self.copy = copy
        self.selectAll = selectAll
    }

    @objc func copyItem() { copy() }
    @objc func selectAllItem() { selectAll() }
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

/// What the selection asks of a row's text layout, in the row's own top-left coordinates.
protocol ProseLayout {
    var height: CGFloat { get }
    func offset(at point: CGPoint) -> Int
    func point(of offset: Int) -> CGPoint
    func lineHeight(at offset: Int) -> CGFloat
    func rects(for range: Range<Int>) -> [CGRect]
}

extension TextGeometry: ProseLayout {}

extension DrawnText: ProseLayout {
    var height: CGFloat { bounds.maxY }
}

/// Where the characters of a text fall when laid out at a width, so a point finds an offset and
/// an offset finds its rectangles. Bold, italic, code and heading faces are laid out as drawn;
/// the space between blocks is approximated by a blank line.
struct TextGeometry {
    private let storage: NSTextStorage
    private let manager = NSLayoutManager()
    private let container: NSTextContainer
    let length: Int

    init(_ prose: RenderedProse, style: ProseStyle, width: CGFloat) {
        let base = style.nsFont
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = CGFloat(style.lineSpacing)
        storage = NSTextStorage(string: prose.text, attributes: [.font: base, .paragraphStyle: paragraph])
        for run in prose.runs {
            storage.addAttribute(.font, value: Self.font(for: run.face, base: base), range: NSRange(location: run.range.lowerBound, length: run.range.count))
        }
        container = NSTextContainer(size: NSSize(width: max(width, 1), height: .greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        manager.addTextContainer(container)
        storage.addLayoutManager(manager)
        manager.ensureLayout(for: container)
        length = prose.length
    }

    private static func font(for face: RenderedProse.Face, base: NSFont) -> NSFont {
        let manager = NSFontManager.shared
        switch face {
        case .plain: return base
        case .strong: return manager.convert(base, toHaveTrait: .boldFontMask)
        case .emphasis: return manager.convert(base, toHaveTrait: .italicFontMask)
        case .strongEmphasis: return manager.convert(manager.convert(base, toHaveTrait: .boldFontMask), toHaveTrait: .italicFontMask)
        case .code: return NSFont.monospacedSystemFont(ofSize: base.pointSize * 0.88, weight: .regular)
        case let .heading(level):
            let scale: CGFloat = level == 1 ? 1.25 : level == 2 ? 1.125 : 1
            return NSFont.systemFont(ofSize: base.pointSize * scale, weight: .semibold)
        }
    }

    /// The height of the line holding `offset`.
    func lineHeight(at offset: Int) -> CGFloat {
        guard length > 0 else { return NSFont.preferredFont(forTextStyle: .body).boundingRectForFont.height }
        let glyph = manager.glyphIndexForCharacter(at: min(max(offset, 0), length - 1))
        return manager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil).height
    }

    /// The text's height when laid out.
    var height: CGFloat { manager.usedRect(for: container).height }

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
    let prose: RenderedProse
    let style: ProseStyle

    var body: some View {
        GeometryReader { proxy in
            if let range = selection.range(of: id, in: chat, length: prose.length) {
                Path { path in
                    let rects = selection.drawn[id].flatMap { $0.isEmpty ? nil : $0 }?.rects(for: range)
                        ?? TextGeometry(prose, style: style, width: proxy.size.width).rects(for: range)
                    for rect in rects { path.addRect(rect) }
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
