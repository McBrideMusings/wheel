import AppKit
import GhosttyTerminal
import SwiftUI
import WheelKit

/// Space tabs above the grid of the active space's panes. Every space stays mounted, so a
/// terminal keeps its screen while its space is hidden; only the active one draws.
struct SpacesView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            SpaceTabs(model: model)
            ZStack {
                ForEach(model.spaces) { space in
                    let shown = space.id == model.active
                    SpaceGrid(model: model, space: space)
                        .opacity(shown ? 1 : 0)
                        .allowsHitTesting(shown)
                        .accessibilityHidden(!shown)
                }
                if model.spaces.isEmpty {
                    NoSpaces(model: model)
                }
            }
        }
    }
}

/// Ask for a project folder and make a space for it; the daemon names it after the folder.
@MainActor
func openSpace(_ model: AppModel) {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = true
    panel.canChooseFiles = false
    panel.allowsMultipleSelection = false
    panel.prompt = "Open Space"
    panel.message = "Choose a project folder for the new space."
    panel.begin { response in
        guard response == .OK, let url = panel.url else { return }
        MainActor.assumeIsolated { model.createSpace(cwd: url.path) }
    }
}

struct SpaceTabs: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 4) {
            ForEach(model.spaces) { space in
                SpaceTab(model: model, space: space, active: space.id == model.active)
            }
            Button { openSpace(model) } label: {
                Image(systemName: "plus").frame(width: 20, height: 20)
            }
            .buttonStyle(.borderless)
            .help("New space from a folder")
            Spacer(minLength: 8)
            if let error = model.layoutError {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.caption).foregroundStyle(.orange).lineLimit(1)
            }
        }
        .frame(height: 26)
    }
}

struct SpaceTab: View {
    @ObservedObject var model: AppModel
    let space: SpaceInfo
    let active: Bool
    @State private var confirmingClose = false

    /// Closing a space kills every shell and ends every agent session in it, so a space with
    /// any asks first.
    private var running: (shells: Int, agents: Int) {
        space.root.panes.reduce((0, 0)) { count, pane in
            switch pane.kind {
            case .terminal: (count.0 + 1, count.1)
            case .agent, .supervisor: (count.0, count.1 + 1)
            default: count
            }
        }
    }

    private var closeMessage: String {
        let (shells, agents) = running
        var parts: [String] = []
        if shells > 0 { parts.append(shells == 1 ? "its terminal's shell is killed" : "its \(shells) terminals' shells are killed") }
        if agents > 0 { parts.append(agents == 1 ? "its agent's session ends" : "its \(agents) agents' sessions end") }
        let text = parts.joined(separator: " and ")
        return text.prefix(1).uppercased() + text.dropFirst() + "."
    }

    private var marks: SpaceMarks { SpaceMarks.of(space, infos: model.supervisorInfos) }

    var body: some View {
        HStack(spacing: 2) {
            Button { model.focus(space: space.id) } label: {
                HStack(spacing: 5) {
                    Text(space.name)
                        .fontWeight(active ? .semibold : .regular)
                        .foregroundStyle(active ? .primary : .secondary)
                        .lineLimit(1)
                    if let text = marks.stateText {
                        Text(text).font(.caption.monospacedDigit()).foregroundStyle(marks.held ? .orange : .secondary)
                    }
                    if let text = marks.forYouText {
                        Text(text).font(.caption2.weight(.semibold)).foregroundStyle(.white)
                            .padding(.horizontal, 5).padding(.vertical, 1)
                            .background(.orange, in: Capsule())
                    }
                }
            }
            .buttonStyle(.borderless)
            .help(space.cwd)
            .accessibilityAddTraits(active ? .isSelected : [])
            Button {
                if running != (0, 0) { confirmingClose = true } else { model.closeSpace(space.id) }
            } label: {
                Image(systemName: "xmark").font(.caption2)
            }
            .buttonStyle(.borderless)
            .help("Close space")
            .accessibilityLabel("Close space \(space.name)")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .background(active ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 6))
        .confirmationDialog("Close “\(space.name)”?", isPresented: $confirmingClose) {
            Button("Close Space", role: .destructive) { model.closeSpace(space.id) }
        } message: {
            Text(closeMessage)
        }
    }
}

struct NoSpaces: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "square.split.2x1").font(.largeTitle).foregroundStyle(.secondary)
            Text("No spaces yet").font(.headline)
            Text("A space is a project folder with its own grid of panes.")
                .foregroundStyle(.secondary)
            Button("Open Folder…") { openSpace(model) }
                .disabled(model.connection != .connected)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
    }
}

