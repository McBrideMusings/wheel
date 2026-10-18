import Foundation

/// The wheeld wire protocol (proto/src/lib.rs): one JSON object per WebSocket text frame.
/// Client messages are tagged by `op`, daemon messages by `ev`.

public enum ClientMsg: Sendable, Equatable {
    case status(req: UInt64?)
    case termOpen(req: UInt64?, cwd: String?, cols: Int, rows: Int)
    case termAttach(req: UInt64?, id: String)
    case termDetach(id: String)
    /// End a terminal from `termOpen`; the daemon refuses a pane's terminal.
    case termClose(req: UInt64?, id: String)
    case termInput(req: UInt64?, id: String, data: Data)
    case termResize(req: UInt64?, id: String, cols: Int, rows: Int)
    /// `session` is `lead` for the lead, or the name an agent pane carries.
    case sessionAttach(req: UInt64?, session: String)
    case sessionDetach(session: String)
    case sessionSend(req: UInt64?, session: String, text: String)
    case sessionCancel(req: UInt64?, session: String)
    case permAnswer(req: UInt64?, id: String, choice: String)
    case spaceCreate(req: UInt64?, cwd: String)
    case spaceClose(req: UInt64?, id: String)
    /// Make a space active, or focus a pane (which also makes its space active).
    case focus(req: UInt64?, space: String?, pane: String?)
    case paneSplit(req: UInt64?, pane: String, dir: Dir)
    case paneClose(req: UInt64?, pane: String)
    /// `artifact` names the artifact a `web` pane shows and `scope` pins which root it is under;
    /// the daemon refuses both for any other kind, and with no `scope` resolves local, then repo,
    /// then shared.
    /// `url` is the page a `browser` pane opens, refused for any other kind; with none, the daemon
    /// opens the `browser.homepage` setting as it resolves for that pane's space.
    case paneSetKind(req: UInt64?, pane: String, kind: NewPaneKind, artifact: String? = nil, scope: ArtifactScope? = nil, url: String? = nil)
    /// Start a terminal or an agent in a pane, whatever it showed going. `message` is an agent's
    /// first message. The daemon answers once the agent is ready or has given up, and with an
    /// error when it could not start.
    case paneStart(req: UInt64?, pane: String, kind: StartKind, message: String? = nil)
    /// A browser pane is showing `url` now: sent as the person navigates, so the saved layout
    /// follows them. The daemon refuses a pane that is not a browser pane and a URL that is not
    /// http, https, a file on this machine or about:blank.
    case paneBrowserUrl(req: UInt64?, pane: String, url: String)
    /// Send a browser pane to what the person typed into its address field. The daemon reads the
    /// text — a URL, `example.com`, a path — and replies `opened` with the page, or refuses it
    /// saying why.
    case paneBrowserOpen(req: UInt64?, pane: String, text: String)
    case splitRatio(req: UInt64?, split: String, ratio: Double)
    /// Change one of a session's config options — its model, permission mode or effort level.
    /// `value` is a choice's `value`, or `true`/`false` for a boolean.
    case sessionConfigSet(req: UInt64?, session: String, config: String, value: String)
    /// Run the lead's login preflight now; replies `leadLoginInfo`.
    case leadLoginStatus(req: UInt64?)
    /// Log the lead in with the agent's login method `method` (the daemon's default is
    /// `claude-ai-login`): the daemon opens a terminal pane running it, and replies
    /// `leadLoginFinished` when the command exits.
    case leadLogin(req: UInt64?, method: String?)
    /// Every one of wheel's own settings, resolved for `space` — the space being shown with none.
    /// Replies `settings`.
    case settingsLs(req: UInt64?, space: String?)
    /// Write one setting: into this machine's `wheel.toml` with `machine`, else into `space`'s.
    /// Replies `settings` and tells every connection; refused with the daemon's reason as `error`.
    case settingSet(req: UInt64?, key: String, value: String, space: String?, machine: Bool)
    /// Remove one setting from this machine's `wheel.toml` with `machine`, else from `space`'s, so
    /// the layer below is in force. Replies and tells every connection as `settingSet` does.
    case settingUnset(req: UInt64?, key: String, space: String?, machine: Bool)
    /// List the conversations the session's agent holds for its directory; replies `conversations`.
    case sessionConversationLs(req: UInt64?, session: String)
    /// Put the session in a fresh conversation; replies `conversation`, and every attached client
    /// is sent the new, empty transcript. Refused while a turn runs.
    case sessionConversationNew(req: UInt64?, session: String)
    /// Put the session back in the past conversation `id`; replies `conversation`, and every
    /// attached client is sent its replayed transcript.
    case sessionConversationResume(req: UInt64?, session: String, id: String)
    /// One supervisor with its running workers; replies `supervisor`.
    case supervisorShow(req: UInt64?, id: String)
    /// The decisions a supervisor's loop made, oldest first; replies `supervisor_events`.
    case supervisorEvents(req: UInt64?, id: String)
    /// What waits on a supervisor's agent; replies `supervisor_inbox`.
    case supervisorInbox(req: UInt64?, id: String)
    /// Pause or resume a supervisor; replies `supervisor`.
    case supervisorPause(req: UInt64?, id: String)
    case supervisorResume(req: UInt64?, id: String)
    /// How many workers a supervisor runs at once; replies `supervisor`.
    case supervisorWorkers(req: UInt64?, id: String, workers: Int)
    /// A supervisor's usage cap, 1 to 100, or none to remove it; replies `supervisor`.
    case supervisorCap(req: UInt64?, id: String, cap: Int?)
    /// Show a supervisor's running worker in a pane of its own beside the supervisor's; replies
    /// `created` with the new pane's id.
    case workerPush(req: UInt64?, id: String, session: String)
    /// The lead's inbox, open items first; replies `inbox`.
    case inboxLs(req: UInt64?)
    /// This connection draws any pane it shows as a PNG, so the daemon may send it `renderPane`.
    /// Sent once per connection.
    case renderOffer
    /// The answer to `renderPane` `id`: the pane drawn as a PNG.
    case rendered(id: UInt64, png: Data)
    /// The answer to `renderPane` `id` when this client cannot draw the pane. `RenderRefusal.notShown`
    /// says the client does not show it; anything else is read as a failure to draw.
    case renderRefused(id: UInt64, message: String)

