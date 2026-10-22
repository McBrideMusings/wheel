import SwiftUI
import WheelKit

/// The paired phone's screen: a space dropdown on top, and the space's panes as swipeable pages.
struct SpaceCarousel: View {
    let model: PhoneModel
    @State private var page: String?
    @State private var newSpacePath = ""
    @State private var askingForPath = false

    var body: some View {
        GeometryReader { geo in
            let landscape = geo.size.width > geo.size.height
            let panes = model.activeSpace?.root.readingOrder ?? []
            VStack(spacing: 0) {
                topBar(panes: panes, dotsHere: landscape)
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
        .onChange(of: page) { _, new in
            // A swipe: tell the daemon, so the Mac's focus follows.
            if let space = model.activeSpace, let new, new != space.focus { model.focus(space: space.id, pane: new) }
        }
        .task { await driveFromEnvironment() }
        .alert("New space", isPresented: $askingForPath) {
            TextField("Path on the Mac", text: $newSpacePath)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            Button("Create") { model.createSpace(path: newSpacePath); newSpacePath = "" }
            Button("Cancel", role: .cancel) { newSpacePath = "" }
        } message: {
            Text("A folder on your Mac, such as ~/Projects/app.")
        }
    }

    @ViewBuilder private func paneView(_ pane: (id: String, kind: PaneKind)) -> some View {
        if case let .agent(session, _) = pane.kind, let chat = model.chats[session] {
            AgentPane(model: model, chat: chat)
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
                        Text("\(space.name) · \(space.root.panes.count) \(space.root.panes.count == 1 ? "pane" : "panes")")
                        if space.id == model.activeSpace?.id { Image(systemName: "checkmark") }
                    }
                }
                Divider()
                Button("New space", systemImage: "plus") { askingForPath = true }
            } label: {
                HStack(spacing: 4) {
                    Text(model.activeSpace?.name ?? "Spaces").font(.headline)
                    Image(systemName: "chevron.down").font(.caption.weight(.semibold))
                }
            }
            Spacer()
            if dotsHere && panes.count > 1 {
                Dots(count: panes.count, current: panes.firstIndex { $0.id == page } ?? 0)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    /// Checks drive the app through launch environment: `WHEEL_START_PAGE=<n>` focuses the n-th
    /// pane (1-based) as a swipe would, `WHEEL_START_SPACE=<name>` picks that space and `WHEEL_START_ORIENTATION=landscape` rotates; `WHEEL_SEND` and `WHEEL_ANSWER` are below.
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

/// What a page shows until the terminal, agent and web panes land.
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
