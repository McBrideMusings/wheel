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
            case .agent: (count.0, count.1 + 1)
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

    var body: some View {
        HStack(spacing: 2) {
            Button { model.focus(space: space.id) } label: {
                Text(space.name)
                    .fontWeight(active ? .semibold : .regular)
                    .foregroundStyle(active ? .primary : .secondary)
                    .lineLimit(1)
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
                    PaneView(model: model, paneId: pane.id, kind: pane.kind, focused: pane.id == space.focus)
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
    let focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            PaneHeader(model: model, paneId: paneId, kind: kind, focused: focused)
            Divider()
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(.background)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(focused ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.separator), lineWidth: focused ? 2 : 1)
        )
    }

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
        case let .agent(session):
            if let chat = model.chats[session] {
                // A new session in the same pane starts with a fresh draft and scroll position.
                AgentBody(model: model, paneId: paneId, chat: chat)
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
    /// An action that would kill this pane's shell or end its agent session, waiting for the
    /// person to confirm it.
    @State private var killing: Kill?

    enum Kill: Identifiable {
        case terminal, agent, empty, web, browser, close
        var id: Self { self }
    }

    private var isTerminal: Bool { if case .terminal = kind { true } else { false } }
    private var isAgent: Bool { if case .agent = kind { true } else { false } }

    /// Run `action` now, or after confirming when it would kill a shell or end a session.
    private func act(_ action: Kill) {
        if isTerminal || isAgent { killing = action } else { run(action) }
    }

    private func run(_ action: Kill) {
        switch action {
        case .terminal: model.setKind(paneId, .terminal)
        case .agent: model.startPane(paneId, .agent)
        case .empty: model.setKind(paneId, .empty)
        case .web: chooseWebFolder(model, pane: paneId)
        case .browser: model.setKind(paneId, .browser)
        case .close: model.closePane(paneId)
        }
    }

    /// The confirming button's label.
    private func label(_ action: Kill) -> String {
        switch action {
        case .terminal: isTerminal ? "Restart Shell" : "Show a Terminal"
        case .agent: isAgent ? "Start a New Agent" : "Show an Agent"
        case .empty: "Make Empty"
        case .web: "Show an Artifact"
        case .browser: "Show a Browser"
        case .close: "Close Pane"
        }
    }

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: icon).foregroundStyle(.secondary)
            title
                .fontWeight(focused ? .semibold : .regular)
                .lineLimit(1)
            Spacer(minLength: 4)
            Menu {
                if let browser = model.browserPanes[paneId], case .browser = kind {
                    BrowserMenu(model: model, paneId: paneId, browser: browser)
                    Divider()
                }
                if case .unknown = kind {} else {
                    Button("Split Right") { model.split(paneId, .right) }
                    Button("Split Down") { model.split(paneId, .down) }
                    Button("Split Left") { model.split(paneId, .left) }
                    Button("Split Up") { model.split(paneId, .up) }
                    Divider()
                }
                Section("Show") {
                    Button(isTerminal ? "New Terminal" : "Terminal") { act(.terminal) }
                    Button(isAgent ? "New Agent" : "Agent") { act(.agent) }
                    Button("Artifact…") { act(.web) }
                    Button("Browser") { act(.browser) }
                    Button("Nothing (empty pane)") { act(.empty) }
                }
                Divider()
                Button("Close Pane") { act(.close) }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("Split, change or close this pane")
        }
        .font(.callout)
        .padding(.horizontal, 10)
        .frame(height: 28)
        .contentShape(Rectangle())
        .onTapGesture { model.focus(pane: paneId) }
        .confirmationDialog(isAgent ? "End this pane's agent session?" : "Kill this pane's shell?",
                            isPresented: Binding(get: { killing != nil }, set: { if !$0 { killing = nil } }),
                            presenting: killing) { action in
            Button(label(action), role: .destructive) { run(action) }
        } message: { _ in
            Text(isAgent ? "Its conversation and anything it is doing stop." : "Whatever is running in it stops.")
        }
    }

    private var icon: String {
        switch kind {
        case .empty: "rectangle.dashed"
        case .terminal: "terminal"
        case .web: "globe"
        case .browser: "safari"
        case .agent: "bubble.left.and.text.bubble.right"
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

    var body: some View {
        TerminalSurfaceView(context: termState)
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
            chooseWebFolder(model, pane: paneId)
        } label: {
            Label("Web Folder…", systemImage: "globe").fixedSize()
        }
        Button {
            model.setKind(paneId, .browser)
        } label: {
            Label("Browser", systemImage: "safari").fixedSize()
        }
    }
}

/// A pane of a kind this build does not know (saved by a newer Wheel). The daemon keeps it
/// whole; all this window offers is to change it into something it knows, or close it.
struct UnsupportedPane: View {
    @ObservedObject var model: AppModel
    let paneId: String
    let kind: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "questionmark.square.dashed").font(.largeTitle).foregroundStyle(.secondary)
            Text("This version of Wheel can't show a “\(kind)” pane.")
                .multilineTextAlignment(.center)
            HStack {
                Button("Make Terminal") { model.setKind(paneId, .terminal) }
                Button("Make Empty") { model.setKind(paneId, .empty) }
                Button("Close Pane") { model.closePane(paneId) }
            }
        }
        .padding()
    }
}