    /// The frame's JSON text.
    public func json() throws -> String {
        var o: [String: Any]
        func put(_ req: UInt64?) { if let req { o["req"] = req } }
        switch self {
        case let .status(req):
            o = ["op": "status"]; put(req)
        case let .termOpen(req, cwd, cols, rows):
            o = ["op": "term_open", "cols": cols, "rows": rows]; put(req)
            if let cwd { o["cwd"] = cwd }
        case let .termAttach(req, id):
            o = ["op": "term_attach", "id": id]; put(req)
        case let .termDetach(id):
            o = ["op": "term_detach", "id": id]
        case let .termClose(req, id):
            o = ["op": "term_close", "id": id]; put(req)
        case let .termInput(req, id, data):
            o = ["op": "term_input", "id": id, "data": data.base64EncodedString()]; put(req)
        case let .termResize(req, id, cols, rows):
            o = ["op": "term_resize", "id": id, "cols": cols, "rows": rows]; put(req)
        case let .sessionAttach(req, session):
            o = ["op": "session_attach", "session": session]; put(req)
        case let .sessionDetach(session):
            o = ["op": "session_detach", "session": session]
        case let .sessionSend(req, session, text):
            o = ["op": "session_send", "session": session, "text": text]; put(req)
        case let .sessionCancel(req, session):
            o = ["op": "session_cancel", "session": session]; put(req)
        case let .permAnswer(req, id, choice):
            o = ["op": "perm_answer", "id": id, "choice": choice]; put(req)
        case let .spaceCreate(req, cwd):
            o = ["op": "space_create", "cwd": cwd]; put(req)
        case let .spaceClose(req, id):
            o = ["op": "space_close", "id": id]; put(req)
        case let .focus(req, space, pane):
            o = ["op": "focus"]; put(req)
            if let space { o["space"] = space }
            if let pane { o["pane"] = pane }
        case let .paneSplit(req, pane, dir):
            o = ["op": "pane_split", "pane": pane, "dir": dir.rawValue]; put(req)
        case let .paneClose(req, pane):
            o = ["op": "pane_close", "pane": pane]; put(req)
        case let .paneSetKind(req, pane, kind, artifact, scope, url):
            o = ["op": "pane_set_kind", "pane": pane, "kind": kind.rawValue]; put(req)
            if let artifact { o["artifact"] = artifact }
            if let scope { o["scope"] = scope.rawValue }
            if let url { o["url"] = url }
        case let .paneStart(req, pane, kind, message):
            o = ["op": "pane_start", "pane": pane, "kind": kind.rawValue]; put(req)
            if let message { o["message"] = message }
        case let .paneBrowserUrl(req, pane, url):
            o = ["op": "pane_browser_url", "pane": pane, "url": url]; put(req)
        case let .paneBrowserOpen(req, pane, text):
            o = ["op": "pane_browser_open", "pane": pane, "text": text]; put(req)
        case let .splitRatio(req, split, ratio):
            o = ["op": "split_ratio", "split": split, "ratio": ratio]; put(req)
        case let .sessionConfigSet(req, session, config, value):
            o = ["op": "session_config_set", "session": session, "config": config, "value": value]; put(req)
        case let .leadLoginStatus(req):
            o = ["op": "lead_login_status"]; put(req)
        case let .leadLogin(req, method):
            o = ["op": "lead_login"]; put(req)
            if let method { o["method"] = method }
        case let .settingsLs(req, space):
            o = ["op": "settings_ls"]; put(req)
            if let space { o["space"] = space }
        case let .settingSet(req, key, value, space, machine):
            o = ["op": "setting_set", "key": key, "value": value]; put(req)
            if let space { o["space"] = space }
            if machine { o["machine"] = true }
        case let .settingUnset(req, key, space, machine):
            o = ["op": "setting_unset", "key": key]; put(req)
            if let space { o["space"] = space }
            if machine { o["machine"] = true }
        case let .sessionConversationLs(req, session):
            o = ["op": "session_conversation_ls", "session": session]; put(req)
        case let .sessionConversationNew(req, session):
            o = ["op": "session_conversation_new", "session": session]; put(req)
        case let .sessionConversationResume(req, session, id):
            o = ["op": "session_conversation_resume", "session": session, "id": id]; put(req)
        case let .supervisorShow(req, id):
            o = ["op": "supervisor_show", "id": id]; put(req)
        case let .supervisorEvents(req, id):
            o = ["op": "supervisor_events", "id": id]; put(req)
        case let .supervisorInbox(req, id):
            o = ["op": "supervisor_inbox", "id": id]; put(req)
        case let .supervisorPause(req, id):
            o = ["op": "supervisor_pause", "id": id]; put(req)
        case let .supervisorResume(req, id):
            o = ["op": "supervisor_resume", "id": id]; put(req)
        case let .supervisorWorkers(req, id, workers):
            o = ["op": "supervisor_workers", "id": id, "workers": workers]; put(req)
        case let .workerPush(req, id, session):
            o = ["op": "worker_push", "id": id, "session": session]; put(req)
        case let .supervisorCap(req, id, cap):
            o = ["op": "supervisor_cap", "id": id]; put(req)
            if let cap { o["cap"] = cap }
        case let .inboxLs(req):
            o = ["op": "inbox_ls"]; put(req)
        case .renderOffer:
            o = ["op": "render_offer"]
        case let .rendered(id, png):
            o = ["op": "rendered", "id": id, "png": png.base64EncodedString()]
        case let .renderRefused(id, message):
            o = ["op": "render_refused", "id": id, "message": message]
        }
        let data = try JSONSerialization.data(withJSONObject: o, options: [.sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }
}

public struct TermInfo: Codable, Sendable, Equatable {
    public let id: String
    public let pid: UInt32?
    public let cwd: String
    public let cols: Int
    public let rows: Int
    public let alive: Bool

    private enum CodingKeys: String, CodingKey {
        case id, pid, cwd, cols, rows, alive
    }
}

public enum SessionState: Sendable, Equatable {
    case starting, ready
    case busy(queued: Int)
    case failed(message: String)
    case gaveUp(tries: Int, message: String)
    case unknown(String)

    public var label: String {
        switch self {
        case .starting: "Starting"
        case .ready: "Ready"
        case let .busy(queued): queued > 0 ? "Working · \(queued) queued" : "Working"
        case let .failed(message): "Stopped: \(message)"
        case let .gaveUp(tries, message): "Gave up after \(tries) tries: \(message)"
        case let .unknown(phase): phase
        }
    }
}

extension SessionState: Decodable {
    private enum K: String, CodingKey { case phase, queued, message, tries }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .phase) {
        case "starting": self = .starting
        case "ready": self = .ready
        case "busy": self = .busy(queued: try c.decodeIfPresent(Int.self, forKey: .queued) ?? 0)
        case "failed": self = .failed(message: try c.decodeIfPresent(String.self, forKey: .message) ?? "")
        case "gave_up":
            self = .gaveUp(tries: try c.decodeIfPresent(Int.self, forKey: .tries) ?? 0,
                           message: try c.decodeIfPresent(String.self, forKey: .message) ?? "")
        case let other: self = .unknown(other)
        }
    }
}

