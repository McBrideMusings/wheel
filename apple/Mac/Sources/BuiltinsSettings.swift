import SwiftUI
import WheelKit
import WheelUI

/// Settings › Artifacts: the built-in artifacts wheeld carries, what is installed, and the files an
/// update would change that a person edited.
@MainActor
final class BuiltinsModel: ObservableObject {
    @Published var builtins: [BuiltinInfo] = []
    /// A list has arrived since the tab was last asked for one.
    @Published var loaded = false
    /// Why the last request was refused, until the next one.
    @Published var problem: String?
    /// An update that found edited files and changed nothing, waiting on Keep my edits or Replace.
    @Published var edited: (name: String, files: [EditedFile])?
    /// Requests still unanswered, each with the built-in it is about ("" for a listing).
    var reqs: [UInt64: String] = [:]
}

/// The built-in operations: the tab's buttons, its sheet and the debug endpoint all call these.
extension AppModel {
    private func sendBuiltin(_ name: String, _ what: String, _ make: (UInt64) -> ClientMsg) {
        guard connection == .connected else {
            builtinsModel.problem = "Not connected to wheeld."
            return
        }
        let req = client.req()
        builtinsModel.reqs[req] = name
        builtinsModel.problem = nil
        Log.info("built-in \(what)")
        client.send(make(req))
    }

    func askBuiltins() { sendBuiltin("", "ls") { .builtinLs(req: $0) } }

    func installBuiltin(_ name: String) { sendBuiltin(name, "install \(name)") { .builtinInstall(req: $0, name: name) } }

    func updateBuiltin(_ name: String, onEdited: OnEdited = .ask) {
        builtinsModel.edited = nil
        sendBuiltin(name, "update \(name) (\(onEdited.rawValue))") { .builtinUpdate(req: $0, name: name, onEdited: onEdited) }
    }

    /// The tab asks first: the folder goes, and with it any edit a person made there.
    func removeBuiltin(_ name: String) { sendBuiltin(name, "remove \(name)") { .builtinRemove(req: $0, name: name) } }

    /// A reply about built-ins; false when it answers none of this model's requests.
    func receivedBuiltins(_ msg: ServerMsg) -> Bool {
        switch msg {
        case let .builtins(req, builtins):
            guard builtinsModel.reqs.removeValue(forKey: req) != nil else { return false }
            builtinsModel.builtins = builtins
            builtinsModel.loaded = true
            Log.info("built-ins: \(builtins.map { "\($0.name) shipped \($0.shippedVersion) installed \($0.installedVersion.map(String.init) ?? "-") edited \($0.editedFiles.count)" }.joined(separator: ", "))")
            return true
        case let .builtinEdited(req, name, files):
            guard builtinsModel.reqs.removeValue(forKey: req) != nil else { return false }
            builtinsModel.edited = (name, files)
            Log.info("built-in \(name): update stopped on edited files \(files.map(\.path).joined(separator: ", "))")
            return true
        case let .error(req?, _, message):
            guard let name = builtinsModel.reqs.removeValue(forKey: req) else { return false }
            builtinsModel.problem = message
            builtinsModel.loaded = true
            Log.info("built-in \(name.isEmpty ? "ls" : name) refused: \(message)")
            return true
        default:
            return false
        }
    }
}

/// The tab: one row per built-in with its versions and its one action, and Remove when it is
/// installed.
struct BuiltinsSettings: View {
    @ObservedObject var model: AppModel
    @ObservedObject var builtins: BuiltinsModel
    /// The built-in whose Remove is waiting on a yes.
    @State private var removing: BuiltinInfo?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Pages Wheel ships, installed for every project on this Mac into its artifacts folder. Installed pages are plain files you can edit; an update asks before it changes an edited one.")
                .font(ChatFont.line)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let problem = builtins.problem {
                Label(problem, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).font(ChatFont.line)
            }
            if !builtins.loaded {
                Text("Asking wheeld for its built-ins…").font(ChatFont.line).foregroundStyle(.secondary)
            }
            VStack(spacing: 0) {
                ForEach(Array(builtins.builtins.enumerated()), id: \.element.id) { index, builtin in
                    if index > 0 { Divider() }
                    row(builtin)
                }
            }
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
        }
        .onAppear { model.askBuiltins() }
        .alert("Remove “\(removing?.name ?? "")”?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
               presenting: removing) { builtin in
            Button("Remove", role: .destructive) { model.removeBuiltin(builtin.name) }
                .keyboardShortcut(.defaultAction)
            Button("Cancel", role: .cancel) {}
        } message: { builtin in
            Text(builtin.editedFiles.isEmpty
                ? "Its folder and its entry go. Panes showing it show nothing until it is installed again."
                : "Its folder goes, with your edits to \(builtin.editedFiles.joined(separator: ", ")).")
        }
        .sheet(isPresented: Binding(get: { builtins.edited != nil }, set: { if !$0 { builtins.edited = nil } })) {
            if let edited = builtins.edited {
                EditedFilesSheet(model: model, name: edited.name, files: edited.files)
            }
        }
    }

    private func row(_ builtin: BuiltinInfo) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(builtin.name).font(ChatFont.body).fontWeight(.semibold)
                Text(versions(builtin)).font(ChatFont.meta).foregroundStyle(.secondary)
                if !builtin.editedFiles.isEmpty {
                    Text("Edited: \(builtin.editedFiles.joined(separator: ", "))").font(ChatFont.meta).foregroundStyle(.orange)
                }
            }
            Spacer(minLength: 8)
            switch builtin.action {
            case .install: Button("Install") { model.installBuiltin(builtin.name) }
            case .update: Button("Update") { model.updateBuiltin(builtin.name) }
            case .restore: Button("Restore…") { model.updateBuiltin(builtin.name) }
            case .upToDate: EmptyView()
            }
            if builtin.installedVersion != nil {
                Button("Remove") { removing = builtin }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private func versions(_ builtin: BuiltinInfo) -> String {
        switch builtin.installedVersion {
        case nil: "Version \(builtin.shippedVersion) · not installed"
        case let v? where v < builtin.shippedVersion: "Version \(v) installed · \(builtin.shippedVersion) available"
        case let v?: "Version \(v) installed"
        }
    }
}

/// The files an update would change that a person edited, each with its diff, and the choice.
struct EditedFilesSheet: View {
    @ObservedObject var model: AppModel
    let name: String
    let files: [EditedFile]

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("You edited \(files.count == 1 ? "a file" : "\(files.count) files") in “\(name)”").font(.headline)
            Text("The update changes \(files.count == 1 ? "it" : "them") as shown. Keep your edits, or replace them with the new version.")
                .foregroundStyle(.secondary)
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(files) { file in
                        Text(file.path).font(ChatFont.body).fontWeight(.semibold)
                        Text(file.diff)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(8)
                            .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
                    }
                }
            }
            .frame(minHeight: 160, maxHeight: 360)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { model.builtinsModel.edited = nil }
                    .keyboardShortcut(.cancelAction)
                Button("Keep My Edits") { model.updateBuiltin(name, onEdited: .keep) }
                Button("Replace") { model.updateBuiltin(name, onEdited: .replace) }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 560)
    }
}