/// One space's split tree, laid out flat by `GridLayout` so a split or a ratio change moves a
/// terminal's surface instead of rebuilding it.
struct SpaceGrid: View {
    @ObservedObject var model: AppModel
    let space: SpaceInfo

    var body: some View {
        GeometryReader { geo in
            let layout = WheelKit.GridLayout(space.root, in: CGRect(origin: .zero, size: geo.size), ratios: model.dragRatios)
            ZStack(alignment: .topLeading) {
                ForEach(layout.panes) { pane in
                    PaneView(model: model, paneId: pane.id, kind: pane.kind, createdBy: pane.createdBy, prior: pane.prior, focused: pane.id == space.focus)
                        .frame(width: pane.frame.width, height: pane.frame.height)
                        .offset(x: pane.frame.minX, y: pane.frame.minY)
                }
                ForEach(layout.dividers) { divider in
                    DividerHandle(model: model, divider: divider)
                        .frame(width: divider.frame.width, height: divider.frame.height)
                        .offset(x: divider.frame.minX, y: divider.frame.minY)
                }
            }
            .frame(width: geo.size.width, height: geo.size.height, alignment: .topLeading)
            .coordinateSpace(name: "grid")
        }
    }
}

/// The gap between a split's two sides. Dragging it redraws live and tells the daemon once, at
/// the end; VoiceOver's adjust action moves it in steps of 0.05.
struct DividerHandle: View {
    @ObservedObject var model: AppModel
    let divider: WheelKit.GridLayout.Divider
    @State private var hovering = false

    private var cursor: NSCursor { divider.axis == .horizontal ? .resizeLeftRight : .resizeUpDown }

    var body: some View {
        Rectangle()
            .fill(.clear)
            .overlay {
                Capsule()
                    .fill(.tertiary)
                    .frame(width: divider.axis == .horizontal ? 3 : 32, height: divider.axis == .horizontal ? 32 : 3)
                    .opacity(hovering ? 1 : 0)
            }
            .contentShape(Rectangle())
            // `set` rather than push/pop: a divider that vanishes under the pointer never sees
            // the hover end, and a pushed cursor would stay pushed.
            .onHover { inside in
                hovering = inside
                (inside ? cursor : .arrow).set()
            }
            .onDisappear { if hovering { NSCursor.arrow.set() } }
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .named("grid"))
                    .onChanged { model.dragRatio(divider.id, WheelKit.GridLayout.ratio(at: $0.location, in: divider)) }
                    .onEnded { model.setRatio(divider.id, WheelKit.GridLayout.ratio(at: $0.location, in: divider)) }
            )
            .accessibilityElement()
            .accessibilityLabel(divider.axis == .horizontal ? "Divider between left and right panes" : "Divider between top and bottom panes")
            .accessibilityValue("\(Int((divider.ratio * 100).rounded())) percent")
            .accessibilityAdjustableAction { direction in
                let step = direction == .increment ? 0.05 : -0.05
                let ratio = min(max(divider.ratio + step, WheelKit.GridLayout.ratios.lowerBound), WheelKit.GridLayout.ratios.upperBound)
                model.setRatio(divider.id, ratio)
            }
    }
}

