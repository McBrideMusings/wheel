import AppKit
import GhosttyTerminal
import SwiftUI
import WheelKit
import WheelUI

/// The active space's grid, with the spaces beside it in the sidebar or, when the sidebar is
/// closed, as tabs in the window's title row. Every space stays mounted, so a terminal keeps its screen while its
/// space is hidden; only the active one draws. A hidden space keeps the size it last had on screen,
/// so a resize or the sidebar opening lays out and resizes the active space's panes alone.
struct SpacesView: View {
    var model: AppModel
    let windowID: String
    /// The grid's size when each space was last on screen, by space id.
    @State private var sizes: [String: CGSize] = [:]
    /// The grid's size now.
    @State private var gridSize: CGSize = .zero

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

    private func grid(sidebar _: Bool) -> some View {
        let held = model.spaces(in: windowID)
        let active = model.active(in: windowID)
        return VStack(alignment: .leading, spacing: 6) {
            if let error = model.layoutError {
                LayoutErrorLabel(error: error)
            }
            ZStack(alignment: .topLeading) {
                // A space in a little window is drawn there: one terminal surface has one place.
                ForEach(held.filter { model.layout.little(showing: $0.id) == nil }) { space in
                    let shown = space.id == active
                    let kept = shown ? nil : sizes[space.id]
                    // A hidden space draws at its kept size from a zero-size frame, so it never
                    // makes the grid wider or taller than the window gives it.
                    SpaceGrid(model: model, drag: model.gridDrag, space: space)
                        .environment(\.spaceShown, shown)
                        .frame(width: kept?.width, height: kept?.height)
                        .frame(width: kept == nil ? nil : 0, height: kept == nil ? nil : 0, alignment: .topLeading)
                        .transaction { if !shown { $0.disablesAnimations = true } }
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
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.gridFrames[windowID] = $0 }
            .onGeometryChange(for: CGSize.self) { $0.size } action: { size in
                gridSize = size
                // The active space follows the grid; one never shown yet takes the size it would have had.
                if let active { sizes[active] = size }
                for space in held where sizes[space.id] == nil { sizes[space.id] = size }
            }
            .onChange(of: held.map(\.id)) { _, ids in
                sizes = sizes.filter { ids.contains($0.key) }
                guard gridSize != .zero else { return }
                for id in ids where sizes[id] == nil { sizes[id] = gridSize }
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
    var model: AppModel
    let windowID: String

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
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
        }
        }
        .frame(height: 26)
        .background { if model.paneIsOverNewSpace(in: windowID) { PaneDropOutline(cornerRadius: 6) } }
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
    var model: AppModel
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
        .background { if model.paneIsOver(space: space.id) { PaneDropOutline(cornerRadius: 6) } }
        .spacePressTarget(model, windowID: windowID, space: space.id)
        .contextMenu { SpaceMenuItems(model: model, space: space.id, window: windowID) }
    }
}

/// The x on a space's tab or sidebar row. Closing a space kills every shell and ends every agent
/// session in it, so a space with any asks first.
struct CloseSpaceButton: View {
    var model: AppModel
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
    var model: AppModel
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
    var model: AppModel
    @ObservedObject var drag: GridDragState
    let space: SpaceInfo
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Where a pane sits while another is zoomed: pushed past the grid edge it is nearest to, so
    /// the zoomed pane appears to grow over it. Reduced motion keeps it in place and fades it.
    private func slidOut(_ frame: CGRect, in rect: CGRect) -> CGSize {
        let toLeft = frame.midX - rect.minX, toRight = rect.maxX - frame.midX
        let toTop = frame.midY - rect.minY, toBottom = rect.maxY - frame.midY
        switch min(toLeft, toRight, toTop, toBottom) {
        case toLeft: return CGSize(width: rect.minX - frame.maxX, height: 0)
        case toRight: return CGSize(width: rect.maxX - frame.minX, height: 0)
        case toTop: return CGSize(width: 0, height: rect.minY - frame.maxY)
        default: return CGSize(width: 0, height: rect.maxY - frame.minY)
        }
    }

