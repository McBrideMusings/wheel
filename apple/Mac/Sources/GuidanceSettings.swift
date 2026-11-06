import SwiftUI
import WheelKit
import WheelUI

/// One piece of what an agent is given, in the order it arrives: Wheel's own text, the person's
/// text for every agent, then the text for the previewed agent.
struct GuidanceSegment: Identifiable, Equatable {
    /// `wheel`, `all`, or the agent's name.
    let id: String
    let title: String
    /// What this piece adds to the text, heading included; empty when the file adds nothing.
    let text: String
}

/// What Settings › Guidance shows: the text the server gives an agent and the person's own
/// guidance files, as the server last listed them. The server reads and makes the files; this
/// holds only what it answered (ADR-0020).
@MainActor
final class GuidanceModel: ObservableObject {
    /// Wheel's own text, the same for every agent; nil until the server answers.
    @Published private(set) var brief: String?
    /// The text `previewAgent` is given, as the server composed it (brief, then the files).
    @Published private(set) var given = ""
    /// The file for every agent first, then one per agent.
    @Published private(set) var files: [GuidanceFile] = []
    /// Why the server would not list or make a file, shown above the sections.
    @Published private(set) var error: String?
    /// The agent whose composed text the right column shows.
    @Published var previewAgent = "claude"
    /// The segment the left column last pointed at; the right column scrolls to it.
    @Published var focus: String?

    var loaded: Bool { brief != nil }

    var agentFiles: [GuidanceFile] { files.filter { $0.agent != nil } }

    func received(brief: String, given: String, files: [GuidanceFile]) {
        self.brief = brief
        self.given = given
        self.files = files
        if !files.contains(where: { $0.agent == previewAgent }), let first = files.first(where: { $0.agent != nil })?.agent {
            previewAgent = first
        }
        error = nil
    }

    func failed(_ message: String) { error = message }

    /// The composed text cut at the files' boundaries, so each document stays visible in the
    /// whole. The server's text is what is cut: a file's piece is `\n<heading>\n<text>\n`, and a
    /// blank file has none.
    var segments: [GuidanceSegment] {
        guard let brief else { return [] }
        var rest = given.hasPrefix(brief) ? Substring(given.dropFirst(brief.count)) : Substring(given)
        var out = [GuidanceSegment(id: "wheel", title: "Wheel’s guidance", text: given.hasPrefix(brief) ? brief : given)]
        for file in files where file.agent == nil || file.agent == previewAgent {
            let blank = file.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            let length = blank ? 0 : "\n\(file.heading)\n\(file.text)\n".count
            let piece = String(rest.prefix(length))
            rest = rest.dropFirst(length)
            out.append(GuidanceSegment(
                id: file.agent ?? "all",
                title: file.agent == nil ? "Your guidance for all agents" : "Your guidance for \(previewAgent)",
                text: piece))
        }
        return out
    }
}

extension AppModel {
    /// Ask the server for the guidance text and files. Run when the tab appears, when the app
    /// comes back to the front (the person edits a file in another app) and when the previewed
    /// agent changes.
    func askGuidance() {
        guard connection == .connected, serverProtocol >= ServerProtocol.current else { return }
        let req = client.req()
        guidanceReq = req
        client.send(.guidanceLs(req: req, agent: settings.guidance.previewAgent))
    }

    /// Preview `agent`'s composed text on the right.
    func previewGuidance(_ agent: String) {
        guard settings.guidance.previewAgent != agent else { return }
        settings.guidance.previewAgent = agent
        askGuidance()
    }

    /// Open in Editor: the server makes the file when it is missing and says where it is, then the
    /// default text editor opens it. `agent` nil is the file for every agent.
    func openGuidanceFile(_ agent: String?, launch: Bool = true) {
        guard connection == .connected, serverProtocol >= ServerProtocol.current else { return }
        let req = client.req()
        guidanceOpenReqs[req] = launch
        Log.info("guidance: open the file for \(agent ?? "every agent")")
        client.send(.guidancePath(req: req, agent: agent))
    }
}

