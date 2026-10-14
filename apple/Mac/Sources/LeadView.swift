import AppKit
import SwiftUI
import Textual
import WheelKit

/// The chat's type scale (DESIGN.md): three sizes and two weights. Content and the composer read
/// at `body`; tool, reasoning and permission lines at `line`; times, counts and settings at
/// `meta`. Semibold marks a name or a heading, never a whole line. Monospace is for code only.
enum ChatFont {
    static let body = Font.body
    static let line = Font.callout
    static let meta = Font.caption
    static let code = Font.system(.callout, design: .monospaced)
}

/// The lead card (DESIGN.md): header, the transcript, the request card above the composer, and
/// the composer every chat shares.
struct LeadCard: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
            TranscriptView(model: model, chat: chat, empty: "Ask the lead about any of your projects.")
            // Directly above the composer, where a person answering has their hands: the request
            // no tool line draws.
            if let perm = model.boxPerms.first {
                PermissionCard(model: model, perm: perm, position: 1, count: model.boxPerms.count, shortcut: model.inlinePerm == nil)
                    .padding([.horizontal, .top], 10)
            }
            Composer(
                placeholder: "Message the lead",
                draft: $chat.draft,
                error: chat.error,
                enabled: model.connection == .connected,
                send: { model.send(chat) },
                pickFiles: pickFiles
            )
            .padding([.horizontal, .top], 10)
            // Under the composer, where a person's eyes and hands already are when they are about
            // to send: what model answers and what it may do without asking belong beside the box
            // you type in, not in a header you read once.
            ConfigRow(model: model, chat: chat)
                .padding(.horizontal, 14)
                .padding(.top, 6)
                .padding(.bottom, 10)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .background(.background, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.separator))
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "person.crop.circle")
                .font(.title3)
                .foregroundStyle(.secondary)
            Text("Lead").font(ChatFont.body).fontWeight(.semibold)
            Text(chat.state?.label ?? "Not running")
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Spacer()
            StopButton(model: model, chat: chat)
                .buttonStyle(.borderless)
        }
    }
}

/// The controls the agent offers over its own session — model, effort level, permission mode —
/// as a status line under the composer. Each is its value as plain text: a row of bordered
/// pickers costs four lines of the card and reads as a form, when what a person wants is to see
/// what is in force and change it in one click.
struct ConfigRow: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        // Already in draw order: SessionChat.setConfig sorts once, so this and the state query
        // read the same list.
        HStack(spacing: 10) {
            Spacer(minLength: 0)
            ForEach(chat.config) { option in
                ConfigControl(option: option) { value in
                    model.setConfig(chat, option, to: value)
                }
            }
        }
    }
}

/// One option as a word you can click. The menu carries the option's name as its header, so the
/// row itself needs no label: `Sonnet` and `Auto` say what they are, and the header and the
/// tooltip name the control for the cases that don't.
struct ConfigControl: View {
    let option: ConfigOption
    let set: (String) -> Void

    var body: some View {
        switch option.kind {
        // A select the agent sent with no choices has nothing to pick from, so it reads as a
        // value rather than a menu that opens onto nothing.
        case let .select(current, choices) where !choices.isEmpty:
            menu(current: current, values: choices.map { ($0.value, $0.name) })
        case let .select(current, _):
            unsettable(current, why: "The agent offers no values for “\(option.name)”.")
        case let .boolean(current):
            menu(current: current ? "true" : "false", values: [("true", "On"), ("false", "Off")])
        case .unknown:
            unsettable(option.name, why: "This version of Wheel can't show a “\(option.name)” control.")
        }
    }

    /// A control shown but not settable: there is nothing to choose, so it does not pretend to
    /// open a menu.
    private func unsettable(_ text: String, why: String) -> some View {
        Text(text)
            .font(ChatFont.meta)
            .foregroundStyle(.tertiary)
            .lineLimit(1)
            .help(why)
    }

