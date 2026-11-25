import SwiftUI
import WheelKit

enum SettingsTab: String, CaseIterable, Identifiable {
    case global = "Global"
    case project = "Project"

    var id: String { rawValue }
}

/// The phone's Settings: the two `wheel.toml` layers the Mac's Settings window edits, written
/// through the same `setting_set` and `setting_unset` ops, on the wheeld of the session shown. The
/// title names that server, in its mark with several (ADR-0057).
struct SettingsView: View {
    let model: PhoneModel
    let servers: ServerList
    @Binding var tab: SettingsTab
    @Binding var project: String?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                Picker("Settings", selection: $tab) {
                    ForEach(SettingsTab.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
                switch tab {
                case .global: SettingsList(model: model, layer: .global)
                case .project: projectTab
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    VStack(spacing: 0) {
                        Text("Settings").font(.headline)
                        Group {
                            if servers.several { ServerMark(model: model) } else { Text(model.serverName) }
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("settings server")
                    }
                }
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .task(id: project) { model.askSettings(for: project) }
    }

    @ViewBuilder private var projectTab: some View {
        if model.spaces.isEmpty {
            ContentUnavailableView("No projects yet", systemImage: "folder",
                                   description: Text("A project's settings live in the wheel.toml at its folder's root."))
        } else {
            Picker("Project", selection: $project) {
                ForEach(model.spaces) { Text($0.name).tag(Optional($0.id)) }
            }
            .padding(.horizontal, 16)
            SettingsList(model: model, layer: .project)
        }
    }
}

/// Every key the daemon reports, a section per key prefix, each row editing this layer's file.
private struct SettingsList: View {
    let model: PhoneModel
    let layer: SettingField.Layer

    var body: some View {
        if !model.settingsLoaded {
            ContentUnavailableView(model.isConnected ? "Asking the Mac for its settings…" : "Not connected to the Mac",
                                   systemImage: "gearshape")
        } else {
            Form {
                Section {
                    NavigationLink("Appearance") { AppearanceView(model: model, layer: layer) }
                        .accessibilityIdentifier("appearance-\(layer.rawValue)")
                }
                ForEach(SettingSection.grouping(model.settings), id: \.name) { section in
                    Section(section.name) {
                        ForEach(section.settings) { setting in
                            SettingRow(model: model, setting: setting, field: SettingField(layer: layer, key: setting.key))
                        }
                    }
                }
            }
        }
    }
}

/// One key in one layer: a field showing what that file sets, the layer below's value as its
/// placeholder while the file sets nothing, and the key's help. Return saves; blank removes the key.
private struct SettingRow: View {
    let model: PhoneModel
    let setting: SettingInfo
    let field: SettingField
    @State private var text = ""
    @FocusState private var focused: Bool

    private var stored: String? { field.layer == .global ? setting.machine : setting.project }
    private var inherited: String { field.layer == .global ? setting.default : setting.global }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(SettingSection.rowLabel(setting.key))
                TextField(inherited, text: $text)
                    .multilineTextAlignment(.trailing)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focused)
                    .onSubmit { model.setSetting(field, to: text) }
                    .accessibilityIdentifier("setting-\(field.layer.rawValue)-\(setting.key)")
            }
            if let refusal = model.settingRefusals[field] {
                Label(refusal, systemImage: "xmark").font(.caption).foregroundStyle(.red)
            }
            if let problem = setting.problem {
                Label(problem, systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.orange)
            }
            Text(help).font(.caption).foregroundStyle(.secondary)
            Text(setting.key).font(.system(.caption2, design: .monospaced)).foregroundStyle(.secondary)
        }
        .onAppear { text = stored ?? "" }
        // Another client's change shows here unless this field is being typed in.
        .onChange(of: stored) { _, new in if !focused { text = new ?? "" } }
    }

    private var help: String {
        switch field.layer {
        case .global: stored == nil ? "\(setting.help) Not set: wheel's default." : "\(setting.help) Default: \(setting.default)."
        case .project: stored == nil ? "\(setting.help) Not set here: the global value." : "\(setting.help) Global: \(setting.global)."
        }
    }
}
