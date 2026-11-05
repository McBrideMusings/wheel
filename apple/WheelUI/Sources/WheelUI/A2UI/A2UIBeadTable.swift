import A2UISwiftCore
import A2UISwiftUI
import SwiftUI

// MARK: BeadTable — grouped boxes of one-line rows, as the Beads web page draws them
// column {key, label, width|null, tree, wide}; section {title, count, rows}; row {id, path, depth, kids, collapsed, dim, sel,
// cells [{text, tone, mono, chip}], buttons [{label, script}]}. A `wide` column shows only when the pane is 640 pt across.
// Raises select_bead {id} (or the tool `rowAction` names), toggle_path {path}, run_script {bead, script}.
// A titled section is a heading over a box of at most 10 rows, and Show more makes the box taller; an untitled one
// is one box that fills the pane.
struct A2UIBeadTableView: View {
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
        let t = WebTheme.shared
        let narrow = pane.width < 640
        let cols = (p["columns"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.filter { !(narrow && $0["wide"]?.boolValue == true) }
        let sections = (p["sections"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let empty = sections.allSatisfy { ($0["rows"]?.arrayValue ?? []).isEmpty }
        let rowAction = p["rowAction"]?.stringValue ?? "select_bead"
        let treeRows = max(8, min(40, Int((pane.height - 230) / WebTheme.rowHeight)))
        VStack(alignment: .leading, spacing: 0) {
            if empty {
                Text(p["empty"]?.stringValue ?? "Nothing matches the search and filters.")
                    .font(.system(size: WebTheme.body)).foregroundStyle(t.secondary)
                    .padding(14).frame(maxWidth: .infinity, alignment: .leading)
                    .background(t.group, in: RoundedRectangle(cornerRadius: WebTheme.radius))
                    .overlay(RoundedRectangle(cornerRadius: WebTheme.radius).strokeBorder(t.separator, lineWidth: 1))
                    .padding(.top, 24)
            }
            ForEach(Array(sections.enumerated()), id: \.offset) { _, sec in
                let rows = (sec["rows"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
                let title = sec["title"]?.stringValue ?? ""
                let open = expanded.contains(title)
                let maxRows = title.isEmpty ? treeRows : (open ? Self.tall : Self.cap)
                if !rows.isEmpty {
                    if !title.isEmpty {
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(title).font(.system(size: WebTheme.body, weight: .bold)).foregroundStyle(t.label)
                            Text("\(Int(sec["count"]?.numberValue ?? 0))")
                                .font(.system(size: WebTheme.body, weight: .medium).monospacedDigit()).foregroundStyle(t.secondary)
                            Spacer(minLength: 0)
                            if rows.count > Self.cap {
                                Button(open ? "Show fewer" : "Show more") {
                                    if open { expanded.remove(title) } else { expanded.insert(title) }
                                }
                                .buttonStyle(.plain)
                                .font(.system(size: WebTheme.callout)).foregroundStyle(t.blue)
                                .frame(minWidth: 70, alignment: .trailing)
                            }
                        }
                        .padding(.horizontal, 2).padding(.top, 24).padding(.bottom, 8)
                    }
                    box(rows: rows, cols: cols, maxRows: maxRows, narrow: narrow, rowAction: rowAction)
                        .padding(.top, title.isEmpty ? 24 : 0)
                }
            }
        }
    }

    @ViewBuilder
    private func box(rows: [[String: AnyCodable]], cols: [[String: AnyCodable]], maxRows: Int, narrow: Bool, rowAction: String) -> some View {
        let t = WebTheme.shared
        let height = CGFloat(min(rows.count, maxRows)) * WebTheme.rowHeight
        let content = LazyVStack(spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.offset) { i, r in
                RowView(row: r, cols: cols, narrow: narrow, first: i == 0, fire: fire, rowAction: rowAction)
            }
        }
        Group {
            if rows.count > maxRows {
                ScrollView { content }.scrollIndicators(.hidden).frame(height: height)
            } else {
                content
            }
        }
        .background(t.group)
        .clipShape(RoundedRectangle(cornerRadius: WebTheme.radius))
        .overlay(RoundedRectangle(cornerRadius: WebTheme.radius).strokeBorder(t.separator, lineWidth: 1))
    }

    private struct RowView: View {
        let row: [String: AnyCodable]
        let cols: [[String: AnyCodable]]
        let narrow: Bool
        let first: Bool
        let fire: (String, [String: AnyCodable]) -> Void
        let rowAction: String
        @State private var hover = false

        var body: some View {
            let t = WebTheme.shared
            let cells = (row["cells"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
            let buttons = narrow ? [] : (row["buttons"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
            let id = row["id"]?.stringValue ?? ""
            let dim = row["dim"]?.boolValue == true
            let sel = row["sel"]?.boolValue == true
            let depth = row["depth"]?.numberValue ?? 0
            let showButtons: Bool = {
                #if os(macOS)
                return hover || sel
                #else
                return true
                #endif
            }()
            HStack(spacing: 10) {
                ForEach(Array(cols.enumerated()), id: \.offset) { _, c in
                    column(c, cells: cells, depth: depth, dim: dim)
                }
                if !buttons.isEmpty { buttonsView(buttons, id: id, shown: showButtons) }
            }
            .padding(.horizontal, 12)
            .frame(height: WebTheme.rowHeight)
            .background(sel || hover ? t.fill : Color.clear)
            .contentShape(Rectangle())
            .onTapGesture { fire(rowAction, ["id": .string(id)]) }
            #if os(macOS)
            .onHover { hover = $0 }
            #endif
            .overlay(alignment: .top) { if !first { Rectangle().fill(t.separator).frame(height: 1) } }
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
                            if row["kids"]?.boolValue == true { fire("toggle_path", ["path": .string(row["path"]?.stringValue ?? "")]) }
                        }
                }
                cellView(cell, key: key, dim: dim)
            }
            .frame(width: width, alignment: .leading)
            .frame(maxWidth: width == nil ? .infinity : nil, alignment: .leading)
        }

        @ViewBuilder
        private func buttonsView(_ buttons: [[String: AnyCodable]], id: String, shown: Bool) -> some View {
            let t = WebTheme.shared
            HStack(spacing: 4) {
                ForEach(Array(buttons.enumerated()), id: \.offset) { _, b in
                    Button {
                        fire("run_script", ["bead": .string(id), "script": .string(b["script"]?.stringValue ?? "")])
                    } label: {
                        Text(b["label"]?.stringValue ?? "").font(.system(size: WebTheme.caption)).foregroundStyle(t.label)
                            .padding(.horizontal, 9).frame(height: 20)
                            .background(t.group, in: RoundedRectangle(cornerRadius: 5))
                            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(t.separatorStrong, lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                    .opacity(shown ? 1 : 0)
                }
            }
        }

        @ViewBuilder
        private func cellView(_ cell: [String: AnyCodable], key: String, dim: Bool) -> some View {
            let t = WebTheme.shared
            let text = cell["text"]?.stringValue ?? ""
            let tone = t.tone(cell["tone"]?.stringValue)
            let chip = cell["chip"]?.boolValue == true
            let mono = cell["mono"]?.boolValue == true
            let secondary = dim || cell["tone"]?.stringValue == "muted"
            Text(text)
                .font(mono ? .system(size: WebTheme.caption, design: .monospaced)
                      : chip ? .system(size: WebTheme.callout, weight: .semibold)
                      : .system(size: WebTheme.body, weight: key == "title" ? .medium : .regular))
                .foregroundStyle(tone ?? (secondary ? t.secondary : t.label))
                .lineLimit(1).truncationMode(.tail)
        }
    }
}