    /// The label reads from the agent's own list, so it always shows what the agent took — a pick
    /// the agent refuses or coerces snaps back when its list arrives.
    private func menu(current: String, values: [(value: String, name: String)]) -> some View {
        Menu {
            Section(option.name) {
                ForEach(values, id: \.value) { value in
                    Button {
                        if value.value != current { set(value.value) }
                    } label: {
                        // A real checkmark image, not spaces padded to look like a column:
                        // VoiceOver reads the padding out. An unselected row draws no icon at
                        // all rather than an empty symbol name, which renders as a blank box.
                        // The current value stays clickable, so re-picking it re-asserts it
                        // after the agent moved it itself.
                        Label {
                            Text(value.name)
                        } icon: {
                            if value.value == current { Image(systemName: "checkmark") }
                        }
                    }
                }
            }
        } label: {
            // One weight for the whole line: these are settings in force, not the message, and a
            // long model name in full-strength colour outshouts the composer above it.
            Text(values.first { $0.value == current }?.name ?? current)
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(option.description.map { "\(option.name): \($0)" } ?? option.name)
    }
}

/// Shown while a session is working; stops its current turn.
struct StopButton: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        if case .busy = chat.state {
            Button("Stop", systemImage: "stop.circle") { model.cancel(chat) }
                .labelStyle(.iconOnly)
                .help("Stop the current turn")
        }
    }
}

// MARK: - Transcript