public struct PermOption: Codable, Sendable, Equatable {
    public let id: String
    public let name: String
    public let kind: String
}

public struct PermInfo: Codable, Sendable, Equatable, Identifiable {
    public let id: String
    public let session: String
    public let title: String
    public let options: [PermOption]
    /// The ACP tool call this request gates; nil for the daemon's own tools.
    public let toolCall: String?

    private enum CodingKeys: String, CodingKey {
        case id, session, title, options
        case toolCall = "tool_call"
    }
}

public struct PlanEntry: Codable, Sendable, Equatable {
    public let content: String
    public let status: String

    public init(content: String, status: String) {
        self.content = content
        self.status = status
    }
}

/// What a tool call produced or proposes, as the agent reported it.
public enum ToolContent: Sendable, Equatable {
    case text(String)
    /// A file change; `oldText` is nil for a new file.
    case diff(path: String, oldText: String?, newText: String)
    /// A terminal the agent created, by its ACP terminal id.
    case terminal(id: String)
    /// A content type this client does not know yet.
    case other(String)
}

extension ToolContent: Decodable {
    private enum K: String, CodingKey { case type, text, path, old_text, new_text, id }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .type) {
        case "text": self = .text(try c.decode(String.self, forKey: .text))
        case "diff":
            self = .diff(path: try c.decode(String.self, forKey: .path), oldText: try c.decodeIfPresent(String.self, forKey: .old_text),
                         newText: try c.decode(String.self, forKey: .new_text))
        case "terminal": self = .terminal(id: try c.decode(String.self, forKey: .id))
        case let other: self = .other(other)
        }
    }
}

