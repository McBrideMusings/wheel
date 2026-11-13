import SwiftUI
import WheelKit

/// The background tasks an agent has running, one row each, above its composer: a dot (amber once
/// its output has been quiet five minutes), its name, its latest output line or a subagent's
/// progress, and how long it has run. A click opens the row to its last lines; ⤢ opens the full
/// view over the transcript. Draws nothing while no task runs.
public struct RunningStrip: View {
    @ObservedObject var chat: AgentChat

    public init(chat: AgentChat) { self.chat = chat }

    public var body: some View {
        let tasks = chat.footer?.tasks ?? []
        if !tasks.isEmpty {
            TimelineView(.periodic(from: .now, by: 1)) { timeline in
                VStack(spacing: 0) {
                    ForEach(Array(tasks.enumerated()), id: \.element.id) { index, task in
                        if index > 0 { Divider() }
                        StripRow(chat: chat, task: task, now: timeline.date)
                    }
                }
            }
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Color.primary.opacity(0.1)))
        }
    }
}

private struct StripRow: View {
    @ObservedObject var chat: AgentChat
    let task: RunningTask
    let now: Date

    private var open: Bool { chat.expandedTasks.contains(task.id) }

    var body: some View {
        let quiet = task.quiet(at: now)
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Circle().fill(quiet == nil ? Color.green : Color.orange).frame(width: 6, height: 6)
                Text(task.name)
                    .font(.callout.weight(.semibold))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: 220, alignment: .leading)
                    .fixedSize(horizontal: true, vertical: false)
                Text(open ? "" : quiet.map { "no output for \(Int($0 / 60))m · \(task.latest)" } ?? task.latest)
                    .font(.caption.monospaced())
                    .foregroundStyle(quiet == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.orange))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(RunningTask.clock(task.elapsed(at: now)))
                    .font(.callout.monospacedDigit())
                    .frame(minWidth: 52, alignment: .trailing)
                Button { chat.fullTask = task.id } label: {
                    Image(systemName: "arrow.up.left.and.arrow.down.right").font(.caption)
                }
                .buttonStyle(.borderless)
                .help("Show this task's whole output over the transcript")
                .accessibilityIdentifier("task-full-\(task.id)")
            }
            .contentShape(Rectangle())
            .onTapGesture {
                if open { chat.expandedTasks.remove(task.id) } else { chat.expandedTasks.insert(task.id) }
            }
            .help(task.command ?? task.name)
            if open { StripDetail(task: task, now: now).padding(.leading, 14) }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .accessibilityIdentifier("task-row-\(task.id)")
    }
}

/// A strip row opened: its last lines (a subagent's brief instead), when it started, how long
/// since its output grew, and where the output goes.
private struct StripDetail: View {
    let task: RunningTask
    let now: Date

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            let lines = task.kind == .agent ? [task.command ?? task.latest] : (task.tail.isEmpty ? [task.latest] : task.tail)
            Text(lines.joined(separator: "\n"))
                .font(.caption.monospaced())
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 7)
                .padding(.vertical, 4)
                .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 5))
            Text(TaskFacts.line(task, now: now))
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }
}

enum TaskFacts {
    static let time: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "h:mma"
        f.amSymbol = "am"
        f.pmSymbol = "pm"
        return f
    }()

    /// `started 1:19pm · last output 2s ago · /private/tmp/…/b8jhu969e.output`, with only the
    /// facts the task has.
    static func line(_ task: RunningTask, now: Date) -> String {
        var parts = ["started \(time.string(from: Date(timeIntervalSince1970: Double(task.startedMs) / 1000)))"]
        if task.kind == .agent {
            if let tokens = task.tokens { parts.append(String(format: "%.1fk tokens", Double(tokens) / 1000)) }
        } else if let last = task.lastOutputMs {
            let ago = max(0, now.timeIntervalSince1970 - Double(last) / 1000)
            parts.append("last output \(formatElapsed(ago)) ago")
        }
        if task.outputBytes > 0 { parts.append(ByteCountFormatter.string(fromByteCount: Int64(task.outputBytes), countStyle: .file)) }
        if let path = task.outputPath { parts.append(path) }
        return parts.joined(separator: " · ")
    }
}

