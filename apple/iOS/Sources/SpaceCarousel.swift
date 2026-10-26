import SwiftUI
import WheelKit
import WheelUI

/// The paired phone's screen: a space dropdown on top, and the space's panes as swipeable pages.
struct SpaceCarousel: View {
    let model: PhoneModel
    @State private var page: String?
    @State private var newSpaceOpen = false
    @State private var settingsOpen = false
    @State private var settingsTab: SettingsTab = .global
    @State private var settingsProject: String?

    var body: some View {
        GeometryReader { geo in
            let landscape = geo.size.width > geo.size.height
            let panes = model.activeSpace?.root.readingOrder ?? []
            VStack(spacing: 0) {
                topBar(panes: panes, dotsHere: landscape)
                UsageStrip(model: model)
                if panes.isEmpty {
                    ContentUnavailableView(connectionText, systemImage: "rectangle.on.rectangle")
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
        .onChange(of: model.activeSpace?.root.readingOrder.map(\.id)) { _, _ in model.show(pane: page) }
        .onChange(of: page) { _, new in
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
    }

    @ViewBuilder private func paneView(_ pane: (id: String, kind: PaneKind)) -> some View {
        if case let .terminal(term, _) = pane.kind {
            let terminal = model.terminal(term)
            TerminalPage(model: model, terminal: terminal, termState: terminal.view)
        } else if case let .agent(session, _) = pane.kind, let chat = model.chats[session] {
            AgentPane(model: model, chat: chat)
        } else if let web = model.webPanes[pane.id] {
            WebBody(web: web)
        } else {
            PanePlaceholder(id: pane.id, kind: pane.kind)
        }
    }

    private var connectionText: String {
        switch model.connection {
        case .connecting: "Connecting…"
        case .connected: "No panes"
        case let .unreachable(why): "Cannot reach the Mac: \(why)"
        }
    }

    private func topBar(panes: [(id: String, kind: PaneKind)], dotsHere: Bool) -> some View {
        HStack {
            Menu {
                ForEach(model.spaces) { space in
                    Button {
                        model.select(space: space.id)
                    } label: {
                        Text("\(space.name) · \(space.root.panes.count) \(space.root.panes.count == 1 ? "pane" : "panes")\(menuCaption(space))")
                        if space.id == model.activeSpace?.id { Image(systemName: "checkmark") }
                    }
                }
                Divider()
                Button("New space", systemImage: "plus") { newSpaceOpen = true }
            } label: {
                HStack(spacing: 6) {
                    if let space = model.activeSpace {
                        SpaceDots(space: space, shown: model.statusDots)
                    }
                    Text(model.activeSpace?.name ?? "Spaces").font(.headline)
                    Image(systemName: "chevron.down").font(.caption.weight(.semibold))
                }
            }
            Spacer()
            if dotsHere && panes.count > 1 {
                Dots(count: panes.count, current: panes.firstIndex { $0.id == page } ?? 0)
            }
            Button { openSettings(.global) } label: { Image(systemName: "gearshape") }
                .accessibilityLabel("Settings")
            if let current = panes.first(where: { $0.id == page }) ?? panes.first {
                StatusDot(status: model.paneStatus(current.id), shown: model.statusDots)
                PaneMenu(model: model, pane: current.id, kind: current.kind)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    /// What a menu row says beside the name for a space that needs you or stopped: a menu item
    /// cannot carry a coloured dot, so the words stand in.
    private func menuCaption(_ space: SpaceInfo) -> String {
        guard let status = space.status, model.statusDots.contains(status.rank), let caption = status.caption else { return "" }
        return " · \(caption)"
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
        // `WHEEL_SEND=<text>` types the text into the first agent pane's composer and sends it
        // through the same call the send button makes; `WHEEL_ANSWER=<option id>` answers that
        // pane's oldest waiting request as its button does, once one waits.
        if let text = env["WHEEL_SEND"] {
            for _ in 0 ..< 30 {
                let panes = model.activeSpace?.root.readingOrder ?? []
                let session = panes.lazy.compactMap { pane -> String? in
                    if case let .agent(session, _) = pane.kind { session } else { nil }
                }.first
                if let session, let chat = model.chats[session] {
                    chat.draft = text
                    model.send(chat)
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