/// One pane: a title bar with its menu above whatever the pane shows.
struct PaneView: View {
    @ObservedObject var model: AppModel
    let paneId: String
    let kind: PaneKind
    let createdBy: CreatedBy
    let prior: CreatedBy?
    let focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            PaneHeader(model: model, paneId: paneId, kind: kind, focused: focused)
            Divider()
            ManagedPaneView(model: model, paneId: paneId, kind: kind, createdBy: createdBy, prior: prior) { content }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .overlay {
                    if let palette = model.palettes[paneId] {
                        PaletteOverlay(model: model, palette: palette, paneId: paneId, kind: kind)
                    }
                }
        }
        .background(.background)
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.paneFrames[paneId] = $0 }
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(ringed ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.separator), lineWidth: ringed ? 2 : 1)
        )
        // The pointer is on the badge of a pane this supervisor's pane manages.
        .overlay {
            if let manager = model.highlightedManager, model.pane(of: manager) == paneId {
                RoundedRectangle(cornerRadius: 10).strokeBorder(Color.accentColor, lineWidth: 3).allowsHitTesting(false)
            }
        }
    }

    /// The pane with its palette open is the one the palette is for, whichever pane has focus.
    private var ringed: Bool { focused || model.palettes[paneId] != nil }

    @ViewBuilder private var content: some View {
        switch kind {
        case let .terminal(term):
            if let terminal = model.terminals[term] {
                TerminalBody(model: model, paneId: paneId, terminal: terminal, termState: terminal.view)
            } else {
                ProgressView()
            }
        case .web:
            if let web = model.webPanes[paneId] {
                WebBody(web: web)
            } else {
                ProgressView()
            }
        case let .supervisor(supervisor):
            if let chat = model.chats[supervisor.session], let board = model.supervisorPanes[paneId] {
                SupervisorBody(model: model, paneId: paneId, pane: board, chat: chat)
                    .id(supervisor.session)
            } else {
                ProgressView()
            }
        case let .agent(session):
            if let chat = model.chats[session] {
                // A new session in the same pane starts with a fresh draft and scroll position.
                AgentBody(model: model, paneId: paneId, chat: chat)
                    .id(session)
            } else {
                ProgressView()
            }
        case let .worker(session):
            if let chat = model.chats[session] {
                WorkerPaneBody(model: model, paneId: paneId, chat: chat)
                    .id(session)
            } else {
                ProgressView()
            }
        case .empty:
            EmptyPane(model: model, paneId: paneId)
                .contentShape(Rectangle())
                .onTapGesture { model.focus(pane: paneId) }
        case .browser:
            if let browser = model.browserPanes[paneId] {
                BrowserBody(model: model, paneId: paneId, browser: browser)
            } else {
                ProgressView()
            }
        case let .unknown(kind, _):
            UnsupportedPane(model: model, paneId: paneId, kind: kind)
                .contentShape(Rectangle())
                .onTapGesture { model.focus(pane: paneId) }
        }
    }
}

struct PaneHeader: View {
    @ObservedObject var model: AppModel
    let paneId: String
    let kind: PaneKind
    let focused: Bool
    /// Closing would kill this pane's shell or end its agent session, waiting for the person to confirm.
    @State private var confirmingClose = false

    private var isTerminal: Bool { if case .terminal = kind { true } else { false } }
    private var isSupervisor: Bool { if case .supervisor = kind { true } else { false } }
    private var isAgent: Bool {
        switch kind {
        case .agent, .supervisor: true
        default: false
        }
    }

    private func close() {
        if isTerminal || isAgent { confirmingClose = true } else { model.closePane(paneId) }
    }

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: icon).foregroundStyle(.secondary)
            title
                .fontWeight(focused ? .semibold : .regular)
                .lineLimit(1)
            Spacer(minLength: 4)
            if let browser = model.browserPanes[paneId], case .browser = kind {
                Menu {
                    BrowserMenu(model: model, paneId: paneId, browser: browser)
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Back, forward, reload and home")
                .accessibilityLabel("Browser controls")
            }
            if case .unknown = kind {} else {
                headerButton("square.split.2x1", "Split Right") { model.split(paneId, .right) }
                headerButton("square.split.1x2", "Split Down") { model.split(paneId, .down) }
            }
            headerButton("xmark", "Close Pane", action: close)
        }
        .font(.callout)
        .padding(.horizontal, 10)
        .frame(height: 28)
        .contentShape(Rectangle())
        .onTapGesture { model.focus(pane: paneId) }
        .confirmationDialog(isAgent ? "End this pane's agent session?" : "Kill this pane's shell?",
                            isPresented: $confirmingClose) {
            Button("Close Pane", role: .destructive) { model.closePane(paneId) }
        } message: {
            Text(isSupervisor ? "The supervisor stops, and so do its workers." : isAgent ? "Its conversation and anything it is doing stop." : "Whatever is running in it stops.")
        }
    }

    private func headerButton(_ symbol: String, _ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
        }
        .buttonStyle(.borderless)
        .help(label)
        .accessibilityLabel(label)
    }

    private var icon: String {
        switch kind {
        case .empty: "rectangle.dashed"
        case .terminal: "terminal"
        case .web: "doc.text"
        case .browser: "safari"
        case .agent: "bubble.left.and.text.bubble.right"
        case .supervisor: "person.3.sequence"
        case .worker: "hammer"
        case .unknown: "questionmark.square.dashed"
        }
    }

    @ViewBuilder private var title: some View {
        switch kind {
        case .empty: Text("Empty pane")
        case let .terminal(term):
            if let terminal = model.terminals[term] { TerminalTitle(state: terminal.view) } else { Text("Terminal") }
        case let .web(artifact):
            if let web = model.webPanes[paneId] { WebTitle(web: web) } else { Text(artifact.name) }
        case let .browser(url):
            if let browser = model.browserPanes[paneId] { BrowserTitle(browser: browser) } else { Text(url) }
        case let .agent(session):
            if let chat = model.chats[session] { AgentTitle(model: model, chat: chat) } else { Text("Agent \(session)") }
        case let .supervisor(supervisor):
            if let board = model.supervisorPanes[paneId] { SupervisorTitle(model: model, pane: board) } else { Text("Supervisor \(supervisor.scope)") }
        case let .worker(session): WorkerPaneTitle(model: model, session: session)
        case let .unknown(kind, _): Text(kind)
        }
    }
}

