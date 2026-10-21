import SwiftUI
import WheelKit
import WheelUI

/// Which `wheel.toml` a field writes: this machine's, or the chosen project's.
enum SettingLayer: String, CaseIterable {
    case global, project
}

/// The Settings window's tabs: the two `wheel.toml` layers, then the devices that can pair.
enum SettingsTab: String, CaseIterable {
    case global, project, devices
}

/// What the Settings window shows: wheel's settings as the daemon resolved them for the project
/// chosen in the Project tab — each carrying this machine's value and that project's — and, per
/// field, what the person typed and not saved and what the daemon refused. The daemon names every
/// key, its help and its default, so nothing here knows any key by name.
@MainActor
final class SettingsModel: ObservableObject {
    /// The window is on screen.
    @Published var open = false
    /// Counts requests to show the window; the debug endpoint is the only sender, standing in for
    /// the app menu's Settings… which a program cannot click.
    @Published var openRequest = 0
    @Published var tab: SettingsTab = .global
    /// The space whose `wheel.toml` the Project tab edits; nil when there are none.
    @Published var project: String?
    /// The rows, in the daemon's order, resolved for `project`. Empty until the first reply.
    @Published private(set) var settings: [SettingInfo] = []
    /// The space `settings` was resolved for.
    @Published private(set) var space: String?
    /// A reply has arrived for the project being asked about.
    @Published private(set) var loaded = false
    /// Why the daemon would not list the settings, shown in place of the rows.
    @Published private(set) var listError: String?
    /// Text typed into a field and not yet saved, by field.
    @Published private(set) var drafts: [Field: String] = [:]
    /// The daemon's reason for refusing a field's last change, until it is typed in again.
    @Published private(set) var refusals: [Field: String] = [:]
    /// Fields whose change is on its way to the daemon.
    @Published private(set) var saving: Set<Field> = []
    /// The `wheel` command's link and wheeld's shell-integration state.
    let commandLine = CommandLineModel()
    /// The Devices tab: how remote clients reach this daemon and the pairing code for one.
    let devices = DevicesModel()

    /// One key in one file: the thing a person types into. A project field names its project, so
    /// what was typed for one project, its refusal and a late answer to its change never land on
    /// another; this machine's fields name none.
    struct Field: Hashable {
        let layer: SettingLayer
        let key: String
        let project: String?

        init(layer: SettingLayer, key: String, project: String?) {
            self.layer = layer
            self.key = key
            self.project = layer == .project ? project : nil
        }
    }

    /// The field for `key` in `layer` on the project the Project tab is on.
    func field(_ layer: SettingLayer, _ key: String) -> Field { Field(layer: layer, key: key, project: project) }

    /// The outstanding `settings_ls`, and each outstanding change by the field it changes.
    var listReq: UInt64?
    var changeReqs: [UInt64: Field] = [:]

    func setting(_ key: String) -> SettingInfo? { settings.first { $0.key == key } }

    /// What the layer's file sets the key to; nil when it sets nothing.
    func stored(_ field: Field) -> String? {
        let setting = setting(field.key)
        return field.layer == .global ? setting?.machine : setting?.project
    }

    /// What a field shows when its file sets nothing: the value the layer below gives.
    func inherited(_ field: Field) -> String {
        guard let setting = setting(field.key) else { return "" }
        return field.layer == .global ? setting.default : setting.global
    }

    /// The text a field shows: what was typed, else what its file sets, else nothing (the field
    /// then shows `inherited` as its placeholder).
    func text(_ field: Field) -> String { drafts[field] ?? stored(field) ?? "" }

    /// The field holds something other than what its file sets.
    func dirty(_ field: Field) -> Bool {
        guard let draft = drafts[field] else { return false }
        return draft != (stored(field) ?? "")
    }

    func type(_ field: Field, _ text: String) {
        // The field writes its text back when it gains or loses focus; that is not typing, and a
        // draft made of it would go stale the moment another client changed the value.
        guard text != self.text(field) else { return }
        // Text that matches what the file sets is no draft, for the same reason: it would hide
        // the next value another client writes.
        drafts[field] = text == (stored(field) ?? "") ? nil : text
        refusals[field] = nil
    }

    /// Esc in a field: what its file sets comes back.
    func revert(_ field: Field) {
        drafts[field] = nil
        refusals[field] = nil
    }

    /// A list resolved for `space` replaces the rows; a field being edited keeps its draft, so a
    /// broadcast from another client never overwrites what a person is typing.
    func show(_ settings: [SettingInfo], for space: String?) {
        self.settings = settings
        self.space = space
        loaded = true
        listError = nil
        for field in drafts.keys where setting(field.key) == nil { drafts[field] = nil }
    }

    func changed(_ field: Field) {
        saving.remove(field)
        drafts[field] = nil
        refusals[field] = nil
    }

