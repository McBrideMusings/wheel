import SwiftUI
import WheelKit

/// The New Space sheet: recent folders and the Mac's browsing roots, a search over them scored the
/// way the Mac palette scores, and a tap on a folder opens its subfolders. "Create space here" on
/// any level makes the space. There is no system picker on iOS, so unlike the Mac there is no Finder row.
struct NewSpaceSheet: View {
    let model: PhoneModel
    @Environment(\.dismiss) private var dismiss
    @State private var path: [String] = []
    @State private var query = ""

    var body: some View {
        let sections = sections
        NavigationStack(path: $path) {
            List {
                ForEach(sections, id: \.title) { section in
                    Section(section.title) {
                        ForEach(section.rows) { item in FolderRow(item: item) }
                    }
                }
                if !sections.contains(where: { !$0.rows.isEmpty }) {
                    Text(query.isEmpty ? "Looking for folders on your Mac…" : "No folder matches.").foregroundStyle(.secondary)
                }
            }
            .navigationTitle("New space")
            .navigationBarTitleDisplayMode(.inline)
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search folders")
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
            .navigationDestination(for: String.self) { folder in
                FolderLevel(model: model, folder: folder) { create($0) }
            }
        }
        .task {
            model.askFolders()
            await drive()
        }
        .onChange(of: query) { _, new in
            Log.info("new space search \"\(new)\": \(self.sections.flatMap(\.rows).map(\.title).joined(separator: ", "))")
            // A path query lists that folder's subfolders: ask for them.
            if let (parent, _) = CommandPalette.splitPath(new) {
                let key = CommandPalette.listingKey(parent)
                if model.folderListings[key] == nil { model.askDirList(key) }
            }
        }
        .onDisappear { model.stopAskingFolders() }
    }

    /// Without a query: recent folders, then the roots. With one: every folder it matches, best first.
    private var sections: [(title: String, rows: [PaletteItem])] {
        let open = model.spaces.map { CommandPalette.OpenSpace(id: $0.id, name: $0.name, dir: $0.cwd) }
        let trimmed = query.trimmingCharacters(in: .whitespaces)
        if !trimmed.isEmpty {
            let rows = CommandPalette.newSpaceRows(spaces: open, recents: model.recentDirs, roots: model.folderRoots,
                                                   listings: model.folderListings, query: trimmed)
            return [(title: "Matches", rows: rows.map(\.item).filter { if case .openFolder = $0 { false } else { true } })]
        }
        let openID = Dictionary(open.map { ($0.dir, $0.id) }, uniquingKeysWith: { first, _ in first })
        var seen = Set<String>()
        let recents = model.recentDirs.sorted { $0.openedMs > $1.openedMs }.filter { seen.insert($0.dir).inserted }
            .map { PaletteItem.folder(path: $0.dir, space: openID[$0.dir]) }
        let roots = model.folderRoots.map { PaletteItem.folder(path: $0, space: openID[$0]) }
        return [(title: "Recent", rows: recents), (title: "Folders", rows: roots)].filter { !$0.rows.isEmpty }
    }

    private func create(_ folder: String) {
        model.createSpace(path: folder)
        dismiss()
    }

    /// Checks drive the sheet through `WHEEL_NEW_SPACE`: `search:<text>` types into the search field,
    /// `browse:<path>` pushes that folder, `create:<path>` pushes it and presses Create space here.
    private func drive() async {
        #if DEBUG
        guard let spec = ProcessInfo.processInfo.environment["WHEEL_NEW_SPACE"] else { return }
        try? await Task.sleep(for: .seconds(1))
        if spec.hasPrefix("search:") {
            for _ in 0 ..< 20 where model.folderListings.isEmpty { try? await Task.sleep(for: .milliseconds(500)) }
            query = String(spec.dropFirst("search:".count))
        } else if spec.hasPrefix("browse:") {
            path = String(spec.dropFirst("browse:".count)).split(separator: ",").map(String.init)
        } else if spec.hasPrefix("create:") {
            let folder = String(spec.dropFirst("create:".count))
            path = [folder]
            try? await Task.sleep(for: .seconds(1))
            create(folder)
        }
        #endif
    }
}

/// One folder in a list: its name, its path, and a mark when a space is open there.
private struct FolderRow: View {
    let item: PaletteItem

    var body: some View {
        if case let .missing(path) = item {
            Text("No folder at \(path)").foregroundStyle(.secondary)
        } else if case let .folder(path, space) = item {
            NavigationLink(value: path) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(CommandPalette.lastComponent(of: path)).font(.body)
                    Text(space == nil ? path : "open · \(path)").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// One folder's subfolders, and the button that makes a space of the folder itself.
private struct FolderLevel: View {
    let model: PhoneModel
    let folder: String
    let create: (String) -> Void

    private var listing: CommandPalette.Listing? { model.folderListings[CommandPalette.listingKey(folder)] }

    var body: some View {
        List {
            if let listing {
                if let error = listing.error {
                    Text(error).foregroundStyle(.secondary)
                } else if listing.dirs.isEmpty {
                    Text("No subfolders").foregroundStyle(.secondary)
                }
                ForEach(listing.dirs, id: \.path) { dir in FolderRow(item: .folder(path: dir.path, space: dir.space)) }
            } else {
                ProgressView()
            }
        }
        .navigationTitle(CommandPalette.lastComponent(of: folder))
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom) {
            Button { create(folder) } label: {
                Text("Create space here").frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .padding()
            .background(.bar)
        }
        .task { model.askDirList(folder) }
    }
}
