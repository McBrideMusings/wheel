import SwiftUI
import WheelKit
import WheelUI

extension VerticalAlignment {
    private enum TitleRow: AlignmentID {
        static func defaultValue(in d: ViewDimensions) -> CGFloat { d[VerticalAlignment.center] }
    }
    /// The title row's middle: the bar's buttons line up with it, so the lines under the title never move them.
    static let titleRow = VerticalAlignment(TitleRow.self)
}

/// What a title-menu action asks the carousel to present once the popover is gone: an alert or a
/// confirmation shown from inside the popover would close with it.
enum TitleMenuRequest: Equatable {
    case rename(space: String, name: String)
    case close(space: String, question: String)
    /// New session on that server (`ObjectIdentifier` of its model); nil: the shown one.
    case newSession(server: ObjectIdentifier?)
    case newSpace
    case addServer
    case forget(server: ObjectIdentifier)
}

/// The phone's title, at the bar's left: the mode menu ("Spaces ⌄"), which only switches modes, and the context line
/// under it. In Spaces mode the line is the shown space and page, with the space's dots and how many agents wait for the
/// person in any space, and a tap opens the space switcher; in Agents mode it is the shown session and opens the same
/// switcher; in Orchestrator mode the carousel's `detail` stands there (the topic, then the model or the page's name).
/// The count has a reserved slot, blank at zero, so the name never moves.
struct TitleButton: View {
    let model: PhoneModel
    /// The space switcher's sheet.
    @Binding var open: Bool
    /// The shown page's name, for Spaces mode's context line.
    var pageName: String?
    /// Orchestrator mode's lines under the title.
    var detail: AnyView?

    /// The shown space's name.
    private var name: String { model.activeSpace?.name ?? "No space" }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ModeMenu(mode: model.mode) { model.setMode($0) }
                .frame(height: Self.rowHeight)
                .alignmentGuide(.titleRow) { $0[VerticalAlignment.center] }
            if model.mode == WindowMode.orchestrator {
                detail
            } else {
                contextLine
            }
        }
    }

    private var contextLine: some View {
        let waiting = model.needsYouCount
        let spaces = model.mode == WindowMode.spaces
        let session = model.shownSession?.name ?? "No session"
        return Button { open = true } label: {
            HStack(spacing: 4) {
                Image(systemName: spaces ? "folder" : "rectangle.stack").font(.caption)
                // The space and the page are two texts, so the space's name stays put as the page changes.
                Text(spaces ? name : session).lineLimit(1)
                if spaces, let pageName { Text("· \(pageName)").lineLimit(1).truncationMode(.middle) }
                Image(systemName: "chevron.right").font(.caption2.weight(.semibold))
                if spaces, let space = model.activeSpace { SpaceDots(space: space, shown: model.statusDots) }
                Text(waiting > 0 ? "\(waiting)" : "")
                    .font(.footnote.weight(.semibold).monospacedDigit())
                    .foregroundStyle(.orange)
                    .frame(minWidth: 22, alignment: .leading)
                    .accessibilityIdentifier("needs you count")
            }
            .font(.footnote)
            .frame(minHeight: 22)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.tint)
        .accessibilityIdentifier("space switcher")
        .accessibilityLabel(spaces ? "\(name), session \(session)" : "Session \(session)")
        .accessibilityValue(waiting > 0 ? "\(waiting) waiting" : "")
    }

    /// The title row's height in every mode, so switching modes moves no control.
    private static let rowHeight: CGFloat = 22
}

/// The bar's title: the shown mode's name and a ⌄; a tap opens the three modes, a check on the shown one (M11, S3).
private struct ModeMenu: View {
    let mode: String
    let pick: (String) -> Void

    var body: some View {
        let shown = WindowMode.entry(mode)
        Menu {
            Picker("Mode", selection: Binding(get: { mode }, set: { new in
                Log.info("mode menu: picked \(new)")
                pick(new)
            })) {
                ForEach(WindowMode.menu, id: \.id) { entry in
                    Text(entry.title).tag(entry.id)
                }
            }
            .pickerStyle(.inline)
        } label: {
            HStack(spacing: 4) {
                Text(shown.title).font(.headline).lineLimit(1)
                Image(systemName: "chevron.down").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            }
            .foregroundStyle(.primary)
            .contentShape(Rectangle())
        }
        // A menu's label wears the accent; the title reads as a title, as Spaces mode's name did.
        .tint(.primary)
        .accessibilityLabel("Mode")
        .accessibilityValue(shown.title)
        .accessibilityIdentifier("mode")
    }
}

