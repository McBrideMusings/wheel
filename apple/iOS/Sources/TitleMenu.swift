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
    case newSession
    case newSpace
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
    @Binding var tab: ThumbTab
    @Binding var scope: AgentScope
    /// Closes the sheet, then asks the carousel for what comes next (nil: nothing).
    let done: (TitleMenuRequest?) -> Void
    @State private var search = ""

    var body: some View {
        VStack(spacing: 0) {
            if tab == .spaces {
                spaceList
            } else {
                AgentsList(model: model, scope: scope, place: "agents tab") { done(nil) }
            }
            Divider()
            VStack(spacing: 10) {
                // One slot for both tabs: search over the spaces, the filter over the agents.
                Group {
                    if tab == .spaces {
                        TextField("Search \(model.shownSession?.name ?? "spaces")", text: $search)
                            .textFieldStyle(.roundedBorder)
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                            .accessibilityIdentifier("space search")
                    } else {
                        Picker("Agents", selection: $scope) {
                            Text("This space").tag(AgentScope.thisSpace)
                            Text("All").tag(AgentScope.all)
                        }
                        .pickerStyle(.segmented)
                        .accessibilityIdentifier("agent scope")
                    }
                }
                .frame(height: 34)
                tabControl
                sessionBar
            }
            .padding(.horizontal, 16).padding(.top, 10).padding(.bottom, 8)
        }
        .onAppear { logShown() }
        .onChange(of: model.view.session) { _, _ in
            search = ""
            logShown()
        }
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
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        // The sheet opens at the end of the list, and a short list sits at the bottom.
        .defaultScrollAnchor(.bottom)
    }

    /// Spaces | Agents; the Agents label carries the number of agents waiting on the person.
    private var tabControl: some View {
        HStack(spacing: 8) {
            tabButton("Spaces", target: .spaces, badge: 0)
            tabButton("Agents", target: .agents, badge: model.sessionNeedsYou)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("tab control")
    }

    @ViewBuilder private func tabButton(_ title: String, target: ThumbTab, badge: Int) -> some View {
        let label = HStack(spacing: 6) {
            Text(title)
            if badge > 0 {
                Text("\(badge)").font(.caption.weight(.bold).monospacedDigit()).foregroundStyle(.white)
                    .frame(minWidth: 20, minHeight: 20).background(Color.orange, in: Capsule())
            }
        }
        .frame(maxWidth: .infinity, minHeight: 32)
        if tab == target {
            Button { tab = target } label: { label }.buttonStyle(.borderedProminent).accessibilityIdentifier("tab \(title.lowercased())")
        } else {
            Button { tab = target } label: { label }.buttonStyle(.bordered).accessibilityIdentifier("tab \(title.lowercased())")
        }
    }

    /// Main · Mobile · Games: a tap on one, or a swipe along the bar, shows that session; the lists
    /// above reload in place and the bar never moves. The + makes a new session.
    private var sessionBar: some View {
        HStack(spacing: 8) {
            ForEach(model.sessions) { session in
                let shown = session.id == model.view.session
                let label = HStack(spacing: 4) {
                    Text(session.name).lineLimit(1).minimumScaleFactor(0.7)
                    Text("\(session.spaces.count)").monospacedDigit().opacity(0.7)
                }
                .font(.subheadline)
                .frame(maxWidth: .infinity, minHeight: 32)
                Group {
                    if shown {
                        Button { model.selectSession(session.id) } label: { label }.buttonStyle(.borderedProminent)
                    } else {
                        Button { model.selectSession(session.id) } label: { label }.buttonStyle(.bordered)
                    }
                }
                .accessibilityIdentifier("session \(session.name)")
                .accessibilityValue(shown ? "shown" : "")
            }
            Button { done(.newSession) } label: { Image(systemName: "plus").frame(width: 32, height: 32) }
                .buttonStyle(.bordered)
                .accessibilityLabel("New session")
                .accessibilityIdentifier("new session")
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("session bar")
        .simultaneousGesture(DragGesture(minimumDistance: 24).onEnded { drag in
            guard abs(drag.translation.width) > abs(drag.translation.height) else { return }
            stepSession(drag.translation.width < 0 ? 1 : -1)
        })
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
                    Text(agents == 0 ? "No agents" : agents == 1 ? "1 agent" : "\(agents) agents")
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 8)
                if waiting > 0 {
                    Text("\(waiting)").font(.caption.weight(.bold).monospacedDigit()).foregroundStyle(.white)
                        .frame(minWidth: 22, minHeight: 22)
                        .background(Color.orange, in: Capsule())
                        .accessibilityLabel("\(waiting) waiting")
                }
                if checked ?? (space.id == model.active) { Image(systemName: "checkmark").foregroundStyle(.tint) }
            }
            .padding(.horizontal, 16).padding(.vertical, 8)
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
            .foregroundStyle(.primary)
            .padding(.horizontal, 16).padding(.vertical, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(id)
    }
}