public enum SessionEvent: Sendable, Equatable {
    case user(turn: UInt64, text: String)
    case turnStart(turn: UInt64)
    case text(String)
    case thought(String)
    /// A non-text block in the reply (`image`, `audio`, `resource`, `resource_link`); the bytes never travel.
    case block(kind: String, name: String?)
    case toolCall(id: String, title: String, kind: String, status: String, content: [ToolContent])
    /// `content`, when present, replaces the call's whole content.
    case toolUpdate(id: String, title: String?, status: String?, content: [ToolContent]?)
    case plan([PlanEntry])
    /// The agent changed its own mode (`mode` is the new mode's id) and offers no `mode` option to show it in.
    case mode(String)
    case turnEnd(turn: UInt64, stopReason: String)
    case error(turn: UInt64?, message: String)
    case other(String)
}

extension SessionEvent: Decodable {
    private enum K: String, CodingKey { case kind, turn, text, mode, block_kind, name, id, title, tool_kind, status, content, entries, stop_reason, message }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "user": self = .user(turn: try c.decode(UInt64.self, forKey: .turn), text: try c.decode(String.self, forKey: .text))
        case "turn_start": self = .turnStart(turn: try c.decode(UInt64.self, forKey: .turn))
        case "text": self = .text(try c.decode(String.self, forKey: .text))
        case "thought": self = .thought(try c.decode(String.self, forKey: .text))
        case "block": self = .block(kind: try c.decode(String.self, forKey: .block_kind), name: try c.decodeIfPresent(String.self, forKey: .name))
        case "tool_call":
            self = .toolCall(id: try c.decode(String.self, forKey: .id), title: try c.decode(String.self, forKey: .title),
                             kind: try c.decode(String.self, forKey: .tool_kind), status: try c.decode(String.self, forKey: .status),
                             content: try c.decodeIfPresent([ToolContent].self, forKey: .content) ?? [])
        case "tool_update":
            self = .toolUpdate(id: try c.decode(String.self, forKey: .id), title: try c.decodeIfPresent(String.self, forKey: .title),
                               status: try c.decodeIfPresent(String.self, forKey: .status),
                               content: try c.decodeIfPresent([ToolContent].self, forKey: .content))
        case "plan": self = .plan(try c.decode([PlanEntry].self, forKey: .entries))
        case "mode": self = .mode(try c.decode(String.self, forKey: .mode))
        case "turn_end": self = .turnEnd(turn: try c.decode(UInt64.self, forKey: .turn), stopReason: try c.decode(String.self, forKey: .stop_reason))
        case "error": self = .error(turn: try c.decodeIfPresent(UInt64.self, forKey: .turn), message: try c.decode(String.self, forKey: .message))
        case let other: self = .other(other)
        }
    }
}

/// A session event with the moment the daemon recorded it. `at` is nil for history an agent
/// replayed when it loaded a past conversation, which carries no times.
public struct StampedEvent: Sendable, Equatable, Decodable {
    public let at: Date?
    public let event: SessionEvent

    public init(_ event: SessionEvent, at: Date? = nil) {
        self.event = event
        self.at = at
    }

    private enum K: String, CodingKey { case at }
    public init(from decoder: Decoder) throws {
        // The stamp sits beside the event's own fields, in milliseconds since the epoch.
        let ms = try decoder.container(keyedBy: K.self).decodeIfPresent(UInt64.self, forKey: .at)
        at = ms.map { Date(timeIntervalSince1970: Double($0) / 1000) }
        event = try SessionEvent(from: decoder)
    }
}