    func refused(_ field: Field, _ message: String) {
        saving.remove(field)
        refusals[field] = message
    }

    func sending(_ field: Field) { saving.insert(field) }

    func listFailed(_ message: String) {
        loaded = true
        listError = message
    }

    func asking(for space: String?) {
        if space != self.space { loaded = false }
    }

    /// A new connection: answers to the old one's requests never come.
    func forgetRequests() {
        listReq = nil
        changeReqs = [:]
        saving = []
    }
}

/// The Settings window (wheel-gti.6): a Global tab for this machine's `wheel.toml` and a Project
/// tab, with every project listed down its side, for each project's own. Both draw whatever keys
/// the daemon reports, a section per key prefix.
struct SettingsView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel

    var body: some View {
        TabView(selection: $settings.tab) {
            Tab("Global", systemImage: "globe", value: SettingsTab.global) {
                content(.global) { SettingsForm(model: model, settings: settings, layer: .global) }
                    .padding(.horizontal, 28)
                    .padding(.top, 20)
                    .padding(.bottom, 26)
            }
            Tab("Project", systemImage: "folder", value: SettingsTab.project) {
                if model.connection == .connected, model.spaces.isEmpty {
                    notice(icon: nil, title: "No projects yet.",
                           detail: "A project’s settings live in the wheel.toml at its folder’s root. Open a folder as a space to set them here.")
                        .padding(.horizontal, 28)
                } else {
                    HStack(alignment: .top, spacing: 0) {
                        projectList
                        Divider()
                        content(.project) { SettingsForm(model: model, settings: settings, layer: .project) }
                            .padding(.horizontal, 22)
                            .padding(.top, 18)
                            .padding(.bottom, 22)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    }
                    .frame(minHeight: 300, alignment: .top)
                }
            }
            Tab("Devices", systemImage: "iphone", value: SettingsTab.devices) {
                DevicesSettings(model: model, devices: settings.devices)
                    .padding(.horizontal, 28)
                    .padding(.top, 20)
                    .padding(.bottom, 26)
                    .background(Color(nsColor: .windowBackgroundColor))
            }
        }
        .frame(width: 660)
        .onAppear { model.settingsAppeared() }
        .onDisappear { model.settingsDisappeared() }
    }

    /// The rows, or why there are none: the daemon is out of reach, refused the list, or has not
    /// answered yet.
    @ViewBuilder private func content<F: View>(_ layer: SettingLayer, _ form: () -> F) -> some View {
        if model.connection != .connected {
            notice(icon: "exclamationmark.triangle", title: unreachableText,
                   detail: "Settings live in the daemon; they show here once it answers.")
        } else if let error = settings.listError {
            notice(icon: "exclamationmark.triangle", title: error, detail: nil)
        } else if !settings.loaded {
            notice(icon: nil, title: "Asking wheeld for its settings…", detail: nil)
        } else {
            form()
        }
    }

    private var unreachableText: String {
        if case let .unreachable(reason) = model.connection { return "wheeld unreachable: \(reason). Retrying every 2 seconds." }
        return "Connecting to wheeld…"
    }

    private var projectList: some View {
        List(selection: Binding(get: { settings.project }, set: { if let id = $0 { model.selectSettingsProject(id) } })) {
            Section("Projects") {
                ForEach(model.spaces, id: \.id) { space in
                    VStack(alignment: .leading, spacing: 1) {
                        Text(space.name).font(ChatFont.body)
                        Text(SettingsView.tilde(space.cwd)).font(ChatFont.meta).foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.middle)
                    }
                    .tag(space.id)
                }
            }
        }
        .listStyle(.sidebar)
        .frame(width: 190)
    }

    private func notice(icon: String?, title: String, detail: String?) -> some View {
        VStack(spacing: 6) {
            if let icon { Image(systemName: icon).imageScale(.large).foregroundStyle(.orange) }
            Text(title).font(ChatFont.body)
            if let detail { Text(detail).font(ChatFont.line).foregroundStyle(.secondary) }
        }
        .multilineTextAlignment(.center)
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 20)
        .padding(.vertical, 60)
    }

    /// A path under the home directory as `~/…`.
    static func tilde(_ path: String) -> String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return path == home || path.hasPrefix(home + "/") ? "~" + path.dropFirst(home.count) : path
    }
}

