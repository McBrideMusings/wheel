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

/// The phone's title: the shown space, its session's name under it. A tap opens the title menu.
struct TitleButton: View {
    let model: PhoneModel
    @Binding var open: Bool

    var body: some View {
        Button { open = true } label: {
            VStack(spacing: 1) {
                HStack(spacing: 4) {
                    if let space = model.activeSpace { SpaceDots(space: space, shown: model.statusDots) }
                    Text(model.activeSpace?.name ?? "No space").font(.headline).lineLimit(1).truncationMode(.middle)
                    Image(systemName: "chevron.down").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                }
                .foregroundStyle(.primary)
                Text(model.shownSession?.name ?? "Spaces").font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }
            // Narrow enough to leave the bar's buttons on either side uncovered.
            .frame(maxWidth: 180)
        }
        .buttonStyle(.plain)
        .tint(.primary)
        .accessibilityIdentifier("title")
        .accessibilityLabel("\(model.activeSpace?.name ?? "No space"), session \(model.shownSession?.name ?? "none")")
    }
}

/// The title menu: the shown session's spaces, then Switch session… and New space; or, after
/// Switch session…, every session and New session. A long press on a space offers Rename, New pane,
/// Move to session… and Close space.
struct TitleMenu: View {
    let model: PhoneModel
    /// Closes the popover, then asks the carousel for what comes next (nil: nothing).
    let done: (TitleMenuRequest?) -> Void
    @State private var listingSessions = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if listingSessions { sessionList } else { spaceList }
        }
        .frame(width: 320)
        .onAppear { logShown() }
        .onChange(of: listingSessions) { _, _ in logShown() }
    }

    private func logShown() {
        if listingSessions {
            Log.info("title menu: sessions [\(model.sessions.map(\.id).joined(separator: ", "))]")
        } else {
            Log.info("title menu: session \(model.shownSession?.id ?? "none") spaces [\(model.sessionSpaces.map(\.id).joined(separator: ", "))]")
        }
    }

    private var spaceList: some View {
        let listed = model.sessionSpaces
        return Group {
            header("\(model.shownSession?.name ?? "No session") · Spaces")
            ForEach(listed) { space in
                row(dot: AnyView(SpaceDots(space: space, shown: model.statusDots)), name: space.name,
                    caption: SessionCaption.space(space, shown: model.statusDots), checked: space.id == model.active) {
                    model.select(space: space.id)
                    done(nil)
                }
                .accessibilityIdentifier("space \(space.name)")
                .contextMenu { spaceActions(space) } preview: {
                    row(dot: AnyView(SpaceDots(space: space, shown: model.statusDots)), name: space.name,
                        caption: SessionCaption.space(space, shown: model.statusDots), checked: false) {}
                        .frame(width: 320)
                }
                Divider().padding(.leading, 16)
            }
            if listed.isEmpty {
                Text("No spaces in this session").font(.subheadline).foregroundStyle(.secondary).padding(16)
                Divider()
            }
            action("Switch session…", symbol: nil, id: "switch session") { listingSessions = true }
            Divider()
            action("New space", symbol: "plus", id: "new space") { done(.newSpace) }
        }
    }

    private var sessionList: some View {
        Group {
            header("Sessions")
            ForEach(model.sessions) { session in
                row(dot: AnyView(StatusDot(status: topStatus(session), shown: model.statusDots)), name: session.name,
                    caption: SessionCaption.session(session, spaces: model.spaces, shown: model.statusDots),
                    checked: session.id == model.view.session) {
                    model.selectSession(session.id)
                    done(nil)
                }
                .accessibilityIdentifier("session \(session.name)")
                Divider().padding(.leading, 16)
            }
            action("New session", symbol: "plus", id: "new session") { done(.newSession) }
        }
    }

    /// The most urgent status among a session's spaces, for its dot.
    private func topStatus(_ session: SessionInfo) -> PaneStatus? {
        guard let rank = SessionCaption.rank(of: session, spaces: model.spaces) else { return nil }
        return model.spaces.lazy.filter { session.spaces.contains($0.id) }.compactMap(\.status).first { $0.rank == rank }
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

    private func header(_ text: String) -> some View {
        Text(text.uppercased()).font(.caption).foregroundStyle(.secondary)
            .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, 6)
    }

    private func row(dot: AnyView, name: String, caption: String, checked: Bool, tap: @escaping () -> Void) -> some View {
        Button(action: tap) {
            HStack(spacing: 10) {
                dot.frame(minWidth: 10)
                VStack(alignment: .leading, spacing: 2) {
                    Text(name).font(.body).foregroundStyle(.primary).lineLimit(1)
                    Text(caption).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 8)
                if checked { Image(systemName: "checkmark").foregroundStyle(.tint) }
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