/// One control an agent offers over its own session: which model it runs, how much it thinks,
/// whether it asks before editing. The agent names the whole set, so a client draws whatever
/// arrives rather than knowing any option by name.
public struct ConfigOption: Sendable, Equatable, Identifiable, Decodable {
    public let id: String
    /// What to label the control.
    public let name: String
    public let description: String?
    /// What the option is for: `mode`, `model`, `thought_level`, `model_config`, or anything else
    /// an agent names. Nil when the agent gave no category.
    public let category: String?
    public let kind: Kind

    public enum Kind: Sendable, Equatable {
        /// One value out of `choices`; `current` is one of their `value`s.
        case select(current: String, choices: [ConfigChoice])
        case boolean(current: Bool)
        /// A kind of control this wheel cannot draw. Shown, never set.
        case unknown
    }

    private enum K: String, CodingKey { case id, name, description, category, type, current, choices }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        description = try c.decodeIfPresent(String.self, forKey: .description)
        category = try c.decodeIfPresent(String.self, forKey: .category)
        // A control whose payload does not parse becomes `.unknown` rather than throwing: this
        // type is decoded inside `session_attached`, so one malformed option would otherwise drop
        // the whole frame — the transcript and the session's state with it.
        switch try? c.decode(String.self, forKey: .type) {
        case "select":
            if let current = try? c.decode(String.self, forKey: .current) {
                let choices = (try? c.decodeIfPresent([ConfigChoice].self, forKey: .choices)) ?? nil
                kind = .select(current: current, choices: choices ?? [])
            } else {
                kind = .unknown
            }
        case "boolean":
            if let current = try? c.decode(Bool.self, forKey: .current) {
                kind = .boolean(current: current)
            } else {
                kind = .unknown
            }
        default:
            kind = .unknown
        }
    }

    /// What the control shows as its value; nil for a kind this wheel cannot draw.
    public var currentLabel: String? {
        switch kind {
        case let .select(current, choices):
            return choices.first { $0.value == current }?.name ?? current
        case let .boolean(current):
            return current ? "On" : "Off"
        case .unknown:
            return nil
        }
    }
}

/// One value a `select` option can take.
public struct ConfigChoice: Sendable, Equatable, Identifiable, Decodable {
    public var id: String { value }
    public let value: String
    public let name: String
    public let description: String?
}

/// One of wheel's own settings as the daemon resolved it for one space: the value in force, which
/// `wheel.toml` it came from, and what the key is for. The daemon names every key, so a client
/// draws whatever arrives rather than knowing any key by name.
public struct SettingInfo: Sendable, Equatable, Identifiable, Decodable {
    public var id: String { key }
    /// Dotted: `browser.homepage`.
    public let key: String
    public let value: String
    public let origin: SettingOrigin
    /// What the value falls back to when neither file sets it.
    public let `default`: String
    /// One line to show beside the key.
    public let help: String
    /// What this machine's `wheel.toml` sets, when it sets a value the key takes — present even
    /// where the project's value wins over it.
    public let machine: String?
    /// What the `wheel.toml` of the space this list was resolved for sets, likewise.
    public let project: String?
    /// Why a layer this key could have come from sets nothing: a file wheel cannot read, or a
    /// value in it the key does not take. Shown verbatim, since it is why the value is not the
    /// one a person wrote.
    public let problem: String?

    public init(key: String, value: String, origin: SettingOrigin, default: String, help: String,
                machine: String? = nil, project: String? = nil, problem: String? = nil) {
        self.key = key
        self.value = value
        self.origin = origin
        self.default = `default`
        self.help = help
        self.machine = machine
        self.project = project
        self.problem = problem
    }

    /// What a project that sets nothing gets: this machine's value, else wheel's default.
    public var global: String { machine ?? `default` }
}

/// Which layer a setting's value came from: this project's `wheel.toml` wins over this machine's,
/// key by key, and the built-in default is what neither sets.
public enum SettingOrigin: String, Sendable, Equatable, Decodable {
    case `default`, machine, project
}

/// One conversation an agent holds for a directory, as `session/list` reports it. The agent's
/// store belongs to the directory, so this can be one a terminal Claude Code session started.
public struct ConversationInfo: Sendable, Equatable, Identifiable, Decodable {
    /// The agent's own id for it, which `sessionConversationResume` takes.
    public let id: String
    public let cwd: String
    public let title: String?
    /// ISO 8601, as the agent sent it.
    public let updatedAt: String?
    /// Whether the session is in this conversation now.
    public let current: Bool

    private enum CodingKeys: String, CodingKey {
        case id, cwd, title, current
        case updatedAt = "updated_at"
    }

    public init(id: String, cwd: String, title: String?, updatedAt: String?, current: Bool) {
        self.id = id
        self.cwd = cwd
        self.title = title
        self.updatedAt = updatedAt
        self.current = current
    }