    var body: some View {
        GeometryReader { geo in
            let rect = CGRect(origin: .zero, size: geo.size)
            let layout = WheelKit.GridLayout(space.root, in: rect, ratios: drag.ratios)
            let zoomed = model.zoomedPane(in: space)
            ZStack(alignment: .topLeading) {
                ForEach(layout.panes) { pane in
                    let slide = zoomed != nil && zoomed != pane.id && !reduceMotion ? slidOut(pane.frame, in: rect) : .zero
                    PaneView(model: model, paneId: pane.id, kind: pane.kind, focused: pane.id == space.focus)
                        .frame(width: zoomed == pane.id ? rect.width : pane.frame.width,
                               height: zoomed == pane.id ? rect.height : pane.frame.height)
                        .offset(x: zoomed == pane.id ? 0 : pane.frame.minX + slide.width,
                                y: zoomed == pane.id ? 0 : pane.frame.minY + slide.height)
                        .opacity(zoomed != nil && zoomed != pane.id && reduceMotion ? 0 : drag.pane?.pane == pane.id ? 0.7 : 1)
                        .allowsHitTesting(zoomed == nil || zoomed == pane.id)
                        .accessibilityHidden(zoomed != nil && zoomed != pane.id)
                        .zIndex(zoomed == pane.id ? 1 : 0)
                        .transition(.scale(scale: 0.96).combined(with: .opacity))
                }
                if let moving = drag.pane, let drop = moving.drop, let zone = dropZone(moving, drop) {
                    RoundedRectangle(cornerRadius: 10)
                        .fill(Color.accentColor.opacity(0.18))
                        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color.accentColor, lineWidth: 2))
                        .frame(width: zone.width, height: zone.height)
                        .offset(x: zone.minX, y: zone.minY)
                        .allowsHitTesting(false)
                }
                if case let .beside(target, dir, held)? = drag.pane?.away, held == space.id,
                   let frame = model.baseLayouts[space.id]?.panes.first(where: { $0.id == target })?.frame {
                    let zone = PaneDragging.highlight(.edge(dir), in: frame)
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
            .clipped()
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
    /// a dock the half of the target the pane would take.
    private func dropZone(_ drag: AppModel.PaneDrag, _ drop: PaneDrop) -> CGRect? {
        guard space.root.panes.contains(where: { $0.id == drag.pane }),
              let frame = model.baseLayouts[space.id]?.panes.first(where: { $0.id == drop.target })?.frame else { return nil }
        return PaneDragging.highlight(drop.zone, in: frame)
    }
}

/// The gap between a split's two sides. Dragging it redraws live and tells the daemon once, at
/// the end; VoiceOver's adjust action moves it in steps of 0.05.
struct DividerHandle: View {
    var model: AppModel
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
    var model: AppModel
    let paneId: String
    let kind: PaneKind
    let focused: Bool

    var body: some View {
        let _ = GridMetrics.shared.body(of: paneId)
        VStack(spacing: 0) {
            PaneHeader(model: model, paneId: paneId, kind: kind, focused: focused)
            Divider()
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { if holdsPane { model.paneBodyFrames[paneId] = $0 } }
                .overlay {
                    if let palette = model.palettes[paneId] {
                        PaletteOverlay(model: model, palette: palette, paneId: paneId, kind: kind)
                    }
                }
        }
        .background(.background)
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { if holdsPane { model.paneFrames[paneId] = $0 } }
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(ringed ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.separator), lineWidth: ringed ? 2 : 1)
        )
    }

    /// False for a pane shrinking out after a close: its frames must not re-enter the maps `apply` just pruned.
    private var holdsPane: Bool { model.spaces.contains { $0.root.panes.contains { $0.id == paneId } } }

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
                // makeNSView runs once per view identity, so a pane switched to another artifact
                // (a new WebPane) must be a new identity or the window keeps drawing the old view.
                WebBody(web: web)
                    .id(ObjectIdentifier(web))
            } else {
                ProgressView()
            }
        case let .agent(session, agent, _):
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
    var model: AppModel
    let paneId: String
    let kind: PaneKind
    let focused: Bool

