import SwiftUI
import WheelKit
import WheelUI

/// The panes button's list, in place of the page: PANES, the shown space's pages in order, each with its symbol, name and
/// detail, a check on the one shown and an orange dot on a waiting one; then AGENTS, the space's agents most in need
/// first, with the backgrounded ones behind "Show <n> backgrounded". A row or tile opens its pane and closes the list.
struct PanesList: View {
    let model: PhoneModel
    let panes: [(id: String, kind: PaneKind)]
    /// The pane on screen under the list.
    let shown: String?
    /// Opens a pane from a row.
    let open: (String) -> Void
    /// Closes the list once a tile has opened its pane.
    let done: () -> Void
    @State private var showBackgrounded = false

    var body: some View {
        let rows = model.agentRows(scope: .thisSpace)
        let hidden = rows.filter(\.backgrounded).count
        let listed = showBackgrounded ? rows : rows.filter { !$0.backgrounded }
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header("Panes")
                VStack(spacing: 0) {
                    ForEach(Array(panes.enumerated()), id: \.element.id) { i, pane in
                        if i > 0 { Divider().padding(.leading, 52) }
                        row(pane)
                    }
                }
                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                header("Agents").padding(.top, 14)
                VStack(spacing: 10) {
                    if rows.isEmpty {
                        Text("No agents in this space")
                            .font(.subheadline).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity).padding(.vertical, 16)
                            .accessibilityIdentifier("no agents")
                    }
                    AgentTiles(model: model, rows: listed, place: "panes list", showsState: true, done: done)
                    if hidden > 0 {
                        Button(showBackgrounded ? "Hide \(hidden) backgrounded" : "Show \(hidden) backgrounded") {
                            showBackgrounded.toggle()
                            Log.info("panes list: \(showBackgrounded ? "show" : "hide") \(hidden) backgrounded")
                        }
                        .font(.subheadline)
                        .frame(maxWidth: .infinity, minHeight: 36)
                        .accessibilityIdentifier("backgrounded toggle")
                    }
                }
            }
            .padding(.horizontal, 16).padding(.bottom, 16)
        }
        .background(Color(.systemBackground))
        .accessibilityIdentifier("panes list")
        .onAppear {
            Log.info("panes list: panes [\(panes.map(\.id).joined(separator: ", "))], agents \(rows.map { "\($0.agent)=\($0.rank.title)\($0.backgrounded ? "(bg)" : "")" }.joined(separator: ", "))")
        }
    }

    private func header(_ title: String) -> some View {
        Text(title.uppercased())
            .font(.footnote).foregroundStyle(.secondary)
            .padding(.horizontal, 16).padding(.top, 8).padding(.bottom, 6)
    }

    private func row(_ pane: (id: String, kind: PaneKind)) -> some View {
        let status = model.paneStatus(pane.id)
        let waiting = if case .needsYou = status { true } else { false }
        let name = model.activeSpace.map { model.pageName(pane.id, in: $0) } ?? pane.id
        return Button { open(pane.id) } label: {
            HStack(spacing: 12) {
                Image(systemName: paneSymbol(pane.kind))
                    .foregroundStyle(.secondary)
                    .frame(width: 24)
                VStack(alignment: .leading, spacing: 2) {
                    Text(name).font(.body).foregroundStyle(.primary).lineLimit(1)
                    Text(detail(pane, status: status)).font(.footnote).foregroundStyle(.secondary)
                        .lineLimit(1).truncationMode(.middle)
                }
                Spacer(minLength: 8)
                Circle().fill(Color.orange).frame(width: 8, height: 8).opacity(waiting ? 1 : 0)
                Image(systemName: "checkmark").font(.body.weight(.semibold)).foregroundStyle(.tint)
                    .opacity(pane.id == shown ? 1 : 0)
            }
            .padding(.horizontal, 16).padding(.vertical, 10)
            .frame(minHeight: 56)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("pane row \(pane.id)")
        .accessibilityValue([pane.id == shown ? "shown" : nil, waiting ? "needs you" : nil].compactMap { $0 }.joined(separator: ", "))
    }

    /// The line under a pane's name: a browser's page, an agent's state, an artifact's scope, a shell's kind.
    private func detail(_ pane: (id: String, kind: PaneKind), status: PaneStatus?) -> String {
        switch pane.kind {
        case let .browser(url): model.browsers[model.mirror(pane.id)?.source ?? pane.id]?.address ?? url
        case .agent, .terminal(_, _?): status.map(PhoneCommands.stateWords) ?? "idle"
        case .terminal: "shell"
        case let .web(artifact): "\(artifact.scope.rawValue) artifact"
        case .empty: "empty"
        case let .unknown(kind, _): kind
        }
    }
}