/// One session's conversation: the person's messages, and each turn's parts — reply text,
/// reasoning, tool calls — in the order they arrived.
struct TranscriptView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat
    /// What an empty conversation says.
    let empty: String

    private var isLead: Bool { chat.name == AppModel.lead }

    /// Go to the last item while the view follows the end.
    private func follow(_ proxy: ScrollViewProxy, _ items: [ChatItem], why: String) {
        guard chat.pinned, let last = items.last else { return }
        Log.info("transcript \(chat.name): to the end (\(why))")
        proxy.scrollTo(last.id, anchor: .bottom)
    }

    var body: some View {
        if chat.rows.isEmpty {
            // Outside the scroll view, so it sits in the middle of the space the transcript has
            // rather than pinned to the bottom of an empty scroll.
            Text(empty)
                .font(ChatFont.body)
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
            waiting: Set(model.perms.filter { $0.session == chat.name }.compactMap(\.toolCall)),
            inline: isLead ? model.inlinePerm : nil,
            busy: chat.isBusy,
            expanded: chat.expanded
        )
        // Anchored to the bottom: new output stays in view while you are at the end, and leaves
        // you alone while you scroll back through history.
        return ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                        ChatItemView(model: model, chat: chat, item: item, copyable: replyEnds.contains(item.id), context: context)
                            .padding(.top, gap(before: index, in: items))
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
            }
            .defaultScrollAnchor(.bottom)
            // A bottom anchor alone loses the end: a lazy stack corrects its estimated heights as
            // rows are built and as a line opens, and a scroll to the end lands short while it
            // does. So the view keeps whether it follows the end, and only a person's own scroll
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
                        Log.info("transcript \(chat.name): \(pinned ? "follows the end" : "left the end"), \(Int(new.distanceToBottom)) from it")
                        chat.pinned = pinned
                    }
                } else if new.content != old.content || new.visible != old.visible {
                    follow(proxy, items, why: "content \(Int(old.content))→\(Int(new.content)), visible \(Int(old.visible))→\(Int(new.visible))")
                } else if new.offset < old.offset - 1, new.distanceToBottom > 24, chat.pinned {
                    // Moving up and away from the end with nothing else changing is a person's
                    // scroll whatever phase it reported — a notched mouse wheel may report none.
                    // Layout settling moves the view too, but only within the end or past it.
                    Log.info("transcript \(chat.name): left the end, scrolled up to \(Int(new.distanceToBottom)) from it")
                    chat.pinned = false
                }
            }
            .onChange(of: items.count) { follow(proxy, items, why: "\(items.count) items") }
            .onChange(of: model.leadScroll) { _, scroll in
                guard isLead, let item = scroll.toTop ? items.first : items.last else { return }
                // A program's scroll moves the view the way a person's would.
                chat.pinned = !scroll.toTop
                proxy.scrollTo(item.id, anchor: scroll.toTop ? .top : .bottom)
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

    /// Space above an item: 24 between turns, 12 between a message and the reply under it, 8
    /// between the parts of one reply.
    private func gap(before index: Int, in items: [ChatItem]) -> CGFloat {
        guard index > 0 else { return 0 }
        if items[index].isUser { return 24 }
        return items[index - 1].isUser ? 12 : 8
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

/// What a row draws that the row itself does not hold, as a value: the transcript, which observes
/// the model and the chat, hands it down, and a row redraws when it changes rather than for every
/// change to the model.
struct RowContext: Equatable {
    /// This session's tool calls with a request waiting on a person.
    var waiting: Set<String> = []
    /// The request drawn expanded on its tool line, when this transcript draws one.
    var inline: PermInfo?
    /// The session is working on a turn.
    var busy = false
    /// Rows and groups the person opened, by id.
    var expanded: Set<Int> = []

    func isWaiting(_ call: String?) -> Bool { call.map(waiting.contains) ?? false }

    func perm(_ call: String?) -> PermInfo? {
        guard let call, inline?.toolCall == call else { return nil }
        return inline
    }
}

/// One laid-out entry: a row, or a group of tool calls.
struct ChatItemView: View {
    let model: AppModel
    let chat: SessionChat
    let item: ChatItem
    /// The last part of a finished turn, which carries the turn's copy button.
    let copyable: Bool
    let context: RowContext

    var body: some View {
        switch item {
        case let .row(row):
            TranscriptRowView(model: model, chat: chat, row: row, copyable: copyable, context: context)
        case let .tools(group):
            ToolGroupView(model: model, chat: chat, group: group, context: context)
        }
    }
}

/// One row, observing its own `RowModel` so a chunk streamed into it redraws it and no other row.
struct TranscriptRowView: View {
    /// Not observed: a row redraws for its own changes and for `context`, not for every change to
    /// the model.
    let model: AppModel
    let chat: SessionChat
    @ObservedObject var row: RowModel
    let copyable: Bool
    let context: RowContext

    var body: some View {
        let _ = row.drew()
        let current = row.row
        RowView(model: model, chat: chat, row: current, copyable: copyable, context: context)
            .onAppear { if let call = current.toolCall, chat.name == AppModel.lead { model.leadRow(call, visible: true) } }
            .onDisappear { if let call = current.toolCall, chat.name == AppModel.lead { model.leadRow(call, visible: false) } }
    }
}

extension TranscriptRow.Kind {
    /// The kind a view keeps: reply and thought text trimmed into a new string. SwiftUI holds the
    /// last value it drew, so a view holding the transcript's own string would make the next
    /// appended chunk copy the whole reply.
    var displayed: TranscriptRow.Kind {
        switch self {
        case let .agent(text): .agent(Self.trimmed(text))
        case let .thought(text): .thought(Self.trimmed(text))
        default: self
        }
    }

    private static func trimmed(_ text: String) -> String {
        let start = text.firstIndex { !$0.isWhitespace } ?? text.endIndex
        let end = text.lastIndex { !$0.isWhitespace }.map { text.index(after: $0) } ?? start
        return String(decoding: text.utf8[start..<end], as: UTF8.self)
    }
}

struct RowView: View {
    /// Not observed: the row view above redraws this for the row's own changes.
    let model: AppModel
    let chat: SessionChat
    let row: TranscriptRow
    let copyable: Bool
    let context: RowContext

    var body: some View {
        switch row.kind.displayed {
        case let .user(text, queued):
            VStack(alignment: .trailing, spacing: 2) {
                Text(text)
                    .font(ChatFont.body)
                    .textSelection(.enabled)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .background(Color.primary.opacity(queued ? 0.04 : 0.08), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .foregroundStyle(queued ? .secondary : .primary)
                if queued {
                    Text("Queued").font(ChatFont.meta).foregroundStyle(.secondary)
                }
            }
            .padding(.leading, 40)
            .frame(maxWidth: .infinity, alignment: .trailing)
        case let .agent(text):
            AgentText(text: text, copyable: copyable)
        case let .thought(text):
            ReasoningLine(chat: chat, row: row, text: text, context: context)
        case let .block(kind, name):
            Label(name.map { "\(kind.replacingOccurrences(of: "_", with: " ")): \($0)" } ?? kind.replacingOccurrences(of: "_", with: " "), systemImage: "paperclip")
                .font(ChatFont.line)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        case let .tool(title, kind, status, content):
            ToolLine(model: model, chat: chat, row: row, title: title, kind: kind, status: status, content: content, context: context)
        case let .plan(entries):
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                    Label(entry.content, systemImage: entry.status == "completed" ? "checkmark.square" : "square")
                        .font(ChatFont.line)
                        .foregroundStyle(entry.status == "completed" ? .secondary : .primary)
                }
            }
        case let .mode(mode):
            Label("Mode: \(mode)", systemImage: "arrow.triangle.swap").font(ChatFont.meta).foregroundStyle(.secondary)
        case let .notice(text):
            Text(text).font(ChatFont.meta).foregroundStyle(.secondary)
        case let .error(text):
            Label(text, systemImage: "exclamationmark.triangle").font(ChatFont.line).foregroundStyle(.red)
        }
    }
}

extension SessionChat {
    var isBusy: Bool {
        if case .busy = state { return true }
        return false
    }
}

/// A reply: the agent's Markdown at body size, with headings kept close to it so a `#` does not
/// shout over the conversation. The copy button sits under the last reply of a finished turn,
/// and under any other reply while the pointer is over it.
struct AgentText: View {
    let text: String
    let copyable: Bool
    @State private var hovering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            // A half-received fence still renders as text, which is what makes this safe to use on
            // a row that grows chunk by chunk.
            StructuredText(markdown: text)
                .font(ChatFont.body)
                .textual.headingStyle(ChatHeadingStyle())
                .textual.inlineStyle(.chat)
                .textual.textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            // Only the end of a finished turn keeps a line for the button; an earlier reply shows it
            // over its own corner while the pointer is on it, so the parts of a turn stay close.
            if copyable { copy }
        }
        .overlay(alignment: .topTrailing) {
            if hovering && !copyable { copy.offset(y: -2) }
        }
        .onHover { hovering = $0 }
    }

    private var copy: some View {
        Button {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        } label: {
            Image(systemName: "doc.on.doc").font(ChatFont.meta)
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .help("Copy this reply")
    }
}