    private func close() { model.askClosePane(paneId) }

    /// The other way this pane can show its conversation: a chat for a terminal running Claude
    /// Code, the Claude Code TUI for an agent pane.
    private var otherView: ConversationView? {
        switch kind {
        case let .terminal(_, agent) where agent?.isClaude == true && model.agentFound("claude"): .agent
        case let .agent(_, agent, _): agent == "claude" ? .terminal : nil
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
            let zoom = model.zoomButton(for: paneId)
            Button { model.toggleZoom(paneId) } label: { Image(systemName: zoom.symbol) }
                .buttonStyle(.borderless)
                .foregroundStyle(zoom.on ? Color.accentColor : Color.primary)
                .disabled(!zoom.enabled)
                .help(zoom.title)
                .accessibilityLabel(zoom.title)
            Menu { menuContent } label: { Image(systemName: "ellipsis") }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Pane menu")
                .accessibilityLabel("Pane menu")
            headerButton("xmark", "Close Pane", action: close)
        }
        .font(.callout)
        .padding(.horizontal, 10)
        .frame(height: 28)
        .contentShape(Rectangle())
        .onTapGesture(count: 2) { model.toggleZoom(paneId) }
        .onTapGesture { model.focus(pane: paneId) }
        .contextMenu { menuContent }
        // Dragging the header swaps this pane with another or docks it beside one; the grid
        // previews the result and the daemon changes the tree once the pointer is released.
        .gesture(
            DragGesture(minimumDistance: 6, coordinateSpace: .named("grid"))
                .onChanged {
                    model.beginPaneDrag(paneId, grabbedAt: $0.startLocation)
                    model.dragPane(to: $0.location)
                }
                .onEnded { _ in model.endPaneDrag() }
        )
    }

    /// The pane menu, shared by the header's right-click and its ⋯ button so the two never differ.
    @ViewBuilder private var menuContent: some View {
        let items = model.headerMenu(for: paneId, kind: kind)
        ForEach(items.filter { !$0.id.hasPrefix("move-space:") && !$0.id.hasPrefix("kind:") }, id: \.id) { item in
            if item.id == "close" { Divider() }
            if item.id == "move-new-space" {
                changeTypeMenu(items)
                Divider()
                MovePaneMenu(model: model, pane: paneId)
            }
            Button(item.title, role: item.id == "close" ? .destructive : nil) {
                if item.id == "close" { close() } else { model.runHeaderMenu(item.id, pane: paneId) }
            }
            .disabled(!item.enabled)
        }
    }

