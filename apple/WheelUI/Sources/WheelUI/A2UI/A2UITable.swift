import A2UISwiftCore
import A2UISwiftUI
import SwiftUI

// MARK: Table — the system's List of sections of one-line rows (ADR-0029: it knows nothing of what the rows are)
// column {key, label, width|null, tree, wide}; section {title, count, rows}; row {id, path, depth, kids, collapsed, dim, sel,
// cells [{key, text, tone, mono, chip, buttons [{icon, help, action, context}]}], menu [{label, action, context}]}. A `wide` column shows only when the pane is 640 pt across.
// `fit` sizes the table to its rows. A table with no `rowAction` takes no row picks. Raises the table's `rowAction` with {id} when a row is picked, its `foldAction` with {path} when a tree row is folded, and a
// menu item's own `action` with its `context`.
struct A2UITableView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @Environment(\.a2uiPane) private var pane
    @State private var expanded: Set<String> = []

    private static let cap = 10, tall = 22

    private func fire(_ name: String, _ context: [String: AnyCodable]) {
        surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: context)
    }

    var body: some View {
        let p = node.instance.properties
        let narrow = pane.width < 640
        let cols = (p["columns"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.filter { !(narrow && $0["wide"]?.boolValue == true) }
        let sections = (p["sections"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let rowAction = p["rowAction"]?.stringValue ?? ""
        let selected = Binding<String?>(
            get: { nil },
            set: { path in
                guard let path, !rowAction.isEmpty else { return }
                for sec in sections { for r in (sec["rows"]?.arrayValue ?? []).compactMap({ $0.dictionaryValue }) where r["path"]?.stringValue == path {
                    fire(rowAction, ["id": .string(r["id"]?.stringValue ?? "")])
                } }
            })
        List(selection: selected) {
            ForEach(Array(sections.enumerated()), id: \.offset) { _, sec in
                let rows = (sec["rows"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
                let title = sec["title"]?.stringValue ?? ""
                if title.isEmpty {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, r in line(r, cols, narrow) }
                } else {
                    Section {
                        ForEach(Array(rows.enumerated()), id: \.offset) { _, r in line(r, cols, narrow) }
                    } header: {
                        Text("\(title)  \(Int(sec["count"]?.numberValue ?? 0))")
                    }
                }
            }
        }
        #if os(macOS)
        .listStyle(.inset(alternatesRowBackgrounds: true))
        #else
        .listStyle(.plain)
        #endif
        .overlay {
            if sections.allSatisfy({ ($0["rows"]?.arrayValue ?? []).isEmpty }) {
                Text(p["empty"]?.stringValue ?? "Nothing matches the search and filters.").foregroundStyle(.secondary)
            }
        }
        .frame(height: p["fit"]?.boolValue == true
               ? CGFloat(sections.reduce(0) { $0 + ($1["rows"]?.arrayValue ?? []).count }) * (WebTheme.rowHeight + 8) + 12
               : max(320, pane.height - 250))
    }

    private func line(_ r: [String: AnyCodable], _ cols: [[String: AnyCodable]], _ narrow: Bool) -> some View {
        let foldAction = node.instance.properties["foldAction"]?.stringValue ?? ""
        return RowView(row: r, cols: cols, narrow: narrow, first: true, fire: fire, rowAction: "", foldAction: foldAction).tag(r["path"]?.stringValue)
    }

    private struct RowView: View {
        let row: [String: AnyCodable]
        let cols: [[String: AnyCodable]]
        let narrow: Bool
        let first: Bool
        let fire: (String, [String: AnyCodable]) -> Void
        let rowAction: String
        let foldAction: String

        var body: some View {
            let cells = (row["cells"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
            let menu = (row["menu"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
            let id = row["id"]?.stringValue ?? ""
            let dim = row["dim"]?.boolValue == true
            let depth = row["depth"]?.numberValue ?? 0
            HStack(spacing: 10) {
                ForEach(Array(cols.enumerated()), id: \.offset) { _, c in
                    column(c, cells: cells, depth: depth, dim: dim)
                }
            }
            .frame(height: WebTheme.rowHeight)
            .contentShape(Rectangle())
            .contextMenu {
                ForEach(Array(menu.enumerated()), id: \.offset) { _, item in
                    Button(item["label"]?.stringValue ?? "") {
                        fire(item["action"]?.stringValue ?? "", item["context"]?.dictionaryValue ?? [:])
                    }
                }
            }
        }

        @ViewBuilder
        private func column(_ c: [String: AnyCodable], cells: [[String: AnyCodable]], depth: Double, dim: Bool) -> some View {
            let t = WebTheme.shared
            // A row's cells carry their column's key, so a hidden column leaves the others in place.
            let key = c["key"]?.stringValue ?? ""
            let cell = cells.first { $0["key"]?.stringValue == key } ?? [:]
            let width: CGFloat? = c["width"]?.numberValue.map { CGFloat($0) }
            HStack(spacing: 0) {
                if c["tree"]?.boolValue == true {
                    Color.clear.frame(width: CGFloat(depth) * 18, height: 1)
                    Text(row["kids"]?.boolValue == true ? (row["collapsed"]?.boolValue == true ? "▸" : "▾") : "")
                        .font(.system(size: 11)).foregroundStyle(t.secondary)
                        .frame(width: 16)
                        .contentShape(Rectangle())
                        .onTapGesture {
                            if row["kids"]?.boolValue == true { fire(foldAction, ["path": .string(row["path"]?.stringValue ?? "")]) }
                        }
                }
                cellView(cell, key: key, dim: dim)
            }
            .frame(width: width, alignment: .leading)
            .frame(maxWidth: width == nil ? .infinity : nil, alignment: .leading)
        }

        @ViewBuilder
        private func cellView(_ cell: [String: AnyCodable], key: String, dim: Bool) -> some View {
            let t = WebTheme.shared
            let text = cell["text"]?.stringValue ?? ""
            let tone = t.tone(cell["tone"]?.stringValue)
            let chip = cell["chip"]?.boolValue == true
            let mono = cell["mono"]?.boolValue == true
            let secondary = dim || cell["tone"]?.stringValue == "muted"
            let buttons = (cell["buttons"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
            if !buttons.isEmpty {
                HStack(spacing: 6) {
                    Spacer(minLength: 0)
                    ForEach(Array(buttons.enumerated()), id: \.offset) { _, b in
                        Button {
                            fire(b["action"]?.stringValue ?? "", b["context"]?.dictionaryValue ?? [:])
                        } label: { Image(systemName: b["icon"]?.stringValue ?? "circle") }
                        .buttonStyle(.bordered).controlSize(.small)
                        .help(b["help"]?.stringValue ?? "")
                        .accessibilityLabel(b["help"]?.stringValue ?? "")
                    }
                }
            } else {
            Text(text)
                .font(mono ? .system(size: WebTheme.caption, design: .monospaced)
                      : chip ? .system(size: WebTheme.callout, weight: .semibold)
                      : .system(size: WebTheme.body, weight: key == "title" ? .medium : .regular))
                .foregroundStyle(tone ?? (secondary ? t.secondary : t.label))
                .lineLimit(1).truncationMode(.tail)
            }
        }
    }
}
