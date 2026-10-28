import Foundation

/// One line of a ```diff or ```patch fence.
public struct DiffFenceLine: Equatable, Sendable {
    public enum Kind: Sendable { case file, hunk, added, removed, context }
    public var kind: Kind
    public var text: String
}

public enum DiffFence {
    /// Whether a fence's language hint names a diff.
    public static func handles(_ language: String?) -> Bool {
        guard let language = language?.lowercased() else { return false }
        return language == "diff" || language == "patch"
    }

    /// The lines of a unified diff, each tagged by what it is. `---`/`+++` headers and `diff --git`
    /// lines are file headers, not removals and additions.
    public static func parse(_ text: String) -> [DiffFenceLine] {
        var lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if lines.last == "" { lines.removeLast() }
        var inHunk = false
        return lines.map { line in
            if line.hasPrefix("diff ") { inHunk = false }
            if inHunk, line.hasPrefix("+") || line.hasPrefix("-") || line.hasPrefix(" ") {
                return DiffFenceLine(kind: line.hasPrefix("+") ? .added : line.hasPrefix("-") ? .removed : .context, text: line)
            }
            if line.hasPrefix("@@") { inHunk = true }
            if line.hasPrefix("diff ") || line.hasPrefix("index ") || line.hasPrefix("--- ") || line.hasPrefix("+++ ")
                || line.hasPrefix("new file mode") || line.hasPrefix("deleted file mode") || line.hasPrefix("similarity index")
                || line.hasPrefix("rename ") {
                return DiffFenceLine(kind: .file, text: line)
            }
            if line.hasPrefix("@@") { return DiffFenceLine(kind: .hunk, text: line) }
            if line.hasPrefix("+") { return DiffFenceLine(kind: .added, text: line) }
            if line.hasPrefix("-") { return DiffFenceLine(kind: .removed, text: line) }
            return DiffFenceLine(kind: .context, text: line)
        }
    }
}

public enum SVGFence {
    /// Whether a fence's language hint names SVG.
    public static func handles(_ language: String?) -> Bool { language?.lowercased() == "svg" }

    /// The SVG rebuilt from an XML parse that keeps only drawing elements and attributes from an
    /// allow-list (shapes, paths, groups, gradients, text, transforms, presentation attributes).
    /// Everything else is dropped with its subtree: scripts, `foreignObject`, `image`, `style`,
    /// filters, animation, unknown or prefixed elements, event handlers, and any `href` or `url(…)`
    /// that points outside the document. A DOCTYPE or entity declaration, a parse error, or a root
    /// that is not `svg`, or a tree past 2000 elements or 32 levels, gives nil, so the caller shows the source instead.
    public static func sanitize(_ source: String) -> String? {
        guard let start = source.range(of: "<svg", options: .caseInsensitive) else { return nil }
        let text = String(source[start.lowerBound...])
        if text.range(of: "<!DOCTYPE", options: .caseInsensitive) != nil
            || text.range(of: "<!ENTITY", options: .caseInsensitive) != nil { return nil }
        guard let data = text.data(using: .utf8) else { return nil }
        let builder = Builder()
        let parser = XMLParser(data: data)
        parser.shouldProcessNamespaces = false
        parser.shouldResolveExternalEntities = false
        parser.delegate = builder
        guard parser.parse(), builder.sawRoot else { return nil }
        return builder.output
    }

    /// Caps on the parsed tree, so a nest of `use` references cannot stall the draw.
    private static let maxElements = 2000
    private static let maxDepth = 32

    private static let elements: Set<String> = [
        "svg", "g", "defs", "symbol", "use", "rect", "circle", "ellipse", "line", "polyline", "polygon",
        "path", "text", "tspan", "textPath", "linearGradient", "radialGradient", "stop", "pattern",
        "clipPath", "mask", "marker", "title", "desc",
    ]

    /// Elements whose character data is drawn or labels the picture.
    private static let textual: Set<String> = ["text", "tspan", "textPath", "title", "desc"]

    /// Elements that may point at another element in the same document with `href`.
    private static let referencing: Set<String> = ["use", "textPath", "linearGradient", "radialGradient", "pattern"]

