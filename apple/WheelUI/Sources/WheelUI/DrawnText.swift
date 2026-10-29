import SwiftUI

/// Marks a `Text` inside a reply that is not the reply's prose (a code block's header, its line
/// numbers, a diff line), so the selection leaves it out of the drawn layout.
struct ChatNonProse: TextAttribute {}

/// One text layout SwiftUI published, with where it sits in the reply's own coordinates.
struct DrawnLayout: Equatable {
    let layout: Text.Layout
    let origin: CGPoint
}

/// The characters of one reply where SwiftUI drew them. Textual lays a reply out block by block
/// as `Text`; each `Text` publishes its layout, whose lines carry the text they draw and the
/// rectangle they fill. A point finds its character here, and a character its rectangle, from the
/// layout on screen, not from laying the text out again. Offsets count `RenderedProse.text`, so a
/// block's text is found in it in reading order.
public final class DrawnText {
    private struct Line {
        let rect: CGRect
        let fragment: NSTextLineFragment
        /// The prose offset of the fragment string's first character.
        let base: Int

        var lower: Int { base + fragment.characterRange.location }
        var upper: Int { base + fragment.characterRange.location + fragment.characterRange.length }
    }

    private let layouts: [DrawnLayout]
    private let prose: RenderedProse
    private lazy var lines: [Line] = makeLines()

    /// `layouts` are the reply's own text in reading order; `blocks` are the layouts of code blocks
    /// that scroll, which Textual hides from the reply's, each placed after the text above it.
    init(layouts: [DrawnLayout], blocks: [[DrawnLayout]], prose: RenderedProse) {
        var merged = layouts
        for block in blocks.sorted(by: { ($0.first?.origin.y ?? 0) < ($1.first?.origin.y ?? 0) }) {
            guard let top = block.first?.origin.y else { continue }
            let at = merged.lastIndex { $0.origin.y <= top }.map { $0 + 1 } ?? 0
            merged.insert(contentsOf: block, at: at)
        }
        self.layouts = merged
        self.prose = prose
    }

    private func makeLines() -> [Line] {
        let source = prose.text as NSString
        var found: [Line] = []
        var bases: [ObjectIdentifier: Int] = [:]
        var cursor = 0
        for drawnLayout in layouts {
            let layout = drawnLayout.layout, origin = drawnLayout.origin
            if layout.first?.first?[ChatNonProse.self] != nil { continue }
            for line in layout {
                guard let fragment = line.lineFragment else { continue }
                let string = fragment.attributedString
                let base: Int
                if let known = bases[ObjectIdentifier(string)] {
                    base = known
                } else {
                    let text = string.string as NSString
                    let range = text.length == 0 || cursor > source.length ? NSRange(location: NSNotFound, length: 0)
                        : source.range(of: text as String, options: [], range: NSRange(location: cursor, length: source.length - cursor))
                    // A short fragment is a list marker, which sits right where the cursor is; one
                    // found further on is the same characters in a later sentence. A fragment the
                    // prose does not hold (a marker it spells another way) leaves the cursor alone.
                    let found = range.location != NSNotFound && (text.length > 3 || range.location - cursor <= 4)
                    base = found ? range.location : min(cursor, source.length)
                    if found { cursor = base + text.length }
                    bases[ObjectIdentifier(string)] = base
                }
                found.append(Line(rect: line.typographicBounds.rect.offsetBy(dx: origin.x, dy: origin.y), fragment: fragment, base: base))
            }
        }
        return found
    }

    /// The union of every line, in the reply's own top-left coordinates.
    public var bounds: CGRect { lines.reduce(CGRect.null) { $0.union($1.rect) } }

    public var isEmpty: Bool { lines.isEmpty }

    /// The length of the prose the layout was matched against.
    var proseLength: Int { prose.length }

    /// The end of the line's text in the fragment, leaving out its line break.
    private func visibleUpper(of line: Line) -> Int {
        let string = line.fragment.attributedString.string as NSString
        var end = line.fragment.characterRange.location + line.fragment.characterRange.length
        while end > line.fragment.characterRange.location, end <= string.length,
              let scalar = Unicode.Scalar(string.character(at: end - 1)), CharacterSet.newlines.contains(scalar) {
            end -= 1
        }
        return line.base + end
    }

    private func x(of offset: Int, in line: Line) -> CGFloat {
        let index = min(max(offset - line.base, line.fragment.characterRange.location),
                        line.fragment.characterRange.location + line.fragment.characterRange.length)
        return line.rect.minX + line.fragment.locationForCharacter(at: index).x
    }