    /// `updatedAt` as a date, with or without fractional seconds; nil when absent or unreadable.
    public var updated: Date? {
        guard let updatedAt else { return nil }
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = f.date(from: updatedAt) { return date }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: updatedAt)
    }

    /// How long ago it was last worked in, as a menu row shows it: `5 min. ago`; nil with no date.
    public func age(now: Date = Date()) -> String? {
        guard let updated else { return nil }
        // Under a minute reads "now" rather than counting seconds that are stale by the time the
        // menu opens.
        if now.timeIntervalSince(updated) < 60 { return "just now" }
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .short
        f.locale = Locale(identifier: "en_US_POSIX")
        return f.localizedString(for: updated, relativeTo: now)
    }
}

/// One way to log the lead's agent in, as the agent advertised it.
public struct AuthMethodInfo: Sendable, Equatable, Decodable, Identifiable {
    public let id: String
    public let name: String
    public let description: String?
    public init(id: String, name: String, description: String? = nil) {
        self.id = id
        self.name = name
        self.description = description
    }
}

/// What `claude auth status --json` said for the lead's config directory.
public struct LoginPreflight: Sendable, Equatable, Decodable {
    public let loggedIn: Bool
    public let authMethod: String?
    public let raw: String
    private enum CodingKeys: String, CodingKey { case loggedIn = "logged_in", authMethod = "auth_method", raw }
}

/// The lead's login (proto `LeadLogin`): `state` is `ok`, `needed` or `unknown`.
public struct LeadLogin: Sendable, Equatable, Decodable {
    public let state: String
    public let preflight: LoginPreflight?
    public let agentRequired: Bool
    public let methods: [AuthMethodInfo]
    public let keptMessages: [String]
    public let disagreement: String?
    /// The pane a login is running in, while one runs.
    public let runningPane: String?
    /// How the last login ended: its exit code.
    public let lastExitCode: UInt32?
    public let lastExitSeen: Bool

    private enum CodingKeys: String, CodingKey {
        case state, preflight, methods, disagreement, running, last_exit
        case agentRequired = "agent_required", keptMessages = "kept_messages"
    }
    private struct Run: Decodable { let pane: String }
    private struct Exit: Decodable { let code: UInt32? }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        state = try c.decode(String.self, forKey: .state)
        preflight = try c.decodeIfPresent(LoginPreflight.self, forKey: .preflight)
        agentRequired = try c.decodeIfPresent(Bool.self, forKey: .agentRequired) ?? false
        methods = try c.decodeIfPresent([AuthMethodInfo].self, forKey: .methods) ?? []
        keptMessages = try c.decodeIfPresent([String].self, forKey: .keptMessages) ?? []
        disagreement = try c.decodeIfPresent(String.self, forKey: .disagreement)
        runningPane = try c.decodeIfPresent(Run.self, forKey: .running)?.pane
        let exit = try c.decodeIfPresent(Exit.self, forKey: .last_exit)
        lastExitSeen = exit != nil
        lastExitCode = exit?.code
    }

    /// Whether the lead chat shows the login picker: the lead needs a login, or the two signals
    /// disagree.
    public var showsPicker: Bool { state == "needed" || disagreement != nil }
}

/// What a client says in `renderRefused` (the daemon matches the text).
public enum RenderRefusal {
    /// The client does not show the pane it was asked to draw.
    public static let notShown = "not shown"
}

