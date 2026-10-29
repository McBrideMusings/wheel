import SwiftUI
import WheelKit

/// The title-bar button that opens the space's process list: every process its panes started.
/// The daemon lists them when the card opens and every 2 seconds while it stays open.
struct ProcessesButton: View {
    var model: AppModel
    let space: String
    @State private var shown = false

    var body: some View {
        Button { shown.toggle() } label: { Image(systemName: "list.bullet.rectangle") }
            .help("Processes started from this space's panes")
            .accessibilityLabel("Processes")
            .popover(isPresented: $shown, arrowEdge: .bottom) {
                ProcessesCard(model: model, space: space)
                    .frame(width: 380)
            }
            .onChange(of: shown) { _, now in
                if now { model.requestProcs(space) }
            }
    }
}

/// The card: a filter field, one row per process (name, pane, CPU), and under the chosen row its
/// command line, directory and a Stop button.
struct ProcessesCard: View {
    var model: AppModel
    let space: String
    @State private var filter = ""
    @State private var chosen: UInt32?
    private let tick = Timer.publish(every: 2, on: .main, in: .common).autoconnect()

    var body: some View {
        let all = model.procs[space] ?? []
        let rows = all.filter {
            filter.isEmpty || $0.command.localizedCaseInsensitiveContains(filter)
                || $0.pane.localizedCaseInsensitiveContains(filter)
        }
        VStack(alignment: .leading, spacing: 8) {
            TextField("Filter", text: $filter).textFieldStyle(.roundedBorder)
            if rows.isEmpty {
                Text("No processes are reporting activity")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 60)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(rows) { proc in
                            ProcessRow(proc: proc, open: chosen == proc.pid) {
                                chosen = chosen == proc.pid ? nil : proc.pid
                            } stop: {
                                model.stopProc(space, pid: proc.pid)
                            }
                        }
                    }
                }
                .frame(maxHeight: 320)
            }
        }
        .padding(12)
        .onReceive(tick) { _ in model.requestProcs(space) }
    }
}

struct ProcessRow: View {
    let proc: ProcInfo
    let open: Bool
    let toggle: () -> Void
    let stop: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button(action: toggle) {
                HStack {
                    Text(proc.name).fontWeight(.medium).lineLimit(1)
                    Text(proc.pane).foregroundStyle(.secondary).lineLimit(1)
                    Spacer()
                    Text(String(format: "%.1f%%", proc.cpu)).monospacedDigit().foregroundStyle(.secondary)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if open {
                VStack(alignment: .leading, spacing: 4) {
                    Text("pid \(proc.pid) · \(proc.memKb / 1024) MB · up \(proc.ageSecs)s")
                        .font(.caption).foregroundStyle(.secondary)
                    Text(proc.command).font(.caption.monospaced()).textSelection(.enabled)
                    if let cwd = proc.cwd {
                        Text(cwd).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    Button("Stop", role: .destructive, action: stop)
                }
                .padding(.leading, 8)
            }
        }
        .padding(.vertical, 3)
    }
}
