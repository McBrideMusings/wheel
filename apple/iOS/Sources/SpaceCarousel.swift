import SwiftUI
import WheelKit
import WheelUI

/// The paired phone's screen: the title menu on top, and the shown space's panes as swipeable pages.
struct SpaceCarousel: View {
    let model: PhoneModel
    let servers: ServerList
    @State private var page: String?
    /// The backgrounded pane the last page shows in place; none shows the list.
    @State private var openTray: String?
    @State private var newSpaceOpen = false
    @State private var titleOpen = false
    /// While the keyboard is up the page handle goes, so the composer sits on the keyboard (wheel-eg4e.19.41).
    @State private var keyboardUp = false
    @State private var sheetTab = ThumbTab.spaces
    @State private var sheetScope = AgentScope.all
    @State private var paletteOpen = false
    /// The eye button's sheet: every agent of the shown session.
    @State private var agentsOpen = false
    /// The queue tray's sheet, in Orchestrator mode.
    @State private var queueOpen = false
    /// What the title menu asked for, presented once its sheet has closed.
    @State private var queued: TitleMenuRequest?
    @State private var renaming: (space: String, name: String)?
    @State private var closing: (space: String, question: String)?
    @State private var newSessionOpen = false
    @State private var newSessionName = ""
    /// The server New Session makes the session on.
    @State private var newSessionServer: PhoneModel?
    @State private var addServerOpen = false
    @State private var forgetting: PhoneModel?
    @State private var settingsOpen = false
    @State private var settingsTab: SettingsTab = .global
    @State private var settingsProject: String?