public enum ServerMsg: Sendable {
    case status(req: UInt64?, version: String, logPath: String, uptimeSecs: UInt64, terms: [TermInfo], lead: SessionState?, perms: [PermInfo],
                spaces: [SpaceInfo], active: String?)
    case termOpened(req: UInt64?, id: String, pid: UInt32?)
    case termAttached(req: UInt64?, id: String, replayBytes: Int)
    case termOutput(id: String, data: Data)
    case termExited(id: String, code: UInt32?)
    case sessionQueued(req: UInt64, session: String, turn: UInt64)
    case sessionAttached(req: UInt64?, session: String, state: SessionState, events: [StampedEvent], config: [ConfigOption], now: Date)
    case session(session: String, event: StampedEvent)
    case sessionStateChanged(session: String, state: SessionState)
    /// The session's config options changed — a set took, or the agent moved one itself.
    /// Replaces the whole list for that session.
    case sessionConfig(session: String, options: [ConfigOption])
    /// The conversations a session's agent holds for its directory, newest first.
    case conversations(req: UInt64, session: String, conversations: [ConversationInfo])
    /// The conversation a session is in after a new or a resume: the agent's id for it.
    case conversation(req: UInt64, session: String, id: String)
    case permRequested(PermInfo)
    case permResolved(id: String, choice: String)
    /// The lead's login after a preflight, in reply to `leadLoginStatus`.
    case leadLoginInfo(req: UInt64?, login: LeadLogin)
    /// The login command exited with `code` (nil: a signal), in reply to `leadLogin`.
    case leadLoginFinished(req: UInt64?, code: UInt32?)
    /// The lead's login changed; sent to every connection.
    case leadLoginChanged(login: LeadLogin)
    /// The whole layout, after any change to spaces, panes or focus.
    case spaces(spaces: [SpaceInfo], active: String?)
    /// A file in web pane `pane`'s folder changed: reload it.
    case paneReload(pane: String)
    /// Browser pane `pane` is showing `url` now — the one field that changed, because a person
    /// browsing changes it far too often to resend the whole layout for. Apply it to the pane
    /// already held; a client with no such pane ignores it.
    case paneUrl(pane: String, url: String)
    /// Wheel's settings resolved for `space` (nil: no space, the machine-wide view), in reply to
    /// `settingsLs` or `settingSet`, and to every connection after a write. A client showing
    /// another space's settings asks again for that space rather than taking this list for it.
    case settings(req: UInt64?, space: String?, settings: [SettingInfo])
    case created(req: UInt64, id: String)
    /// `paneStart` finished: the pane, and for an agent its session's name.
    case started(req: UInt64, pane: String, session: String?)
    /// `pane_browser_open` sent `pane` to `url`.
    case opened(req: UInt64, pane: String, url: String)
    /// Draw `pane` as a PNG and answer with `rendered` or `renderRefused` naming `id`. Sent only to
    /// a connection that sent `renderOffer`.
    case renderPane(id: UInt64, pane: String)
    /// A supervisor, in reply to any `supervisor_*` request about one.
    case supervisor(req: UInt64, info: SupervisorInfo)
    /// A supervisor's loop decisions, in reply to `supervisor_events`.
    case supervisorEvents(req: UInt64, events: [LoopEvent])
    /// What waits on a supervisor's agent, in reply to `supervisor_inbox`.
    case supervisorInbox(req: UInt64, items: [SupervisorInboxItem])
    /// The lead's inbox, in reply to `inbox_ls`.
    case inbox(req: UInt64, items: [InboxItem])
    /// The lead's inbox badge, sent to every connection whenever `new` or `open` changes.
    case inboxCount(new: Int, open: Int)
    case ok(req: UInt64)
    /// A request failed; `term` names the terminal when it was a terminal op.
    case error(req: UInt64?, term: String?, message: String)
    /// A message this client does not know yet.
    case unknown(String)

    public static func decode(_ text: String) throws -> ServerMsg {
        try JSONDecoder().decode(ServerMsg.self, from: Data(text.utf8))
    }
}

