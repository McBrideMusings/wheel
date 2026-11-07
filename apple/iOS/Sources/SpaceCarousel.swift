import SwiftUI
import WheelKit
import WheelUI

/// The paired phone's screen: the title menu on top, and the shown space's panes as swipeable pages.
struct SpaceCarousel: View {
    let model: PhoneModel
    @State private var page: String?
    /// The backgrounded pane the last page shows in place; none shows the list.
    @State private var openTray: String?
    @State private var newSpaceOpen = false
    @State private var titleOpen = false
    @State private var paletteOpen = false
    /// What the title menu asked for, presented once its sheet has closed.
    @State private var queued: TitleMenuRequest?
    @State private var renaming: (space: String, name: String)?
    @State private var closing: (space: String, question: String)?
    @State private var newSessionOpen = false
    @State private var newSessionName = ""
    @State private var settingsOpen = false
    @State private var settingsTab: SettingsTab = .global
    @State private var settingsProject: String?

    var body: some View {
        GeometryReader { geo in
            let landscape = geo.size.width > geo.size.height
            let pages = Pages(model.activeSpace)
            let panes = pages.panes
            let dots = Dots(kinds: pages.dotKinds, current: pages.index(of: page), trayNeedsYou: trayNeedsYou(pages.tray))
            VStack(spacing: 0) {
                topBar(panes: panes, dots: dots, dotsHere: landscape)
                if panes.isEmpty {
                    ContentUnavailableView(connectionText, systemImage: "rectangle.on.rectangle")
                        .accessibilityIdentifier("no panes")
                } else {
                    TabView(selection: $page) {
                        ForEach(panes, id: \.id) { pane in
                            paneView(pane).tag(Optional(pane.id))
                        }
                        if !pages.tray.isEmpty {
                            BackgroundedPage(model: model, tray: pages.tray, open: $openTray) { paneView($0) }
                                .tag(Optional(Pages.trayID))
                        }
                    }
                    .tabViewStyle(.page(indexDisplayMode: .never))
                }
                if !landscape && pages.count > 1 {
                    dots.padding(.vertical, 10)
                }
            }
            .onChange(of: trayIDs) { _, ids in trayChanged(ids) }
        }
        .onChange(of: model.activeSpace?.focus, initial: true) { _, focus in
            // The daemon moved focus (the Mac, or the CLI): the page follows. A backgrounded pane is
            // on the last page, open in place.
            guard let focus, page != focus else { return }
            if model.activeSpace?.site(of: focus) == .backgrounded {
                openTray = focus
                page = Pages.trayID
            } else {
                page = focus
            }
        }
        .onChange(of: model.activeSpace?.id) { _, _ in page = model.activeSpace?.focus }
        .onChange(of: page, initial: true) { _, new in model.show(pane: new == Pages.trayID ? openTray : new) }
        .onChange(of: openTray) { _, open in if page == Pages.trayID { model.show(pane: open) } }
        .onChange(of: page) { _, new in
            // Another page closes the keyboard; nothing on the new page takes it until a tap.
            let closed = UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
            Log.info("page \(new ?? "none"): keyboard \(closed ? "closed" : "was not up")")
            // A swipe: tell the daemon, so the Mac's focus follows.
            if let space = model.activeSpace, let new, new != Pages.trayID, new != space.focus { model.focus(space: space.id, pane: new) }
        }
        .task { await driveFromEnvironment() }
        .sheet(isPresented: $settingsOpen) {
            SettingsView(model: model, tab: $settingsTab, project: $settingsProject)
        }
        .alert("Could not change the pane", isPresented: Binding(get: { model.menuError != nil }, set: { if !$0 { model.menuError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.menuError ?? "")
        }
        .sheet(isPresented: $newSpaceOpen) { NewSpaceSheet(model: model) }
        .sheet(isPresented: $paletteOpen) {
            CommandPaletteSheet(model: model) { request in
                queued = request
                paletteOpen = false
            }
        }
        .onChange(of: titleOpen) { _, open in presentQueued(whenClosed: !open) }
        .onChange(of: paletteOpen) { _, open in presentQueued(whenClosed: !open) }
        .alert("Rename space", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: Binding(get: { renaming?.name ?? "" }, set: { renaming?.name = $0 }))
                .accessibilityIdentifier("rename field")
            Button("Cancel", role: .cancel) {}
            Button("Rename") { if let renaming { model.renameSpace(renaming.space, to: renaming.name) } }
        }
        // An alert, not a confirmation dialog: iOS 26 draws a dialog with no anchor as a card with no
        // Cancel that a tap outside does not close, so the person could not back out.
        .alert(closing?.question ?? "", isPresented: Binding(get: { closing != nil }, set: { if !$0 { closing = nil } })) {
            Button("Close space", role: .destructive) { if let closing { model.closeSpace(closing.space) } }
            Button("Cancel", role: .cancel) {}
        }
        .alert("New session", isPresented: $newSessionOpen) {
            TextField("Name", text: $newSessionName).accessibilityIdentifier("session name field")
            Button("Cancel", role: .cancel) {}
            Button("Create") { model.newSession(named: newSessionName) }
        }
        .alert("Could not change the space", isPresented: Binding(get: { model.spaceError != nil }, set: { if !$0 { model.spaceError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.spaceError ?? "")
        }
    }

    private func presentQueued(whenClosed closed: Bool) {
        guard closed, let request = queued else { return }
        queued = nil
        present(request)
    }

    private func present(_ request: TitleMenuRequest) {
        Log.info("title menu asks: \(request)")
        switch request {
        case let .rename(space, name): renaming = (space, name)
        case let .close(space, question): closing = (space, question)
        case .newSession:
            newSessionName = ""
            newSessionOpen = true
        case .newSpace: newSpaceOpen = true
        }
    }

    @ViewBuilder private func paneView(_ pane: (id: String, kind: PaneKind)) -> some View {
        if case let .terminal(term, _) = pane.kind {
            let terminal = model.terminal(term)
            TerminalPage(model: model, terminal: terminal, termState: terminal.view)
        } else if case let .agent(agent, harness, _) = pane.kind, let chat = model.chats[agent] {
            AgentPane(model: model, pane: pane.id, agent: agent, harness: harness, chat: chat)
        } else if let a2ui = model.a2uiPanes[pane.id], a2ui.isNative {
            A2UIPaneView(pane: a2ui)
        } else if let web = model.webPanes[pane.id] {
            WebBody(web: web)
        } else {
            PanePlaceholder(id: pane.id, kind: pane.kind)
        }
    }

    private var connectionText: String {
        switch model.connection {
        case .connecting: "Connecting…"
        case .connected: model.activeSpace == nil ? "No spaces in \(model.shownSession?.name ?? "this session")" : "No panes"
        case let .unreachable(why): "Cannot reach the Mac: \(why)"
        }
    }

    private var trayIDs: [String] { Pages(model.activeSpace).tray.map(\.id) }

    /// The pane open in place left the tray, or the tray emptied: back to the list, or to the grid.
    private func trayChanged(_ ids: [String]) {
        if let open = openTray, !ids.contains(open) { openTray = nil }
        if ids.isEmpty, page == Pages.trayID { page = model.activeSpace?.focus }
    }

    private func trayNeedsYou(_ tray: [(id: String, kind: PaneKind)]) -> Bool {
        tray.contains { if case .needsYou = model.paneStatus($0.id) { true } else { false } }
    }

    private func topBar(panes: [(id: String, kind: PaneKind)], dots: Dots, dotsHere: Bool) -> some View {
        HStack {
            TitleButton(model: model, open: $titleOpen)
                // A sheet, not a popover: a popover is as tall as its content and clips it, so a
                // long list of spaces could not be scrolled or reached.
                .sheet(isPresented: $titleOpen) {
                    TitleMenu(model: model) { request in
                        queued = request
                        titleOpen = false
                    }
                    .presentationDetents([.medium, .large])
                    .presentationDragIndicator(.visible)
                }
                // Leaves the bar's buttons on the right uncovered.
                .frame(maxWidth: 240, alignment: .leading)
            Spacer()
            if dotsHere && dots.kinds.count > 1 {
                dots
            }
            Button { paletteOpen = true } label: { Image(systemName: "magnifyingglass") }
                .accessibilityLabel("Search")
                .accessibilityIdentifier("search")
            // One ⋯ holds the pane's actions and Settings. The pane's logo and status dot are in the
            // agent header below, once. With no pane to act on, the ⋯ holds Settings alone.
            if let current = panes.first(where: { $0.id == page }) ?? panes.first {
                PaneMenu(model: model, pane: current.id, kind: current.kind) { openSettings(.global) }
            } else {
                Menu {
                    Button("Settings", systemImage: "gearshape") { openSettings(.global) }
                } label: {
                    Image(systemName: "ellipsis.circle").font(.title3)
                }
                .accessibilityLabel("Pane menu")
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .frame(minHeight: 50)
    }

    private func openSettings(_ tab: SettingsTab) {
        settingsTab = tab
        if settingsProject == nil || !model.spaces.contains(where: { $0.id == settingsProject }) {
            settingsProject = model.activeSpace?.id
        }
        settingsOpen = true
    }

    /// Checks drive the app through launch environment: `WHEEL_START_PAGE=<n>` focuses the n-th
    /// pane (1-based; `tray` for the last page, with `WHEEL_TRAY_OPEN=<pane>` opening that backgrounded pane in it) as a swipe would, `WHEEL_MENU=<item>,…` runs pane-menu items on the current page (`PaneMenu.drive`), `WHEEL_SETTINGS=<global|project|devices>` opens Settings on that tab, `WHEEL_SET=<global|project>:<key>=<value>` writes a setting as a field's Return does (a blank value clears it), `WHEEL_NEW_SPACE=<1|search:<text>|browse:<path>[,<path>]|create:<path>>` opens the New Space sheet and drives it (`NewSpaceSheet.drive`), `WHEEL_START_SPACE=<name>` picks that space and `WHEEL_START_ORIENTATION=landscape` rotates; `WHEEL_TYPE`, `WHEEL_SEND` and `WHEEL_ANSWER` are below.
    private func driveFromEnvironment() async {
        #if DEBUG
        let env = ProcessInfo.processInfo.environment
        try? await Task.sleep(for: .seconds(2))
        if env["WHEEL_START_ORIENTATION"] == "landscape",
           let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene {
            Log.info("rotate to landscape")
            scene.requestGeometryUpdate(.iOS(interfaceOrientations: .landscapeRight))
            try? await Task.sleep(for: .seconds(1))
        }
        if let name = env["WHEEL_START_SPACE"], let space = model.spaces.first(where: { $0.name == name }), space.id != model.activeSpace?.id {
            model.select(space: space.id)
            try? await Task.sleep(for: .seconds(1))
        }
        if env["WHEEL_START_PAGE"] == "tray" {
            Log.info("start page tray")
            openTray = env["WHEEL_TRAY_OPEN"]
            withAnimation { page = Pages.trayID }
        } else if let n = env["WHEEL_START_PAGE"].flatMap(Int.init), case let panes = Pages(model.activeSpace).panes, (1...max(panes.count, 1)).contains(n) {
            Log.info("start page \(n)")
            withAnimation { page = panes[n - 1].id }
        }
        // `WHEEL_TYPE=<text>` pastes the text, then Return, into the shown terminal.
        if let text = env["WHEEL_TYPE"] {
            try? await Task.sleep(for: .seconds(1))
            model.typeIntoShown(text)
        }
        // `WHEEL_SWIPE=<n>` raises the keyboard in the shown terminal, then swipes to the n-th page.
        if let n = env["WHEEL_SWIPE"].flatMap(Int.init), case let panes = Pages(model.activeSpace).panes, (1...max(panes.count, 1)).contains(n) {
            model.focusShown()
            try? await Task.sleep(for: .seconds(2))
            Log.info("swipe to page \(n)")
            withAnimation { page = panes[n - 1].id }
        }
        if env["WHEEL_NEW_SPACE"] != nil {
            Log.info("open new space sheet")
            newSpaceOpen = true
        }
        if let tab = env["WHEEL_SETTINGS"].flatMap({ SettingsTab(rawValue: $0.capitalized) }) {
            Log.info("open settings \(tab.rawValue)")
            openSettings(tab)
        }
        if let spec = env["WHEEL_SET"], let colon = spec.firstIndex(of: ":"), let eq = spec.firstIndex(of: "="), colon < eq,
           let layer = SettingField.Layer(rawValue: String(spec[..<colon])) {
            if !model.settingsLoaded { model.askSettings(for: model.activeSpace?.id) }
            for _ in 0 ..< 30 where !model.settingsLoaded { try? await Task.sleep(for: .seconds(1)) }
            model.setSetting(SettingField(layer: layer, key: String(spec[spec.index(after: colon)..<eq])), to: String(spec[spec.index(after: eq)...]))
        }
        if let items = env["WHEEL_MENU"] {
            for item in items.split(separator: ",") {
                let panes = model.activeSpace?.root.readingOrder ?? []
                if let current = panes.first(where: { $0.id == page }) ?? panes.first {
                    await PaneMenu.drive(String(item), model: model, pane: current.id, kind: current.kind)
                }
                try? await Task.sleep(for: .seconds(2))
            }
        }
        // `WHEEL_ATTACH=<path>[,<path>]` adds each file to the first agent pane's composer through
        // the same call the document picker makes (HEIC converted, the size checked), once the
        // server has said it takes uploads; `WHEEL_ATTACH_REMOVE=<n>` then takes the n-th chip
        // back once every upload is done, as its ✕ does.
        let firstAgent: () -> (pane: String, agent: String)? = {
            let panes = model.activeSpace?.root.readingOrder ?? []
            return panes.lazy.compactMap { pane -> (String, String)? in
                if case let .agent(agent, _, _) = pane.kind { (pane.id, agent) } else { nil }
            }.first
        }
        if let paths = env["WHEEL_ATTACH"] {
            for _ in 0 ..< 30 where !(model.canAttach && firstAgent() != nil) { try? await Task.sleep(for: .seconds(1)) }
            if let agent = firstAgent(), model.canAttach, let tray = model.trays[agent.pane] {
                let pane = agent.pane
                for path in paths.split(separator: ",") { AttachmentMedia.addFile(URL(fileURLWithPath: String(path)), to: tray) }
                for _ in 0 ..< 120 where !tray.ready { try? await Task.sleep(for: .milliseconds(500)) }
                Log.info("attachments in \(pane): \(tray.chips.map { "\($0.name)=\($0.state)" }.joined(separator: ", "))")
                if let n = env["WHEEL_ATTACH_REMOVE"].flatMap(Int.init), tray.chips.indices.contains(n - 1) {
                    tray.remove(tray.chips[n - 1].id)
                    try? await Task.sleep(for: .seconds(1))
                }
            } else {
                Log.info("WHEEL_ATTACH: no paperclip (server protocol \(model.serverProtocol), agent pane \(firstAgent()?.pane ?? "none"))")
            }
        }
        // `WHEEL_SEND=<text>` types the text into the first agent pane's composer and sends it,
        // with whatever that composer holds, through the same call the send button makes (an
        // empty text sends the attachments alone); `WHEEL_ANSWER=<option id>` answers that
        // pane's oldest waiting request as its button does, once one waits.
        if let text = env["WHEEL_SEND"] {
            for _ in 0 ..< 30 {
                if let agent = firstAgent(), let chat = model.chats[agent.agent] {
                    chat.draft = text
                    model.send(chat, attachments: model.trays[agent.pane])
                    break
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
        if let choice = env["WHEEL_ANSWER"] {
            for _ in 0 ..< 120 {
                if let perm = model.perms.first {
                    model.answer(perm, choice)
                    break
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
        #endif
    }
}

/// What a page shows until the agent and web panes land.
private struct PanePlaceholder: View {
    let id: String
    let kind: PaneKind

    var body: some View {
        VStack(spacing: 8) {
            if case let .agent(_, agent, _) = kind {
                AgentLogo(agent: agent).frame(width: 40, height: 40)
            } else {
                Image(systemName: symbol).font(.largeTitle)
            }
            Text(kind.name).font(.title2)
            Text(id).font(.caption).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.quaternary.opacity(0.4))
    }

    private var symbol: String {
        switch kind {
        case .terminal: "terminal"
        case .agent: "bubble.left.and.text.bubble.right"
        case .web, .browser: "globe"
        case .empty: "square.dashed"
        case .unknown: "questionmark.square.dashed"
        }
    }
}

/// The pages of the shown space: grid panes in reading order, then each popped window's panes,
/// then, only when the tray holds any, one last page listing the backgrounded panes.
private struct Pages {
    /// The tray page's tag: no pane has this id.
    static let trayID = "tray"

    let grid: [(id: String, kind: PaneKind)]
    let popped: [(id: String, kind: PaneKind)]
    let tray: [(id: String, kind: PaneKind)]

    init(_ space: SpaceInfo?) {
        grid = space?.root.readingOrder ?? []
        popped = space?.popped.flatMap(\.readingOrder) ?? []
        tray = space?.backgrounded.flatMap(\.panes) ?? []
    }

    var panes: [(id: String, kind: PaneKind)] { grid + popped }
    var count: Int { panes.count + (tray.isEmpty ? 0 : 1) }

    var dotKinds: [Dots.Kind] {
        grid.map { _ in .grid } + popped.map { _ in .popped } + (tray.isEmpty ? [] : [.tray])
    }

    func index(of page: String?) -> Int {
        if page == Self.trayID, !tray.isEmpty { return panes.count }
        return panes.firstIndex { $0.id == page } ?? 0
    }
}

/// The last page: one row per backgrounded pane, its own icon and title and nothing else. A tap
/// opens the pane in place under a back control.
private struct BackgroundedPage<Content: View>: View {
    let model: PhoneModel
    let tray: [(id: String, kind: PaneKind)]
    @Binding var open: String?
    @ViewBuilder let content: ((id: String, kind: PaneKind)) -> Content

    var body: some View {
        if let id = open, let pane = tray.first(where: { $0.id == id }) {
            VStack(spacing: 0) {
                HStack {
                    Button { open = nil } label: { Image(systemName: "chevron.left").frame(width: 44, height: 36) }
                        .accessibilityLabel("Back")
                        .accessibilityIdentifier("tray back")
                    Spacer()
                }
                .padding(.horizontal, 8)
                content(pane)
            }
        } else {
            List(tray, id: \.id) { pane in
                Button { open = pane.id } label: {
                    HStack(spacing: 12) {
                        Image(systemName: paneSymbol(pane.kind))
                            .foregroundStyle(Color.secondary)
                            .frame(width: 24)
                        Text(PhoneCommands.paneName(pane.kind, status: nil) ?? pane.id)
                            .foregroundStyle(.primary)
                            .lineLimit(1)
                        Spacer()
                    }
                }
                .accessibilityIdentifier("tray row \(pane.id)")
            }
            .listStyle(.plain)
            .accessibilityIdentifier("tray list")
        }
    }
}

/// The icon a pane of this kind shows wherever a phone lists it.
private func paneSymbol(_ kind: PaneKind) -> String {
    switch kind {
    case .terminal: "terminal"
    case .agent: "bubble.left.and.text.bubble.right"
    case .web, .browser: "globe"
    case .empty: "square.dashed"
    case .unknown: "questionmark.square.dashed"
    }
}

/// One mark per page: a filled dot for a grid pane, a ring for a popped-out one, and for the
/// tray page a rounded square, which carries the needs-you badge when a backgrounded pane needs the person.
private struct Dots: View {
    enum Kind { case grid, popped, tray }

    let kinds: [Kind]
    let current: Int
    let trayNeedsYou: Bool

    var body: some View {
        HStack(spacing: 6) {
            ForEach(Array(kinds.enumerated()), id: \.offset) { i, kind in
                mark(kind, selected: i == current)
            }
        }
        .accessibilityIdentifier("page dots")
    }

    @ViewBuilder private func mark(_ kind: Kind, selected: Bool) -> some View {
        let color = selected ? Color.primary : Color.secondary.opacity(0.35)
        switch kind {
        case .grid:
            Circle().fill(color).frame(width: 7, height: 7)
        case .popped:
            Circle().strokeBorder(color, lineWidth: 1.5).frame(width: 7, height: 7)
        case .tray:
            RoundedRectangle(cornerRadius: 2).fill(color).frame(width: 7, height: 7)
                .overlay(alignment: .topTrailing) {
                    if trayNeedsYou { Circle().fill(Color.orange).frame(width: 6, height: 6).offset(x: 3, y: -3) }
                }
        }
    }
}