    var body: some View {
        GeometryReader { geo in
            let landscape = geo.size.width > geo.size.height
            let pages = self.pages
            let panes = pages.panes
            let dots = Dots(kinds: pages.dotKinds, current: pages.index(of: page), trayNeedsYou: trayNeedsYou(pages.tray))
            let spacesMode = showsPages
            VStack(spacing: 0) {
                topBar(pages: pages, dots: dots, landscape: landscape)
                // The sheet lies over the pages and the dots bar, so moving it moves neither. Its header never leaves the
                // screen, so while a pane is pinned the pages and the dots bar end above it and it covers no control.
                ZStack(alignment: .bottom) {
                    VStack(spacing: 0) {
                        if model.mode == WindowMode.agents {
                            ModeBody(mode: model.mode)
                        } else if model.mode == WindowMode.orchestrator && model.activeSpace == nil {
                            NoOrchestrator(model: model)
                        } else if panes.isEmpty && pages.tray.isEmpty {
                            ContentUnavailableView(pinnedIDs.isEmpty ? connectionText : "Every pane is pinned", systemImage: "rectangle.on.rectangle")
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
                        // Orchestrator mode has no page handle, so the composer sits on the home indicator.
                        if !landscape && !keyboardUp && !orchestratorMode {
                            pageHandle(pages: pages, dots: dots, spacesMode: spacesMode)
                        }
                    }
                    .overlay { floatButton(pages: pages, landscape: landscape) }
                    // Voice mode shrunk floats over whichever pane is showing.
                    .overlay(alignment: .top) {
                        if model.voice.stage != .off, model.voice.layout == .pill { VoicePill(model: model).padding(.top, 6) }
                    }
                    .padding(.bottom, spacesMode && !orchestratorMode && model.activeSpace.flatMap { model.sheet(in: $0.id) } != nil ? PinSheetView<EmptyView>.headerHeight : 0)
                    if spacesMode, !orchestratorMode, let space = model.activeSpace, let sheet = model.sheet(in: space.id),
                       let pane = space.allPanes.first(where: { $0.id == sheet.pane }) {
                        PinSheetView(model: model, space: space, sheet: sheet, pane: pane) { kindView($0) }
                    }
                }
            }
            .onChange(of: trayIDs) { _, ids in trayChanged(ids) }
            .onChange(of: pinnedIDs) { old, new in pinsChanged(from: old, to: new) }
            .onChange(of: orchestratorMode ? pages.pageIDs : [], initial: true) { _, ids in
                if !ids.isEmpty { Log.info("orchestrator pages: \(ids.joined(separator: ", "))") }
            }
            // A pane link in a reply slides to its page.
            .onChange(of: model.orchestratorPage) { _, shown in
                if let chat = model.orchestratorPane, (shown ?? chat) != page { go(shown ?? chat) }
            }
            // Entering the mode shows the page it showed last, else the chat.
            .onChange(of: model.orchestratorPane, initial: true) { _, chat in
                if let chat { page = model.orchestratorPage.flatMap { pages.pageIDs.contains($0) ? $0 : nil } ?? chat }
            }
            .onChange(of: shownPageName(pages: pages), initial: true) { _, name in Log.info("page name: \(name ?? "none")") }
        }
        // Orchestrator mode's pages are the phone's own: no other client's focus moves them.
        .onChange(of: model.activeSpace?.focus, initial: true) { _, focus in if !orchestratorMode { follow(focus) } }
        .onChange(of: model.activeSpace?.id) { _, _ in if !orchestratorMode { follow(model.activeSpace?.focus) } }
        .onChange(of: page, initial: true) { _, new in model.show(pane: new == Pages.trayID ? openTray : new) }
        .onChange(of: openTray) { _, open in
            guard page == Pages.trayID else { return }
            model.show(pane: open)
            // A tap on a row, as a swipe does: tell the daemon what the phone shows.
            if let space = model.activeSpace, let open, open != space.focus { model.focus(space: space.id, pane: open) }
        }
        .onChange(of: page) { _, new in
            // Another page closes the keyboard; nothing on the new page takes it until a tap.
            let closed = UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
            Log.info("page \(new ?? "none"): keyboard \(closed ? "closed" : "was not up")")
            // A swipe: tell the daemon, so the Mac's focus follows.
            if let space = model.activeSpace, let new, new != Pages.trayID, new != space.focus { model.focus(space: space.id, pane: new) }
        }
        .task { await driveFromEnvironment() }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillShowNotification)) { note in
            keyboardUp = true
            let top = (note.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect)?.minY ?? 0
            Log.info("keyboard up, top \(Int(top.rounded()))")
        }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in keyboardUp = false }
        .sheet(isPresented: $settingsOpen) {
            SettingsView(model: model, servers: servers, tab: $settingsTab, project: $settingsProject)
        }
        .modifier(ServerPrompts(servers: servers, adding: $addServerOpen, forgetting: $forgetting))
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
            Button("Create") {
                let server = newSessionServer ?? model
                servers.show(server)
                server.newSession(named: newSessionName)
            }
        }
        .alert("Could not change the space", isPresented: Binding(get: { model.spaceError != nil }, set: { if !$0 { model.spaceError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.spaceError ?? "")
        }
    }

    /// The daemon moved focus (the Mac, the CLI, or a tile in the agent sheet), or the phone changed
    /// space: the page follows. A backgrounded pane is on the last page, open in place.
    private func follow(_ focus: String?) {
        guard let focus else { return }
        if pinnedIDs.contains(focus) {
            // A pinned pane is the sheet's, never a page: the sheet shows it and the page stays.
            model.showPinned(focus)
        } else if model.activeSpace?.site(of: focus) == .backgrounded {
            if openTray != focus { openTray = focus }
            if page != Pages.trayID { page = Pages.trayID }
        } else if page != focus {
            page = focus
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
        case let .newSession(server):
            newSessionServer = servers.servers.first { $0.id == server }
            newSessionName = ""
            newSessionOpen = true
        case .addServer: addServerOpen = true
        case let .forget(server): forgetting = servers.servers.first { $0.id == server }
        case .newSpace: newSpaceOpen = true
        }
    }

    @ViewBuilder private func paneView(_ pane: (id: String, kind: PaneKind)) -> some View {
        if let mirror = model.mirror(pane.id) {
            // The phone's pages have no header, so a mirror names what it shows in a line of its own.
            VStack(spacing: 0) {
                Text(mirror.title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 4)
                    .accessibilityIdentifier("mirror title \(pane.id)")
                kindView(pane)
            }
        } else {
            kindView(pane)
        }
    }

    /// The page a pane of this kind draws.
    @ViewBuilder private func kindView(_ pane: (id: String, kind: PaneKind)) -> some View {
        if case let .terminal(term, _) = pane.kind, model.mirror(pane.id) != nil {
            // A mirror can sit in its source's space, beside the source's page, so it draws a surface of its own.
            TerminalEmbed(model: model, host: pane.id, term: term).id(term)
                .overlay(SwipeArea.tint.allowsHitTesting(false))
        } else if case let .terminal(term, _) = pane.kind {
            TerminalPageSurface(model: model, term: term)
                // The surface draws its own opaque background, so the tint goes over it.
                .overlay(SwipeArea.tint.allowsHitTesting(false))
        } else if case let .agent(agent, harness, _) = pane.kind, let chat = model.chats[agent] {
            AgentPane(model: model, pane: pane.id, agent: agent, harness: harness, chat: chat,
                      orchestrator: pane.id == model.orchestratorPane)
                // Behind the page, so what scrolls sideways on its own fill (a code block) stays untinted.
                .background(SwipeArea.tint)
        } else if let link = model.pluginPanes[pane.id] {
            PluginPage(model: model, pane: link.pane, plugin: link.plugin)
        } else if let web = model.webPanes[pane.id] {
            WebBody(web: web)
        } else if case .browser = pane.kind {
            // A mirror draws its source's browser: wheeld knows only the source as a browser pane.
            BrowserPage(model: model, pane: model.mirror(pane.id)?.source ?? pane.id)
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

    /// The shown space's pages; in Orchestrator mode the orchestrator's chat leads them.
    private var pages: Pages { Pages(model.activeSpace, pinned: pinnedIDs, chat: model.orchestratorPane) }

    private var trayIDs: [String] { pages.tray.map(\.id) }

    /// The shown page's name: the context line's pane in Spaces mode, and in Orchestrator mode the line under the topic
    /// on every page but the chat. Nil on the chat, on the backgrounded panes' list, and with no page.
    private func shownPageName(pages: Pages) -> String? {
        let ids = pages.pageIDs
        guard let space = model.activeSpace, let shown = ids.contains(page ?? "") ? page : ids.first, shown != model.orchestratorPane else { return nil }
        if shown == Pages.trayID { return openTray.map { model.pageName($0, in: space) } }
        return model.pageName(shown, in: space)
    }

    /// The shown space's pinned panes, drawn in the sheet and left out of the pages.
    /// Orchestrator mode draws no pin sheet, so a pane pinned in Spaces mode is one of its pages.
    private var pinnedIDs: [String] { orchestratorMode ? [] : model.activeSpace.map { model.pins.pinned(in: $0.id) } ?? [] }

    /// The page on screen was pinned: its neighbour takes its place, the next page else the one before, the backgrounded
    /// panes' page included. With no page on screen (every pane was pinned), the first page there is now.
    private func pinsChanged(from old: [String], to new: [String]) {
        let now = Pages(model.activeSpace, pinned: new, chat: model.orchestratorPane).pageIDs
        guard page == nil || !now.contains(page ?? "") else { return }
        let was = Pages(model.activeSpace, pinned: old, chat: model.orchestratorPane).pageIDs
        guard let page, let i = was.firstIndex(of: page) else {
            self.page = now.first
            return
        }
        self.page = (was[(i + 1)...] + was[..<i].reversed()).first(where: now.contains) ?? now.first
    }

    /// The pane open in place left the tray, or the tray emptied: back to the list, or to the grid.
    private func trayChanged(_ ids: [String]) {
        if let open = openTray, !ids.contains(open) { openTray = nil }
        if ids.isEmpty, page == Pages.trayID { page = model.activeSpace?.focus }
    }

    private var orchestratorMode: Bool { model.mode == WindowMode.orchestrator }

    /// Whether the body shows a space's pages: Spaces mode's space, or the orchestrator's space in Orchestrator mode.
    private var showsPages: Bool { model.mode != WindowMode.agents && model.activeSpace != nil }

    private func trayNeedsYou(_ tray: [(id: String, kind: PaneKind)]) -> Bool {
        tray.contains { if case .needsYou = model.paneStatus($0.id) { true } else { false } }
    }

    private func topBar(pages: Pages, dots: Dots, landscape: Bool) -> some View {
        let panes = pages.panes
        let spacesMode = showsPages
        return HStack(alignment: .titleRow) {
            TitleButton(model: model, open: $titleOpen, pageName: shownPageName(pages: pages), detail: orchestratorDetail(pages: pages))
                // The space switcher is a sheet, not a popover: a popover is as tall as its content and clips it, so a
                // long list of spaces could not be scrolled or reached.
                .sheet(isPresented: $titleOpen) {
                    TitleMenu(model: model, servers: servers, tab: $sheetTab, scope: $sheetScope) { request in
                        queued = request
                        titleOpen = false
                    }
                    .presentationDetents([.medium, .large])
                    .presentationDragIndicator(.visible)
                }
                // Leaves the bar's buttons on the right uncovered.
                .frame(maxWidth: 240, alignment: .leading)
            Spacer()
            // Left of the right-hand group, so the dots going in another mode move no control.
            if landscape && spacesMode && !orchestratorMode && dots.kinds.count > 1 {
                dots
            }
            // Orchestrator mode is the one orchestrator: no agents sheet; its queue tray takes the eye's slot, and with no
            // Queue pane in the orchestrator's space the slot is gone.
            if orchestratorMode {
                if let queue = model.orchestratorQueue {
                    QueueButton(queue: queue) {
                        Log.info("queue sheet: tray tapped, \(queue.count) waiting")
                        queueOpen = true
                    }
                    .frame(width: 28)
                    .sheet(isPresented: $queueOpen) {
                        QueueSheet(queue: model.orchestratorQueue ?? queue) { item in
                            queueOpen = false
                            model.ask(about: item)
                        }
                        .presentationDetents([.medium, .large])
                        .presentationDragIndicator(.visible)
                    }
                }
            } else {
                AgentsButton(needsYou: model.sessionNeedsYou > 0) {
                    Log.info("agents sheet: eye tapped, \(model.sessionNeedsYou) waiting")
                    agentsOpen = true
                }
                .frame(width: 28)
                .sheet(isPresented: $agentsOpen) {
                    AgentsList(model: model, scope: .all, place: "agents sheet") { agentsOpen = false }
                        .presentationDetents([.medium, .large])
                        .presentationDragIndicator(.visible)
                }
            }
            // Orchestrator mode takes the whole screen for the orchestrator: no palette, and no empty slot for it.
            if !orchestratorMode {
                Button { paletteOpen = true } label: { Image(systemName: "magnifyingglass") }
                    .accessibilityLabel("Search")
                    .accessibilityIdentifier("search")
            }
            // One ⋯ holds the pane's actions and Settings. The pane's logo and status dot are in the
            // agent header below, once. With no pane to act on (none in the space, or another mode
            // in place of the pages), the ⋯ holds Settings alone.
            // Orchestrator mode is for talking to the orchestrator: no pane to split, change or close, so Settings alone.
            if spacesMode, !orchestratorMode, let current = panes.first(where: { $0.id == page }) ?? panes.first {
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
        // From the top, so the lines under the title in each mode never move the title row or its buttons.
        .frame(minHeight: 50, alignment: .top)
    }

    /// The bar under the composer: the page dots on the swipe tint, full width and flush with the
    /// bottom edge. It sits outside the pager, so a horizontal scroller in a page can never take
    /// its drag: a drag across it changes page, and a swipe up opens the sheet on the tab last
    /// used, a long one on Agents. The composer above wears the same tint, since a swipe there
    /// changes page too.
    /// In Agents mode the bar keeps its place and its swipe up, with no dots and no page to change. Orchestrator mode has
    /// no bar.
    private func pageHandle(pages: Pages, dots: Dots, spacesMode: Bool) -> some View {
        let ids = spacesMode ? pages.pageIDs : []
        let index = spacesMode ? pages.index(of: page) : 0
        let prev = index > 0 ? index - 1 : nil
        let next = index + 1 < ids.count ? index + 1 : nil
        return Group {
            if ids.count > 1 { dots } else { Color.clear.frame(height: 7) }
        }
        .frame(maxWidth: .infinity, minHeight: 40)
        .background(SwipeArea.tint.ignoresSafeArea(edges: .bottom))
        .contentShape(Rectangle())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("page handle")
        .accessibilityAction(named: "Open spaces and agents") { titleOpen = true }
        .accessibilityAdjustableAction { direction in
            switch direction {
            case .increment: if let next { go(ids[next]) }
            case .decrement: if let prev { go(ids[prev]) }
            @unknown default: break
            }
        }
        .gesture(DragGesture(minimumDistance: Self.shortSwipe).onEnded { drag in
            let across = drag.translation.width, up = -drag.translation.height
            if abs(across) > abs(up) {
                guard abs(across) > Self.sideSwipe else { return }
                let target = across < 0 ? next : prev
                Log.info("page handle: swipe \(across < 0 ? "left" : "right"), page \(target.map { ids[$0] } ?? "none")")
                if let target { go(ids[target]) }
                return
            }
            guard up > 0 else { return }
            if up > Self.longSwipe { sheetTab = .agents }
            Log.info("page handle: swipe up \(Int(up)) pt, sheet on \(sheetTab)")
            titleOpen = true
        })
    }

    private func go(_ id: String) { withAnimation { page = id } }

    /// Under Orchestrator mode's title: the current topic's line on every page, which opens its notes, then the
    /// shown page's name on every page but the chat (whose model menu is on its composer).
    private func orchestratorDetail(pages: Pages) -> AnyView? {
        guard orchestratorMode else { return nil }
        let topic = model.orchestratorTopic
        let onChat = model.orchestratorPane != nil
            && (pages.pageIDs.contains(page ?? "") ? page : pages.pageIDs.first) == model.orchestratorPane
        let name = onChat ? nil : shownPageName(pages: pages)
        guard topic != nil || name != nil else { return nil }
        return AnyView(VStack(alignment: .leading, spacing: 0) {
            if let topic { TopicLine(topic: topic) }
            if let name {
                Text(name).font(.footnote).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                    .frame(minHeight: 22)
                    .accessibilityIdentifier("page name")
            }
        })
    }

    /// Orchestrator mode's way back to the chat, over every page but the chat, clear of the bottom bar.
    @ViewBuilder private func floatButton(pages: Pages, landscape: Bool) -> some View {
        if let chatPane = model.orchestratorPane, let shown = pages.pageIDs.contains(page ?? "") ? page : pages.pageIDs.first, shown != chatPane,
           case let .agent(agent, _, _)? = model.activeSpace?.allPanes.first(where: { $0.id == chatPane })?.kind, let chat = model.chats[agent] {
            OrchestratorFloat(chat: chat, waiting: model.perms.filter { $0.agent == chat.name }.count) {
                Log.info("float: back to the chat")
                go(chatPane)
            }
        }
    }

    /// A swipe up this far from the dots bar opens the sheet, and one this far opens the Agents tab;
    /// a swipe sideways this far across the pager changes page.
    private static let shortSwipe: CGFloat = 24
    private static let longSwipe: CGFloat = 160
    private static let sideSwipe: CGFloat = 24

    private func openSettings(_ tab: SettingsTab) {
        settingsTab = tab
        if settingsProject == nil || !model.spaces.contains(where: { $0.id == settingsProject }) {
            settingsProject = model.spaces.first { $0.id == model.view.space }?.id ?? model.spaces.first?.id
        }
        settingsOpen = true
    }

    /// Checks drive the app through launch environment: `WHEEL_START_PAGE=<n>` focuses the n-th
    /// pane (1-based; `tray` for the last page, with `WHEEL_TRAY_OPEN=<pane>` opening that backgrounded pane in it) as a swipe would, `WHEEL_MENU=<item>,…` runs pane-menu items on the current page (`PaneMenu.drive`), `WHEEL_SETTINGS=<global|project>` opens Settings on that tab, `WHEEL_SET=<global|project>:<key>=<value>` writes a setting as a field's Return does (a blank value clears it), `WHEEL_NEW_SPACE=<1|search:<text>|browse:<path>[,<path>]|create:<path>>` opens the New Space sheet and drives it (`NewSpaceSheet.drive`), `WHEEL_START_SPACE=<name>` picks that space and `WHEEL_START_ORIENTATION=landscape` rotates; `WHEEL_TYPE`, `WHEEL_SEND` and `WHEEL_ANSWER` are below.
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
        } else if let n = env["WHEEL_START_PAGE"].flatMap(Int.init), case let panes = pages.panes, (1...max(panes.count, 1)).contains(n) {
            Log.info("start page \(n)")
            withAnimation { page = panes[n - 1].id }
        }
        // `WHEEL_TYPE=<text>` pastes the text, then Return, into the shown terminal, or a shown plugin page's terminal embed
        // once it is attached.
        if let text = env["WHEEL_TYPE"] {
            try? await Task.sleep(for: .seconds(1))
            for _ in 0 ..< 30 {
                if model.typeIntoShown(text) { break }
                try? await Task.sleep(for: .seconds(1))
            }
        }
        // `WHEEL_SWIPE=<n>` raises the keyboard in the shown terminal, then swipes to the n-th page and logs what the
        // page's surface holds once the swipe settles.
        if let n = env["WHEEL_SWIPE"].flatMap(Int.init), case let panes = pages.panes, (1...max(panes.count, 1)).contains(n) {
            model.focusShown()
            try? await Task.sleep(for: .seconds(2))
            Log.info("swipe to page \(n)")
            withAnimation { page = panes[n - 1].id }
            try? await Task.sleep(for: .seconds(2))
            model.logShownScreen()
        }
        if env["WHEEL_NEW_SPACE"] != nil {
            Log.info("open new space sheet")
            newSpaceOpen = true
        }
        // `WHEEL_SHOW_SESSION=<server>/<session>` taps that session's chip in the session bar, by names.
        if let spec = env["WHEEL_SHOW_SESSION"], let slash = spec.firstIndex(of: "/") {
            let (serverName, sessionName) = (String(spec[..<slash]), String(spec[spec.index(after: slash)...]))
            for _ in 0 ..< 30 where !servers.servers.contains(where: { $0.serverName == serverName && $0.sessions.contains { $0.name == sessionName } }) {
                try? await Task.sleep(for: .seconds(1))
            }
            if let server = servers.servers.first(where: { $0.serverName == serverName }), let session = server.sessions.first(where: { $0.name == sessionName }) {
                Log.info("session bar: tap \(serverName)/\(sessionName)")
                servers.show(session: session.id, of: server)
            } else {
                Log.info("session bar: no session \(spec)")
            }
            try? await Task.sleep(for: .seconds(1))
        }
        // `WHEEL_SESSIONS_OPEN=1` opens the spaces sheet, whose foot is the session bar, as a swipe up does.
        if env["WHEEL_SESSIONS_OPEN"] == "1" {
            Log.info("open the spaces sheet")
            titleOpen = true
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
                Log.info("WHEEL_ATTACH: no paperclip (server protocol \(model.serverOffer.protocolVersion), agent pane \(firstAgent()?.pane ?? "none"))")
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
        // `WHEEL_DRAFT=<text>` puts the text in the first agent pane's composer without sending it.
        if let text = env["WHEEL_DRAFT"] {
            for _ in 0 ..< 30 {
                if let agent = firstAgent(), let chat = model.chats[agent.agent] {
                    chat.draft = text
                    break
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
        // `WHEEL_VOICE_SHOT=listening|hearing|speaking|thinking|muted|card|pill|asking` shows voice mode in that state over the
        // first agent pane with no lease and no microphone; `WHEEL_DICTATE_SHOT=1` shows the composer's mic recording.
        if let shot = env["WHEEL_VOICE_SHOT"] ?? env["WHEEL_DICTATE_SHOT"].map({ _ in "dictate" }) {
            for _ in 0 ..< 30 {
                if let agent = firstAgent(), let chat = model.chats[agent.agent], model.voiceOffered {
                    model.demoVoice(chat, pane: agent.pane, shot: shot)
                    break
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
        // `WHEEL_PLUGIN_PRESS=<component id>[,<component id>…]` presses each component of the first plugin pane, as a tap on
        // it would, once its script has loaded.
        if let ids = env["WHEEL_PLUGIN_PRESS"] {
            for _ in 0 ..< 30 {
                if let link = model.pluginPanes.values.sorted(by: { $0.pane < $1.pane }).first, link.plugin.tree.root != nil {
                    for id in ids.split(separator: ",").map(String.init) {
                        Log.info("plugin pane \(link.pane): press \(id) -> \(link.plugin.press(component: id))")
                    }
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

/// The eye at the top: no text and no number, and one orange dot above it only while an agent in
/// the shown session needs the person. A tap opens the agent sheet.
private struct AgentsButton: View {
    let needsYou: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "eye")
                .overlay(alignment: .topTrailing) {
                    Circle().fill(Color.orange).frame(width: 7, height: 7)
                        .offset(x: 4, y: -4)
                        .opacity(needsYou ? 1 : 0)
                }
        }
        .accessibilityLabel("Agents")
        .accessibilityValue(needsYou ? "needs you" : "")
        .accessibilityIdentifier("agents")
    }
}

/// What Agents mode shows in place of the pages until that mode draws its own surface.
private struct ModeBody: View {
    let mode: String

    var body: some View {
        let entry = WindowMode.entry(mode)
        VStack(spacing: 6) {
            Image(systemName: entry.symbol).font(.system(size: 28)).foregroundStyle(.secondary)
            Text(entry.title).font(.headline)
            Text("Every agent of the session will be here.")
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("mode body")
    }
}

/// Orchestrator mode's body while no space is open on the orchestrator's folder (ADR-0054): where to open one.
private struct NoOrchestrator: View {
    let model: PhoneModel

    var body: some View {
        let entry = WindowMode.entry(WindowMode.orchestrator)
        VStack(spacing: 6) {
            Image(systemName: entry.symbol).font(.system(size: 28)).foregroundStyle(.secondary)
            Text(entry.title).font(.headline)
            Text(model.orchestratorDir.map { "Open a space with an agent pane on \($0)" } ?? "Waiting for wheeld's settings")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 24)
                .accessibilityIdentifier("orchestrator empty")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("no orchestrator")
    }
}

/// A plugin pane's page: the script's view, and over its top edge the script's last failure while it lasts, as the
/// Mac shows it in the pane's header. The line lies over the view, so nothing in the view moves when it comes or goes;
/// the last good view stays under it.
private struct PluginPage: View {
    let model: PhoneModel
    /// The plugin pane's id.
    let pane: String
    let plugin: PluginPane

    var body: some View {
        PluginPaneView(pane: plugin)
            .environment(\.paneEmbed, { embed in AnyView(PluginEmbed(model: model, host: pane, embed: embed)) })
            .overlay(alignment: .top) {
                if let error = plugin.error {
                    Text(error)
                        .font(.footnote)
                        .foregroundStyle(Color(red: 0xe0 / 255, green: 0x71 / 255, blue: 0x5f / 255))
                        .lineLimit(2)
                        .truncationMode(.tail)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(.regularMaterial)
                        .accessibilityIdentifier("plugin.error")
                }
            }
    }
}

/// A pane a plugin pane's view shows inside itself (`Terminal{pane}` or `AgentChat{pane}`, ADR-0042), drawn as its own
/// page draws it, at the height the script gave, or an artifact (`WebView{artifact}`, ADR-0047) as a web pane draws it. A
/// pane that is gone or holds the other kind says so.
private struct PluginEmbed: View {
    let model: PhoneModel
    /// The plugin pane whose page this is.
    let host: String
    let embed: PaneEmbed

    var body: some View {
        let kind = model.kind(ofPane: embed.pane)
        Group {
            switch (embed.kind, kind) {
            case (.artifact, _):
                model.artifactEmbed(host: host, name: embed.pane)
            case let (.agentChat, .agent(agent, harness, _)?) where model.chats[agent] != nil:
                AgentPane(model: model, pane: embed.pane, agent: agent, harness: harness, chat: model.chats[agent]!)
            case let (.terminal, .terminal(term, _)?):
                // A new identity per term: a pane that starts another shell gets a new surface.
                TerminalEmbed(model: model, host: host, term: term).id(term)
            default:
                note(kind == nil ? "Pane \(embed.pane) is gone" : "Pane \(embed.pane) holds no \(embed.kind == .terminal ? "terminal" : "agent")")
            }
        }
        .onChange(of: "\(embed.pane) \(kind?.name ?? "gone")", initial: true) { _, shown in Log.info("plugin embed \(embed.kind.rawValue) \(shown)") }
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.footnote)
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

}

/// A terminal pane inside a plugin pane's page, or a mirror's page of its source terminal: a Ghostty surface of its own,
/// never the terminal page's (one surface draws in one view), streamed while the page is on screen and sizing the shell
/// while the page is the one shown.
private struct TerminalEmbed: View {
    let model: PhoneModel
    @StateObject private var terminal: PhoneTerminal

    init(model: PhoneModel, host: String, term: String) {
        self.model = model
        _terminal = StateObject(wrappedValue: model.embedTerminal(host: host, term: term))
    }

    var body: some View {
        TerminalPage(model: model, terminal: terminal, termState: terminal.view)
            .onAppear { model.embedShown(terminal) }
            .onDisappear { model.embedHidden(terminal) }
    }
}

/// A terminal pane's own page: the model's surface for the term, made when the page appears, so body sends nothing.
private struct TerminalPageSurface: View {
    let model: PhoneModel
    let term: String

    var body: some View {
        Group {
            if let terminal = model.terminals[term] {
                TerminalPage(model: model, terminal: terminal, termState: terminal.view)
            } else {
                Color.clear
            }
        }
        .onAppear { model.pageAppeared(term) }
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
/// then, only when the tray holds any, one last page listing the backgrounded panes. In Orchestrator mode the global
/// orchestrator's chat is the first page wherever its pane sits, and there is no tray page: that surface lists no panes.
private struct Pages {
    /// The tray page's tag: no pane has this id.
    static let trayID = "tray"

    let grid: [(id: String, kind: PaneKind)]
    let popped: [(id: String, kind: PaneKind)]
    let tray: [(id: String, kind: PaneKind)]

    /// The pages of `space`, less the `pinned` panes the sheet draws, led by the `chat` pane when there is one.
    init(_ space: SpaceInfo?, pinned: [String] = [], chat: String? = nil) {
        let leaving = Set(pinned + [chat].compactMap { $0 })
        let lead = space?.allPanes.first { $0.id == chat }
        grid = (lead.map { [$0] } ?? []) + (space?.root.readingOrder ?? []).filter { !leaving.contains($0.id) }
        popped = (space?.popped.flatMap(\.readingOrder) ?? []).filter { !leaving.contains($0.id) }
        tray = lead != nil ? [] : (space?.backgrounded.flatMap(\.panes) ?? []).filter { !leaving.contains($0.id) }
    }

    var panes: [(id: String, kind: PaneKind)] { grid + popped }
    var count: Int { panes.count + (tray.isEmpty ? 0 : 1) }

    var dotKinds: [Dots.Kind] {
        grid.map { _ in .grid } + popped.map { _ in .popped } + (tray.isEmpty ? [] : [.tray])
    }

    /// Every page's id in order, the tray page last.
    var pageIDs: [String] { panes.map(\.id) + (tray.isEmpty ? [] : [Self.trayID]) }

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
                    Button { Log.info("tray: back"); open = nil } label: { Image(systemName: "chevron.left").frame(width: 44, height: 36) }
                        .accessibilityLabel("Back")
                        .accessibilityIdentifier("tray back")
                    Spacer()
                }
                .padding(.horizontal, 8)
                content(pane)
            }
        } else {
            List(tray, id: \.id) { pane in
                Button { Log.info("tray: open \(pane.id)"); open = pane.id } label: {
                    HStack(spacing: 12) {
                        Image(systemName: paneSymbol(pane.kind))
                            .foregroundStyle(Color.secondary)
                            .frame(width: 24)
                        Text(model.mirror(pane.id)?.title ?? PhoneCommands.paneName(pane.kind, status: nil) ?? pane.id)
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
    enum Kind: String { case grid, popped, tray }

    let kinds: [Kind]
    let current: Int
    let trayNeedsYou: Bool

    var body: some View {
        HStack(spacing: 6) {
            ForEach(Array(kinds.enumerated()), id: \.offset) { i, kind in
                mark(kind, selected: i == current)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityIdentifier("page dots")
        .accessibilityLabel("Page \(current + 1) of \(kinds.count)")
        .accessibilityValue(value)
    }

    /// Each page's mark in order, `grid`, `popped` or `tray`, the current one in brackets and the
    /// tray's badge as `tray!`: `grid [popped] tray!`.
    var value: String {
        kinds.enumerated().map { i, kind in
            let name = kind == .tray && trayNeedsYou ? "tray!" : kind.rawValue
            return i == current ? "[\(name)]" : name
        }.joined(separator: " ")
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

/// The colour every area wears where a horizontal swipe changes the pane: the agent and terminal
/// pages, the terminal's key row, the composer and the dots bar. A web page is not tinted: its
/// horizontal scrollers cannot be told from the rest of it.
enum SwipeArea {
    static let tint = Color.primary.opacity(0.08)
}

/// The session menu's server prompts: Add Server…'s pairing sheet and Forget Server's question.
private struct ServerPrompts: ViewModifier {
    let servers: ServerList
    @Binding var adding: Bool
    @Binding var forgetting: PhoneModel?

    func body(content: Content) -> some View {
        content
            .sheet(isPresented: $adding, onDismiss: { servers.cancelAdding() }) {
                PairView(model: servers.adding, adding: true)
            }
            // A code from Add Server… connected (a new server, or one held paired again): a fresh model takes its place, and the sheet is done.
            .onChange(of: ObjectIdentifier(servers.adding)) { _, _ in adding = false }
            .alert("Forget \(forgetting?.serverName ?? "")?", isPresented: Binding(get: { forgetting != nil }, set: { if !$0 { forgetting = nil } })) {
                Button("Forget", role: .destructive) { if let forgetting { servers.remove(forgetting) } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("The phone stops connecting to it. Pairing again takes a new code from that server.")
            }
    }
}