/// Every key the daemon reports for one layer, grouped into a box per key prefix (`browser.` is
/// Browser), in the daemon's order.
private struct SettingsForm: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel
    let layer: SettingLayer

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if layer == .global {
                AgentsSettings(agents: model.agents)
                UsageAgentsSettings(model: model, usage: model.usage)
                CommandLineSettings(commandLine: settings.commandLine)
            }
            ForEach(Self.sections(settings.settings), id: \.name) { section in
                VStack(alignment: .leading, spacing: 6) {
                    Text(section.name).font(ChatFont.body).fontWeight(.semibold)
                    VStack(spacing: 0) {
                        ForEach(Array(section.settings.enumerated()), id: \.element.key) { index, setting in
                            if index > 0 { Divider() }
                            SettingRow(model: model, settings: settings, setting: setting, field: settings.field(layer, setting.key))
                        }
                    }
                    .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
                }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
    }

    struct Section {
        let name: String
        var settings: [SettingInfo]
    }

    /// Keys grouped by the segment before their first dot, sections in the order their first key
    /// arrived.
    static func sections(_ settings: [SettingInfo]) -> [Section] {
        var out: [Section] = []
        for setting in settings {
            let name = label(String(setting.key.split(separator: ".").first ?? Substring(setting.key)))
            if let i = out.firstIndex(where: { $0.name == name }) { out[i].settings.append(setting) } else { out.append(Section(name: name, settings: [setting])) }
        }
        return out
    }

    /// A key segment as a label: `homepage` is Homepage, `font_size` is Font size.
    static func label(_ segment: String) -> String {
        let words = segment.replacingOccurrences(of: "_", with: " ")
        return words.prefix(1).uppercased() + words.dropFirst()
    }
}

/// One key in one layer: its label and field, a clear button when the layer's file sets it, and
/// under a divider what saving will do or why it was refused, the key's help, any problem with
/// the file, and the key itself.
private struct SettingRow: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel
    let setting: SettingInfo
    let field: SettingsModel.Field

    var body: some View {
        let refusal = settings.refusals[field]
        let stored = settings.stored(field)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                Text(SettingsForm.label(String(setting.key.split(separator: ".").last ?? Substring(setting.key))))
                    .font(ChatFont.body)
                    .frame(width: 96, alignment: .leading)
                TextField(setting.key, text: Binding(get: { settings.text(field) }, set: { settings.type(field, $0) }), prompt: Text(""))
                    .labelsHidden()
                    .textFieldStyle(.roundedBorder)
                    .font(ChatFont.body)
                    // What the layer below gives, italic and grey, while this file sets nothing: a
                    // macOS field drops a styled prompt's italics, so it is drawn over the field.
                    .overlay(alignment: .leading) {
                        if settings.text(field).isEmpty {
                            Text(settings.inherited(field))
                                .font(ChatFont.body)
                                .italic()
                                .foregroundStyle(.tertiary)
                                .lineLimit(1)
                                .padding(.leading, 4)
                                .allowsHitTesting(false)
                        }
                    }
                    .disabled(settings.saving.contains(field))
                    .overlay {
                        if refusal != nil { RoundedRectangle(cornerRadius: 5).strokeBorder(.red, lineWidth: 1.5) }
                    }
                    .onSubmit { model.saveSetting(field) }
                    .onExitCommand { settings.revert(field) }
                if stored != nil {
                    Button { model.clearSetting(field) } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                    }
                    .buttonStyle(.borderless)
                    .help(field.layer == .global ? "Use wheel’s default" : "Use the global value")
                    .accessibilityLabel(field.layer == .global ? "Use wheel’s default" : "Use the global value")
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            Divider().padding(.horizontal, 14)
            VStack(alignment: .leading, spacing: 4) {
                if let refusal {
                    Label(refusal, systemImage: "xmark").foregroundStyle(.red)
                } else if settings.dirty(field) {
                    Text("\(settings.text(field).trimmingCharacters(in: .whitespaces).isEmpty ? "Return clears it" : "Return saves") for \(scope) · Esc puts it back")
                        .foregroundStyle(.secondary)
                }
                if let problem {
                    Label(problem, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                }
                Text(help).foregroundStyle(.secondary)
                Text(setting.key).font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
            }
            .font(ChatFont.line)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.top, 8)
            .padding(.bottom, 10)
        }
    }

    /// Whose file a change reaches: every project for the global value, else the project's name.
    private var scope: String {
        guard field.layer == .project else { return "every project" }
        return model.spaces.first { $0.id == settings.project }?.name ?? "this project"
    }

    private var help: String {
        let stored = settings.stored(field)
        switch field.layer {
        case .global: return stored == nil ? "\(setting.help) Not set: wheel’s default." : "\(setting.help) Default: \(setting.default)."
        case .project: return stored == nil ? "\(setting.help) Not set here: the global value." : "\(setting.help) Global: \(setting.global)."
        }
    }

    /// The daemon's problem, on the tab of the file it names: a project's `wheel.toml` sits at its
    /// folder's root, so a problem naming that path is the project's and any other is this
    /// machine's.
    private var problem: String? {
        guard let problem = setting.problem else { return nil }
        let projectFile = model.spaces.first { $0.id == settings.space }.map { "\($0.cwd)/wheel.toml" }
        let isProject = projectFile.map { problem.hasPrefix($0) } ?? false
        return isProject == (field.layer == .project) ? problem : nil
    }
}