    private static let attributes: Set<String> = [
        "id", "class", "style", "transform", "viewBox", "preserveAspectRatio", "width", "height", "x", "y",
        "x1", "y1", "x2", "y2", "cx", "cy", "r", "rx", "ry", "fx", "fy", "fr", "d", "points", "pathLength",
        "dx", "dy", "rotate", "textLength", "lengthAdjust", "startOffset", "method", "spacing",
        "offset", "stop-color", "stop-opacity", "gradientUnits", "gradientTransform", "spreadMethod",
        "patternUnits", "patternContentUnits", "patternTransform", "clipPathUnits", "maskUnits",
        "maskContentUnits", "clip-path", "clip-rule", "mask", "markerWidth", "markerHeight", "markerUnits",
        "refX", "refY", "orient", "marker-start", "marker-mid", "marker-end",
        "fill", "fill-opacity", "fill-rule", "stroke", "stroke-width", "stroke-opacity", "stroke-linecap",
        "stroke-linejoin", "stroke-miterlimit", "stroke-dasharray", "stroke-dashoffset", "opacity",
        "color", "display", "visibility", "overflow", "vector-effect", "paint-order",
        "font-family", "font-size", "font-style", "font-weight", "font-variant", "font-stretch",
        "text-anchor", "text-decoration", "dominant-baseline", "alignment-baseline", "baseline-shift",
        "letter-spacing", "word-spacing", "writing-mode", "direction", "xml:space",
    ]

    /// Whether every `url(…)` in `value` points at an in-document `#id`, and nothing in it can
    /// import, escape or evaluate.
    private static func isInert(_ value: String) -> Bool {
        let lower = value.lowercased()
        if lower.contains("\\") || lower.contains("@import") || lower.contains("expression(")
            || lower.contains("javascript:") || lower.contains("image(") || lower.contains("image-set(")
            || lower.contains("src(") { return false }
        var rest = Substring(lower)
        while let open = rest.range(of: "url(") {
            let after = rest[open.upperBound...].drop { $0 == " " || $0 == "\t" || $0 == "\n" || $0 == "'" || $0 == "\"" }
            guard after.hasPrefix("#") else { return false }
            rest = after
        }
        return true
    }

    /// `style` declarations kept whole or dropped whole, so a stripped value cannot rejoin another.
    private static func cleanStyle(_ style: String) -> String {
        style.split(separator: ";").filter { declaration in
            guard let colon = declaration.firstIndex(of: ":") else { return false }
            let name = declaration[..<colon].trimmingCharacters(in: .whitespaces).lowercased()
            return attributes.contains(name) && name != "style" && name != "id" && name != "class"
                && isInert(String(declaration[declaration.index(after: colon)...]))
        }.joined(separator: ";")
    }

    private static func escape(_ text: String, attribute: Bool) -> String {
        var out = ""
        for character in text {
            switch character {
            case "&": out += "&amp;"
            case "<": out += "&lt;"
            case ">": out += "&gt;"
            case "\"" where attribute: out += "&quot;"
            default: out.append(character)
            }
        }
        return out
    }

    private final class Builder: NSObject, XMLParserDelegate {
        var output = ""
        var sawRoot = false
        private var skipDepth = 0
        private var count = 0
        private var stack: [String] = []

        func parser(_ parser: XMLParser, didStartElement name: String, namespaceURI: String?,
                    qualifiedName: String?, attributes attrs: [String: String]) {
            if skipDepth > 0 { skipDepth += 1; return }
            count += 1
            if count > SVGFence.maxElements || stack.count >= SVGFence.maxDepth { parser.abortParsing(); return }
            if stack.isEmpty {
                guard name == "svg" else { parser.abortParsing(); return }
                sawRoot = true
            }
            guard SVGFence.elements.contains(name) else { skipDepth = 1; return }
            var out = "<\(name)"
            if stack.isEmpty {
                out += " xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\""
            }
            for key in attrs.keys.sorted() {
                guard var value = attrs[key] else { continue }
                if (key == "href" || key == "xlink:href") && SVGFence.referencing.contains(name) {
                    guard value.trimmingCharacters(in: .whitespaces).hasPrefix("#"), SVGFence.isInert(value) else { continue }
                } else if key == "style" {
                    value = SVGFence.cleanStyle(value)
                    if value.isEmpty { continue }
                } else {
                    guard SVGFence.attributes.contains(key), SVGFence.isInert(value) else { continue }
                }
                out += " \(key)=\"\(SVGFence.escape(value, attribute: true))\""
            }
            output += out + ">"
            stack.append(name)
        }

        func parser(_ parser: XMLParser, didEndElement name: String, namespaceURI: String?, qualifiedName: String?) {
            if skipDepth > 0 { skipDepth -= 1; return }
            if stack.popLast() != nil { output += "</\(name)>" }
        }

        func parser(_ parser: XMLParser, foundCharacters string: String) {
            guard skipDepth == 0, let top = stack.last, SVGFence.textual.contains(top) else { return }
            output += SVGFence.escape(string, attribute: false)
        }
    }
}
