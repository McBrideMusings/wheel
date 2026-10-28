import SwiftUI

/// One text selection across a whole transcript, held over the model's text and not over the row
/// views: a List builds only the rows near its viewport, and a view's own selection cannot span
/// sibling views. Offsets count UTF-16 units into a row's plain text. The pointer and keyboard
/// that drive it belong to the platform, which keeps its state in `input`.
@MainActor
public final class TranscriptSelection: ObservableObject {
    public struct Point: Equatable, Sendable {
        public var row: Int
        public var offset: Int

        public init(row: Int, offset: Int) {
            self.row = row
            self.offset = offset
        }
    }

    /// Where a built row's prose sits in the window (top-left origin), and the style it is drawn in.
    public struct Target {
        public var frame: CGRect
        public var style: ProseStyle

        public init(frame: CGRect, style: ProseStyle) {
            self.frame = frame
            self.style = style
        }
    }

    @Published public private(set) var anchor: Point?
    @Published public var head: Point?
    /// The prose each built row draws, by row id. Not published: rows report it as they lay out.
    public var targets: [Int: Target] = [:]
    /// The platform's pointer and key handling for this selection, held while the transcript is on screen.
    public var input: AnyObject?

    public init() {}

    public var isEmpty: Bool { anchor == nil || anchor == head }

    public func clear() {
        guard anchor != nil || head != nil else { return }
        anchor = nil
        head = nil
    }

    public func select(from: Point, to: Point) {
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
    public func rows(in chat: SessionChat) -> [(id: Int, range: Range<Int>)] {
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
    public func text(in chat: SessionChat) -> String {
        let items = chat.items
        return rows(in: chat).compactMap { row in
            guard let item = items.first(where: { $0.id == row.id }) else { return nil }
            return (item.plainText(expanded: chat.expanded) as NSString).substring(with: NSRange(location: row.range.lowerBound, length: row.range.count))
        }.joined(separator: "\n\n")
    }

    /// The selected range of one row's text of `length`, for a row to draw its highlight.
    public func range(of id: Int, in chat: SessionChat, length: Int) -> Range<Int>? {
        let items = chat.items
        guard let ends = ends(in: items), let index = items.firstIndex(where: { $0.id == id }) else { return nil }
        return span(index: index, length: length, ends: ends)
    }

    /// Select every row that has text.
    public func selectAll(in chat: SessionChat) {
        let withText = chat.items.filter { !$0.plainText(expanded: chat.expanded).isEmpty }
        guard let first = withText.first, let last = withText.last else { return }
        select(from: Point(row: first.id, offset: 0),
               to: Point(row: last.id, offset: last.plainText(expanded: chat.expanded).utf16.count))
    }

    // MARK: Words, paragraphs and keyboard extension

    private func prose(of row: Int, in chat: SessionChat) -> RenderedProse? {
        chat.items.first { $0.id == row }?.prose(expanded: chat.expanded)
    }

    /// Select the word at `point`, as a double-click does. False when no word is there.
    @discardableResult
    public func selectWord(at point: Point, in chat: SessionChat) -> Bool {
        guard let range = prose(of: point.row, in: chat)?.wordRange(at: point.offset) else { return false }
        select(from: Point(row: point.row, offset: range.lowerBound), to: Point(row: point.row, offset: range.upperBound))
        return true
    }

    /// Select the paragraph (or list item, or code line) at `point`, as a triple-click does.
    @discardableResult
    public func selectParagraph(at point: Point, in chat: SessionChat) -> Bool {
        guard let range = prose(of: point.row, in: chat)?.paragraphRange(at: point.offset) else { return false }
        select(from: Point(row: point.row, offset: range.lowerBound), to: Point(row: point.row, offset: range.upperBound))
        return true
    }

    public enum Step: Sendable {
        case character, word
    }

    /// Move the selection's free end one character or word back (`forward` false) or on,
    /// crossing into the neighbouring row that has text when it runs off an end.
    public func extendHead(forward: Bool, by step: Step, in chat: SessionChat) {
        guard let head, let prose = prose(of: head.row, in: chat) else { return }
        let string = prose.text as NSString
        var offset = head.offset
        if forward {
            if offset < string.length {
                offset = step == .word ? wordEdge(string, from: offset, forward: true) : string.composedBounds(at: offset).upperBound
                self.head = Point(row: head.row, offset: offset)
            } else if let next = neighbour(of: head.row, forward: true, in: chat) {
                self.head = Point(row: next.id, offset: 0)
            }
        } else {
            if offset > 0 {
                offset = step == .word ? wordEdge(string, from: offset, forward: false) : string.composedBounds(at: offset - 1).lowerBound
                self.head = Point(row: head.row, offset: offset)
            } else if let previous = neighbour(of: head.row, forward: false, in: chat) {
                self.head = Point(row: previous.id, offset: previous.length)
            }
        }
    }

    /// The nearest row before or after `row` that has text.
    public func neighbour(of row: Int, forward: Bool, in chat: SessionChat) -> (id: Int, length: Int)? {
        let items = chat.items
        guard let index = items.firstIndex(where: { $0.id == row }) else { return nil }
        let candidates = forward ? Array(items[(index + 1)...]) : Array(items[..<index].reversed())
        for item in candidates {
            let length = item.prose(expanded: chat.expanded).length
            if length > 0 { return (item.id, length) }
        }
        return nil
    }

    private func wordEdge(_ string: NSString, from offset: Int, forward: Bool) -> Int {
        var edge = forward ? string.length : 0
        string.enumerateSubstrings(in: NSRange(location: 0, length: string.length), options: [.byWords, .substringNotRequired]) { _, range, _, stop in
            if forward, range.location + range.length > offset {
                edge = range.location + range.length
                stop.pointee = true
            } else if !forward, range.location < offset {
                edge = range.location
            }
        }
        return edge
    }
}

extension NSString {
    fileprivate func composedBounds(at index: Int) -> (lowerBound: Int, upperBound: Int) {
        let range = rangeOfComposedCharacterSequence(at: index)
        return (range.location, range.location + range.length)
    }
}