extension ServerMsg: Decodable {
    private enum K: String, CodingKey {
        case ev, req, version, log_path, uptime_secs, terms, lead, perms, id, pid, replay_bytes, data, code, turn, state, events, event, session, perm, choice, message, spaces, active, pane, term, config, options, url, space, settings, now, conversations, login, method, items, supervisor, new, open
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        let req = try c.decodeIfPresent(UInt64.self, forKey: .req)
        switch try c.decode(String.self, forKey: .ev) {
        case "status":
            self = .status(req: req, version: try c.decode(String.self, forKey: .version), logPath: try c.decode(String.self, forKey: .log_path),
                           uptimeSecs: try c.decode(UInt64.self, forKey: .uptime_secs),
                           terms: try c.decode([TermInfo].self, forKey: .terms), lead: try c.decodeIfPresent(SessionState.self, forKey: .lead),
                           perms: try c.decodeIfPresent([PermInfo].self, forKey: .perms) ?? [],
                           spaces: try c.decodeIfPresent([SpaceInfo].self, forKey: .spaces) ?? [], active: try c.decodeIfPresent(String.self, forKey: .active))
        case "term_opened": self = .termOpened(req: req, id: try c.decode(String.self, forKey: .id), pid: try c.decodeIfPresent(UInt32.self, forKey: .pid))
        case "term_attached": self = .termAttached(req: req, id: try c.decode(String.self, forKey: .id), replayBytes: try c.decode(Int.self, forKey: .replay_bytes))
        case "term_output":
            let b64 = try c.decode(String.self, forKey: .data)
            guard let data = Data(base64Encoded: b64) else {
                throw DecodingError.dataCorruptedError(forKey: .data, in: c, debugDescription: "term_output data is not base64")
            }
            self = .termOutput(id: try c.decode(String.self, forKey: .id), data: data)
        case "term_exited": self = .termExited(id: try c.decode(String.self, forKey: .id), code: try c.decodeIfPresent(UInt32.self, forKey: .code))
        case "session_queued":
            self = .sessionQueued(req: try c.decode(UInt64.self, forKey: .req), session: try c.decode(String.self, forKey: .session),
                                  turn: try c.decode(UInt64.self, forKey: .turn))
        case "session_attached":
            self = .sessionAttached(req: req, session: try c.decode(String.self, forKey: .session), state: try c.decode(SessionState.self, forKey: .state),
                                    events: try c.decode([StampedEvent].self, forKey: .events),
                                    config: try c.decodeIfPresent([ConfigOption].self, forKey: .config) ?? [],
                                    now: Date(timeIntervalSince1970: Double(try c.decode(UInt64.self, forKey: .now)) / 1000))
        case "session": self = .session(session: try c.decode(String.self, forKey: .session), event: try c.decode(StampedEvent.self, forKey: .event))
        case "session_state_changed":
            self = .sessionStateChanged(session: try c.decode(String.self, forKey: .session), state: try c.decode(SessionState.self, forKey: .state))
        case "session_config":
            self = .sessionConfig(session: try c.decode(String.self, forKey: .session),
                                  options: try c.decodeIfPresent([ConfigOption].self, forKey: .options) ?? [])
        case "conversations":
            self = .conversations(req: try c.decode(UInt64.self, forKey: .req), session: try c.decode(String.self, forKey: .session),
                                  conversations: try c.decode([ConversationInfo].self, forKey: .conversations))
        case "conversation":
            self = .conversation(req: try c.decode(UInt64.self, forKey: .req), session: try c.decode(String.self, forKey: .session),
                                 id: try c.decode(String.self, forKey: .id))
        case "lead_login_info": self = .leadLoginInfo(req: req, login: try c.decode(LeadLogin.self, forKey: .login))
        case "lead_login_finished": self = .leadLoginFinished(req: req, code: try c.decodeIfPresent(UInt32.self, forKey: .code))
        case "lead_login_changed": self = .leadLoginChanged(login: try c.decode(LeadLogin.self, forKey: .login))
        case "perm_requested": self = .permRequested(try c.decode(PermInfo.self, forKey: .perm))
        case "perm_resolved": self = .permResolved(id: try c.decode(String.self, forKey: .id), choice: try c.decode(String.self, forKey: .choice))
        case "spaces": self = .spaces(spaces: try c.decode([SpaceInfo].self, forKey: .spaces), active: try c.decodeIfPresent(String.self, forKey: .active))
        case "pane_reload": self = .paneReload(pane: try c.decode(String.self, forKey: .pane))
        case "pane_url":
            self = .paneUrl(pane: try c.decode(String.self, forKey: .pane), url: try c.decode(String.self, forKey: .url))
        case "settings":
            self = .settings(req: req, space: try c.decodeIfPresent(String.self, forKey: .space),
                             settings: try c.decode([SettingInfo].self, forKey: .settings))
        case "created": self = .created(req: try c.decode(UInt64.self, forKey: .req), id: try c.decode(String.self, forKey: .id))
        case "started": self = .started(req: try c.decode(UInt64.self, forKey: .req), pane: try c.decode(String.self, forKey: .pane), session: try c.decodeIfPresent(String.self, forKey: .session))
        case "opened": self = .opened(req: try c.decode(UInt64.self, forKey: .req), pane: try c.decode(String.self, forKey: .pane), url: try c.decode(String.self, forKey: .url))
        case "render_pane": self = .renderPane(id: try c.decode(UInt64.self, forKey: .id), pane: try c.decode(String.self, forKey: .pane))
        case "supervisor": self = .supervisor(req: try c.decode(UInt64.self, forKey: .req), info: try c.decode(SupervisorInfo.self, forKey: .supervisor))
        case "supervisor_events": self = .supervisorEvents(req: try c.decode(UInt64.self, forKey: .req), events: try c.decode([LoopEvent].self, forKey: .events))
        case "supervisor_inbox": self = .supervisorInbox(req: try c.decode(UInt64.self, forKey: .req), items: try c.decode([SupervisorInboxItem].self, forKey: .items))
        case "inbox": self = .inbox(req: try c.decode(UInt64.self, forKey: .req), items: try c.decode([InboxItem].self, forKey: .items))
        case "inbox_count": self = .inboxCount(new: try c.decode(Int.self, forKey: .new), open: try c.decode(Int.self, forKey: .open))
        case "ok": self = .ok(req: try c.decode(UInt64.self, forKey: .req))
        case "error": self = .error(req: req, term: try c.decodeIfPresent(String.self, forKey: .term), message: try c.decode(String.self, forKey: .message))
        case let other: self = .unknown(other)
        }
    }
}
