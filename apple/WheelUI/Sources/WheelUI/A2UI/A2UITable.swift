import SwiftUI

// MARK: Table — SwiftUI's Table of rows under section titles (ADR-0029: it knows nothing of what the rows are)
// column {key, label, width|null, tree, wide}; section {title, count, rows}; row {id, path, depth, kids, collapsed, dim, sel,
// cells [{key, text, sort, tone, mono, chip}], menu [{label, action, context}]}. A `wide` column shows only when the pane is 640 pt across.
// A column with `sortable: true` sorts the rows of each section when its header is clicked, by each cell's `sort` (a number or a
// string) or its `text`; the rows stay in the page's order until a header is clicked, and a column without it never sorts.
// Raises the table's `rowAction` with {id} when a row is picked, its `foldAction` with {path} when a tree row is folded, and a
// menu item's own `action` with its `context`. With a `sortAction` the page owns the order: the header arrow shows the table's
// `sort` {key, asc}, a header click raises `sortAction` with the new {key, asc} and the rows stay in the page's order until the
// page sends them sorted; without one a header click sorts the rows here. With a `selectAction` the page owns the selection
// too: the rows whose ids are in `selected` draw selected, a click, a shift-click range, a ⌘-click, a click on empty ground
// or Escape raises `selectAction` with {ids} (every row selected now), and a double-click or Return raises `rowAction` with
// the row's {id}; without one a plain click raises `rowAction`.
struct A2UITableView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @Environment(\.a2uiPane) private var pane

    /// One line of the table: a section's title, or a row.
    private struct Line: Identifiable {
        let id: String
        let title: String?
        let row: [String: AnyCodable]
    }

    /// Orders two rows by one column's cells: numbers by value, text the way Finder does.
    private struct CellSort: SortComparator {
        typealias Compared = Line
        var key: String
        var order: SortOrder = .forward

        private static func value(_ row: [String: AnyCodable], _ key: String) -> (Double?, String) {
            let cell = (row["cells"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.first { $0["key"]?.stringValue == key } ?? [:]
            if let n = cell["sort"]?.numberValue { return (n, "") }
            return (nil, cell["sort"]?.stringValue ?? cell["text"]?.stringValue ?? "")
        }

        func rows(_ a: [String: AnyCodable], _ b: [String: AnyCodable]) -> ComparisonResult {
            if key.isEmpty { return .orderedSame }
            let (x, y) = (Self.value(a, key), Self.value(b, key))
            var result: ComparisonResult = .orderedSame
            if let m = x.0, let n = y.0 { result = m < n ? .orderedAscending : m > n ? .orderedDescending : .orderedSame }
            else { result = x.1.localizedStandardCompare(y.1) }
            return order == .forward ? result : (result == .orderedAscending ? .orderedDescending : result == .orderedDescending ? .orderedAscending : .orderedSame)
        }

        func compare(_ a: Line, _ b: Line) -> ComparisonResult { rows(a.row, b.row) }
    }

    @State private var sortOrder: [CellSort] = []

    private func fire(_ name: String, _ context: [String: AnyCodable]) {
        surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: context)
    }

    private static func lines(_ sections: [[String: AnyCodable]], sort: CellSort?) -> [Line] {
        var lines: [Line] = []
        for (si, sec) in sections.enumerated() {
            var rows = (sec["rows"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
            if let sort { rows.sort { sort.rows($0, $1) == .orderedAscending } }
            if let title = sec["title"]?.stringValue, !title.isEmpty {
                lines.append(Line(id: "s\(si)", title: "\(title)  \(Int(sec["count"]?.numberValue ?? 0))", row: [:]))
            }
            for (ri, r) in rows.enumerated() { lines.append(Line(id: r["path"]?.stringValue ?? "r\(si).\(ri)", title: nil, row: r)) }
        }
        return lines
    }

    var body: some View {
        let p = node.instance.properties
        let narrow = pane.width < 640
        let cols = (p["columns"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.filter { !(narrow && $0["wide"]?.boolValue == true) }
        let sections = (p["sections"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let sortAction = p["sortAction"]?.stringValue ?? ""
        let lines = Self.lines(sections, sort: sortAction.isEmpty ? sortOrder.first : nil)
        let rowAction = p["rowAction"]?.stringValue ?? ""
        let foldAction = p["foldAction"]?.stringValue ?? ""
        let selectAction = p["selectAction"]?.stringValue ?? ""
        let rowId = { (line: Line) in line.row["id"]?.stringValue ?? "" }
        // The Table selects by line id (a row's path); the page names rows by id.
        let pageSelected = Set((p["selected"]?.arrayValue ?? []).compactMap { $0.stringValue })
        let selected = Binding<Set<String>>(
            get: { selectAction.isEmpty ? [] : Set(lines.filter { $0.title == nil && pageSelected.contains(rowId($0)) }.map(\.id)) },
            set: { picked in
                let rows = lines.filter { $0.title == nil && picked.contains($0.id) }
                if selectAction.isEmpty {
                    guard let line = rows.first else { return }
                    fire(rowAction, ["id": .string(rowId(line))])
                } else {
                    let ids = Set(rows.map(rowId))
                    guard ids != pageSelected else { return }
                    fire(selectAction, ["ids": .array(ids.sorted().map { .string($0) })])
                }
            })
        let open = { (ids: Set<String>) in
            guard !selectAction.isEmpty, let line = lines.first(where: { ids.contains($0.id) && $0.title == nil }) else { return }
            fire(rowAction, ["id": .string(rowId(line))])
        }
        // With a sortAction the header arrow reads the page's `sort` and a click only tells the page; the rows never
        // re-order here. Without one the table keeps its own order in `sortOrder`.
        let pageSort = p["sort"]?.dictionaryValue
        let pageKey = pageSort?["key"]?.stringValue ?? ""
        let pageAsc = pageSort?["asc"]?.boolValue ?? true
        let order = sortAction.isEmpty ? $sortOrder : Binding<[CellSort]>(
            get: { [CellSort(key: pageKey, order: pageAsc ? .forward : .reverse)] },
            set: { new in
                guard let s = new.first, (s.key, s.order == .forward) != (pageKey, pageAsc) else { return }
                fire(sortAction, ["key": .string(s.key), "asc": .bool(s.order == .forward)])
            })
        grid(lines, cols, selected: selected, order: order, rowAction: rowAction, foldAction: foldAction, open: open)
        .overlay {
            if lines.isEmpty { Text(p["empty"]?.stringValue ?? "Nothing matches the search and filters.").foregroundStyle(.secondary) }
        }
        // A Table fills the height it is given and scrolls itself; a `frame` modifier on it sets that height.
        .frame(maxHeight: .infinity)
        .frame(minHeight: 120)
        .accessibilityIdentifier("a2ui.table")
    }

    /// The Mac draws SwiftUI's Table. On a phone a Table shows only its first column, so a List of one-line rows draws the same cells.
    @ViewBuilder
    private func grid(_ lines: [Line], _ cols: [[String: AnyCodable]], selected: Binding<Set<String>>, order: Binding<[CellSort]>, rowAction: String, foldAction: String, open: @escaping (Set<String>) -> Void) -> some View {
        let first = cols.first?["key"]?.stringValue ?? ""
        #if os(macOS)
        // A table with a sortable column shows sort arrows on every header; one without draws plain headers and never sorts.
        Group {
            if cols.contains(where: { $0["sortable"]?.boolValue == true }) {
                Table(lines, selection: selected, sortOrder: order) {
                    TableColumnForEach(Array(cols.enumerated()), id: \.offset) { _, c in
                        // A column that is not sortable compares every pair as equal, so its header changes nothing.
                        TableColumn(c["label"]?.stringValue ?? "", sortUsing: CellSort(key: c["sortable"]?.boolValue == true ? c["key"]?.stringValue ?? "" : "")) { line in cell(line, c, foldAction, first: first) }
                            // A column with a width is capped at it (it can be dragged narrower), so the leftover goes to the columns without one.
                            .width(min: c["width"]?.numberValue.map { _ in 40 }, ideal: c["width"]?.numberValue.map { CGFloat($0) }, max: c["width"]?.numberValue.map { CGFloat($0) })
                    }
                }
            } else {
                Table(lines, selection: selected) {
                    TableColumnForEach(Array(cols.enumerated()), id: \.offset) { _, c in
                        TableColumn(c["label"]?.stringValue ?? "") { line in cell(line, c, foldAction, first: first) }
                            .width(min: c["width"]?.numberValue.map { _ in 40 }, ideal: c["width"]?.numberValue.map { CGFloat($0) }, max: c["width"]?.numberValue.map { CGFloat($0) })
                    }
                }
            }
        }
        .contextMenu(forSelectionType: String.self) { ids in
            if let id = ids.first, let line = lines.first(where: { $0.id == id }) {
                ForEach(Array((line.row["menu"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.enumerated()), id: \.offset) { _, item in
                    Button(item["label"]?.stringValue ?? "") { fire(item["action"]?.stringValue ?? "", item["context"]?.dictionaryValue ?? [:]) }
                }
            }
        } primaryAction: { ids in open(ids) }
        // Escape clears a selection the page owns.
        .onExitCommand { if !selected.wrappedValue.isEmpty { selected.wrappedValue = [] } }
        #else
        List(lines) { line in
            if let title = line.title {
                Text(title).font(.system(size: 13, weight: .semibold))
            } else {
                HStack(spacing: 8) {
                    ForEach(Array(cols.enumerated()), id: \.offset) { _, c in
                        let width = c["width"]?.numberValue.map { CGFloat($0) }
                        cell(line, c, foldAction, first: first)
                            .frame(width: width, alignment: .leading)
                            .frame(maxWidth: width == nil ? .infinity : nil, alignment: .leading)
                    }
                }
                .contentShape(Rectangle())
                .onTapGesture { fire(rowAction, ["id": .string(line.row["id"]?.stringValue ?? "")]) }
                .contextMenu {
                    ForEach(Array((line.row["menu"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.enumerated()), id: \.offset) { _, item in
                        Button(item["label"]?.stringValue ?? "") { fire(item["action"]?.stringValue ?? "", item["context"]?.dictionaryValue ?? [:]) }
                    }
                }
            }
        }
        .listStyle(.plain)
        #endif
    }

    @ViewBuilder
    private func cell(_ line: Line, _ c: [String: AnyCodable], _ foldAction: String, first: String) -> some View {
        let key = c["key"]?.stringValue ?? ""
        if let title = line.title {
            if key == first { Text(title).font(.system(size: 13, weight: .semibold)) } else { Text("") }
        } else {
            let cells = (line.row["cells"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
            let cell = cells.first { $0["key"]?.stringValue == key } ?? [:]
            HStack(spacing: 0) {
                if c["tree"]?.boolValue == true {
                    Color.clear.frame(width: CGFloat(line.row["depth"]?.numberValue ?? 0) * 18, height: 1)
                    if line.row["kids"]?.boolValue == true {
                        Button(line.row["collapsed"]?.boolValue == true ? "▸" : "▾") {
                            fire(foldAction, ["path": .string(line.row["path"]?.stringValue ?? "")])
                        }
                        .buttonStyle(.borderless).frame(width: 16)
                    } else { Color.clear.frame(width: 16, height: 1) }
                }
                Text(cell["text"]?.text ?? "")
                    // Point sizes, not text styles: a text style is 17 pt on a phone and 13 pt on a Mac.
                    .font(cell["mono"]?.boolValue == true ? .system(size: 11, design: .monospaced) : cell["chip"]?.boolValue == true ? .system(size: 12, weight: .semibold) : .system(size: 13))
                    .foregroundStyle(cellColor(cell, dim: line.row["dim"]?.boolValue == true))
                    .lineLimit(1).truncationMode(.tail)
            }
        }
    }

    private func cellColor(_ cell: [String: AnyCodable], dim: Bool) -> Color {
        switch cell["tone"]?.stringValue {
        case "ok": .green
        case "warn": .orange
        case "bad": .red
        case "accent", "focus": .accentColor
        case "purple": .purple
        case "muted", "faint", "todo": .secondary
        default: dim ? .secondary : .primary
        }
    }
}
