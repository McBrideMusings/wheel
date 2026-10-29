import SwiftUI
import WheelKit
import WheelUI

/// Which `wheel.toml` a field writes: this machine's, or the chosen project's.
enum SettingLayer: String, CaseIterable {
    case global, project
}

/// The Settings window's toolbar tabs: a page per topic of this machine's `wheel.toml`, the
/// projects' own, then the devices that can pair.
enum SettingsTab: String, CaseIterable {
    case general, agents, usage, keys, chat, terminal, artifacts, project, devices
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
    @Published var tab: SettingsTab = .general
    /// The topic the Project tab shows for the chosen project.
    @Published var projectTopic: SettingTopic = .general
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
    /// Whether wheeld's log lines reach the store at `logs.push_url`, shown under that field; nil
    /// until wheeld reports it.
    @Published var logShipping: LogShipping?
    /// Which guarded folders wheeld can list, as its last `status` said; nil until it says.
    @Published var hostAccess: HostAccess?
    /// A Chat or Terminal page's search text, the file it edits, and whether it prints that file.
    struct Page {
        var query = ""
        var layer: SettingLayer = .global
        var showToml = false
    }
    @Published var pages: [SettingTopic: Page] = [:]
    func page(_ topic: SettingTopic) -> Page { pages[topic] ?? Page() }
    func pageBinding<V>(_ topic: SettingTopic, _ path: WritableKeyPath<Page, V>) -> Binding<V> {
        Binding(get: { self.page(topic)[keyPath: path] }, set: { self.pages[topic, default: Page()][keyPath: path] = $0 })
    }
    /// The key whose row the shown page scrolls to; set by the debug endpoint.
    @Published var scrollTarget: String?
    /// The home folders wheeld found for each agent, by agent name.
    @Published private(set) var homeLists: [String: [AgentHome]] = [:]
    /// What wheeld read of the folder a home field names, by field.
    @Published private(set) var homeChecks: [Field: AgentHome] = [:]
    /// The outstanding `agent_homes` requests, each with the field and the text it asked about.
    var homeReqs: [UInt64: (field: Field, text: String)] = [:]
    /// The outstanding `status` asked for `logShipping`.
    var statusReq: UInt64?
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

    /// The agent a home key belongs to (`agent.claude.home` is claude's); nil for any other key.
    static func homeAgent(_ key: String) -> String? {
        guard key.hasPrefix("agent."), key.hasSuffix(".home") else { return nil }
        return String(key.dropFirst("agent.".count).dropLast(".home".count))
    }

    /// An `agent_homes` answer: the list for the agent, and the reading of the folder the field
    /// held when asked, kept only while the field still holds that text.
    func homesListed(_ ask: (field: Field, text: String), agent: String, homes: [AgentHome], checked: AgentHome?) {
        homeLists[agent] = homes
        if text(ask.field) == ask.text { homeChecks[ask.field] = checked }
    }

    func setting(_ key: String) -> SettingInfo? { settings.first { $0.key == key } }

    /// The setting `key` as `layer`'s page draws it; nil for a key the daemon keeps to this machine
    /// when the layer is a project's.
    func setting(_ key: String, in layer: SettingLayer) -> SettingInfo? {
        setting(key).flatMap { layer == .project && $0.machineOnly ? nil : $0 }
    }

    /// The keys `layer`'s page draws for `topic`.
    func settings(in layer: SettingLayer, topic: SettingTopic) -> [SettingInfo] {
        topic.filter(settings).filter { layer == .global || !$0.machineOnly }
    }

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
        statusReq = nil
        changeReqs = [:]
        homeReqs = [:]
        saving = []
    }
}

/// The Settings window (wheel-gti.6): a toolbar tab per topic (General, Agents, Usage) for this
/// machine's `wheel.toml`, a Project tab, with every project listed down its side and a topic
/// picker, for each project's own, and the Devices tab. The topic tabs draw whatever keys the
/// daemon reports for the topic, a section per key prefix.
struct SettingsView: View {
    var model: AppModel
    @ObservedObject var settings: SettingsModel
    /// The shown page's content height, before it is capped.
    @State private var pageHeight: CGFloat = 0

