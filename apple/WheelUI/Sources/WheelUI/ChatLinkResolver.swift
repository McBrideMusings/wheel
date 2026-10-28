import Observation
import SwiftUI
import WheelKit

/// What the chat's links know and do: which files and artifacts exist (asked of the daemon, once
/// each), which panes exist (read from the client's layout), and what a click does. The app makes
/// one, fills in its closures, and hands it to the transcript through the environment.
@MainActor @Observable
public final class ChatLinkResolver {
    /// Where a session's pane is.
    public struct Place: Sendable {
        public var space: String
        public var cwd: String
        public var pane: String

        public init(space: String, cwd: String, pane: String) {
            self.space = space
            self.cwd = cwd
            self.pane = pane
        }
    }

    /// Where the pane of agent session `session` is; nil when the layout does not hold it.
    @ObservationIgnored public var locate: (String) -> Place? = { _ in nil }
    /// The chip text for pane `id` ("p1958 · Claude"); nil when no pane has that id.
    @ObservationIgnored public var paneChip: (String) -> String? = { _ in nil }
    /// Send a request; returns its id, or nil while the client is offline.
    @ObservationIgnored public var request: (_ make: (UInt64) -> ClientMsg) -> UInt64? = { _ in nil }
    /// Do what a click on a link asks, for the chat of `session`.
    @ObservationIgnored public var perform: (_ action: Action, _ session: String) -> Void = { _, _ in }

    /// What the app is asked to do for a click that is not a preview.
    public enum Action: Equatable, Sendable {
        case focusPane(String)
        /// Open `page` in a browser pane beside the chat's pane.
        case openPage(String)
        case openArtifact(String)
        /// Open the file in a pane beside the chat's pane.
        case openFile(String)
        case openInSystemBrowser(String)
    }

    /// Each file asked about, by `cwd` and the path as the reply wrote it.
    private(set) var files: [String: FileProbe] = [:]
    /// The artifact names each space has, by space id; absent until the daemon answered.
    private(set) var artifacts: [String: Set<String>] = [:]
    @ObservationIgnored private var asked = Set<String>()
    /// When each space's artifact names were listed, so one made later is found.
    @ObservationIgnored private var listedAt: [String: Date] = [:]
    /// When each missing file was found missing, so one that is made later is asked about again.
    @ObservationIgnored private var missingAt: [String: Date] = [:]
    private static let recheckAfter: TimeInterval = 15
    @ObservationIgnored private var waitingPeek: [UInt64: (key: String, then: (FilePeek) -> Void)] = [:]
    @ObservationIgnored private var waitingArtifacts: [UInt64: String] = [:]

    public init() {}

    private static func key(_ path: String, _ cwd: String) -> String { "\(cwd)\u{0}\(path)" }

    /// The Markdown of a reply with the links that exist written into it, asking the daemon
    /// about the paths and artifact names it has not heard of yet.
    public func linked(_ markdown: String, look: ChatAppearance, session: String, settled: Bool = true) -> String {
        let settings = look.links
        let place = locate(session)
        let knowledge = ChatLinkKnowledge(
            file: { path in
                guard let place else { return .missing }
                return self.files[Self.key(path, place.cwd)] ?? .unknown
            },
            pane: { settings.panes ? self.paneChip($0) : nil },
            artifact: { name in place.map { self.artifacts[$0.space]?.contains(name) ?? false } ?? false })
        if let place {
            // While a reply streams, its last word may be half a path: ask about the whole words.
            var asking = markdown
            if !settled, let space = asking.lastIndex(where: \.isWhitespace) { asking = String(asking[..<space]) }
            for target in ChatLinks.targets(in: asking, settings: settings) {
                switch target {
                case let .file(path, _): probe(path, place: place)
                case .artifact: listArtifacts(space: place.space)
                default: break
                }
            }
        }
        return ChatLinks.rewrite(markdown, settings: settings, knowledge: knowledge)
    }

