import SwiftUI
import WheelKit
import WheelUI

/// What a title-menu action asks the carousel to present once the popover is gone: an alert or a
/// confirmation shown from inside the popover would close with it.
enum TitleMenuRequest: Equatable {
    case rename(space: String, name: String)
    case close(space: String, question: String)
    case newSession
    case newSpace
}

/// The phone's title, at the bar's left: the shown space, a ▾, and how many agents wait for the
/// person in any space. The count has a reserved slot, blank at zero, so the name never moves.
/// A tap opens the title menu.
struct TitleButton: View {
    let model: PhoneModel
    @Binding var open: Bool

    var body: some View {
        let waiting = model.needsYouCount
        Button { open = true } label: {
            HStack(spacing: 4) {
                if let space = model.activeSpace { SpaceDots(space: space, shown: model.statusDots) }
                Text(model.activeSpace?.name ?? "No space").font(.headline).lineLimit(1).truncationMode(.middle)
                Image(systemName: "chevron.down").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                Text(waiting > 0 ? "\(waiting)" : "")
                    .font(.subheadline.weight(.semibold).monospacedDigit())
                    .foregroundStyle(.orange)
                    .frame(minWidth: 22, alignment: .leading)
                    .accessibilityIdentifier("needs you count")
            }
            .foregroundStyle(.primary)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .tint(.primary)
        .accessibilityIdentifier("title")
        .accessibilityLabel("\(model.activeSpace?.name ?? "No space"), session \(model.shownSession?.name ?? "none")")
        .accessibilityValue(waiting > 0 ? "\(waiting) waiting" : "")
    }
}

/// The Thumb sheet: the shown session's spaces as a flat list in the session's own order (the Mac
/// sidebar's), anchored to the bottom so the last space is nearest the thumb, then search, the
/// Spaces | Agents control and the session bar at the bottom edge. A long press on a space offers
/// Rename, New pane, Move to session… and Close space.
struct TitleMenu: View {
    let model: PhoneModel
    /// Closes the sheet, then asks the carousel for what comes next (nil: nothing).
    let done: (TitleMenuRequest?) -> Void
    @State private var search = ""

    var body: some View {
        VStack(spacing: 0) {
            spaceList
            Divider()
            VStack(spacing: 10) {
                TextField("Search \(model.shownSession?.name ?? "spaces")", text: $search)
                    .textFieldStyle(.roundedBorder)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .accessibilityIdentifier("space search")
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

    /// Spaces | Agents. Agents has no content yet, so it is disabled.
    private var tabControl: some View {
        HStack(spacing: 8) {
            Button { } label: { Text("Spaces").frame(maxWidth: .infinity, minHeight: 32) }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("tab spaces")
            Button { } label: { Text("Agents").frame(maxWidth: .infinity, minHeight: 32) }
                .buttonStyle(.bordered)
                .disabled(true)
                .accessibilityIdentifier("tab agents")
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
