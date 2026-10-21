import AppKit
import GhosttyTerminal
import SwiftUI
import WheelKit

/// The active space's grid, with the spaces beside it in the sidebar or, when the sidebar is
/// closed, as tabs above it. Every space stays mounted, so a terminal keeps its screen while its
/// space is hidden; only the active one draws.
struct SpacesView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            if model.sidebarOpen {
                SpacesSidebar(model: model)
                    .frame(width: 230)
            }
            grid
        }
    }

    private var grid: some View {
        VStack(alignment: .leading, spacing: 6) {
            if model.sidebarOpen {
                if let error = model.layoutError {
                    LayoutErrorLabel(error: error)
                }
            } else {
                SpaceTabs(model: model)
            }
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
                LayoutErrorLabel(error: error)
            }
        }
        .frame(height: 26)
    }
}

struct LayoutErrorLabel: View {
    let error: String

    var body: some View {
        Label(error, systemImage: "exclamationmark.triangle")
            .font(.caption).foregroundStyle(.orange).lineLimit(1)
    }
}

struct SpaceTab: View {
    @ObservedObject var model: AppModel
    let space: SpaceInfo
    let active: Bool

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
            CloseSpaceButton(model: model, space: space)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .background(active ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 6))
    }
}

/// The x on a space's tab or sidebar row. Closing a space kills every shell and ends every agent
/// session in it, so a space with any asks first.
struct CloseSpaceButton: View {
    @ObservedObject var model: AppModel
    let space: SpaceInfo
    @State private var confirmingClose = false

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
        Button {
            if running != (0, 0) { confirmingClose = true } else { model.closeSpace(space.id) }
        } label: {
            Image(systemName: "xmark").font(.caption2)
        }
        .buttonStyle(.borderless)
        .help("Close space")
        .accessibilityLabel("Close space \(space.name)")
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
            let rect = CGRect(origin: .zero, size: geo.size)
            let layout = WheelKit.GridLayout(model.shownRoot(of: space), in: rect, ratios: model.dragRatios)
            ZStack(alignment: .topLeading) {
                ForEach(layout.panes) { pane in
                    PaneView(model: model, paneId: pane.id, kind: pane.kind, focused: pane.id == space.focus)
                        .frame(width: pane.frame.width, height: pane.frame.height)
                        .offset(x: pane.frame.minX, y: pane.frame.minY)
                        .opacity(model.paneDrag?.pane == pane.id ? 0.7 : 1)
                }
                if let drag = model.paneDrag, let drop = drag.drop, let zone = dropZone(drag, drop, layout, rect) {
                    RoundedRectangle(cornerRadius: 10)
                        .fill(Color.accentColor.opacity(0.18))
                        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color.accentColor, lineWidth: 2))
                        .frame(width: zone.width, height: zone.height)
                        .offset(x: zone.minX, y: zone.minY)
                        .allowsHitTesting(false)
                }
                ForEach(layout.dividers) { divider in
                    DividerHandle(model: model, divider: divider)
                        .frame(width: divider.frame.width, height: divider.frame.height)
                        .offset(x: divider.frame.minX, y: divider.frame.minY)
                }
            }
            .frame(width: geo.size.width, height: geo.size.height, alignment: .topLeading)
            .coordinateSpace(name: "grid")
            .onChange(of: space.root, initial: true) { _, root in
                model.baseLayouts[space.id] = WheelKit.GridLayout(root, in: rect)
            }
            .onChange(of: geo.size) { _, size in
                model.baseLayouts[space.id] = WheelKit.GridLayout(space.root, in: CGRect(origin: .zero, size: size))
            }
        }
    }

    /// The part of the grid that lights up for the drop: the target's own frame for a swap, and for
    /// a dock the place the dragged pane has moved to in the preview.
    private func dropZone(_ drag: AppModel.PaneDrag, _ drop: PaneDrop, _ shown: WheelKit.GridLayout, _ rect: CGRect) -> CGRect? {
        guard space.root.panes.contains(where: { $0.id == drag.pane }) else { return nil }
        switch drop.zone {
        case .center: return model.baseLayouts[space.id]?.panes.first { $0.id == drop.target }?.frame
        case .edge: return shown.panes.first { $0.id == drag.pane }?.frame
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
    }

    /// The pane with its palette open is the one the palette is for, whichever pane has focus.
    private var ringed: Bool { focused || model.palettes[paneId] != nil }

    @ViewBuilder private var content: some View {
        switch kind {
        case let .terminal(term, _):
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
        case let .agent(session, _):
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
    /// Closing would kill this pane's shell or end its agent session, waiting for the person to confirm.
    @State private var confirmingClose = false

    private var isTerminal: Bool { if case .terminal = kind { true } else { false } }
    private var isAgent: Bool { if case .agent = kind { true } else { false } }

    private func close() {
        if isTerminal || isAgent { confirmingClose = true } else { model.closePane(paneId) }
    }

    /// The other way this pane can show its conversation: a chat for a terminal running Claude
    /// Code, the Claude Code TUI for an agent pane.
    private var otherView: ConversationView? {
        switch kind {
        case let .terminal(_, agent) where agent?.isClaude == true && model.agentFound("claude"): .agent
        case let .agent(_, agent): agent == "claude" ? .terminal : nil
        default: nil
        }
    }

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: kind.icon).foregroundStyle(.secondary)
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
            if let view = otherView {
                headerButton(view == .agent ? "bubble.left.and.text.bubble.right" : "terminal",
                             view == .agent ? "Show as Chat" : "Show in Terminal") { model.switchView(paneId, to: view) }
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
        // Dragging the header swaps this pane with another or docks it beside one; the grid
        // previews the result and the daemon changes the tree once the pointer is released.
        .gesture(
            DragGesture(minimumDistance: 6, coordinateSpace: .named("grid"))
                .onChanged {
                    model.beginPaneDrag(paneId)
                    model.dragPane(to: $0.location)
                }
                .onEnded { _ in model.endPaneDrag() }
        )
        .confirmationDialog(isAgent ? "End this pane's agent session?" : "Kill this pane's shell?",
                            isPresented: $confirmingClose) {
            Button("Close Pane", role: .destructive) { model.closePane(paneId) }
        } message: {
            Text(isAgent ? "Its conversation and anything it is doing stop." : "Whatever is running in it stops.")
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

    @ViewBuilder private var title: some View {
        switch kind {
        case .empty: Text("Empty pane")
        case let .terminal(term, agent):
            HStack(spacing: 6) {
                if let terminal = model.terminals[term] { TerminalTitle(state: terminal.view) } else { Text("Terminal") }
                if let context = agent?.context {
                    ContextMeter(model: model, paneId: paneId, context: context)
                }
            }
        case let .web(artifact):
            if let web = model.webPanes[paneId] { WebTitle(web: web) } else { Text(artifact.name) }
        case let .browser(url):
            if let browser = model.browserPanes[paneId] { BrowserTitle(browser: browser) } else { Text(url) }
        case let .agent(session, agent):
            if let chat = model.chats[session] {
                AgentTitle(model: model, chat: chat, paneId: paneId, agent: agent)
            } else {
                HStack(spacing: 6) { AgentName(agent: agent); Text(session) }
            }
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

/// An agent's mark and name, which lead a pane's header.
struct AgentName: View {
    let agent: String

    var body: some View {
        HStack(spacing: 4) {
            UsageIcon(agent: agent)
            Text(AgentInfo.title(of: agent)).fontWeight(.semibold)
        }
        .fixedSize()
    }
}

/// An agent pane's title: its session name, what it is doing, and Stop while it works.
struct AgentTitle: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    let paneId: String
    let agent: String

    var body: some View {
        HStack(spacing: 6) {
            AgentName(agent: agent)
            Text(chat.name)
            Text(model.stateLabel(chat))
                .font(.caption)
                .foregroundStyle(.secondary)
            StopButton(model: model, chat: chat)
                .buttonStyle(.borderless)
            if let context = chat.context {
                ContextMeter(model: model, paneId: paneId, context: context)
            }
        }
    }
}

/// How full the conversation's context window is: a ring and the percent, the exact tokens on
/// hover. A click switches this pane's label between the percent and the raw figures. Callers draw
/// it only once a figure has been reported, since the daemon never guesses.
struct ContextMeter: View {
    @ObservedObject var model: AppModel
    let paneId: String
    let context: ContextUse

    var body: some View {
        HStack(spacing: 4) {
            ZStack {
                Circle().stroke(Color.primary.opacity(0.15), lineWidth: 2.5)
                Circle()
                    .trim(from: 0, to: context.fraction)
                    .stroke(tint(context), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
            .frame(width: 12, height: 12)
            Text(context.text(raw: model.rawContextPanes.contains(paneId)))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .fixedSize()
        .contentShape(Rectangle())
        .onTapGesture { model.toggleContextLabel(paneId) }
        .help(context.helpText)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(context.helpText)
    }

    private func tint(_ context: ContextUse) -> Color {
        switch context.percent {
        case ..<75: .accentColor
        case ..<90: .orange
        default: .red
        }
    }
}

/// An agent pane: the agent's conversation, the request it waits on, a composer, and the controls
/// it offers over its own session.
struct AgentBody: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var chat: SessionChat

    var body: some View {
        VStack(spacing: 0) {
            TranscriptView(model: model, chat: chat, empty: "Give this agent a task.")
            // Above the composer: a status about this conversation, such as one that would not
            // reopen. The person closes it; it is never a line of the transcript.
            if let notice = chat.notice {
                NoticeBanner(text: notice) { model.dismissNotice(chat) }
                    .padding([.horizontal, .top], 10)
            }
            // Directly above the composer, where a person answering has their hands: this agent's
            // oldest waiting request.
            if let perm = model.perms.first(where: { $0.session == chat.name }) {
                PermissionCard(model: model, perm: perm, position: model.position(of: perm), count: model.perms.count,
                               shortcut: model.paneIsFocused(paneId))
                    .padding([.horizontal, .top], 10)
            }
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
            .padding([.horizontal, .top], 10)
            ConfigRow(model: model, chat: chat)
                .padding(.horizontal, 14)
                .padding(.top, 6)
                .padding(.bottom, 10)
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
        ForEach(AgentInfo.pickable(model.agents), id: \.name) { agent in
            Button {
                model.startPane(paneId, .agent, agent: agent.name)
            } label: {
                Label { Text(agent.title).fixedSize() } icon: { UsageIcon(agent: agent.name) }
            }
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

extension PaneKind {
    /// The SF Symbol that stands for this kind in a pane header.
    var icon: String {
        switch self {
        case .empty: "rectangle.dashed"
        case .terminal: "terminal"
        case .web: "doc.text"
        case .browser: "safari"
        case .agent: "bubble.left.and.text.bubble.right"
        case .unknown: "questionmark.square.dashed"
        }
    }
}