/// The shell's own title (OSC 0/2) when it set one.
struct TerminalTitle: View {
    @ObservedObject var state: TerminalViewState

    var body: some View {
        Text(state.title.isEmpty ? "Terminal" : state.title)
    }
}

struct TerminalBody: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var terminal: PaneTerminal
    @ObservedObject var termState: TerminalViewState

    @Environment(\.displayScale) private var scale

    private func laidOut(_ size: CGSize) {
        terminal.layout = (size, scale)
        model.settle(terminal)
    }

    var body: some View {
        TerminalSurfaceView(context: termState)
            .background(GeometryReader { geo in
                Color.clear
                    .onChange(of: geo.size, initial: true) { _, _ in laidOut(geo.size) }
                    .onChange(of: scale) { _, _ in laidOut(geo.size) }
            })
            .onChange(of: termState.isFocused) { _, focused in
                if focused { model.paneFocused(pane: paneId) }
            }
            .overlay(alignment: .center) {
                if !terminal.attached {
                    ProgressView(model.connection == .connected ? "Attaching…" : "Waiting for wheeld…")
                        .padding()
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
                }
            }
    }
}

/// An agent pane's title: its session name, what it is doing, and Stop while it works.
struct AgentTitle: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        HStack(spacing: 6) {
            Text("Agent \(chat.name)")
            Text(model.stateLabel(chat))
                .font(.caption)
                .foregroundStyle(.secondary)
            StopButton(model: model, chat: chat)
                .buttonStyle(.borderless)
        }
    }
}

/// An agent pane: the agent's conversation and a composer, the same pieces the lead card uses.
struct AgentBody: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var chat: SessionChat

    var body: some View {
        VStack(spacing: 0) {
            TranscriptView(model: model, chat: chat, empty: "Give this agent a task.")
            Composer(
                placeholder: "Message \(chat.name)",
                draft: $chat.draft,
                error: chat.error,
                enabled: model.connection == .connected,
                send: { model.send(chat) },
                pickFiles: pickFiles,
                onFocus: { model.paneFocused(pane: paneId) },
                focusRequest: chat.composerFocusRequest
            )
            .padding(10)
        }
        .simultaneousGesture(TapGesture().onEnded { model.paneFocused(pane: paneId) })
    }
}

struct EmptyPane: View {
    @ObservedObject var model: AppModel
    let paneId: String

    var body: some View {
        VStack(spacing: 10) {
            Text("What should this pane show?")
                .font(.headline)
                .multilineTextAlignment(.center)
            // A row when it fits, a column when the pane is too narrow for full labels.
            ViewThatFits(in: .horizontal) {
                HStack { choices }
                VStack { choices }
            }
            .controlSize(.large)
        }
        .padding()
    }

    @ViewBuilder private var choices: some View {
        Button {
            model.setKind(paneId, .terminal)
        } label: {
            Label("Terminal", systemImage: "terminal").fixedSize()
        }
        Button {
            model.startPane(paneId, .agent)
        } label: {
            Label("Agent", systemImage: "bubble.left.and.text.bubble.right").fixedSize()
        }
        Button {
            model.openPalette(paneId)
        } label: {
            Label("Artifact…", systemImage: "doc.text").fixedSize()
        }
        Button {
            model.setKind(paneId, .browser)
        } label: {
            Label("Browser", systemImage: "safari").fixedSize()
        }
    }
}

/// A pane of a kind this build does not know (saved by a newer Wheel). The daemon keeps it
/// whole; all this window offers is to close it.
struct UnsupportedPane: View {
    @ObservedObject var model: AppModel
    let paneId: String
    let kind: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "questionmark.square.dashed").font(.largeTitle).foregroundStyle(.secondary)
            Text("This version of Wheel can't show a “\(kind)” pane.")
                .multilineTextAlignment(.center)
            Button("Close Pane") { model.closePane(paneId) }
        }
        .padding()
    }
}