    /// The offset nearest a point in the reply's own top-left coordinates.
    public func offset(at point: CGPoint) -> Int {
        guard !lines.isEmpty else { return 0 }
        func distance(_ rect: CGRect) -> CGFloat {
            let dy = max(rect.minY - point.y, 0, point.y - rect.maxY)
            let dx = max(rect.minX - point.x, 0, point.x - rect.maxX)
            return dy * 1000 + dx
        }
        guard let line = lines.min(by: { distance($0.rect) < distance($1.rect) }) else { return 0 }
        let lower = line.lower, upper = visibleUpper(of: line)
        guard upper > lower else { return lower }
        if point.x <= x(of: lower, in: line) { return lower }
        if point.x >= x(of: upper, in: line) { return upper }
        var low = lower, high = upper
        while high - low > 1 {
            let middle = (low + high) / 2
            if x(of: middle, in: line) <= point.x { low = middle } else { high = middle }
        }
        // `low` and `high` straddle the point; the nearer edge wins.
        return point.x - x(of: low, in: line) < x(of: high, in: line) - point.x ? low : high
    }

    private func line(holding offset: Int) -> Line? {
        if let inside = lines.first(where: { $0.lower <= offset && offset < visibleUpper(of: $0) }) { return inside }
        if let ending = lines.last(where: { visibleUpper(of: $0) == offset }) { return ending }
        return lines.min { gap($0, offset) < gap($1, offset) }
    }

    private func gap(_ line: Line, _ offset: Int) -> Int {
        offset < line.lower ? line.lower - offset : max(0, offset - visibleUpper(of: line))
    }

    /// The point of the insertion position before `offset`, in the reply's own top-left coordinates.
    public func point(of offset: Int) -> CGPoint {
        guard let line = line(holding: offset) else { return CGPoint(x: 0, y: 5) }
        let clamped = min(max(offset, line.lower), visibleUpper(of: line))
        let inside = clamped == visibleUpper(of: line) && clamped > line.lower
        return CGPoint(x: x(of: clamped, in: line) + (inside ? -1 : 1), y: line.rect.midY)
    }

    /// The height of the line holding `offset`.
    public func lineHeight(at offset: Int) -> CGFloat {
        line(holding: offset)?.rect.height ?? 16
    }

    /// One rectangle per line the range covers.
    public func rects(for range: Range<Int>) -> [CGRect] {
        lines.compactMap { line in
            let upper = visibleUpper(of: line)
            let from = max(range.lowerBound, line.lower), to = min(range.upperBound, max(upper, line.lower))
            guard from < to || (range.lowerBound <= line.lower && range.upperBound > upper && upper == line.lower) else { return nil }
            let left = x(of: from, in: line), right = x(of: to, in: line)
            return CGRect(x: left, y: line.rect.minY, width: max(right - left, 4), height: line.rect.height)
        }
    }
}

extension Text.Layout.Line {
    /// The line SwiftUI laid out as a TextKit fragment, which knows its string and where each
    /// character sits. SwiftUI keeps it in a private field; Textual reads the same one.
    fileprivate var lineFragment: NSTextLineFragment? {
        let mirror = Mirror(reflecting: self)
        if #available(macOS 26.0, iOS 26.0, *) {
            return mirror.descendant("_line", "nsLine", 0) as? NSTextLineFragment
        }
        return mirror.descendant("_line", "nsLine") as? NSTextLineFragment
    }
}

/// Where a reply's code blocks report the text they draw. A block that scrolls sits where Textual
/// clears the layouts it publishes, so the block reports its own, in the reply's coordinates.
struct DrawnTextSink {
    let space: String
    let chat: SessionChat
    let row: Int
    let report: @MainActor (UUID, [DrawnLayout]) -> Void
    let forget: @MainActor (UUID) -> Void
}

private struct DrawnTextSinkKey: EnvironmentKey {
    static let defaultValue: DrawnTextSink? = nil
}

extension EnvironmentValues {
    var drawnTextSink: DrawnTextSink? {
        get { self[DrawnTextSinkKey.self] }
        set { self[DrawnTextSinkKey.self] = newValue }
    }
}

private struct DrawnTextRecorder: ViewModifier {
    let prose: RenderedProse
    let id: Int
    let chat: SessionChat

