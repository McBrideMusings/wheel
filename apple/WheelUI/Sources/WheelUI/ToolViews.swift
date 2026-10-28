import SwiftUI
import WheelKit

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
/// onto what the agent sent with it. A call waiting on a person says so; the card above the
/// composer holds the request.
struct ToolLine: View {
    @Environment(\.chatAppearance) private var look
    let chat: SessionChat
    let row: TranscriptRow
    let title: String
    let kind: String
    let status: String
    let content: [ToolContent]
    let context: RowContext

    private var expanded: Bool { context.expanded.contains(row.id) }

    /// The request on this line waits its turn and is answered in the card.
    private var waitLabel: String {
        guard context.waitingCount > 1, let place = row.toolCall.flatMap({ context.places[$0] }) else {
            return "Needs your input"
        }
        return "Waiting, \(place) of \(context.waitingCount)"
    }

    var body: some View {
        let waiting = context.isWaiting(row.toolCall)
        let phase = ToolPhase(status: status, open: row.open, waiting: waiting)
        VStack(alignment: .leading, spacing: 6) {
            Button { chat.toggle(row.id) } label: {
                HStack(spacing: 6) {
                    phase.icon.font(look.meta)
                    Text(title)
                        .font(look.line)
                        .foregroundStyle(phase == .failed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .shimmer(phase == .running)
                    if waiting {
                        Text(waitLabel)
                            .font(look.meta)
                            .foregroundStyle(.orange)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 1)
                            .background(Color.orange.opacity(0.15), in: Capsule())
                    }
                    Elapsed(row: row)
                    Chevron(open: expanded)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(title)
            if expanded {
                ToolDetail(title: title, content: content)
                    .padding(.leading, 20)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(.easeOut(duration: 0.2), value: expanded)
    }
}

/// The disclosure mark at the end of a tool, group or reasoning line, at the line's metadata size.
struct Chevron: View {
    @Environment(\.chatAppearance) private var look
    let open: Bool

    var body: some View {
        Image(systemName: "chevron.right")
            .font(look.meta)
            .fontWeight(.semibold)
            .foregroundStyle(.tertiary)
            .rotationEffect(.degrees(open ? 90 : 0))
    }
}

/// A run of tool calls ("Ran 1 command, read 2 files"), or a whole turn's work ("Worked for 38s"),
/// as one line that opens onto each row's own line. It opens by itself while one of its calls
/// waits on a person.
struct ToolGroupView: View {
    /// Whether the line stands for a whole turn.
    var turn = false
    @Environment(\.chatAppearance) private var look
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
                    phase.icon.font(look.meta)
                    Text(turn ? turnLabel(current, running: running > 0) : summary(current, running: running, failed: failed))
                        .font(look.line)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .shimmer(phase == .running)
                    if !turn || running > 0 { GroupElapsed(rows: current) }
                    Chevron(open: open)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if open {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(rows) { row in
                        TranscriptRowView(chat: chat, row: row, copyable: false, context: context)
                    }
                }
                .padding(.leading, 20)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(.easeOut(duration: 0.2), value: open)
    }

    private func summary(_ rows: [TranscriptRow], running: Int, failed: Int) -> String {
        let words = ToolWords.summary(of: rows)
        if running > 0 { return words.replacingOccurrences(of: "Ran ", with: "Running ") }
        return failed > 0 ? "\(words) · \(failed) failed" : words
    }

    /// "Worked for 38s" once the turn's work is over; "Working" and the call under way while it runs.
    private func turnLabel(_ rows: [TranscriptRow], running: Bool) -> String {
        guard running else {
            guard let start = rows.compactMap(\.started).min(), let end = rows.compactMap(\.ended).max() else { return "Worked" }
            return "Worked for \(formatElapsed(max(0, end.timeIntervalSince(start))))"
        }
        if let current = rows.last(where: \.open), case let .tool(title, _, _, _) = current.kind { return "Working · \(title)" }
        return "Working"
    }
}

/// How long a tool call has run, ticking each second while it runs. Nothing for a call with no
/// time (replayed history).
struct Elapsed: View {
    @Environment(\.chatAppearance) private var look
    let row: TranscriptRow

    var body: some View {
        if row.started != nil {
            TimelineView(.periodic(from: .now, by: row.open ? 1 : 3600)) { context in
                if let seconds = row.elapsed(now: context.date) {
                    Text(formatElapsed(seconds))
                        .font(look.meta)
                        .monospacedDigit()
                        .foregroundStyle(.tertiary)
                }
            }
        }
    }
}

/// A group's span: from its first call's start to its last call's end, or to now while one runs.
struct GroupElapsed: View {
    @Environment(\.chatAppearance) private var look
    let rows: [TranscriptRow]

    var body: some View {
        let open = rows.contains { $0.open }
        if let start = rows.compactMap(\.started).min() {
            TimelineView(.periodic(from: .now, by: open ? 1 : 3600)) { context in
                let end = open ? context.date : rows.compactMap(\.ended).max()
                if let end {
                    Text(formatElapsed(max(0, end.timeIntervalSince(start))))
                        .font(look.meta)
                        .monospacedDigit()
                        .foregroundStyle(.tertiary)
                }
            }
        }
    }
}

/// `<1s`, `1.2s` under ten seconds, `12s`, `1m 5s`.
public func formatElapsed(_ seconds: TimeInterval) -> String {
    if seconds < 1 { return "<1s" }
    if seconds < 10 { return String(format: "%.1fs", seconds) }
    if seconds < 60 { return "\(Int(seconds))s" }
    return "\(Int(seconds) / 60)m \(Int(seconds) % 60)s"
}

/// What a tool call carries: its full title, and the agent's text and diffs.
struct ToolDetail: View {
    @Environment(\.chatAppearance) private var look
    let title: String
    let content: [ToolContent]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(look.code)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(Array(content.enumerated()), id: \.offset) { _, item in
                ContentItem(item: item)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

/// What a request asks to do, in words, from the tool kind.
public func action(_ kind: String) -> String {
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
public struct ContentItem: View {
    @Environment(\.chatAppearance) private var look
    let item: ToolContent

    public init(item: ToolContent) { self.item = item }

    public var body: some View {
        switch item {
        case let .text(text):
            Text(text)
                .font(look.line)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        case let .diff(path, oldText, newText):
            DiffView(path: path, oldText: oldText, newText: newText)
        case let .other(type):
            Text("Content this version can't show: \(type)").font(look.meta).foregroundStyle(.secondary)
        }
    }
}

/// A file change as removed and added lines, capped in height so a large edit scrolls inside its
/// box instead of pushing the conversation away.
struct DiffView: View {
    @Environment(\.chatAppearance) private var look
    let path: String
    let oldText: String?
    let newText: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(oldText == nil ? "\(path) (new file)" : path)
                .font(look.meta)
                .foregroundStyle(.secondary)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                        Text(line.text)
                            .font(look.code)
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

/// The agent's reasoning as one line: "Thinking" while it streams, with the latest of it showing
/// under the line; "Thought for 4s" once it ends, collapsed, opening onto the whole of it.
struct ReasoningLine: View {
    @Environment(\.chatAppearance) private var look
    let chat: SessionChat
    let row: TranscriptRow
    let text: String
    let context: RowContext

    private var expanded: Bool { context.expanded.contains(row.id) }
    /// The newest of the thought shows under the line while it streams: `chat.thinking` live.
    private var streaming: Bool { row.open && look.layout.thinking == .live }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button { chat.toggle(row.id) } label: {
                HStack(spacing: 6) {
                    if row.open {
                        ToolPhase.running.icon.font(look.meta)
                    } else {
                        Image(systemName: "brain").font(look.meta).foregroundStyle(.secondary).frame(width: 14)
                    }
                    Text(label).font(look.line).foregroundStyle(.secondary).shimmer(row.open)
                    if row.open { Elapsed(row: row) }
                    Chevron(open: expanded || streaming)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if expanded || streaming {
                // While it streams and is not opened, the newest of it: the last 400 characters,
                // cut at a word, so the line under "Thinking" follows the thought as it grows.
                let shown = expanded || !row.open ? text : Self.tail(text)
                Text(shown)
                    .font(look.line)
                    .foregroundStyle(.secondary)
                    .selectableText(.plain(expanded ? text : ""), id: row.id, chat: chat, style: look.lineProse)
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
    /// thought is still going. Still under Reduce Motion, and paused while the line is scrolled out
    /// of view, because each frame of the sweep redraws the window even when nobody sees it.
    func shimmer(_ active: Bool) -> some View {
        modifier(Shimmer(active: active))
    }
}

private struct Shimmer: ViewModifier {
    let active: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// Whether any of the line shows in its scroll view; true outside one.
    @State private var inView = true

    func body(content: Content) -> some View {
        sweep(content)
            .onScrollVisibilityChange(threshold: 0.01) { inView = $0 }
    }

    @ViewBuilder
    private func sweep(_ content: Content) -> some View {
        if active && inView && !reduceMotion {
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

/// The card that ends a turn which edited files: how many, the lines added and removed, a row per
/// file, and "View changes", which opens the combined diff in the card.
struct EditsCard: View {
    @Environment(\.chatAppearance) private var look
    let chat: SessionChat
    @ObservedObject var group: ToolGroupModel
    let context: RowContext

    private var open: Bool { context.expanded.contains(group.id) }

    var body: some View {
        let files = Edits.files(in: group.rows.map(\.row))
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text(Edits.title(count: files.count)).font(look.line).fontWeight(.semibold)
                Counts(added: files.map(\.added).reduce(0, +), removed: files.map(\.removed).reduce(0, +))
                Spacer(minLength: 8)
                Button(open ? "Hide changes" : "View changes") { chat.toggle(group.id) }
                    .buttonStyle(.plain)
                    .font(look.line)
                    .foregroundStyle(Color.accentColor)
            }
            ForEach(files, id: \.path) { file in
                HStack(spacing: 8) {
                    Text(file.path)
                        .font(look.code)
                        .foregroundStyle(Color.accentColor)
                        .lineLimit(1)
                        .truncationMode(.head)
                    Spacer(minLength: 8)
                    Counts(added: file.added, removed: file.removed)
                }
                if open {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(file.changes.enumerated()), id: \.offset) { _, line in
                            Text((line.added ? "+ " : "- ") + line.text)
                                .font(look.code)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(line.added ? Color.green.opacity(0.14) : Color.red.opacity(0.14))
                        }
                    }
                    .textSelection(.enabled)
                    .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .animation(.easeOut(duration: 0.2), value: open)
    }
}

/// `+24 −4`, green and red, at the line size.
private struct Counts: View {
    @Environment(\.chatAppearance) private var look
    let added: Int
    let removed: Int

    var body: some View {
        HStack(spacing: 4) {
            Text("+\(added)").foregroundStyle(.green)
            Text("\u{2212}\(removed)").foregroundStyle(.red)
        }
        .font(look.line)
        .monospacedDigit()
    }
}
