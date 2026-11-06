import SwiftUI
import WheelKit

/// The phone's command palette over WheelKit's `CommandPalette`: spaces, panes with their agent
/// state, artifacts, sessions, and the actions the phone can run. Choosing a row acts at once.
enum PhoneCommands {
    static let renameID = "rename-space"
    static let closeID = "close-space"
    static let newAgentID = "new-agent-pane"

    /// The agent a "new agent pane" starts: Claude when it is installed, else the first one found.
    static func newAgent(_ agents: [AgentInfo]) -> String? {
        let found = AgentInfo.pickable(agents)
        return (found.first { $0.name == "claude" } ?? found.first)?.name
    }

    /// The palette over what the phone shows now. Rename and Close act on the shown space; the new
    /// agent pane needs an agent installed on the Mac.
    @MainActor static func palette(for model: PhoneModel, query: String = "") -> CommandPalette {
        var palette = CommandPalette()
        let spaces = model.spaces
        var actions: [PaletteAction] = []
        if let space = model.activeSpace {
            actions.append(PaletteAction(id: renameID, title: "Rename space…"))
            actions.append(PaletteAction(id: closeID, title: "Close space"))
            if let agent = newAgent(model.agents), space.focus != nil {
                actions.append(PaletteAction(id: newAgentID, title: "New \(agent) pane"))
            }
        }
        palette.list(
            spaces: spaces.map { .init(id: $0.id, name: $0.name, dir: $0.cwd) },
            panes: spaces.flatMap { space in
                space.allPanes.compactMap { pane in
                    paneName(pane.kind, status: model.paneStatus(pane.id)).map {
                        CommandPalette.OpenPane(id: pane.id, space: space.id, spaceName: space.name, name: $0)
                    }
                }
            },
            artifacts: model.artifacts ?? [],
            sessions: model.sessions.map { .init(id: $0.id, name: $0.name) },
            recents: [], actions: actions)
        palette.type(query)
        return palette
    }

    /// What a pane is called in the list: its agent's name and state, or what it shows.
    static func paneName(_ kind: PaneKind, status: PaneStatus?) -> String? {
        func agent(_ name: String) -> String {
            guard let status else { return name }
            return "\(name) — \(stateWords(status))"
        }
        switch kind {
        case let .agent(name, _, _): return agent(name)
        case let .terminal(_, name?): return agent(name.name)
        case .terminal: return "Terminal"
        case let .web(artifact): return artifact.name
        case .browser: return "Browser"
        case .empty, .unknown: return nil
        }
    }

    static func stateWords(_ status: PaneStatus) -> String {
        switch status {
        case .needsYou: "needs you"
        case .stopped: "stopped"
        case .working: "working"
        case .done: "done"
        case .idle: "idle"
        }
    }

    /// The small grey label at the row's left.
    static func kindLabel(_ item: PaletteItem) -> String {
        switch item {
        case .space: "Space"
        case .pane: "Pane"
        case .artifact: "Artifact"
        case .session: "Session"
        case let .action(id, _, _): id == renameID || id == closeID ? "Space" : "Action"
        case .newSpaceMode: "Action"
        case .recent, .folder, .missing, .openFolder, .newSpaceHere, .verb, .target: ""
        }
    }

    /// The grey words at the row's right.
    static func rowDetail(_ item: PaletteItem) -> String {
        switch item {
        case let .space(_, _, dir): CommandPalette.lastComponent(of: dir)
        case let .pane(_, _, _, spaceName): spaceName
        case let .artifact(scope, _, _): scope.rawValue
        case let .action(id, _, _): id == newAgentID ? "split right" : ""
        default: ""
        }
    }
}

struct CommandPaletteSheet: View {
    let model: PhoneModel
    /// Closes the sheet, then asks the carousel for what comes next (nil: nothing): an alert shown
    /// from inside the sheet would close with it.
    let done: (TitleMenuRequest?) -> Void
    @State private var query = ""
    @FocusState private var focused: Bool

    private var palette: CommandPalette { PhoneCommands.palette(for: model, query: query) }

    var body: some View {
        let rows = palette.rows
        VStack(spacing: 0) {
            TextField("Search spaces, panes, actions", text: $query)
                .textFieldStyle(.plain)
                .font(.title3)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .focused($focused)
                .submitLabel(.go)
                .onSubmit { if let first = rows.first { run(first.item) } }
                .padding(16)
                .accessibilityIdentifier("palette field")
            Divider()
            if rows.isEmpty {
                ContentUnavailableView("Nothing matches \u{201C}\(query)\u{201D}", systemImage: "magnifyingglass")
            } else {
                List(rows) { row in
                    Button { run(row.item) } label: {
                        HStack(spacing: 12) {
                            Text(PhoneCommands.kindLabel(row.item)).font(.footnote).foregroundStyle(.secondary)
                                .frame(width: 64, alignment: .leading)
                            Text(row.item.title).foregroundStyle(.primary).lineLimit(1)
                            Spacer(minLength: 8)
                            Text(PhoneCommands.rowDetail(row.item)).font(.footnote).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                    .accessibilityIdentifier("palette \(row.item.id)")
                }
                .listStyle(.plain)
            }
        }
        .presentationDetents([.medium, .large])
        .onAppear {
            focused = true
            model.listArtifacts()
            Log.info("palette opened: \(palette.rows.map(\.id).joined(separator: ", "))")
        }
        .onChange(of: query) { _, new in
            Log.info("palette \"\(new)\": \(palette.rows.map(\.id).joined(separator: ", "))")
        }
    }

    private func run(_ item: PaletteItem) {
        Log.info("palette run \(item.id)")
        let space = model.activeSpace
        switch item {
        case let .space(id, _, _):
            model.select(space: id)
            done(nil)
        case let .pane(id, spaceID, _, _):
            model.focus(space: spaceID, pane: id)
            done(nil)
        case let .session(id, _):
            model.selectSession(id)
            done(nil)
        case let .artifact(scope, name, _):
            if let space, let info = model.artifacts?.first(where: { $0.scope == scope && $0.name == name }) {
                model.openArtifact(info, in: space.id)
            }
            done(nil)
        case .newSpaceMode:
            done(.newSpace)
        case let .action(id, _, _):
            guard let space else { return done(nil) }
            switch id {
            case PhoneCommands.renameID:
                done(.rename(space: space.id, name: space.name))
            case PhoneCommands.closeID:
                if let question = SessionCaption.closeQuestion(space) {
                    done(.close(space: space.id, question: question))
                } else {
                    model.closeSpace(space.id)
                    done(nil)
                }
            case PhoneCommands.newAgentID:
                if let agent = PhoneCommands.newAgent(model.agents) { model.newAgentPane(in: space.id, agent: agent) }
                done(nil)
            default:
                done(nil)
            }
        case .recent, .folder, .missing, .openFolder, .newSpaceHere, .verb, .target:
            done(nil)
        }
    }
}