/// The full view of the running tasks, over the transcript: one tab per task, the command or
/// brief, the whole recent output following its end, and the task's facts. Escape closes it.
public struct TaskFullView: View {
    @ObservedObject var chat: AgentChat
    /// Asks the daemon for a task's recent output; the reply lands in `chat.taskOutputs`.
    let read: (String) -> Void
    @State private var following = true

    public init(chat: AgentChat, read: @escaping (String) -> Void) {
        self.chat = chat
        self.read = read
    }

    public var body: some View {
        let tasks = chat.footer?.tasks ?? []
        if let id = chat.fullTask, let task = tasks.first(where: { $0.id == id }) {
            TimelineView(.periodic(from: .now, by: 1)) { timeline in
                VStack(spacing: 0) {
                    header(task, now: timeline.date)
                    Divider()
                    if tasks.count > 1 { tabs(tasks, now: timeline.date); Divider() }
                    if let command = task.command {
                        Text(task.kind == .shell ? "$ \(command)" : command)
                            .font(.caption.monospaced())
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                            .lineLimit(task.kind == .shell ? 2 : 6)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                        Divider()
                    }
                    output(task)
                    Divider()
                    Text(TaskFacts.line(task, now: timeline.date) + (following ? " · following the end" : ""))
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                }
            }
            .background(.background)
            .onAppear { read(task.id) }
            .onChange(of: task.id) { read(task.id) }
            .onChange(of: task.outputBytes) { read(task.id) }
            .accessibilityIdentifier("task-full-view")
        }
    }

    private func header(_ task: RunningTask, now: Date) -> some View {
        HStack(spacing: 8) {
            Circle().fill(task.quiet(at: now) == nil ? Color.green : Color.orange).frame(width: 7, height: 7)
            Text(task.name).font(.headline).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 8)
            Text(RunningTask.clock(task.elapsed(at: now))).font(.body.monospacedDigit()).frame(minWidth: 60, alignment: .trailing)
            Button("Show in Transcript") {
                chat.revealCall = task.call
                chat.fullTask = nil
            }
            .disabled(task.call == nil)
            Button("Close") { chat.fullTask = nil }
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private func tabs(_ tasks: [RunningTask], now: Date) -> some View {
        Picker("Task", selection: Binding(get: { chat.fullTask ?? "" }, set: { chat.fullTask = $0 })) {
            ForEach(tasks) { task in
                Text("\(task.name)  \(RunningTask.clock(task.elapsed(at: now)))").tag(task.id)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }

    @ViewBuilder private func output(_ task: RunningTask) -> some View {
        let read = chat.taskOutputs[task.id]
        let lines = Self.lines(of: task, read: read)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if read?.truncated == true {
                        Text("Earlier output left out: this shows the last 256 KB.")
                            .font(.caption).foregroundStyle(.tertiary).padding(.bottom, 4)
                    }
                    ForEach(Array(lines.enumerated()), id: \.offset) { index, line in
                        Text(line.isEmpty ? " " : line)
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .id(index)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .onScrollGeometryChange(for: Bool.self) { geometry in
                geometry.contentSize.height - geometry.contentOffset.y - geometry.visibleRect.height < 24
            } action: { _, atEnd in
                following = atEnd
            }
            .onChange(of: lines.count) { if following, lines.count > 0 { proxy.scrollTo(lines.count - 1, anchor: .bottom) } }
            .onAppear { if lines.count > 0 { proxy.scrollTo(lines.count - 1, anchor: .bottom) } }
        }
    }

    /// What the output area shows: the output read for the task; until that arrives, its last
    /// lines from the footer; for a subagent, which writes no output file, its progress.
    static func lines(of task: RunningTask, read: (text: String, truncated: Bool)?) -> [String] {
        if let read, !read.text.isEmpty {
            var lines = read.text.components(separatedBy: "\n")
            if lines.last == "" { lines.removeLast() }
            return lines
        }
        if let error = task.outputError { return [error] }
        if task.kind == .agent { return [task.latest] }
        return task.tail.isEmpty ? ["no output yet"] : task.tail
    }
}
