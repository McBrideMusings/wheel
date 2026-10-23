import Foundation

/// One built-in artifact: a page wheeld carries and installs for every project on this machine.
public struct BuiltinInfo: Sendable, Equatable, Identifiable, Decodable {
    public var id: String { name }
    public let name: String
    /// The version this wheeld carries.
    public let shippedVersion: UInt32
    /// The version installed on this machine; nil when it is not installed.
    public let installedVersion: UInt32?
    /// Installed files a person changed or deleted since Wheel wrote them.
    public let editedFiles: [String]

    public init(name: String, shippedVersion: UInt32, installedVersion: UInt32? = nil, editedFiles: [String] = []) {
        self.name = name
        self.shippedVersion = shippedVersion
        self.installedVersion = installedVersion
        self.editedFiles = editedFiles
    }

    private enum CodingKeys: String, CodingKey {
        case name, shippedVersion = "shipped_version", installedVersion = "installed_version", editedFiles = "edited_files"
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        shippedVersion = try c.decode(UInt32.self, forKey: .shippedVersion)
        installedVersion = try c.decodeIfPresent(UInt32.self, forKey: .installedVersion)
        editedFiles = try c.decodeIfPresent([String].self, forKey: .editedFiles) ?? []
    }

    /// What its row offers: Install when it is not installed, Update when this wheeld carries a
    /// newer version, Restore when a person edited the installed one, else nothing but Remove.
    public var action: Action {
        guard let installed = installedVersion else { return .install }
        if installed < shippedVersion { return .update }
        return editedFiles.isEmpty ? .upToDate : .restore
    }

    public enum Action: String, Sendable { case install, update, restore, upToDate = "up_to_date" }
}

/// An installed file a person changed, and the unified diff an update would apply to it.
public struct EditedFile: Sendable, Equatable, Identifiable, Decodable {
    public var id: String { path }
    public let path: String
    public let diff: String

    public init(path: String, diff: String) {
        self.path = path
        self.diff = diff
    }
}

/// What an update does with installed files a person changed.
public enum OnEdited: String, Sendable {
    case ask, keep, replace
}
