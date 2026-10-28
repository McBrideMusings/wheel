import Foundation

/// How a chat draws, as the daemon resolved the `chat.*` settings for one space: the type, the
/// spacing and the reading width. A key the daemon does not list, or a value this build does not
/// know, leaves the built-in default for that field, so an older daemon draws a chat as before.
public struct ChatAppearance: Sendable, Equatable {
    public enum Family: String, Sendable, CaseIterable { case system, serif, rounded, mono }
    public enum Density: String, Sendable, CaseIterable { case compact, comfortable, spacious }
    public enum QuoteStyle: String, Sendable, CaseIterable { case bar, fill, italic }
    public enum CodeTheme: String, Sendable, CaseIterable {
        case wheelMuted = "wheel-muted", xcode, github, mono, solarized
    }
    public enum CodeFont: String, Sendable, CaseIterable {
        case sfMono = "sf-mono", menlo, monaco, courierNew = "courier-new"
    }
    public enum CodeWrap: String, Sendable, CaseIterable { case scroll, wrap }
    /// Where the agent's home directory shows.
    public enum HomePlacement: String, Sendable, CaseIterable { case row, header, transcript, off }

    public var family: Family = .system
    /// Points. Tool lines and times are drawn a step or two under it.
    public var size: Double = 13
    /// A multiple of the size.
    public var lineHeight: Double = 1.45
    /// Points between the paragraphs and blocks of a reply.
    public var paragraphSpacing: Double = 8
    /// The widest a reply and a message may be, in points; nil is the whole pane.
    public var maxWidth: Double?
    public var density: Density = .comfortable
    public enum MessageStyle: String, Sendable, CaseIterable { case bubble, accent, plain }
    public enum MessageAlign: String, Sendable, CaseIterable { case right, left }
    public var messageStyle: MessageStyle = .accent
    public var messageAlign: MessageAlign = .right
    /// Show the send time under each message.
    public var timestamps = false
    /// What the transcript folds into lines and cards.
    public var layout = TranscriptLayout.standard
    public var quoteStyle: QuoteStyle = .bar
    public var codeTheme: CodeTheme = .wheelMuted
    public var codeFont: CodeFont = .sfMono
    /// Points.
    public var codeSize: Double = 12
    public var codeWrap: CodeWrap = .scroll
    /// Number the lines of a code block over three lines.
    public var lineNumbers = false
    /// What a file path, a pane id and a web link in a reply do.
    public var links = ChatLinkSettings()
    public var home: HomePlacement = .row

    public static let standard = ChatAppearance()

    public init() {}

    /// The appearance in force for the space `settings` was resolved for.
    public init(settings: [SettingInfo]) {
        func value(_ key: String) -> String? { settings.first { $0.key == key }?.value }
        if let v = value("chat.font.family").flatMap(Family.init(rawValue:)) { family = v }
        if let v = value("chat.font.size").flatMap(Double.init) { size = v }
        if let v = value("chat.line_height").flatMap(Double.init) { lineHeight = v }
        if let v = value("chat.paragraph_spacing").flatMap(Double.init) { paragraphSpacing = v }
        maxWidth = value("chat.max_width").flatMap(Double.init)
        if let v = value("chat.density").flatMap(Density.init(rawValue:)) { density = v }
        if let v = value("chat.message.style").flatMap(MessageStyle.init(rawValue:)) { messageStyle = v }
        if let v = value("chat.message.align").flatMap(MessageAlign.init(rawValue:)) { messageAlign = v }
        if let v = value("chat.timestamps") { timestamps = v == "true" }
        if let v = value("chat.tools.summary").flatMap(TranscriptLayout.ToolSummary.init(rawValue:)) { layout.tools = v }
        if let v = value("chat.thinking").flatMap(TranscriptLayout.Thinking.init(rawValue:)) { layout.thinking = v }
        if let v = value("chat.edits.card") { layout.editsCard = v == "true" }
        if let v = value("chat.quote.style").flatMap(QuoteStyle.init(rawValue:)) { quoteStyle = v }
        if let v = value("code.theme").flatMap(CodeTheme.init(rawValue:)) { codeTheme = v }
        if let v = value("code.font").flatMap(CodeFont.init(rawValue:)) { codeFont = v }
        if let v = value("code.size").flatMap(Double.init) { codeSize = v }
        if let v = value("code.wrap").flatMap(CodeWrap.init(rawValue:)) { codeWrap = v }
        if let v = value("code.line_numbers") { lineNumbers = v == "true" }
        if let v = value("chat.links.files").flatMap(ChatLinkSettings.Files.init(rawValue:)) { links.files = v }
        if let v = value("chat.links.file_label").flatMap(ChatLinkSettings.FileLabel.init(rawValue:)) { links.fileLabel = v }
        if let v = value("chat.links.panes") { links.panes = v == "true" }
        if let v = value("chat.links.web").flatMap(ChatLinkSettings.Web.init(rawValue:)) { links.web = v }
        if let v = value("chat.links.patterns") { links.patterns = ChatLinkPattern.parse(v) }
        if let v = value("chat.session.home").flatMap(HomePlacement.init(rawValue:)) { home = v }
    }

    /// Size of tool, reasoning and permission lines.
    public var lineSize: Double { size - 1 }
    /// Size of times, counts and settings.
    public var metaSize: Double { size - 3 }

    /// Extra points between the lines of text of `size`, over the font's own line height (about
    /// 1.2 times its size).
    public func lineSpacing(for size: Double) -> Double { Swift.max(0, size * (lineHeight - 1.2)) }

    /// Space above a message that starts a turn.
    public var turnGap: Double { switch density { case .compact: 14; case .comfortable: 24; case .spacious: 36 } }
    /// Space between a message and the reply under it.
    public var replyGap: Double { switch density { case .compact: 8; case .comfortable: 12; case .spacious: 18 } }
    /// Space between the parts of one reply.
    public var partGap: Double { switch density { case .compact: 4; case .comfortable: 8; case .spacious: 12 } }
}

/// The appearance in force for each space, kept by a client from the settings lists the daemon
/// answers with. A chat draws from its own space's entry, so a project that sets `chat.font.size`
/// changes its chats and no other space's.
public struct ChatAppearances: Equatable, Sendable {
    public private(set) var bySpace: [String: ChatAppearance] = [:]

    public init() {}

    /// A settings list resolved for `space`; a list with no space is the machine view, which no
    /// chat draws from.
    public mutating func store(_ list: [SettingInfo], for space: String?) {
        guard let space else { return }
        bySpace[space] = ChatAppearance(settings: list)
    }

    /// Forget the spaces that are gone, and every entry when `reset`.
    public mutating func keep(only spaces: [SpaceInfo], reset: Bool = false) {
        let ids = Set(spaces.map(\.id))
        bySpace = reset ? [:] : bySpace.filter { ids.contains($0.key) }
    }

    /// The spaces in `spaces` with no entry yet.
    public func missing(in spaces: [SpaceInfo]) -> [String] {
        spaces.map(\.id).filter { bySpace[$0] == nil }
    }

    /// The appearance of the chat showing `session`: its space's, else the defaults.
    public func appearance(forSession session: String, in spaces: [SpaceInfo]) -> ChatAppearance {
        for space in spaces {
            for pane in space.root.panes {
                if case let .agent(name, _) = pane.kind, name == session { return bySpace[space.id] ?? .standard }
            }
        }
        return .standard
    }
}