    func body(content: Content) -> some View {
        let space = "reply-\(id)"
        let selection = chat.selection
        content
            .environment(\.drawnTextSink, DrawnTextSink(
                space: space,
                chat: chat,
                row: id,
                report: { selection.record(block: $0, layouts: $1, id: id) },
                forget: { selection.forget(block: $0, id: id) }))
            .coordinateSpace(name: space)
            .overlayPreferenceValue(Text.LayoutKey.self) { value in
                GeometryReader { geometry in
                    let _ = selection.record(main: value.map { DrawnLayout(layout: $0.layout, origin: geometry[$0.origin]) }, prose: prose, id: id)
                    Color.clear
                }
                .allowsHitTesting(false)
            }
            .onDisappear { selection.forgetDrawn(id) }
    }
}

/// Reports the text of one code block that scrolls to the reply that holds it.
struct DrawnBlock: ViewModifier {
    let id: UUID
    let sink: DrawnTextSink?

    func body(content: Content) -> some View {
        content
            .overlayPreferenceValue(Text.LayoutKey.self) { value in
                GeometryReader { geometry in
                    if let sink {
                        let corner = geometry.frame(in: .named(sink.space)).origin
                        let _ = sink.report(id, value.map {
                            let at = geometry[$0.origin]
                            return DrawnLayout(layout: $0.layout, origin: CGPoint(x: corner.x + at.x, y: corner.y + at.y))
                        })
                    }
                    Color.clear
                }
                .allowsHitTesting(false)
            }
            .background {
                if let sink { BlockHighlight(selection: sink.chat.selection, chat: sink.chat, row: sink.row, space: sink.space) }
            }
            .onDisappear { sink?.forget(id) }
    }
}

/// The selection over a code block's text. The block draws its own ground over the reply's
/// highlight, so the block draws the part of it that falls on its text.
private struct BlockHighlight: View {
    @ObservedObject var selection: TranscriptSelection
    let chat: SessionChat
    let row: Int
    let space: String

    var body: some View {
        GeometryReader { geometry in
            if let drawn = selection.drawn[row], let range = selection.range(of: row, in: chat, length: drawn.proseLength) {
                let frame = geometry.frame(in: .named(space))
                Path { path in
                    for rect in drawn.rects(for: range) where rect.intersects(frame) {
                        path.addRect(rect.offsetBy(dx: -frame.minX, dy: -frame.minY))
                    }
                }
                .fill(Self.fill)
            }
        }
        .allowsHitTesting(false)
    }

    private static var fill: Color {
        #if canImport(AppKit)
        Color(nsColor: .selectedTextBackgroundColor)
        #else
        Color.accentColor.opacity(0.3)
        #endif
    }
}

extension View {
    /// Reports the layout this reply's text is drawn in, for the transcript selection.
    func drawnText(_ prose: RenderedProse, id: Int, chat: SessionChat) -> some View {
        modifier(DrawnTextRecorder(prose: prose, id: id, chat: chat))
    }
}

/// The layouts a reply and its code blocks reported, by row id; a change to any of them makes a
/// new `DrawnText` for the row.
struct DrawnParts {
    var main: [DrawnLayout] = []
    var prose = RenderedProse.plain("")
    var blocks: [UUID: [DrawnLayout]] = [:]
}

extension TranscriptSelection {
    @MainActor
    func record(main layouts: [DrawnLayout], prose: RenderedProse, id: Int) {
        var parts = drawnParts[id] ?? DrawnParts()
        guard parts.main != layouts || parts.prose != prose else { return }
        parts.main = layouts
        parts.prose = prose
        update(parts, id: id)
    }

    @MainActor
    func record(block: UUID, layouts: [DrawnLayout], id: Int) {
        var parts = drawnParts[id] ?? DrawnParts()
        guard parts.blocks[block] != layouts else { return }
        parts.blocks[block] = layouts
        update(parts, id: id)
    }

    @MainActor
    func forget(block: UUID, id: Int) {
        guard var parts = drawnParts[id], parts.blocks.removeValue(forKey: block) != nil else { return }
        update(parts, id: id)
    }

    @MainActor
    func forgetDrawn(_ id: Int) {
        drawnParts[id] = nil
        drawn[id] = nil
    }

    /// Keeps `parts` as row `id`'s drawn text. A highlight that exists redraws against it.
    @MainActor
    private func update(_ parts: DrawnParts, id: Int) {
        drawnParts[id] = parts
        drawn[id] = DrawnText(layouts: parts.main, blocks: Array(parts.blocks.values), prose: parts.prose)
        if !isEmpty { DispatchQueue.main.async { [weak self] in self?.objectWillChange.send() } }
    }
}