/// Headings one step above body at most: in a narrow card a 30pt `#` pushes the reply off screen.
struct ChatHeadingStyle: StructuredText.HeadingStyle {
    private static let fontScales: [CGFloat] = [1.25, 1.125, 1, 1, 1, 1]

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .textual.fontScale(Self.fontScales[min(configuration.headingLevel, 6) - 1])
            .textual.blockSpacing(.fontScaled(top: 1.2, bottom: 0.5))
            .fontWeight(.semibold)
    }
}

extension InlineStyle {
    /// Inline code smaller than the text around it, on a faint fill, so a path in a sentence
    /// reads as code without outweighing the words.
    static var chat: InlineStyle {
        InlineStyle()
            .code(.monospaced, .fontScale(0.88), .backgroundColor(Color.primary.opacity(0.07)))
            .strong(.fontWeight(.semibold))
    }
}

// MARK: - Tool calls and reasoning

/// What a tool line's leading icon says.
private enum ToolPhase {
    case running, waiting, completed, failed, stopped

    init(status: String, open: Bool, waiting: Bool) {
        if waiting { self = .waiting }
        else if status == "completed" { self = .completed }
        else if status == "failed" { self = .failed }
        else if open { self = .running }
        else { self = .stopped }
    }

    @ViewBuilder var icon: some View {
        switch self {
        case .running: ProgressView().controlSize(.mini).frame(width: 14, height: 14)
        case .waiting: Image(systemName: "hand.raised.fill").foregroundStyle(.orange).frame(width: 14)
        case .completed: Image(systemName: "checkmark").foregroundStyle(.secondary).frame(width: 14)
        case .failed: Image(systemName: "xmark.circle").foregroundStyle(.red).frame(width: 14)
        case .stopped: Image(systemName: "minus.circle").foregroundStyle(.secondary).frame(width: 14)
        }
    }
}

/// One tool call as a single quiet line — icon, title, how long it ran, a chevron — that opens
/// onto what the agent sent with it. A call waiting on a person opens by itself, with the request.
struct ToolLine: View {
    let model: AppModel
    let chat: SessionChat
    let row: TranscriptRow
    let title: String
    let kind: String
    let status: String
    let content: [ToolContent]
    let context: RowContext

    private var expanded: Bool { context.expanded.contains(row.id) }

