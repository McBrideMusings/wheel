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
        /// Run a command in a new terminal pane beside the chat's pane.
        case runCommand(String)
        case copyText(String)
    }

    /// One lookup a reply's links can depend on: a file, a space's artifact names, or the panes of
    /// the layout. A reply reads the signals of what it looked up (`track`), so an answer redraws
    /// the replies that asked about it and no other.
    @MainActor @Observable
    final class Signal {
        var moves = 0
    }
    @ObservationIgnored private var signals: [String: Signal] = [:]
    private static let layoutSignal = "layout"
    private static func artifactsKey(_ space: String) -> String { "artifacts\u{0}\(space)" }

    private func signal(_ key: String) -> Signal {
        if let held = signals[key] { return held }
        let made = Signal()
        signals[key] = made
        return made
    }

    /// Each file asked about, by `cwd` and the path as the reply wrote it.
    @ObservationIgnored private var files: [String: FileProbe] = [:]
    /// The artifact names each space has, by space id; absent until the daemon answered.
    @ObservationIgnored private var artifacts: [String: Set<String>] = [:]
    @ObservationIgnored private var layout = ""
    /// How often each kind of lookup moved since the last report.
    @ObservationIgnored public private(set) var moves: [String: Int] = [:]

    /// The lookup `key` answered differently: redraw the replies that read it.
    private func moved(_ why: String, key: String) {
        signals[key]?.moves += 1
        moves[why, default: 0] += 1
    }

    /// Redraw the reply whose links were made from `reads` when one of its lookups answers anew.
    /// Call it from the reply's body.
    public func track(_ reads: Reads?) {
        _ = signal(Self.layoutSignal).moves
        for key in reads?.keys ?? [] { _ = signal(key).moves }
    }

    /// The moves counted so far, cleared.
    public func takeMoves() -> [String: Int] {
        defer { moves = [:] }
        return moves
    }

    /// Tell the links the app's layout is now `spaces`; moves `version` when a pane chip or a
    /// session's place changed.
    public func layoutChanged(_ spaces: [SpaceInfo]) {
        let key = ChatLinks.layoutKey(spaces)
        if key != layout {
            layout = key
            moved("layout", key: Self.layoutSignal)
        }
    }
    @ObservationIgnored private var asked = Set<String>()
    /// When each space's artifact names were listed, so one made later is found.
    @ObservationIgnored private var listedAt: [String: Date] = [:]
    /// When each missing file was found missing, so one that is made later is asked about again.
    @ObservationIgnored private var missingAt: [String: Date] = [:]
    private static let recheckAfter: TimeInterval = 15
    @ObservationIgnored private var waitingPeek: [UInt64: (key: String, then: (FilePeek) -> Void)] = [:]
    @ObservationIgnored private var waitingArtifacts: [UInt64: String] = [:]
    @ObservationIgnored private var waitingImage: [UInt64: CheckedContinuation<FileImage, Never>] = [:]

    public init() {}

    private static func key(_ path: String, _ cwd: String) -> String { "\(cwd)\u{0}\(path)" }

    /// What one reply's links were made from: each lookup and its answer. The reply needs making
    /// again only when one of these answers changed.
    public struct Reads: Equatable, Sendable {
        var place: String?
        var files: [String: FileProbe] = [:]
        var panes: [String: String?] = [:]
        var artifacts: [String: Bool] = [:]
        /// The signals of the lookups above.
        var keys: Set<String> = []
    }

    /// The Markdown of a reply with the links that exist written into it, asking the daemon
    /// about the paths and artifact names it has not heard of yet.
    public func linked(_ markdown: String, look: ChatAppearance, session: String, settled: Bool = true) -> String {
        linkedReading(markdown, look: look, session: session, settled: settled).markdown
    }

    /// Whether every lookup in `reads` would answer the same now.
    public func stillReads(_ reads: Reads, look: ChatAppearance, session: String) -> Bool {
        let place = locate(session)
        guard reads.place == place.map({ "\($0.space)\u{0}\($0.cwd)" }) else { return false }
        for (path, was) in reads.files where file(path, place: place) != was { return false }
        for (id, was) in reads.panes where (look.links.panes ? paneChip(id) : nil) != was { return false }
        for (name, was) in reads.artifacts where artifact(name, place: place) != was { return false }
        return true
    }

    private func file(_ path: String, place: Place?) -> FileProbe {
        guard let place else { return .missing }
        return files[Self.key(path, place.cwd)] ?? .unknown
    }

    private func artifact(_ name: String, place: Place?) -> Bool {
        place.map { artifacts[$0.space]?.contains(name) ?? false } ?? false
    }

    /// `linked`, and what it read to make the links.
    public func linkedReading(_ markdown: String, look: ChatAppearance, session: String, settled: Bool = true) -> (markdown: String, reads: Reads) {
        let settings = look.links
        let place = locate(session)
        var reads = Reads(place: place.map { "\($0.space)\u{0}\($0.cwd)" })
        let knowledge = ChatLinkKnowledge(
            file: { path in
                let answer = self.file(path, place: place)
                reads.files[path] = answer
                if let place { reads.keys.insert(Self.key(path, place.cwd)) }
                return answer
            },
            pane: { id in
                let chip = settings.panes ? self.paneChip(id) : nil
                reads.panes[id] = chip
                return chip
            },
            artifact: { name in
                let has = self.artifact(name, place: place)
                reads.artifacts[name] = has
                if let place { reads.keys.insert(Self.artifactsKey(place.space)) }
                return has
            })
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
        let rewritten = ChatLinks.rewrite(markdown, settings: settings, knowledge: knowledge)
        return (rewritten, reads)
    }

    private func probe(_ path: String, place: Place) {
        let key = Self.key(path, place.cwd)
        if files[key] == .missing, let at = missingAt[key], Date().timeIntervalSince(at) > Self.recheckAfter {
            // Due again: one request goes out, and the redraws until it answers find nothing due.
            asked.remove(key)
            missingAt[key] = nil
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
            if files[key] != probe {
                files[key] = probe
                moved("file", key: key)
            }
        })
    }

    private func listArtifacts(space: String) {
        let key = Self.artifactsKey(space)
        if let at = listedAt[space], Date().timeIntervalSince(at) > Self.recheckAfter {
            asked.remove(key)
            listedAt[space] = nil
        }
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

    /// The bytes of the image file `path` on the daemon's machine, read against the directory of
    /// `session`'s pane. A phone gets a file this way as a Mac does.
    func fetchImage(path: String, session: String) async -> FileImage {
        let cwd = locate(session)?.cwd
        return await withCheckedContinuation { continuation in
            let sent = request { .fileImage(req: $0, path: path, cwd: cwd) }
            guard let sent else {
                continuation.resume(returning: FileImage(path: path, found: false, error: "not connected"))
                return
            }
            waitingImage[sent] = continuation
            // A dropped connection never answers: give up rather than hold the picture's task.
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(20))
                self?.waitingImage.removeValue(forKey: sent)?.resume(
                    returning: FileImage(path: path, found: false, error: "the daemon did not answer"))
            }
        }
    }

    /// The image open at full size over a reply: the reply's row and the picture.
    public struct FullImage: Identifiable, Equatable, Sendable {
        public let row: Int
        public let path: String
        public let data: Data
        public var id: String { "\(row) \(path)" }
    }

    /// The full-size image open in each session's chat, if any. One at a time per chat.
    public private(set) var fullImages: [String: FullImage] = [:]

    func openImage(session: String, row: Int, path: String, data: Data) {
        fullImages[session] = FullImage(row: row, path: path, data: data)
    }

    public func closeImage(session: String) {
        fullImages[session] = nil
    }

    /// Take the daemon's answer to a request this resolver made; false for any other message.
    public func received(_ message: ServerMsg) -> Bool {
        switch message {
        case let .fileImage(req, image):
            guard let waiting = waitingImage.removeValue(forKey: req) else { return false }
            waiting.resume(returning: image)
            return true
        case let .filePeek(req, peek):
            guard let waiting = waitingPeek.removeValue(forKey: req) else { return false }
            waiting.then(peek)
            return true
        case let .artifacts(req, list):
            guard let req, let space = waitingArtifacts.removeValue(forKey: req) else { return false }
            let names = Set(list.map(\.name))
            listedAt[space] = Date()
            if artifacts[space] != names {
                artifacts[space] = names
                moved("artifacts", key: Self.artifactsKey(space))
            }
            return true
        case let .error(req, _, _):
            // The daemon refused: ask again at the next redraw rather than never.
            guard let req, let space = waitingArtifacts.removeValue(forKey: req) else { return false }
            asked.remove(Self.artifactsKey(space))
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
        for signal in signals.values { signal.moves += 1 }
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
        case let .run(command):
            perform(.runCommand(command), session)
        case let .copy(text):
            perform(.copyText(text), session)
        case let .loadImage(url):
            ChatImageRegistry.shared.loadRemote(url, session: session, row: row)
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