    @ViewBuilder private func changeTypeMenu(_ items: [AppModel.HeaderMenuItem]) -> some View {
        let kinds = items.filter { $0.id.hasPrefix("kind:") }
        if !kinds.isEmpty {
            Menu("Change Type") {
                ForEach(kinds, id: \.id) { item in
                    Button(item.title.replacingOccurrences(of: "Change Type ▸ ", with: "")) {
                        model.runHeaderMenu(item.id, pane: paneId)
                    }
                    .disabled(!item.enabled)
                }
            }
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
        case let .agent(session, agent, _):
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
    var model: AppModel
    let paneId: String
    @ObservedObject var terminal: PaneTerminal
    @ObservedObject var termState: TerminalViewState

    @Environment(\.displayScale) private var scale
    /// The size last given to `model.settle`, and so the grid the shell is told. The surface itself
    /// follows the pane's frame on every display frame, so its text reflows live while a window or
    /// a layout animation moves it; the shell hears the new size once the frame has held still for
    /// `settleDelay`, one resize per gesture rather than one per frame.
    @State private var told: CGSize?
    @State private var settling: Task<Void, Never>?

    /// How long the pane's frame must hold still before the shell is told its size.
    private static let settleDelay: Duration = .milliseconds(80)

    private func laidOut(_ size: CGSize) {
        told = size
        terminal.layout = (size, scale)
        model.settle(terminal)
    }

    private func frameChanged(_ size: CGSize) {
        settling?.cancel()
        guard size.width > 0, size.height > 0 else { return }
        guard told != nil else {
            laidOut(size)
            return
        }
        guard size != told else { return }
        settling = Task { @MainActor in
            try? await Task.sleep(for: Self.settleDelay)
            guard !Task.isCancelled else { return }
            laidOut(size)
        }
    }

    var body: some View {
        GeometryReader { geo in
            TerminalSurfaceView(context: termState)
                .frame(width: geo.size.width, height: geo.size.height)
                .onChange(of: geo.size, initial: true) { _, size in frameChanged(size) }
                .onChange(of: scale) { _, _ in laidOut(told ?? geo.size) }
                .onDisappear { settling?.cancel() }
        }
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
    var model: AppModel
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

/// An agent pane's title: the conversation's title and its status dot.
struct AgentTitle: View {
    var model: AppModel
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
            if model.appearance(forSession: chat.name).home == .header {
                HomeLabel(agent: agent, home: model.shownHome(session: chat.name, agent: agent))
            }
        }
    }
}

/// How full the conversation's context window is: a small bar and the percent, the exact tokens
/// on hover. A click switches this pane's label between the percent and the raw figures. Callers
/// draw it only once a figure has been reported, since the daemon never guesses.
struct ContextMeter: View {
    var model: AppModel
    let paneId: String
    let context: ContextUse

    var body: some View {
        HStack(spacing: 6) {
            ZStack(alignment: .leading) {
                Capsule().fill(Color.primary.opacity(0.15))
                Capsule()
                    .fill(tint(context))
                    .frame(width: 28 * context.fraction)
            }
            .frame(width: 28, height: 4)
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

/// The agent's home directory as plain text with a folder glyph. It never opens a menu; the hover
/// names the variable the directory is passed in and the setting that places this text.
struct HomeLabel: View {
    let agent: String
    let home: String

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: "folder")
            Text(home).lineLimit(1).truncationMode(.middle)
        }
        .font(ChatFont.meta)
        .foregroundStyle(.tertiary)
        .layoutPriority(-1)
        .help("\(AgentInfo.homeVariable(of: agent)) is \(home). Set by agent.\(agent).home; chat.session.home places this text.")
    }
}

/// An agent pane: the agent's conversation, the request it waits on, a composer, and the controls
/// it offers over its own session.
struct AgentBody: View {
    var model: AppModel
    let paneId: String
    let agent: String
    @ObservedObject var chat: SessionChat

    var body: some View {
        VStack(spacing: 0) {
            AgentTranscript(model: model, chat: chat, empty: "Give this agent a task.", agent: agent)
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
                skillPrefix: SkillSearch.prefix(ofAgent: agent),
                busy: chat.isBusy,
                stop: { model.cancel(chat) },
                takeBack: { chat.takeBack() },
                // Return allows and Escape denies this agent's oldest waiting request.
                permissionKey: model.perms.first(where: { $0.session == chat.name }).map { perm in
                    { (key: PermissionKey) -> Bool in
                        guard let option = key.option(in: perm) else { return false }
                        model.answer(perm, option.id)
                        return true
                    }
                }
            )
            .padding([.horizontal, .top], 10)
            ConfigRow(model: model, chat: chat, paneId: paneId, agent: agent)
                .padding(.horizontal, 14)
                .padding(.top, 6)
                .padding(.bottom, 10)
        }
        .simultaneousGesture(TapGesture().onEnded { model.paneFocused(pane: paneId) })
        .onAppear { chat.composerShown = true }
        .onDisappear { chat.composerShown = false }
    }
}

/// An empty pane's chooser. Focused, it holds the keyboard, and each choice has a key of its own:
/// T, the agents' numbers, A and B, as the buttons show.
struct EmptyPane: View {
    var model: AppModel
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
    var model: AppModel
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

private struct SpaceShownKey: EnvironmentKey {
    static let defaultValue = true
}

extension EnvironmentValues {
    /// Whether the space this view belongs to is the one its window shows.
    var spaceShown: Bool {
        get { self[SpaceShownKey.self] }
        set { self[SpaceShownKey.self] = newValue }
    }
}