    var body: some View {
        let waiting = context.isWaiting(row.toolCall)
        let perm = context.perm(row.toolCall)
        let phase = ToolPhase(status: status, open: row.open, waiting: waiting)
        VStack(alignment: .leading, spacing: 6) {
            Button { chat.toggle(row.id) } label: {
                HStack(spacing: 6) {
                    phase.icon.font(ChatFont.meta)
                    Text(title)
                        .font(ChatFont.line)
                        .foregroundStyle(phase == .failed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .shimmer(phase == .running)
                    if waiting {
                        Text("Needs your input")
                            .font(ChatFont.meta)
                            .foregroundStyle(.orange)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 1)
                            .background(Color.orange.opacity(0.15), in: Capsule())
                    }
                    Elapsed(row: row)
                    Chevron(open: expanded || perm != nil)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(title)
            if expanded || perm != nil {
                ToolDetail(title: title, kind: kind, content: content, perm: perm, answer: model.answer, shortcut: perm != nil)
                    .padding(.leading, 20)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(.easeOut(duration: 0.2), value: expanded)
    }
}

/// The disclosure mark at the end of a tool, group or reasoning line, at the line's metadata size.
struct Chevron: View {
    let open: Bool

    var body: some View {
        Image(systemName: "chevron.right")
            .font(ChatFont.meta)
            .fontWeight(.semibold)
            .foregroundStyle(.tertiary)
            .rotationEffect(.degrees(open ? 90 : 0))
    }
}

/// Two or more tool calls in a row, as one line that opens onto each call's own line. It opens by
/// itself while one of its calls waits on a person.
struct ToolGroupView: View {
    let model: AppModel
    let chat: SessionChat
    /// Observed: the chat tells the group whenever one of its calls changes.
    @ObservedObject var group: ToolGroupModel
    let context: RowContext

    private var expanded: Bool { context.expanded.contains(group.openKey) }

    var body: some View {
        let rows = group.rows
        let current = rows.map(\.row)
        let waiting = current.contains { context.isWaiting($0.toolCall) }
        let running = current.filter(\.open).count
        let failed = current.filter { if case let .tool(_, _, status, _) = $0.kind { status == "failed" } else { false } }.count
        let open = expanded || waiting
        let phase: ToolPhase = waiting ? .waiting : running > 0 ? .running : failed > 0 ? .failed : .completed
        VStack(alignment: .leading, spacing: 4) {
            Button { chat.toggle(group.openKey) } label: {
                HStack(spacing: 6) {
                    phase.icon.font(ChatFont.meta)
                    Text(summary(count: current.count, running: running, failed: failed))
                        .font(ChatFont.line)
                        .foregroundStyle(.secondary)
                        .shimmer(phase == .running)
                    GroupElapsed(rows: current)
                    Chevron(open: open)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if open {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(rows) { row in
                        TranscriptRowView(model: model, chat: chat, row: row, copyable: false, context: context)
                    }
                }
                .padding(.leading, 20)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(.easeOut(duration: 0.2), value: open)
    }

    private func summary(count: Int, running: Int, failed: Int) -> String {
        if running > 0 { return "Running \(count) tools" }
        if failed > 0 { return "Used \(count) tools · \(failed) failed" }
        return "Used \(count) tools"
    }
}

/// How long a tool call has run, ticking each second while it runs. Nothing for a call with no
/// time (replayed history).
struct Elapsed: View {
    let row: TranscriptRow

    var body: some View {
        if row.started != nil {
            TimelineView(.periodic(from: .now, by: row.open ? 1 : 3600)) { context in
                if let seconds = row.elapsed(now: context.date) {
                    Text(formatElapsed(seconds))
                        .font(ChatFont.meta)
                        .monospacedDigit()
                        .foregroundStyle(.tertiary)
                }
            }
        }
    }
}

/// A group's span: from its first call's start to its last call's end, or to now while one runs.
struct GroupElapsed: View {
    let rows: [TranscriptRow]

    var body: some View {
        let open = rows.contains { $0.open }
        if let start = rows.compactMap(\.started).min() {
            TimelineView(.periodic(from: .now, by: open ? 1 : 3600)) { context in
                let end = open ? context.date : rows.compactMap(\.ended).max()
                if let end {
                    Text(formatElapsed(max(0, end.timeIntervalSince(start))))
                        .font(ChatFont.meta)
                        .monospacedDigit()
                        .foregroundStyle(.tertiary)
                }
            }
        }
    }
}

/// `<1s`, `1.2s` under ten seconds, `12s`, `1m 5s`.
func formatElapsed(_ seconds: TimeInterval) -> String {
    if seconds < 1 { return "<1s" }
    if seconds < 10 { return String(format: "%.1fs", seconds) }
    if seconds < 60 { return "\(Int(seconds))s" }
    return "\(Int(seconds) / 60)m \(Int(seconds) % 60)s"
}

/// What a tool call carries: its full title, what kind of action it is, the agent's text and
/// diffs, and — while it waits on a person — the request's choices.
struct ToolDetail: View {
    let title: String
    let kind: String
    let content: [ToolContent]
    let perm: PermInfo?
    let answer: (PermInfo, String) -> Void
    /// Whether ⌘↩ answers this request with its first allow choice.
    let shortcut: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if perm != nil {
                Text(action(kind)).font(ChatFont.line).fontWeight(.semibold)
            }
            Text(title)
                .font(ChatFont.code)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(Array(content.enumerated()), id: \.offset) { _, item in
                ContentItem(item: item)
            }
            if let perm {
                PermChoices(perm: perm, answer: answer, shortcut: shortcut)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay {
            if perm != nil { RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Color.orange.opacity(0.6)) }
        }
    }
}

/// What a request asks to do, in words, from the ACP tool kind.
func action(_ kind: String) -> String {
    switch kind {
    case "read": "Read a file"
    case "edit": "Change a file"
    case "delete": "Delete a file"
    case "move": "Move a file"
    case "search": "Search"
    case "execute": "Run a command"
    case "fetch": "Fetch from the web"
    case "think": "Think"
    default: "Use a tool"
    }
}

/// One item of a tool call's content.
struct ContentItem: View {
    let item: ToolContent

    var body: some View {
        switch item {
        case let .text(text):
            Text(text)
                .font(ChatFont.line)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        case let .diff(path, oldText, newText):
            DiffView(path: path, oldText: oldText, newText: newText)
        case let .terminal(id):
            Label("Terminal \(id)", systemImage: "terminal").font(ChatFont.meta).foregroundStyle(.secondary)
        case let .other(type):
            Text("Content this version can't show: \(type)").font(ChatFont.meta).foregroundStyle(.secondary)
        }
    }
}

/// A file change as removed and added lines, capped in height so a large edit scrolls inside its
/// box instead of pushing the conversation away.
struct DiffView: View {
    let path: String
    let oldText: String?
    let newText: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(oldText == nil ? "\(path) (new file)" : path)
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                        Text(line.text)
                            .font(ChatFont.code)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(line.added ? Color.green.opacity(0.14) : Color.red.opacity(0.14))
                    }
                }
                .textSelection(.enabled)
            }
            .frame(maxHeight: 180)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var lines: [(text: String, added: Bool)] {
        let old = (oldText ?? "").split(separator: "\n", omittingEmptySubsequences: false).dropLast(oldText?.hasSuffix("\n") == true ? 1 : 0)
        let new = newText.split(separator: "\n", omittingEmptySubsequences: false).dropLast(newText.hasSuffix("\n") ? 1 : 0)
        return old.map { ("- \($0)", false) } + new.map { ("+ \($0)", true) }
    }
}

/// A request's choices, in the order a person reads them: the refusals on the left, the
/// allowances on the right with the one-time allow filled, as the default.
struct PermChoices: View {
    let perm: PermInfo
    let answer: (PermInfo, String) -> Void
    let shortcut: Bool

    var body: some View {
        let rejects = perm.options.filter { $0.kind.hasPrefix("reject") }
        let allows = perm.options.filter { !$0.kind.hasPrefix("reject") }
        let primary = allows.first { $0.kind == "allow_once" } ?? allows.first
        HStack(spacing: 8) {
            ForEach(rejects, id: \.id) { option in
                Button(option.name) { answer(perm, option.id) }
            }
            Spacer(minLength: 8)
            ForEach(allows.filter { $0.id != primary?.id }, id: \.id) { option in
                Button(option.name) { answer(perm, option.id) }
            }
            if let primary {
                Button(primary.name) { answer(perm, primary.id) }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(shortcut ? KeyboardShortcut(.return, modifiers: .command) : nil)
                    .help(shortcut ? "\(primary.name) (⌘↩)" : primary.name)
            }
        }
        .controlSize(.small)
    }
}

/// A request no tool line draws, one at a time above the composer: who asks, what for, the full
/// request, and its choices. `position` of `count` says how many more wait behind it.
struct PermissionCard: View {
    let model: AppModel
    let perm: PermInfo
    let position: Int
    let count: Int
    let shortcut: Bool

    var body: some View {
        // The asking session's own tool line, when its transcript holds one: the kind and content
        // that explain the request.
        let tool = perm.toolCall.flatMap { call in
            (model.chats[perm.session] ?? (perm.session == AppModel.lead ? model.lead : nil))?
                .transcript.rows.last { $0.toolCall == call }
        }
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "hand.raised.fill").foregroundStyle(.orange)
                Text(asker).font(ChatFont.line).fontWeight(.semibold)
                Spacer()
                if count > 1 {
                    Text("\(position) of \(count)").font(ChatFont.meta).monospacedDigit().foregroundStyle(.secondary)
                }
            }
            if case let .tool(_, kind, _, content)? = tool?.kind {
                Text(action(kind)).font(ChatFont.line).foregroundStyle(.secondary)
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        title
                        ForEach(Array(content.enumerated()), id: \.offset) { _, item in ContentItem(item: item) }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 220)
                .fixedSize(horizontal: false, vertical: true)
            } else {
                title
            }
            PermChoices(perm: perm, answer: model.answer, shortcut: shortcut)
        }
        .padding(12)
        .background(.background, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Color.orange.opacity(0.6)))
    }

    private var title: some View {
        Text(perm.title)
            .font(ChatFont.code)
            .textSelection(.enabled)
            .lineLimit(12)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var asker: String {
        perm.session == AppModel.lead ? "The lead is asking" : "Agent \(perm.session) is asking"
    }
}

