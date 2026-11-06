import SwiftUI

// MARK: Table — SwiftUI's Table of rows under section titles (ADR-0029: it knows nothing of what the rows are)
// column {key, label, width|null, tree, wide}; section {title, count, rows}; row {id, path, depth, kids, collapsed, dim, sel,
// cells [{key, text, tone, mono, chip}], menu [{label, action, context}]}. A `wide` column shows only when the pane is 640 pt across.
// Raises the table's `rowAction` with {id} when a row is picked, its `foldAction` with {path} when a tree row is folded, and a
// menu item's own `action` with its `context`.
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

    private func fire(_ name: String, _ context: [String: AnyCodable]) {
        surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: context)
    }

    private static func lines(_ sections: [[String: AnyCodable]]) -> [Line] {
        var lines: [Line] = []
        for (si, sec) in sections.enumerated() {
            let rows = (sec["rows"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
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
        let lines = Self.lines(sections)
        let rowAction = p["rowAction"]?.stringValue ?? ""
        let foldAction = p["foldAction"]?.stringValue ?? ""
        let selected = Binding<String?>(
            get: { nil },
            set: { id in
                guard let id, let line = lines.first(where: { $0.id == id }), line.title == nil else { return }
                fire(rowAction, ["id": .string(line.row["id"]?.stringValue ?? "")])
            })
        Table(lines, selection: selected) {
            TableColumnForEach(Array(cols.enumerated()), id: \.offset) { _, c in
                TableColumn(c["label"]?.stringValue ?? "") { line in cell(line, c, foldAction) }
                    .width(min: c["width"]?.numberValue.map { CGFloat($0) }, ideal: c["width"]?.numberValue.map { CGFloat($0) },
                           max: c["width"]?.numberValue.map { CGFloat($0) })
            }
        }
        .contextMenu(forSelectionType: String.self) { ids in
            if let id = ids.first, let line = lines.first(where: { $0.id == id }) {
                ForEach(Array((line.row["menu"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.enumerated()), id: \.offset) { _, item in
                    Button(item["label"]?.stringValue ?? "") { fire(item["action"]?.stringValue ?? "", item["context"]?.dictionaryValue ?? [:]) }
                }
            }
        }
        .overlay {
            if lines.isEmpty { Text(p["empty"]?.stringValue ?? "Nothing matches the search and filters.").foregroundStyle(.secondary) }
        }
        .accessibilityIdentifier("a2ui.table")
    }

    @ViewBuilder
    private func cell(_ line: Line, _ c: [String: AnyCodable], _ foldAction: String) -> some View {
        let key = c["key"]?.stringValue ?? ""
        if let title = line.title {
            if c["tree"]?.boolValue == true || key == (surfaceFirstKey()) { Text(title).font(.headline) } else { Text("") }
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
                    .font(cell["mono"]?.boolValue == true ? .system(.caption, design: .monospaced) : cell["chip"]?.boolValue == true ? .callout.weight(.semibold) : .body)
                    .foregroundStyle(cellColor(cell, dim: line.row["dim"]?.boolValue == true))
                    .lineLimit(1).truncationMode(.tail)
            }
        }
    }

    /// The first column's key, the one a section's title stands in.
    private func surfaceFirstKey() -> String {
        ((node.instance.properties["columns"]?.arrayValue ?? []).first?.dictionaryValue?["key"]?.stringValue) ?? ""
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