    var body: some View {
        TabView(selection: $settings.tab) {
            Tab(SettingTopic.general.title, systemImage: SettingTopic.general.symbol, value: SettingsTab.general) {
                topicPage(.general)
            }
            Tab(SettingTopic.agents.title, systemImage: SettingTopic.agents.symbol, value: SettingsTab.agents) {
                topicPage(.agents)
            }
            Tab(SettingTopic.usage.title, systemImage: SettingTopic.usage.symbol, value: SettingsTab.usage) {
                topicPage(.usage)
            }
            Tab(SettingTopic.keys.title, systemImage: SettingTopic.keys.symbol, value: SettingsTab.keys) {
                topicPage(.keys)
            }
            Tab(SettingTopic.chat.title, systemImage: SettingTopic.chat.symbol, value: SettingsTab.chat) {
                scrolled {
                    content(.global) { TopicSettings(topic: .chat, model: model, settings: settings) }
                        .padding(.horizontal, 28)
                        .padding(.top, 20)
                        .padding(.bottom, 26)
                }
            }
            Tab(SettingTopic.terminal.title, systemImage: SettingTopic.terminal.symbol, value: SettingsTab.terminal) {
                scrolled {
                    content(.global) { TopicSettings(topic: .terminal, model: model, settings: settings) }
                        .padding(.horizontal, 28)
                        .padding(.top, 20)
                        .padding(.bottom, 26)
                }
            }
            Tab("Artifacts", systemImage: "square.stack", value: SettingsTab.artifacts) {
                scrolled {
                    content(.global) { BuiltinsSettings(model: model, builtins: model.builtinsModel) }
                        .padding(.horizontal, 28)
                        .padding(.top, 20)
                        .padding(.bottom, 26)
                }
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
                        VStack(alignment: .leading, spacing: 14) {
                            Picker("Topic", selection: $settings.projectTopic) {
                                ForEach(SettingTopic.allCases.filter { $0 != .chat && $0 != .terminal && !settings.settings(in: .project, topic: $0).isEmpty }) { Text($0.title).tag($0) }
                            }
                            .pickerStyle(.segmented)
                            .labelsHidden()
                            scrolled { content(.project) { SettingsForm(model: model, settings: settings, layer: .project, topic: settings.projectTopic) } }
                        }
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

    /// One topic of this machine's settings as a page of its own.
    private func topicPage(_ topic: SettingTopic) -> some View {
        scrolled {
            VStack(alignment: .leading, spacing: 18) {
                if topic == .general { WindowsSettings(model: model) }
                content(.global) { SettingsForm(model: model, settings: settings, layer: .global, topic: topic) }
            }
                .padding(.horizontal, 28)
                .padding(.top, 20)
                .padding(.bottom, 26)
        }
    }

    /// The page at its own height up to `maxPageHeight`, scrolling beyond it, so a long page never
    /// makes the window taller than the screen.
    private func scrolled<V: View>(@ViewBuilder _ page: () -> V) -> some View {
        let page = page()
        return ScrollViewReader { proxy in
            ScrollView {
                page.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { pageHeight = $0 }
            }
            .onChange(of: settings.scrollTarget) { _, key in
                guard let key else { return }
                proxy.scrollTo(key, anchor: .bottom)
                settings.scrollTarget = nil
            }
        }
        .frame(height: min(max(pageHeight, 1), SettingsView.maxPageHeight))
    }

    private static let maxPageHeight: CGFloat = 620

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
            VStack(alignment: .leading, spacing: 12) {
                if let file = layerFile(layer) {
                    Text("Saved in \(SettingsView.tilde(file))")
                        .font(ChatFont.meta).foregroundStyle(.secondary)
                        .lineLimit(1).truncationMode(.middle)
                        .textSelection(.enabled)
                }
                form()
            }
        }
    }

    /// The `wheel.toml` a layer's fields are written to, as the daemon reported it.
    private func layerFile(_ layer: SettingLayer) -> String? {
        switch layer {
        case .global: settings.settings.first.map(\.machineFile).flatMap { $0.isEmpty ? nil : $0 }
        case .project: settings.settings.first?.projectFile
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

/// The keys the daemon reports for one layer that belong to one topic, grouped into a box per key
/// prefix (`browser.` is Browser), in the daemon's order. This machine's page also carries the
/// topic's rows that are not settings: the command line link, the agents found and the usage
/// agents.
private struct SettingsForm: View {
    var model: AppModel
    @ObservedObject var settings: SettingsModel
    let layer: SettingLayer
    let topic: SettingTopic

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if layer == .global {
                switch topic {
                case .general:
                    CommandLineSettings(commandLine: settings.commandLine)
                    FolderAccessSettings(host: settings.hostAccess)
                    StatusDotsSettings(model: model)
                case .agents: AgentsSettings(agents: model.agents)
                case .usage: UsageAgentsSettings(model: model, usage: model.usage)
                case .keys, .chat, .terminal: EmptyView()
                }
            }
            ForEach(SettingSection.grouping(settings.settings(in: layer, topic: topic).filter { $0.key != "status.dots" }), id: \.name) { section in
                VStack(alignment: .leading, spacing: 6) {
                    Text(section.name).font(ChatFont.body).fontWeight(.semibold)
                    VStack(spacing: 0) {
                        ForEach(Array(section.settings.enumerated()), id: \.element.key) { index, setting in
                            if index > 0 { Divider() }
                            VStack(spacing: 0) {
                                SettingRow(model: model, settings: settings, setting: setting, field: settings.field(layer, setting.key))
                                if layer == .global, setting.key == LogShippingStatus.key {
                                    Divider()
                                    LogShippingStatus(logs: settings.logShipping)
                                }
                            }
                            .id(setting.key)
                        }
                    }
                    .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
                }
            }
            if layer == .global, topic == .keys { KeymapTable(model: model) }
        }
        .fixedSize(horizontal: false, vertical: true)
    }
}

/// Under the `keybind` field: what the setting did not apply, then every action with the key that
/// runs it now, marked where the setting changed it from the built-in one.
private struct KeymapTable: View {
    var model: AppModel

    var body: some View {
        let entries = CommandActions.all(model)
        VStack(alignment: .leading, spacing: 6) {
            ForEach(model.keymap.problems, id: \.self) { problem in
                Label(problem, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).font(ChatFont.line)
            }
            Text("Actions").font(ChatFont.body).fontWeight(.semibold)
            VStack(spacing: 0) {
                ForEach(Array(entries.enumerated()), id: \.element.id) { index, entry in
                    if index > 0 { Divider() }
                    let chord = model.keymap.chords[entry.id]
                    HStack(spacing: 12) {
                        Text(entry.title).font(ChatFont.body)
                        Spacer(minLength: 8)
                        if chord != entry.chord {
                            Text("set").font(ChatFont.meta).foregroundStyle(.secondary)
                        }
                        Text(chord?.symbol ?? "—")
                            .font(.body.monospaced())
                            .foregroundStyle(chord == nil ? .tertiary : .primary)
                            .frame(minWidth: 64, alignment: .trailing)
                        Text(entry.id)
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .frame(width: 150, alignment: .leading)
                            .textSelection(.enabled)
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 6)
                }
            }
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
        }
    }
}

/// The symbol and colour for what a home folder holds, and what it means for the agent.
private func homeLook(_ state: HomeState) -> (symbol: String, color: Color, meaning: String) {
    switch state {
    case .valid: ("checkmark.circle.fill", .green, "Holds this agent’s data")
    case .empty: ("exclamationmark.circle.fill", .orange, "An empty folder: the agent starts a new login here")
    case .missing: ("xmark.circle.fill", .red, "No such folder yet: the agent creates it on first start")
    case .notFolder: ("xmark.circle.fill", .red, "This is a file, not a folder")
    }
}

/// At the end of a home field: whether the folder it names holds the agent's data.
private struct HomeMark: View {
    let home: AgentHome?

    var body: some View {
        if let home {
            let look = homeLook(home.state)
            Image(systemName: look.symbol).foregroundStyle(look.color).help(look.meaning).accessibilityLabel(look.meaning)
        }
    }
}

/// Beside a home field: the folders wheeld found for the agent, each with its mark; choosing one
/// writes it.
private struct HomeMenu: View {
    let homes: [AgentHome]
    let pick: (String) -> Void

    var body: some View {
        Menu {
            ForEach(homes, id: \.path) { home in
                Button { pick(home.path) } label: {
                    Label(SettingsView.tilde(home.path), systemImage: homeLook(home.state).symbol)
                }
            }
        } label: {
            Image(systemName: "chevron.up.chevron.down")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .disabled(homes.isEmpty)
        .help("Folders found in your home folder")
        .accessibilityLabel("Detected folders")
    }
}

/// One key in one layer: its label and field, a clear button when the layer's file sets it, and
/// under a divider what saving will do or why it was refused, the key's help, any problem with
/// the file, and the key itself.
private struct SettingRow: View {
    var model: AppModel
    @ObservedObject var settings: SettingsModel
    let setting: SettingInfo
    let field: SettingsModel.Field

    var body: some View {
        let refusal = settings.refusals[field]
        let stored = settings.stored(field)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                Text(SettingSection.rowLabel(setting.key))
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
                if let agent = SettingsModel.homeAgent(setting.key) {
                    HomeMark(home: settings.homeChecks[field])
                    HomeMenu(homes: settings.homeLists[agent] ?? []) { path in
                        settings.type(field, path)
                        model.saveSetting(field)
                    }
                }
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
            // Asks wheeld for the agent's folders and what the field's folder holds, when the row
            // appears and each time the text changes.
            .task(id: settings.text(field)) { model.askHomes(field) }
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
        case .global:
            if stored == nil { return "\(setting.help) Not set: wheel’s default." }
            return setting.default.isEmpty ? "\(setting.help) Default: none." : "\(setting.help) Default: \(setting.default)."
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

/// Under the push URL on this machine's page: whether wheeld's log lines reach the store — off,
/// waiting for a first push, arriving with the last success time, or failing with the error and
/// what waits on this machine.
struct LogShippingStatus: View {
    static let key = "logs.push_url"
    let logs: LogShipping?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "circle.fill").imageScale(.small).foregroundStyle(color)
            VStack(alignment: .leading, spacing: 2) {
                Text(Self.title(logs)).font(ChatFont.body)
                if let detail = Self.detail(logs) { Text(detail).font(ChatFont.line).foregroundStyle(.secondary) }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }

    private var color: Color {
        switch logs?.state {
        case "ok": .green
        case "failing": .red
        case "waiting": .orange
        default: .secondary
        }
    }

    static func title(_ logs: LogShipping?) -> String {
        guard let logs else { return "Asking wheeld whether its log lines are shipping…" }
        switch logs.state {
        case "off": return "Off: the log, the usage journal and the apps’ lines stay on this Mac."
        case "waiting": return "Waiting: nothing has been pushed to this URL yet."
        case "ok": return "Arriving: last accepted \(logs.lastOkMs.map(Self.time) ?? "just now")."
        case "failing": return "Failing: \(logs.error ?? "the last push did not go through")"
        default: return logs.state
        }
    }

    static func detail(_ logs: LogShipping?) -> String? {
        guard let logs, logs.state != "off" else { return nil }
        let waiting = ByteCountFormatter.string(fromByteCount: Int64(clamping: logs.waitingBytes), countStyle: .file)
        return logs.waitingBytes == 0 ? "Nothing waiting on this Mac." : "\(waiting) waiting on this Mac."
    }

    private static func time(_ ms: UInt64) -> String {
        Date(timeIntervalSince1970: Double(ms) / 1000).formatted(date: .omitted, time: .standard)
    }
}

/// Settings › General › Status dots: one checkbox per rank, writing the machine-wide `status.dots`.
private struct StatusDotsSettings: View {
    var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Status dots").font(ChatFont.body).fontWeight(.semibold)
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(StatusRank.allCases.enumerated()), id: \.element) { index, rank in
                    if index > 0 { Divider() }
                    Toggle(rank.title, isOn: Binding(get: { model.statusDots.contains(rank) }, set: { _ in model.toggleStatusDot(rank) }))
                        .padding(.horizontal, 14).padding(.vertical, 10)
                }
                Divider()
                VStack(alignment: .leading, spacing: 4) {
                    Text("The statuses that show a dot beside a pane, a space’s sidebar row and its tab.").foregroundStyle(.secondary)
                    Text("status.dots").font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
                }
                .font(ChatFont.line)
                .padding(.horizontal, 14).padding(.vertical, 10)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
        }
    }
}

/// Settings › General › Windows: what a launch does with the windows. This app's own preference
/// (ADR-0014), so it shows whether or not wheeld answers.
struct WindowsSettings: View {
    var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Toggle("Restore windows at launch", isOn: Binding(get: { model.restoresWindows }, set: { model.restoresWindows = $0 }))
            Text("On, Wheel reopens the windows as you left them. Off, it opens one window holding every space.")
                .font(ChatFont.line).foregroundStyle(.secondary)
        }
    }
}