/// The agent's reasoning as one line: "Thinking" while it streams, with the latest of it showing
/// under the line; "Thought for 4s" once it ends, collapsed, opening onto the whole of it.
struct ReasoningLine: View {
    let chat: SessionChat
    let row: TranscriptRow
    let text: String
    let context: RowContext

    private var expanded: Bool { context.expanded.contains(row.id) }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button { chat.toggle(row.id) } label: {
                HStack(spacing: 6) {
                    if row.open {
                        ToolPhase.running.icon.font(ChatFont.meta)
                    } else {
                        Image(systemName: "brain").font(ChatFont.meta).foregroundStyle(.secondary).frame(width: 14)
                    }
                    Text(label).font(ChatFont.line).foregroundStyle(.secondary).shimmer(row.open)
                    if row.open { Elapsed(row: row) }
                    Chevron(open: expanded || row.open)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if expanded || row.open {
                // While it streams and is not opened, the newest of it: the last 400 characters,
                // cut at a word, so the line under "Thinking" follows the thought as it grows.
                Text(expanded || !row.open ? text : Self.tail(text))
                    .font(ChatFont.line)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 20)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(.easeOut(duration: 0.2), value: expanded)
    }

    private static func tail(_ text: String) -> String {
        guard text.count > 400 else { return text }
        let cut = text.suffix(400)
        return "…" + (cut.firstIndex(of: " ").map { cut[cut.index(after: $0)...] } ?? cut)
    }

    private var label: String {
        if row.open { return "Thinking" }
        return row.elapsed(now: .now).map { "Thought for \(formatElapsed($0))" } ?? "Thought"
    }
}

extension View {
    /// A highlight sweeping across the view's own shape while `active`: the sign that a tool or a
    /// thought is still going. Still under Reduce Motion.
    func shimmer(_ active: Bool) -> some View {
        modifier(Shimmer(active: active))
    }
}

private struct Shimmer: ViewModifier {
    let active: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        if active && !reduceMotion {
            content.overlay {
                TimelineView(.animation) { context in
                    // One sweep every 1.6 s: 1.2 s crossing, 0.4 s still.
                    let cycle = context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1.6)
                    let progress = min(cycle / 1.2, 1)
                    GeometryReader { geometry in
                        let band = max(40, geometry.size.width * 0.35)
                        LinearGradient(colors: [.clear, Color.primary.opacity(0.85), .clear], startPoint: .leading, endPoint: .trailing)
                            .frame(width: band)
                            .offset(x: -band + progress * (geometry.size.width + band))
                    }
                }
                .mask(content)
                .allowsHitTesting(false)
            }
        } else {
            content
        }
    }
}