/// The Thumb sheet: the shown session's spaces as a flat list in the session's own order (the Mac
/// sidebar's), anchored to the bottom so the last space is nearest the thumb, then search, the
/// Spaces | Agents control and the session bar at the bottom edge. A long press on a space offers
/// Rename, New pane, Move to session… and Close space.
struct TitleMenu: View {
    let model: PhoneModel
    let servers: ServerList
    @Binding var tab: ThumbTab
    @Binding var scope: AgentScope
    /// The height that fits the spaces list and the controls under it: the sheet's first detent.
    @Binding var fit: CGFloat
    /// Closes the sheet, then asks the carousel for what comes next (nil: nothing).
    let done: (TitleMenuRequest?) -> Void
    @State private var search = ""
    @State private var listHeight: CGFloat = 0
    @State private var controlsHeight: CGFloat = 0

    var body: some View {
        VStack(spacing: 0) {
            if tab == .spaces {
                spaceList
            } else {
                AgentsList(model: model, scope: scope, place: "agents tab") { done(nil) }
            }
            VStack(spacing: 10) {
                // One slot for both tabs: search over the spaces, the filter over the agents.
                Group {
                    if tab == .spaces {
                        HStack(spacing: 6) {
                            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                            TextField("Search \(model.shownSession?.name ?? "spaces")", text: $search)
                                .autocorrectionDisabled()
                                .textInputAutocapitalization(.never)
                                .accessibilityIdentifier("space search")
                        }
                        .padding(.horizontal, 10)
                        .frame(maxHeight: .infinity)
                        .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 10))
                    } else {
                        Picker("Agents", selection: $scope) {
                            Text("This space").tag(AgentScope.thisSpace)
                            Text("All").tag(AgentScope.all)
                        }
                        .pickerStyle(.segmented)
                        .accessibilityIdentifier("agent scope")
                    }
                }
                .frame(height: 36)
                tabControl
                sessionBar
            }
            .padding(.horizontal, 16).padding(.top, 10).padding(.bottom, 8)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { controlsHeight = $0; fitSheet() }
        }
        .presentationBackground(Color(.secondarySystemGroupedBackground))
        .onAppear { logShown() }
        .onChange(of: model.view.session) { _, _ in
            search = ""
            logShown()
        }
    }

    /// The sheet opens as tall as its spaces and controls, under the drag indicator.
    private func fitSheet() {
        let next = (listHeight + controlsHeight + 20).rounded()
        if listHeight > 0, controlsHeight > 0, abs(next - fit) > 0.5 { fit = next }
    }

    private func logShown() {
        Log.info("title menu: session \(model.shownSession?.id ?? "none") spaces [\(model.sessionSpaces.map(\.id).joined(separator: ", "))]")
    }

    /// The shown session's spaces matching the search, in session order.
    private var listed: [SpaceInfo] {
        let query = search.trimmingCharacters(in: .whitespaces)
        return query.isEmpty ? model.sessionSpaces : model.sessionSpaces.filter { $0.name.localizedCaseInsensitiveContains(query) }
    }

    private var spaceList: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if model.sessionSpaces.isEmpty {
                    Text("No spaces in this session").font(.subheadline).foregroundStyle(.secondary).padding(16)
                } else if listed.isEmpty {
                    Text("No space matches “\(search)”").font(.subheadline).foregroundStyle(.secondary).padding(16)
                }
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(listed) { space in
                        row(space) {
                            model.select(space: space.id)
                            done(nil)
                        }
                        .accessibilityIdentifier("space \(space.name)")
                        .contextMenu { spaceActions(space) } preview: {
                            row(space, checked: false) {}.frame(width: 320)
                        }
                        Divider().padding(.leading, 16)
                    }
                    action("New space", symbol: "plus", id: "new space") { done(.newSpace) }
                }
                .background(Color(.tertiarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 10))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16).padding(.top, 4)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { listHeight = $0; fitSheet() }
        }
        // The sheet opens at the end of the list, and a short list sits at the bottom.
        .defaultScrollAnchor(.bottom)
    }

    /// Spaces | Agents; the Agents label carries the number of agents waiting on the person.
    private var tabControl: some View {
        HStack(spacing: 2) {
            tabButton("Spaces", target: .spaces, badge: 0)
            tabButton("Agents", target: .agents, badge: model.sessionNeedsYou)
        }
        .padding(2)
        .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 9))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("tab control")
    }

    /// One segment: the chosen one sits on a raised fill, as a segmented control draws it; a
    /// Picker cannot carry the orange count.
    private func tabButton(_ title: String, target: ThumbTab, badge: Int) -> some View {
        let chosen = tab == target
        return Button { tab = target } label: {
            HStack(spacing: 5) {
                Text(title).font(.footnote.weight(.semibold))
                if badge > 0 {
                    Text("\(badge)").font(.caption2.weight(.bold).monospacedDigit()).foregroundStyle(.black)
                        .frame(minWidth: 16, minHeight: 16).background(Color.orange, in: Capsule())
                }
            }
            .foregroundStyle(.primary)
            .frame(maxWidth: .infinity, minHeight: 28)
            .background(chosen ? Color(.systemGray3) : .clear, in: RoundedRectangle(cornerRadius: 7))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("tab \(title.lowercased())")
        .accessibilityAddTraits(chosen ? .isSelected : [])
    }

    /// Main · Mobile · Games: a tap on one, or a swipe along the bar, shows that session; the lists
    /// above reload in place and the bar never moves. With several servers every server's sessions
    /// are here, each server's run led by its mark, chips of one width scrolling sideways, and a down
    /// server's chips dimmed (ADR-0057). The + makes a new session or adds a server.
    @ViewBuilder private var sessionBar: some View {
        HStack(spacing: 0) {
            ViewThatFits(in: .horizontal) {
                // Fits: centred, and a swipe along it steps the session.
                sessionLine.frame(maxWidth: .infinity)
                    .contentShape(Rectangle())
                    .simultaneousGesture(DragGesture(minimumDistance: 24).onEnded { drag in
                        guard !servers.several, abs(drag.translation.width) > abs(drag.translation.height) else { return }
                        stepSession(drag.translation.width < 0 ? 1 : -1)
                    })
                // Too long: it scrolls sideways, fades at the right edge and keeps the shown session in view.
                ScrollViewReader { proxy in
                    ScrollView(.horizontal, showsIndicators: false) { sessionLine }
                        .mask(LinearGradient(stops: [.init(color: .black, location: 0.85), .init(color: .clear, location: 1)], startPoint: .leading, endPoint: .trailing))
                        .onAppear { proxy.scrollTo(shownChip, anchor: .center) }
                        .onChange(of: shownChip) { _, id in withAnimation { proxy.scrollTo(id, anchor: .center) } }
                }
            }
            plusMenu
        }
        .frame(height: 32)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("session bar")
    }

    /// Every server's sessions as one line of text, each server's run led by its mark when there
    /// are several (ADR-0057); a down server's sessions dimmed.
    private var sessionLine: some View {
        HStack(spacing: 0) {
            ForEach(Array((servers.several ? servers.servers : [model]).enumerated()), id: \.offset) { index, server in
                if servers.several {
                    ServerMark(model: server).font(.footnote.weight(.semibold))
                        .padding(.leading, index == 0 ? 0 : 14).padding(.trailing, 6)
                }
                ForEach(Array(server.sessions.enumerated()), id: \.element.id) { at, session in
                    if at > 0 { Text("·").foregroundStyle(.tertiary).padding(.horizontal, 6) }
                    chip(session, of: server).id(Self.chipID(session.id, of: server))
                }
            }
        }
        .fixedSize()
    }

    private static func chipID(_ session: String, of server: PhoneModel) -> String { "\(ObjectIdentifier(server).hashValue)/\(session)" }
    private var shownChip: String { Self.chipID(model.view.session ?? "", of: model) }

    /// A session: its name and space count, bold when shown.
    private func chip(_ session: SessionInfo, of server: PhoneModel) -> some View {
        let shown = server === model && session.id == model.view.session
        return Button { servers.show(session: session.id, of: server) } label: {
            // Laid out semibold whether shown or not, so a chip keeps its width when it becomes the shown one.
            let label = { (weight: Font.Weight) in
                HStack(spacing: 4) {
                    Text(session.name).lineLimit(1)
                    Text("\(session.spaces.count)").monospacedDigit()
                }
                .font(.subheadline.weight(weight))
            }
            label(.semibold).hidden()
                .overlay { label(shown ? .semibold : .regular) }
            .foregroundStyle(shown ? .primary : .secondary)
            .frame(minHeight: 32)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(server.isConnected ? 1 : 0.4)
        .accessibilityIdentifier("session \(session.name)")
        .accessibilityValue(shown ? "shown" : server.isConnected ? "" : "not connected")
    }

    /// New Session (on which server, with several), Add Server… and Forget Server.
    private var plusMenu: some View {
        Menu {
            if servers.several {
                Section("New Session") {
                    ForEach(servers.servers) { server in
                        Button(server.serverName, systemImage: ServerMark.symbol(platform: server.host?.platform)) {
                            done(.newSession(server: server.id))
                        }
                    }
                }
            } else {
                Button("New Session", systemImage: "plus") { done(.newSession(server: nil)) }
            }
            Divider()
            Button("Add Server…", systemImage: "qrcode.viewfinder") { done(.addServer) }
            Menu("Forget Server", systemImage: "xmark") {
                ForEach(servers.servers) { server in
                    Button(server.serverName, role: .destructive) { done(.forget(server: server.id)) }
                }
            }
        } label: {
            Image(systemName: "plus").font(.title3).frame(width: 32, height: 32, alignment: .trailing)
        }
        .accessibilityLabel("New session")
        .accessibilityIdentifier("new session")
    }

    private func stepSession(_ delta: Int) {
        guard let current = model.sessions.firstIndex(where: { $0.id == model.view.session }) else { return }
        let next = current + delta
        guard model.sessions.indices.contains(next) else { return }
        model.selectSession(model.sessions[next].id)
    }

    @ViewBuilder private func spaceActions(_ space: SpaceInfo) -> some View {
        Button("Rename", systemImage: "pencil") { done(.rename(space: space.id, name: space.name)) }
        Button("New pane", systemImage: "plus.square.on.square") {
            model.newPane(in: space.id)
            done(nil)
        }
        let others = model.sessions.filter { $0.id != model.sessions.session(of: space.id)?.id }
        Menu("Move to session…", systemImage: "arrow.left.arrow.right") {
            if others.isEmpty { Text("No other session") }
            ForEach(others) { session in
                Button(session.name) {
                    model.moveSpace(space.id, toSession: session.id)
                    done(nil)
                }
            }
        }
        Divider()
        Button("Close space", systemImage: "xmark", role: .destructive) {
            if let question = SessionCaption.closeQuestion(space) {
                Log.info("close \(space.id): asked, \(question)")
                done(.close(space: space.id, question: question))
            } else {
                Log.info("close \(space.id): no agent working, closed at once")
                model.closeSpace(space.id)
                done(nil)
            }
        }
    }

    /// A space: its dots, name, agent count, the number of agents waiting on the person, and a
    /// check on the one shown.
    private func row(_ space: SpaceInfo, checked: Bool? = nil, tap: @escaping () -> Void) -> some View {
        let agents = space.paneStatuses.count
        let waiting = space.paneStatuses.values.filter { if case .needsYou = $0 { true } else { false } }.count
        return Button(action: tap) {
            HStack(spacing: 10) {
                SpaceDots(space: space, shown: model.statusDots).frame(minWidth: 10)
                VStack(alignment: .leading, spacing: 2) {
                    Text(space.name).font(.body).foregroundStyle(.primary).lineLimit(1)
                    Text(waiting > 0 ? "\(waiting) need\(waiting == 1 ? "s" : "") you" : agents == 0 ? "No agents" : agents == 1 ? "1 agent" : "\(agents) agents")
                        .font(.footnote).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 8)
                if waiting > 0 {
                    Circle().fill(Color.orange).frame(width: 8, height: 8)
                        .accessibilityLabel("\(waiting) waiting")
                }
                if checked ?? (space.id == model.active) {
                    Image(systemName: "checkmark").font(.body.weight(.semibold)).foregroundStyle(.tint)
                } else {
                    Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func action(_ title: String, symbol: String?, id: String, tap: @escaping () -> Void) -> some View {
        Button(action: tap) {
            HStack(spacing: 6) {
                if let symbol { Image(systemName: symbol) }
                Text(title)
                Spacer()
            }
            .foregroundStyle(.tint)
            .padding(.horizontal, 16).padding(.vertical, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(id)
    }
}
