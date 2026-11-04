import SwiftUI
import WheelKit
import WheelUI

/// The paired phone's screen: the title menu on top, and the shown space's panes as swipeable pages.
struct SpaceCarousel: View {
    let model: PhoneModel
    @State private var page: String?
    @State private var newSpaceOpen = false
    @State private var titleOpen = false
    @State private var paletteOpen = false
    /// What the title menu asked for, presented once its popover has closed.
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
            let panes = model.activeSpace?.root.readingOrder ?? []
            VStack(spacing: 0) {
                topBar(panes: panes, dotsHere: landscape)
                if panes.isEmpty {
                    ContentUnavailableView(connectionText, systemImage: "rectangle.on.rectangle")
                        .accessibilityIdentifier("no panes")
                } else {
                    TabView(selection: $page) {
                        ForEach(panes, id: \.id) { pane in
                            paneView(pane).tag(Optional(pane.id))
                        }
                    }
                    .tabViewStyle(.page(indexDisplayMode: .never))
                }
                if !landscape && panes.count > 1 {
                    Dots(count: panes.count, current: panes.firstIndex { $0.id == page } ?? 0)
                        .padding(.vertical, 10)
                }
            }
        }
        .onChange(of: model.activeSpace?.focus, initial: true) { _, focus in
            // The daemon moved focus (the Mac, or the CLI): the page follows.
            if let focus, page != focus { page = focus }
        }
        .onChange(of: model.activeSpace?.id) { _, _ in page = model.activeSpace?.focus }
        .onChange(of: page, initial: true) { _, new in model.show(pane: new) }
        .onChange(of: page) { _, new in
            // Another page closes the keyboard; nothing on the new page takes it until a tap.
            let closed = UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
            Log.info("page \(new ?? "none"): keyboard \(closed ? "closed" : "was not up")")
            // A swipe: tell the daemon, so the Mac's focus follows.
            if let space = model.activeSpace, let new, new != space.focus { model.focus(space: space.id, pane: new) }
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

    private func topBar(panes: [(id: String, kind: PaneKind)], dotsHere: Bool) -> some View {
        HStack {
            Spacer()
            if dotsHere && panes.count > 1 {
                Dots(count: panes.count, current: panes.firstIndex { $0.id == page } ?? 0)
            }
            Button { paletteOpen = true } label: { Image(systemName: "magnifyingglass") }
                .accessibilityLabel("Search")
                .accessibilityIdentifier("search")
            Button { openSettings(.global) } label: { Image(systemName: "gearshape") }
                .accessibilityLabel("Settings")
            if let current = panes.first(where: { $0.id == page }) ?? panes.first {
                StatusDot(status: model.paneStatus(current.id), shown: model.statusDots)
                PaneMenu(model: model, pane: current.id, kind: current.kind)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        // Tall enough for the title's two lines, which sit over the bar rather than in its row.
        .frame(minHeight: 50)
        .overlay {
            TitleButton(model: model, open: $titleOpen)
                .popover(isPresented: $titleOpen) {
                    TitleMenu(model: model) { request in
                        queued = request
                        titleOpen = false
                    }
                    .presentationCompactAdaptation(.popover)
                }
        }
    }

    private func openSettings(_ tab: SettingsTab) {
        settingsTab = tab
        if settingsProject == nil || !model.spaces.contains(where: { $0.id == settingsProject }) {
            settingsProject = model.activeSpace?.id
        }
        settingsOpen = true
    }

    /// Checks drive the app through launch environment: `WHEEL_START_PAGE=<n>` focuses the n-th
    /// pane (1-based) as a swipe would, `WHEEL_MENU=<item>,…` runs pane-menu items on the current page (`PaneMenu.drive`), `WHEEL_SETTINGS=<global|project|devices>` opens Settings on that tab, `WHEEL_SET=<global|project>:<key>=<value>` writes a setting as a field's Return does (a blank value clears it), `WHEEL_NEW_SPACE=<1|search:<text>|browse:<path>[,<path>]|create:<path>>` opens the New Space sheet and drives it (`NewSpaceSheet.drive`), `WHEEL_START_SPACE=<name>` picks that space and `WHEEL_START_ORIENTATION=landscape` rotates; `WHEEL_TYPE`, `WHEEL_SEND` and `WHEEL_ANSWER` are below.
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
        if let n = env["WHEEL_START_PAGE"].flatMap(Int.init), let panes = model.activeSpace?.root.readingOrder, (1...max(panes.count, 1)).contains(n) {
            Log.info("start page \(n)")
            withAnimation { page = panes[n - 1].id }
        }
        // `WHEEL_TYPE=<text>` pastes the text, then Return, into the shown terminal.
        if let text = env["WHEEL_TYPE"] {
            try? await Task.sleep(for: .seconds(1))
            model.typeIntoShown(text)
        }
        // `WHEEL_SWIPE=<n>` raises the keyboard in the shown terminal, then swipes to the n-th page.
        if let n = env["WHEEL_SWIPE"].flatMap(Int.init), let panes = model.activeSpace?.root.readingOrder, (1...max(panes.count, 1)).contains(n) {
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
            Image(systemName: symbol).font(.largeTitle)
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

private struct Dots: View {
    let count: Int
    let current: Int

    var body: some View {
        HStack(spacing: 6) {
            ForEach(0 ..< count, id: \.self) { i in
                Circle().fill(i == current ? Color.primary : Color.secondary.opacity(0.35)).frame(width: 7, height: 7)
            }
        }
    }
}