// MARK: - Composer

/// The composer every chat shares (ADR-0002): Return sends, Shift-Return adds a line, the
/// paperclip inserts file paths and `/` inserts a skill name. It knows nothing about who it talks
/// to: the caller owns the draft and supplies the send action, which clears the draft once the
/// message goes out.
struct Composer: View {
    let placeholder: String
    @Binding var draft: String
    let error: String?
    let enabled: Bool
    let send: () -> Void
    /// The platform's file picker; returns the chosen paths.
    let pickFiles: () -> [String]
    /// Called when the text box takes keyboard focus, however it got it (a click, Tab).
    var onFocus: () -> Void = {}
    /// Changes when the caller wants the keyboard in the text box.
    var focusRequest = 0

    @FocusState private var focused: Bool

    static let skills = [
        ("implement", "Plan, edit, verify and review one tracked item"),
        ("review", "Review the changes in a project"),
        ("spike", "Build a throwaway prototype"),
        ("issues", "File or pick the next bead"),
        ("diagnose", "Track down a bug"),
        ("wrap-up", "Commit and land finished work"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ZStack(alignment: .topLeading) {
                if draft.isEmpty {
                    Text(placeholder)
                        .font(ChatFont.body)
                        .foregroundStyle(.tertiary)
                        .padding(.horizontal, 5)
                        .allowsHitTesting(false)
                }
                TextEditor(text: $draft)
                    .font(ChatFont.body)
                    .scrollContentBackground(.hidden)
                    .scrollIndicators(.never)
                    .frame(minHeight: 20, maxHeight: 120)
                    .fixedSize(horizontal: false, vertical: true)
                    .focused($focused)
                    .onChange(of: focused) { _, now in
                        if now { onFocus() }
                    }
                    .onChange(of: focusRequest) { focused = true }
                    .onKeyPress(.return, phases: .down) { press in
                        if press.modifiers.contains(.shift) { return .ignored }
                        submit()
                        return .handled
                    }
            }
            HStack(spacing: 6) {
                Button("Attach a file", systemImage: "paperclip") {
                    let paths = pickFiles()
                    if !paths.isEmpty { insert(paths.joined(separator: " ") + " ") }
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("Insert a file's path into the message")
                Menu {
                    ForEach(Self.skills, id: \.0) { skill in
                        Button("/\(skill.0) — \(skill.1)") { insert("/\(skill.0) ") }
                    }
                } label: {
                    Label("Run a skill", systemImage: "slash.circle")
                        .labelStyle(.iconOnly)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Run a skill")
                if let error {
                    Text(error).font(ChatFont.meta).foregroundStyle(.red).lineLimit(2)
                }
                Spacer()
                Button("Send", systemImage: "arrow.up.circle.fill", action: submit)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .font(.title2)
                    .disabled(!canSend)
            }
            .foregroundStyle(.secondary)
        }
        .padding(8)
        .background(Color.primary.opacity(0.03), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color.primary.opacity(focused ? 0.25 : 0.1))
        )
        .contentShape(Rectangle())
        .onTapGesture { focused = true }
    }

    private var canSend: Bool {
        enabled && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// The one send path for Return and the button.
    private func submit() {
        guard !draft.isEmpty else { return }
        send()
    }

    private func insert(_ text: String) {
        draft += (draft.isEmpty || draft.hasSuffix(" ") || draft.hasSuffix("\n") ? "" : " ") + text
    }
}

/// The Mac's file picker for the composer's paperclip.
@MainActor
func pickFiles() -> [String] {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = true
    panel.allowsMultipleSelection = true
    return panel.runModal() == .OK ? panel.urls.map(\.path) : []
}
