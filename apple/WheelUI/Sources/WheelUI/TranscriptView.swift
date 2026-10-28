import SwiftUI
import WheelKit

/// One session's conversation: the person's messages, and each turn's parts — reply text,
/// reasoning, tool calls — in the order they arrived.
public struct TranscriptView: View {
    @ObservedObject var chat: SessionChat
    /// What an empty conversation says.
    let empty: String
    /// The permission requests waiting on a person, which the rows mark.
    let permissions: PermissionQueue
    /// How the chat draws, from the `chat.*` settings resolved for the chat's space.
    let look: ChatAppearance
    @Environment(\.transcriptPlatform) private var platform

    public init(chat: SessionChat, empty: String, permissions: PermissionQueue = PermissionQueue(), look: ChatAppearance = .standard) {
        self.chat = chat
        self.empty = empty
        self.permissions = permissions
        self.look = look
    }

    /// Go to the last item while the view follows the end.
    private func follow(_ proxy: ScrollViewProxy, _ items: [ChatItem]) {
        guard chat.pinned, let last = items.last else { return }
        proxy.scrollTo(last.id, anchor: .bottom)
    }

    public var body: some View {
        content.environment(\.chatAppearance, look)
    }

    @ViewBuilder private var content: some View {
        if chat.rows.isEmpty {
            // Outside the scroll view, so it sits in the middle of the space the transcript has
            // rather than pinned to the bottom of an empty scroll.
            Text(empty)
                .font(look.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .padding(24)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            transcript
        }
    }

    private var transcript: some View {
        let items = chat.items
        let replyEnds = lastRepliesOfFinishedTurns(items, busy: chat.isBusy)
        let context = RowContext(
            waiting: permissions.waiting,
            places: permissions.places,
            waitingCount: permissions.count,
            busy: chat.isBusy,
            expanded: chat.expanded
        )
        // Anchored to the bottom: new output stays in view while you are at the end, and leaves
        // you alone while you scroll back through history.
        return ScrollViewReader { proxy in
            // A list, which AppKit draws as a table that builds only the rows near the viewport, so
            // a long conversation costs what its visible rows cost. Not a lazy stack: its prefetch
            // of the rows past the viewport could ask for another update at the end of every
            // update, and the main thread never got back to its run loop.
            List {
                ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                    ChatItemView(chat: chat, item: item, copyable: replyEnds.contains(item.id), context: context)
                        .frame(maxWidth: look.maxWidth ?? .infinity)
                        .frame(maxWidth: .infinity)
                        .listRowInsets(EdgeInsets(
                            top: gap(before: index, in: items) + (index == 0 ? 12 : 0),
                            leading: 14,
                            bottom: index == items.count - 1 ? 12 : 0,
                            trailing: 14))
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color.clear)
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .defaultScrollAnchor(.bottom)
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { chat.frameInWindow = $0 }
            .onDisappear {
                chat.frameInWindow = .zero
                platform.disappear(chat)
            }
            // A bottom anchor alone loses the end: a reply's height settles over several layout
            // passes as it streams in and as a line opens, and a scroll to the end lands short
            // while it does. So the view keeps whether it follows the end, and only a person's own scroll
            // changes that — the one scroll phase a program's scroll never passes through. While
            // it follows, any growth or resize goes to the last item by instruction, so the end of
            // a reply and a request opening on its tool line stay in view.
            .onScrollPhaseChange { _, phase in
                chat.userScrolling = phase == .interacting || phase == .decelerating
            }
            // A scroll view built anew starts at the end (the bottom anchor), with no scroll of the
            // person's under way.
            .onAppear {
                chat.pinned = true
                chat.userScrolling = false
                platform.appear(chat)
            }
            .onScrollGeometryChange(for: ScrollMetrics.self) { geometry in
                ScrollMetrics(
                    content: geometry.contentSize.height,
                    offset: geometry.contentOffset.y + geometry.contentInsets.top,
                    visible: geometry.visibleRect.height
                )
            } action: { old, new in
                chat.fromEnd = new.distanceToBottom
                if chat.userScrolling {
                    let pinned = new.distanceToBottom < 24
                    if pinned != chat.pinned {
                        UILog.info("transcript \(chat.name): \(pinned ? "follows the end" : "left the end"), \(Int(new.distanceToBottom)) from it")
                        chat.pinned = pinned
                    }
                } else if new.content != old.content || new.visible != old.visible {
                    follow(proxy, items)
                } else if new.offset < old.offset - 1, new.distanceToBottom > 24, chat.pinned,
                          Date().timeIntervalSince(chat.wheelAt) < 0.5 {
                    // Moving up and away from the end right after a wheel event over this view or a scrolling key is a
                    // person's scroll, though a notched mouse wheel reports no phase. Without one,
                    // the same move is layout settling and leaves the view following the end.
                    UILog.info("transcript \(chat.name): left the end, scrolled up to \(Int(new.distanceToBottom)) from it")
                    chat.pinned = false
                }
            }
            .onChange(of: items.count) { follow(proxy, items) }
            .onChange(of: chat.endRequest) {
                if !chat.pinned {
                    UILog.info("transcript \(chat.name): follows the end (the End key)")
                }
                chat.pinned = true
                follow(proxy, items)
            }
        }
    }

    /// The ids of the rows that carry a turn's copy button: the last reply text of each finished
    /// turn, whatever follows it in the turn (a tool call, a tool group). A turn runs from one
    /// message to the next; it is finished when a message that has started follows it, or nothing
    /// follows and the session is idle. A queued message below means the turn above it is still
    /// running.
    private func lastRepliesOfFinishedTurns(_ items: [ChatItem], busy: Bool) -> Set<Int> {
        var ends = Set<Int>()
        var finished = !busy
        var found = false
        for item in items.reversed() {
            guard case let .row(row) = item else { continue }
            switch row.row.kind {
            case let .user(_, queued):
                if !queued {
                    finished = true
                    found = false
                }
            case .agent:
                if finished && !found {
                    ends.insert(item.id)
                    found = true
                }
            default:
                break
            }
        }
        return ends
    }

    /// Space above an item: `turnGap` between turns, `replyGap` between a message and the reply
    /// under it, `partGap` between the parts of one reply.
    private func gap(before index: Int, in items: [ChatItem]) -> CGFloat {
        guard index > 0 else { return 0 }
        if items[index].isUser { return look.turnGap }
        return items[index - 1].isUser ? look.replyGap : look.partGap
    }
}

/// The requests waiting on a person, as the transcript marks them on tool lines: this session's
/// tool calls with a request open, each one's place in the queue across every session, and how
/// many wait in all.
public struct PermissionQueue: Equatable {
    public var waiting: Set<String>
    public var places: [String: Int]
    public var count: Int

    public init(waiting: Set<String> = [], places: [String: Int] = [:], count: Int = 0) {
        self.waiting = waiting
        self.places = places
        self.count = count
    }
}

/// What `TranscriptView` reads off its scroll view to tell whether it was at the end before the
/// content grew.
private struct ScrollMetrics: Equatable {
    var content: CGFloat
    var offset: CGFloat
    var visible: CGFloat

    var distanceToBottom: CGFloat { content - offset - visible }
}