/// Settings › Guidance in two columns: on the left one row per document (Wheel's text, the file
/// for every agent, a file per agent) with its path and an Open in Editor button; on the right
/// the text one agent is given, all of it together, read-only, each document in its own block.
/// Wheel carries the text as written and enforces nothing.
struct GuidanceSettings: View {
    var model: AppModel
    @ObservedObject var guidance: GuidanceModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if model.connection != .connected {
                notice("Not connected to wheeld.", "Guidance lives in files the server reads; it shows here once the server answers.")
            } else if model.serverProtocol < ServerProtocol.current {
                VStack(alignment: .leading, spacing: 8) {
                    Label(AppModel.serverUpdatedText, systemImage: "arrow.triangle.2.circlepath").foregroundStyle(.orange)
                    if model.serverControl?.unavailable == nil { ServerRestartControl(model: model) }
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
            } else {
                Text("Guidance is text Wheel gives every agent when its pane starts. Wheel carries it as you wrote it and enforces nothing. An agent keeps the guidance it started with; a pane started or restarted after an edit reads the new text.")
                    .font(ChatFont.line).foregroundStyle(.secondary)
                if let error = guidance.error {
                    Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).font(ChatFont.line)
                }
                if !guidance.loaded && guidance.error == nil {
                    Text("Asking wheeld for its guidance…").foregroundStyle(.secondary)
                } else if guidance.loaded {
                    HStack(alignment: .top, spacing: 18) {
                        documents.frame(width: 250, alignment: .topLeading)
                        preview.frame(maxWidth: .infinity, alignment: .topLeading)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .onAppear { model.askGuidance() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in model.askGuidance() }
    }

    // MARK: Left column: the documents

    private var documents: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Documents").font(ChatFont.body).fontWeight(.semibold)
            row(id: "wheel", title: "Wheel’s guidance", meta: "Ships with Wheel. Read-only.", path: nil, agent: nil, editable: false)
            if let all = guidance.files.first(where: { $0.agent == nil }) {
                row(id: "all", title: "Your guidance for all agents", meta: summary(all), path: all.path, agent: nil, editable: true)
            }
            Text("Per agent").font(ChatFont.line).foregroundStyle(.secondary).padding(.top, 6)
            ForEach(guidance.agentFiles) { file in
                let name = file.agent ?? ""
                row(id: name, title: name, meta: summary(file), path: file.path, agent: name, editable: true)
            }
        }
    }

    private func summary(_ file: GuidanceFile) -> String {
        let lines = file.text.split(separator: "\n", omittingEmptySubsequences: false).count
        return file.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "Empty. Adds nothing." : "\(lines) \(lines == 1 ? "line" : "lines")"
    }

    private func row(id: String, title: String, meta: String, path: String?, agent: String?, editable: Bool) -> some View {
        let selected = guidance.focus == id || (agent != nil && agent == guidance.previewAgent && guidance.focus == nil)
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle().fill(tint(id)).frame(width: 8, height: 8)
                if agent != nil { UsageIcon(agent: id) }
                Text(title).font(ChatFont.body).fontWeight(.medium)
                Spacer(minLength: 4)
                if editable { Button("Open in Editor") { model.openGuidanceFile(agent) }.controlSize(.small) }
            }
            Text(meta).font(ChatFont.line).foregroundStyle(.secondary)
            if let path {
                Text(SettingsView.tilde(path)).font(ChatFont.meta).foregroundStyle(.secondary)
                    .lineLimit(1).truncationMode(.middle)
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(selected ? Color.accentColor.opacity(0.10) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(selected ? Color.accentColor.opacity(0.6) : Color(nsColor: .separatorColor), lineWidth: selected ? 1 : 0.5))
        .contentShape(Rectangle())
        .onTapGesture {
            if let agent { model.previewGuidance(agent) }
            guidance.focus = id
        }
    }

    // MARK: Right column: what the agent is given

    private var preview: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("What").font(ChatFont.body).fontWeight(.semibold)
                HStack(spacing: 5) {
                    UsageIcon(agent: guidance.previewAgent)
                    Text(guidance.previewAgent).font(ChatFont.body).fontWeight(.semibold)
                }
                .padding(.horizontal, 8).padding(.vertical, 2)
                .background(tint(guidance.previewAgent).opacity(0.18), in: Capsule())
                Text("is given").font(ChatFont.body).fontWeight(.semibold)
            }
            Text("Read-only: Wheel’s guidance, then yours for every agent, then yours for this agent. Press an agent on the left to see its text. The pane’s artifacts and seat line are added when the pane starts.")
                .font(ChatFont.line).foregroundStyle(.secondary)
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(guidance.segments) { segment in block(segment).id(segment.id) }
                    }
                    .padding(.vertical, 2)
                }
                .frame(height: 560)
                .onChange(of: guidance.focus) { _, id in
                    guard let id, guidance.segments.contains(where: { $0.id == id }) else { return }
                    withAnimation { proxy.scrollTo(id, anchor: .top) }
                }
            }
        }
    }

    private func block(_ segment: GuidanceSegment) -> some View {
        let empty = segment.text.isEmpty
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle().fill(tint(segment.id)).frame(width: 8, height: 8)
                Text(segment.title).font(ChatFont.meta).foregroundStyle(.secondary)
            }
            Text(empty ? "Empty. This file adds nothing." : segment.text)
                .font(empty ? ChatFont.line : .system(size: 11, design: .monospaced))
                .foregroundStyle(empty ? .secondary : .primary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(8)
        .background(tint(segment.id).opacity(0.10), in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(guidance.focus == segment.id ? tint(segment.id) : Color(nsColor: .separatorColor), lineWidth: guidance.focus == segment.id ? 1.5 : 0.5))
    }

    /// One colour per document, shared by its row on the left and its block on the right: grey for
    /// Wheel's, blue for every agent, green for the agent.
    private func tint(_ id: String) -> Color {
        switch id {
        case "wheel": return .gray
        case "all": return .blue
        default: return .green
        }
    }

    private func notice(_ title: String, _ detail: String) -> some View {
        VStack(spacing: 6) {
            Text(title).font(ChatFont.body)
            Text(detail).font(ChatFont.line).foregroundStyle(.secondary)
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 60)
    }
}
