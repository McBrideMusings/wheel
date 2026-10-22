import SwiftUI
import WheelKit

/// The ⋯ in the top bar: what the Mac's pane header offers for the page on screen. Each item asks
/// the daemon; the layout it broadcasts back is what changes the screen, and a refusal shows as an alert.
struct PaneMenu: View {
    let model: PhoneModel
    let pane: String
    let kind: PaneKind
    @State private var confirmingClose = false
    @State private var confirmingKind: (() -> Void)?
    @State private var pickingArtifact = false
    @State private var askingForAddress = false
    @State private var address = ""

    var body: some View {
        Menu {
            if case .unknown = kind {} else {
                Button("Split Right", systemImage: "square.split.2x1") { model.split(pane, .right) }
                Button("Split Down", systemImage: "square.split.1x2") { model.split(pane, .down) }
            }
            if let view = model.otherView(kind) {
                Button(view == .agent ? "Show as Chat" : "Show in Terminal",
                       systemImage: view == .agent ? "bubble.left.and.text.bubble.right" : "terminal") {
                    model.switchView(pane, to: view)
                }
            }
            Menu("Change kind", systemImage: "arrow.triangle.2.circlepath") {
                Button("Empty", systemImage: "square.dashed") { changeKind { model.setKind(pane, .empty) } }
                Button("Terminal", systemImage: "terminal") { changeKind { model.setKind(pane, .terminal) } }
                ForEach(AgentInfo.pickable(model.agents), id: \.name) { agent in
                    Button(agent.name.capitalized, systemImage: "bubble.left.and.text.bubble.right") {
                        changeKind { model.startPane(pane, .agent, agent: agent.name) }
                    }
                }
                Button("Artifact…", systemImage: "doc.text") {
                    model.listArtifacts()
                    pickingArtifact = true
                }
                Button("Browser…", systemImage: "safari") { askingForAddress = true }
            }
            Divider()
            Button("Close", systemImage: "xmark", role: .destructive) {
                if kind.endsASession { confirmingClose = true } else { model.closePane(pane) }
            }
        } label: {
            Image(systemName: "ellipsis.circle").font(.title3)
        }
        .accessibilityLabel("Pane menu")
        .confirmationDialog("Close this pane?", isPresented: $confirmingClose) {
            Button("Close Pane", role: .destructive) { model.closePane(pane) }
        } message: {
            Text(endsText)
        }
        .confirmationDialog("Change this pane?", isPresented: Binding(get: { confirmingKind != nil }, set: { if !$0 { confirmingKind = nil } })) {
            Button("Change Kind", role: .destructive) { confirmingKind?(); confirmingKind = nil }
        } message: {
            Text(endsText)
        }
        .sheet(isPresented: $pickingArtifact) {
            ArtifactPicker(model: model) { info in
                pickingArtifact = false
                changeKind { model.setKind(pane, .web, artifact: info) }
            }
        }
        .alert("Open a page", isPresented: $askingForAddress) {
            TextField("Address", text: $address)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.URL)
            Button("Open") {
                let url = address.trimmingCharacters(in: .whitespaces)
                address = ""
                changeKind { model.openBrowser(pane, url) }
            }
            Button("Cancel", role: .cancel) { address = "" }
        }
    }

    /// What closing or changing this pane ends, as the Mac's close button words it.
    private var endsText: String {
        switch kind {
        case .terminal: "The shell in this pane is killed."
        case .agent: "This agent's session ends."
        default: ""
        }
    }

    /// Runs `change`, asking first when the pane runs a shell or an agent.
    private func changeKind(_ change: @escaping () -> Void) {
        if kind.endsASession { confirmingKind = change } else { change() }
    }

    /// Runs one menu item without the confirmation a finger would answer, for the debug launch
    /// variable `WHEEL_MENU`: `split-right`, `split-down`, `close`, `view-chat`, `view-terminal`,
    /// `kind-empty`, `kind-terminal`, `kind-agent:<name>`, `kind-web:<artifact name>`,
    /// `kind-browser[:<url>]`.
    @MainActor static func drive(_ item: String, model: PhoneModel, pane: String, kind: PaneKind) async {
        Log.info("menu item \(item) on \(pane)")
        let parts = item.split(separator: ":", maxSplits: 1).map(String.init)
        let arg = parts.count > 1 ? parts[1] : nil
        switch parts.first ?? "" {
        case "split-right": model.split(pane, .right)
        case "split-down": model.split(pane, .down)
        case "close": model.closePane(pane)
        case "view-chat": model.switchView(pane, to: .agent)
        case "view-terminal": model.switchView(pane, to: .terminal)
        case "kind-empty": model.setKind(pane, .empty)
        case "kind-terminal": model.setKind(pane, .terminal)
        case "kind-agent": model.startPane(pane, .agent, agent: arg)
        case "kind-browser": model.openBrowser(pane, arg ?? "")
        case "kind-web":
            model.listArtifacts()
            for _ in 0 ..< 20 where model.artifacts == nil { try? await Task.sleep(for: .milliseconds(250)) }
            if let info = model.artifacts?.first(where: { $0.name == arg }) {
                model.setKind(pane, .web, artifact: info)
            } else {
                Log.info("menu: no artifact named \(arg ?? "-")")
            }
        default: Log.info("menu: unknown item \(item)")
        }
    }
}

/// The artifacts a pane in this space may show, project first then machine.
private struct ArtifactPicker: View {
    let model: PhoneModel
    let pick: (ArtifactInfo) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Group {
                if let list = model.artifacts {
                    if list.isEmpty {
                        ContentUnavailableView("No artifacts for this space", systemImage: "doc.text",
                                               description: Text(ArtifactPalette.emptyHelp))
                    } else {
                        List(list) { info in
                            Button { pick(info) } label: {
                                VStack(alignment: .leading) {
                                    Text(info.title)
                                    Text("\(info.scope.rawValue) · \(info.name)").font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                } else {
                    ProgressView()
                }
            }
            .navigationTitle("Artifact")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }
    }
}