    private func probe(_ path: String, place: Place) {
        let key = Self.key(path, place.cwd)
        if files[key] == .missing, let at = missingAt[key], Date().timeIntervalSince(at) > Self.recheckAfter {
            asked.remove(key)
        }
        guard files[key] == nil || files[key] == .missing, asked.insert(key).inserted else { return }
        let sent = request { .filePeek(req: $0, path: path, cwd: place.cwd, line: nil, context: 0) }
        guard let sent else {
            asked.remove(key)
            return
        }
        waitingPeek[sent] = (key, { [weak self] peek in
            guard let self else { return }
            let probe: FileProbe = peek.found ? .found(peek.path) : .missing
            if probe == .missing { missingAt[key] = Date() }
            // A file still missing changes nothing a reply draws.
            if files[key] != probe { files[key] = probe }
        })
    }

    private func listArtifacts(space: String) {
        let key = "artifacts\u{0}\(space)"
        if let at = listedAt[space], Date().timeIntervalSince(at) > Self.recheckAfter { asked.remove(key) }
        guard asked.insert(key).inserted else { return }
        let sent = request { .artifactLs(req: $0, space: space, paneOnly: false) }
        if let sent { waitingArtifacts[sent] = space } else { asked.remove(key) }
    }

    /// Lines of `path` around `line` for a preview; `done` runs when the daemon answers.
    func preview(path: String, line: Int?, session: String, done: @escaping (FilePeek) -> Void) {
        let cwd = locate(session)?.cwd
        let sent = request { .filePeek(req: $0, path: path, cwd: cwd, line: line, context: 8) }
        guard let sent else {
            done(FilePeek(path: path, found: false, start: 1, lines: [], error: "not connected"))
            return
        }
        waitingPeek[sent] = ("", done)
    }

    /// Take the daemon's answer to a request this resolver made; false for any other message.
    public func received(_ message: ServerMsg) -> Bool {
        switch message {
        case let .filePeek(req, peek):
            guard let waiting = waitingPeek.removeValue(forKey: req) else { return false }
            waiting.then(peek)
            return true
        case let .artifacts(req, list):
            guard let req, let space = waitingArtifacts.removeValue(forKey: req) else { return false }
            let names = Set(list.map(\.name))
            listedAt[space] = Date()
            if artifacts[space] != names { artifacts[space] = names }
            return true
        case let .error(req, _, _):
            // The daemon refused: ask again at the next redraw rather than never.
            guard let req, let space = waitingArtifacts.removeValue(forKey: req) else { return false }
            asked.remove("artifacts\u{0}\(space)")
            return false
        default:
            return false
        }
    }

    /// Forget what was learned: files are created and removed while a chat stays open.
    public func forget() {
        files = [:]
        artifacts = [:]
        asked = []
        missingAt = [:]
        listedAt = [:]
    }

    /// The file preview open over a reply: the reply's row and the file.
    public struct Preview: Identifiable, Equatable, Sendable {
        public let row: Int
        public let path: String
        public let line: Int?
        public var id: String { "\(row) \(path):\(line ?? 0)" }

        public init(row: Int, path: String, line: Int?) {
            self.row = row
            self.path = path
            self.line = line
        }
    }

    /// The preview open in each session's chat, if any. One at a time per chat.
    public private(set) var previews: [String: Preview] = [:]

    public func closePreview(session: String) {
        previews[session] = nil
    }

    /// What a click on a link in reply row `row` of the chat of `session` does.
    public func click(_ action: ChatLinkAction, look: ChatAppearance, session: String, row: Int) {
        switch action {
        case let .file(path, line):
            switch look.links.files {
            case .preview: previews[session] = Preview(row: row, path: path, line: line)
            case .pane: perform(.openFile(path), session)
            case .off: break
            }
        case let .pane(id):
            perform(.focusPane(id), session)
        case let .artifact(name):
            perform(.openArtifact(name), session)
        case let .web(url):
            perform(look.links.web == .system ? .openInSystemBrowser(url) : .openPage(url), session)
        }
    }
}

private struct ChatLinksKey: EnvironmentKey {
    static let defaultValue: ChatLinkResolver? = nil
}

extension EnvironmentValues {
    /// What the chat under this view links to; nil leaves a reply's text as it is.
    public var chatLinks: ChatLinkResolver? {
        get { self[ChatLinksKey.self] }
        set { self[ChatLinksKey.self] = newValue }
    }
}
