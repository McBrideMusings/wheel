import AppKit
import GhosttyTerminal
import SwiftUI
import WheelKit
import WheelUI

/// The active space's grid, with the spaces beside it in the sidebar or, when the sidebar is
/// closed, as tabs above it. Every space stays mounted, so a terminal keeps its screen while its
/// space is hidden; only the active one draws.
struct SpacesView: View {
    @ObservedObject var model: AppModel
    let windowID: String

    var body: some View {
        let sidebar = model.sidebarOpen(in: windowID)
        HStack(alignment: .top, spacing: 10) {
            if sidebar {
                SpacesSidebar(model: model, windowID: windowID)
                    .frame(width: 230)
            }
            grid(sidebar: sidebar)
        }
    }

    private func grid(sidebar: Bool) -> some View {
        let held = model.spaces(in: windowID)
        let active = model.active(in: windowID)
        return VStack(alignment: .leading, spacing: 6) {
            if sidebar {
                if let error = model.layoutError {
                    LayoutErrorLabel(error: error)
                }
            } else {
                SpaceTabs(model: model, windowID: windowID)
            }
            ZStack {
                // A space in a little window is drawn there: one terminal surface has one place.
                ForEach(held.filter { model.layout.little(showing: $0.id) == nil }) { space in
                    let shown = space.id == active
                    SpaceGrid(model: model, space: space)
                        .opacity(shown ? 1 : 0)
                        .allowsHitTesting(shown)
                        .accessibilityHidden(!shown)
                }
                if let space = held.first(where: { $0.id == active && model.layout.little(showing: $0.id) != nil }) {
                    LittlePlaceholder(model: model, space: space)
                }
                if held.isEmpty {
                    NoSpaces(model: model, windowID: windowID)
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
    let windowID: String

    var body: some View {
        HStack(spacing: 4) {
            // The same numbered icon heads this window's group in the menu bar popover.
            if let number = model.windowNumber(windowID), let record = model.layout.window(windowID) {
                PlaceGlyph(kind: .window(number))
                    .foregroundStyle(.secondary)
                    .padding(.trailing, 4)
                    .help("Window \(number), \(model.title(of: record))")
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel("Window \(number), \(model.title(of: record))")
            }
            let held = model.spaces(in: windowID)
            ForEach(Array(held.enumerated()), id: \.element.id) { offset, space in
                SpaceTab(model: model, windowID: windowID, space: space, active: space.id == model.active(in: windowID))
                    .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.rowFrames[windowID, default: [:]][space.id] = $0 }
                    .overlay(alignment: .leading) { if insertion == offset { TabInsertionBar().offset(x: -3) } }
                    .overlay(alignment: .trailing) { if offset == held.count - 1, insertion == held.count { TabInsertionBar().offset(x: 3) } }
            }
            .animation(model.slide, value: model.spaces(in: windowID).map(\.id))
            Button { model.openNewSpacePalette() } label: {
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
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.tabStripFrames[windowID] = $0 }
    }

    /// Where the dragged space would land in this strip: the number of tabs left of the insertion bar.
    private var insertion: Int? {
        if let bar = model.spaceDragInsertion, bar.window == windowID { bar.index } else { nil }
    }
}

/// The accent line between two tabs where the dragged space would land.
struct TabInsertionBar: View {
    var body: some View {
        Capsule()
            .fill(Color.accentColor)
            .frame(width: 3, height: 20)
            .accessibilityLabel("Drop here")
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
    let windowID: String
    let space: SpaceInfo
    let active: Bool

    var body: some View {
        HStack(spacing: 2) {
            PaneStack(space: space)
                .foregroundStyle(active ? .primary : .secondary)
                .padding(.trailing, 4)
            SpaceDots(space: space, shown: model.statusDots)
                .padding(.trailing, 4)
            if model.renamingSpace == space.id {
                InlineName(
                    text: space.name, placeholder: "Space name", error: model.spaceRenameError[space.id],
                    commit: { model.renameSpace(space.id, to: $0) }, cancel: { model.endRenaming() }
                )
                .frame(width: 160)
            } else {
                Text(space.name)
                    .fontWeight(active ? .semibold : .regular)
                    .foregroundStyle(active ? .primary : .secondary)
                    .lineLimit(1)
                    .help(space.cwd)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(space.name)
                    .accessibilityAddTraits(active ? [.isButton, .isSelected] : .isButton)
                    .accessibilityAction { model.tapSpace(space.id, in: windowID) }
                    .accessibilityAction(named: "Rename Space…") { model.beginRename(space: space.id) }
                BackFromLittleButton(model: model, space: space.id, windowID: windowID)
                CloseSpaceButton(model: model, space: space)
            }
        }
        .opacity(model.isDimmed(space: space.id) ? 0.5 : 1)
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .background(active ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 6))
        .opacity(model.isDragged(space.id) ? 0.4 : 1)
        .spacePressTarget(model, windowID: windowID, space: space.id)
        .contextMenu { SpaceMenuItems(model: model, space: space.id, window: windowID) }
    }
}

/// The x on a space's tab or sidebar row. Closing a space kills every shell and ends every agent
/// session in it, so a space with any asks first.
struct CloseSpaceButton: View {
    @ObservedObject var model: AppModel
    let space: SpaceInfo

    var body: some View {
        Button {
            model.askCloseSpace(space.id)
        } label: {
            Image(systemName: "xmark").font(.caption2)
        }
        .buttonStyle(.borderless)
        .help("Close space")
        .accessibilityLabel("Close space \(space.name)")
    }
}

struct NoSpaces: View {
    @ObservedObject var model: AppModel
    let windowID: String

    /// Every space that runs outside this window, by where it is: other windows, then the background.
    private var elsewhere: [(heading: String, spaces: [SpaceInfo])] {
        var groups: [(String, [SpaceInfo])] = []
        for other in model.layout.windows where other.id != windowID {
            let held = model.spaces(in: other.id)
            if !held.isEmpty { groups.append(("In window “\(model.title(of: other))”", held)) }
        }
        if !model.backgroundSpaces.isEmpty { groups.append(("Running in the background", model.backgroundSpaces)) }
        return groups
    }

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "square.split.2x1").font(.largeTitle).foregroundStyle(.secondary)
            if model.spaces.isEmpty {
                Text("No spaces yet").font(.headline)
                Text("A space is a project folder with its own grid of panes.")
                    .foregroundStyle(.secondary)
            } else {
                Text("No spaces in this window").font(.headline)
                Text("Bring one here, or open a folder.").foregroundStyle(.secondary)
                ForEach(elsewhere, id: \.heading) { group in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(group.heading).font(.caption).foregroundStyle(.secondary)
                        ForEach(group.spaces) { space in
                            Button { model.show(space: space.id, in: windowID) } label: {
                                Label(space.name, systemImage: "arrow.down.left.square")
                            }
                            .buttonStyle(.link)
                            .accessibilityLabel("Show \(space.name) in this window")
                        }
                    }
                }
            }
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
            let zoomed = model.zoomedPane(in: space)
            ZStack(alignment: .topLeading) {
                ForEach(layout.panes) { pane in
                    PaneView(model: model, paneId: pane.id, kind: pane.kind, focused: pane.id == space.focus)
                        .frame(width: zoomed == pane.id ? rect.width : pane.frame.width,
                               height: zoomed == pane.id ? rect.height : pane.frame.height)
                        .offset(x: zoomed == pane.id ? 0 : pane.frame.minX, y: zoomed == pane.id ? 0 : pane.frame.minY)
                        .opacity(zoomed != nil && zoomed != pane.id ? 0 : model.paneDrag?.pane == pane.id ? 0.7 : 1)
                        .allowsHitTesting(zoomed == nil || zoomed == pane.id)
                        .accessibilityHidden(zoomed != nil && zoomed != pane.id)
                        .zIndex(zoomed == pane.id ? 1 : 0)
                }
                if let drag = model.paneDrag, let drop = drag.drop, let zone = dropZone(drag, drop, layout, rect) {
                    RoundedRectangle(cornerRadius: 10)
                        .fill(Color.accentColor.opacity(0.18))
                        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color.accentColor, lineWidth: 2))
                        .frame(width: zone.width, height: zone.height)
                        .offset(x: zone.minX, y: zone.minY)
                        .allowsHitTesting(false)
                }
                if zoomed == nil {
                    ForEach(layout.dividers) { divider in
                        DividerHandle(model: model, divider: divider)
                            .frame(width: divider.frame.width, height: divider.frame.height)
                            .offset(x: divider.frame.minX, y: divider.frame.minY)
                    }
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
        case let .agent(session, agent):
            if let chat = model.chats[session] {
                // A new session in the same pane starts with a fresh draft and scroll position.
                AgentBody(model: model, paneId: paneId, agent: agent, chat: chat)
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

    private func close() { model.askClosePane(paneId) }

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
            // An agent pane's dot follows its title, where AgentTitle draws it.
            if case .agent = kind {} else {
                StatusDot(status: model.paneStatus(paneId), shown: model.statusDots)
            }
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
                PaneInfoButton(model: model, paneId: paneId)
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
        .contextMenu {
            ForEach(model.headerMenu(for: paneId, kind: kind), id: \.id) { item in
                if item.id == "close" { Divider() }
                Button(item.title, role: item.id == "close" ? .destructive : nil) {
                    if item.id == "close" { close() } else { model.runHeaderMenu(item.id, pane: paneId) }
                }
                .disabled(!item.enabled)
            }
        }
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
                if let agent {
                    // The daemon's record of the agent decides the header, whatever title the shell set.
                    AgentName(agent: agent.name)
                } else if let terminal = model.terminals[term] {
                    TerminalTitle(state: terminal.view)
                } else {
                    Text("Terminal")
                }
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
                HStack(spacing: 6) {
                    AgentName(agent: agent)
                    StatusDot(status: model.paneStatus(paneId), shown: model.statusDots)
                }
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

/// The ⓘ that opens a pane's info: what the pane is, each value copyable. The daemon builds the
/// list each time the popover opens, so it is current.
struct PaneInfoButton: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @State private var shown = false

    var body: some View {
        Button { shown.toggle() } label: { Image(systemName: "info.circle") }
            .buttonStyle(.borderless)
            .help("Pane info")
            .accessibilityLabel("Pane info")
            .popover(isPresented: $shown, arrowEdge: .bottom) {
                PaneInfoView(fields: model.paneInfo[paneId])
                    .frame(width: 360)
            }
            .onChange(of: shown) { _, now in
                if now { model.requestPaneInfo(paneId) }
            }
    }
}

/// An agent pane's title: the conversation's title, its status dot, and Stop while it works.
struct AgentTitle: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    let paneId: String
    let agent: String

    var body: some View {
        HStack(spacing: 6) {
            AgentName(agent: agent)
            if let title = chat.title {
                Text(title).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
            }
            StatusDot(status: model.paneStatus(paneId), shown: model.statusDots)
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
    let agent: String
    @ObservedObject var chat: SessionChat

    var body: some View {
        VStack(spacing: 0) {
            AgentTranscript(model: model, chat: chat, empty: "Give this agent a task.")
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
                placeholder: "Message \(AgentInfo.title(of: agent))",
                draft: $chat.draft,
                error: chat.error,
                enabled: model.connection == .connected,
                send: { model.send(chat) },
                pickFiles: pickFiles,
                onFocus: { model.paneFocused(pane: paneId) },
                focusRequest: chat.composerFocusRequest,
                skills: chat.accepted(model.agents.first { $0.name == agent }?.skills ?? []),
                skillPrefix: SkillSearch.prefix(ofAgent: agent)
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

/// An empty pane's chooser. Focused, it holds the keyboard, and each choice has a key of its own:
/// T, the agents' numbers, A and B, as the buttons show.
struct EmptyPane: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @FocusState private var keyboard: Bool

    /// The pane is the focused one of the space on screen: a split just made, a pane the keys
    /// moved to, or one clicked.
    private var focused: Bool { model.paneIsFocused(paneId) }

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
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .focusable()
        .focusEffectDisabled()
        .focused($keyboard)
        .onKeyPress(phases: .down) { press in
            guard press.modifiers.isDisjoint(with: [.command, .control, .option]) else { return .ignored }
            return choose(press.characters.lowercased()) ? .handled : .ignored
        }
        .onChange(of: focused, initial: true) { _, now in if now { keyboard = true } }
        .simultaneousGesture(TapGesture().onEnded { keyboard = true })
    }

    /// The agents a key starts, numbered from 1 in the order the buttons show them.
    private var agents: [AgentInfo] { Array(AgentInfo.pickable(model.agents).prefix(9)) }

    /// Run the choice `key` names; false when it names none.
    private func choose(_ key: String) -> Bool {
        Log.info("empty pane \(paneId): key \(key)")
        switch key {
        case "t": model.setKind(paneId, .terminal)
        case "b": model.setKind(paneId, .browser)
        case "a": model.openPalette(paneId)
        default:
            guard let n = Int(key), agents.indices.contains(n - 1) else { return false }
            model.startPane(paneId, .agent, agent: agents[n - 1].name)
        }
        return true
    }

    @ViewBuilder private var choices: some View {
        Button {
            model.setKind(paneId, .terminal)
        } label: {
            Label { KeyHinted("Terminal", key: "T") } icon: { Image(systemName: "terminal") }
        }
        ForEach(Array(agents.enumerated()), id: \.element.name) { index, agent in
            Button {
                model.startPane(paneId, .agent, agent: agent.name)
            } label: {
                Label { KeyHinted(agent.title, key: "\(index + 1)") } icon: { UsageIcon(agent: agent.name) }
            }
        }
        Button {
            model.openPalette(paneId)
        } label: {
            Label { KeyHinted("Artifact…", key: "A") } icon: { Image(systemName: "doc.text") }
        }
        Button {
            model.setKind(paneId, .browser)
        } label: {
            Label { KeyHinted("Browser", key: "B") } icon: { Image(systemName: "safari") }
        }
    }
}

/// A button's title with the key that presses it beside it, dimmed.
struct KeyHinted: View {
    let title: String
    let key: String

    init(_ title: String, key: String) {
        self.title = title
        self.key = key
    }

    var body: some View {
        HStack(spacing: 6) {
            Text(title)
            Text(key).font(.callout.monospaced()).foregroundStyle(.secondary)
        }
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityHint("Key \(key)")
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
