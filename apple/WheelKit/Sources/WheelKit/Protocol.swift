import Foundation

/// The wheeld wire protocol (proto/src/lib.rs): one JSON object per WebSocket text frame.
/// Client messages are tagged by `op`, daemon messages by `ev`.

public enum ClientMsg: Sendable, Equatable {
    case status(req: UInt64?)
    /// The app's own log lines, oldest first, for wheeld to keep and ship; `client` is `mac`,
    /// `iphone` or `ipad`. Answered `ok` once wheeld has them.
    case log(req: UInt64?, client: String, inode: UInt64, lines: [AppLogLine])
    case termOpen(req: UInt64?, cwd: String?, cols: Int, rows: Int)
    case termAttach(req: UInt64?, id: String)
    case termDetach(id: String)
    /// End a terminal from `termOpen`; the daemon refuses a pane's terminal.
    case termClose(req: UInt64?, id: String)
    case termInput(req: UInt64?, id: String, data: Data)
    case termResize(req: UInt64?, id: String, cols: Int, rows: Int)
    /// `session` is the name an agent pane carries, or the pane's id.
    case sessionAttach(req: UInt64?, session: String)
    case sessionDetach(session: String)
    case sessionSend(req: UInt64?, session: String, text: String)
    case sessionCancel(req: UInt64?, session: String)
    case permAnswer(req: UInt64?, id: String, choice: String)
    case spaceCreate(req: UInt64?, cwd: String)
    /// `ifEmpty`: only while the space holds one empty pane; wheeld refuses it otherwise.
    case spaceClose(req: UInt64?, id: String, ifEmpty: Bool = false)
    /// Name a space; the daemon saves the name and tells every client. An empty name is refused.
    case spaceRename(req: UInt64?, space: String, name: String)
    /// This connection owns the Mac windows, so the daemon may send it `windowRequest`.
    case windowOffer
    /// The answer to `windowRequest` `id`: `data`, a JSON document, when it worked; `error` when not.
    case windowAnswer(id: UInt64, data: String?, error: String?)
    /// The directories spaces were made for, newest first; the daemon answers `spaceRecent`.
    case spaceRecent(req: UInt64?)
    /// The immediate subfolders of `path` on the daemon's machine (`~` starts at its user's home);
    /// the daemon answers `dirList`.
    case dirList(req: UInt64?, path: String)
    /// The folders a person starts browsing for a space from (`spaces.roots`); the daemon answers `dirRoots`.
    case dirRoots(req: UInt64?)
    /// The `[runner]` commands of `space`'s wheel.toml over this machine's; the daemon answers `runners`.
    case runnerLs(req: UInt64?, space: String?)
    /// Run the `[runner]` command `name` in a new terminal pane of `space`; the daemon answers `started`.
    case run(req: UInt64?, name: String, space: String?)
    /// Make a space active, or focus a pane (which also makes its space active).
    case focus(req: UInt64?, space: String?, pane: String?)
    case paneSplit(req: UInt64?, pane: String, dir: Dir)
    /// Swap the places of two panes in one space.
    case paneSwap(req: UInt64?, pane: String, with: String)
    /// Move a pane to the `dir` side of another pane in its space.
    case paneMove(req: UInt64?, pane: String, target: String, dir: Dir)
    case paneClose(req: UInt64?, pane: String)
    /// `artifact` names the artifact a `web` pane shows and `scope` pins which root it is under;
    /// the daemon refuses both for any other kind, and with no `scope` resolves project, then
    /// machine.
    /// `url` is the page a `browser` pane opens, refused for any other kind; with none, the daemon
    /// opens the `browser.homepage` setting as it resolves for that pane's space.
    case paneSetKind(req: UInt64?, pane: String, kind: NewPaneKind, artifact: String? = nil, scope: ArtifactScope? = nil, url: String? = nil)
    /// Start a terminal or an agent in a pane, whatever it showed going. `message` is an agent's
    /// first message. The daemon answers once the agent is ready or has given up, and with an
    /// error when it could not start.
    case paneStart(req: UInt64?, pane: String, kind: StartKind, message: String? = nil, agent: String? = nil)
    /// Show a pane's Claude Code conversation the other way: `agent` turns a terminal running
    /// Claude Code into an agent pane on it, `terminal` an agent pane into a terminal running
    /// `claude --resume` on it. The daemon stops the other view's program first and refuses while
    /// a turn runs; with no `to` it only answers which view the pane is in.
    case paneView(req: UInt64?, pane: String, to: ConversationView? = nil)
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
    /// Take down the notice shown over `session`; every client sees it go.
    case sessionNoticeDismiss(req: UInt64?, session: String)
    /// The coding agents' usage: read now (`dueOnly` false), or only when the last read is older
    /// than the reading interval. Replies `usage`.
    case usage(req: UInt64?, dueOnly: Bool)
    /// How remote clients reach this daemon. Replies `remote`.
    case remoteInfo(req: UInt64?)
    /// Make a one-time pairing token; the daemon answers `pairing`. Local clients only.
    case pairingIssue(req: UInt64?)
    /// Drop one unused pairing token; the daemon answers `ok`. Local clients only.
    case pairingCancel(req: UInt64?, token: String)
    /// The paired devices; the daemon answers `devices`. Local clients only.
    case deviceList(req: UInt64?)
    /// Forget a paired device and close its connections. Local clients only.
    case deviceRevoke(req: UInt64?, id: String)
    /// The artifacts of `space` (the active one with none), project first then machine. With
    /// `paneOnly`, only the ones whose entry says `pane = true`: what a pane picker offers.
    /// Replies `artifacts`.
    case artifactLs(req: UInt64?, space: String?, paneOnly: Bool)
    /// Every built-in artifact wheeld carries; replies `builtins`.
    case builtinLs(req: UInt64?)
    /// Install a built-in at machine scope; replies `builtins`.
    case builtinInstall(req: UInt64?, name: String)
    /// Write the carried version over the installed one; replies `builtins`, or `builtinEdited`
    /// with nothing changed when `onEdited` is `ask` and a person edited a file.
    case builtinUpdate(req: UInt64?, name: String, onEdited: OnEdited)
    /// Remove an installed built-in; replies `builtins`.
    case builtinRemove(req: UInt64?, name: String)
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
        case let .log(req, client, inode, lines):
            o = ["op": "log", "client": client, "inode": inode, "lines": lines.map { line -> [String: Any] in
                var l: [String: Any] = ["ts_ms": line.tsMs, "msg": line.msg]
                if let end = line.end { l["end"] = end }
                return l
            }]; put(req)
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
        case let .spaceRecent(req):
            o = ["op": "space_recent"]; put(req)
        case let .dirList(req, path):
            o = ["op": "dir_list", "path": path]; put(req)
        case let .dirRoots(req):
            o = ["op": "dir_roots"]; put(req)
        case let .runnerLs(req, space):
            o = ["op": "runner_ls"]; put(req)
            if let space { o["space"] = space }
        case let .run(req, name, space):
            o = ["op": "run", "name": name]; put(req)
            if let space { o["space"] = space }
        case let .spaceClose(req, id, ifEmpty):
            o = ["op": "space_close", "id": id]; put(req)
            if ifEmpty { o["if_empty"] = true }
        case let .focus(req, space, pane):
            o = ["op": "focus"]; put(req)
            if let space { o["space"] = space }
            if let pane { o["pane"] = pane }
        case let .paneSplit(req, pane, dir):
            o = ["op": "pane_split", "pane": pane, "dir": dir.rawValue]; put(req)
        case let .paneSwap(req, pane, with):
            o = ["op": "pane_swap", "pane": pane, "with": with]; put(req)
        case let .paneMove(req, pane, target, dir):
            o = ["op": "pane_move", "pane": pane, "target": target, "dir": dir.rawValue]; put(req)
        case let .paneClose(req, pane):
            o = ["op": "pane_close", "pane": pane]; put(req)
        case let .paneSetKind(req, pane, kind, artifact, scope, url):
            o = ["op": "pane_set_kind", "pane": pane, "kind": kind.rawValue]; put(req)
            if let artifact { o["artifact"] = artifact }
            if let scope { o["scope"] = scope.rawValue }
            if let url { o["url"] = url }
        case let .paneView(req, pane, to):
            o = ["op": "pane_view", "pane": pane]; put(req)
            if let to { o["to"] = to.rawValue }
        case let .paneStart(req, pane, kind, message, agent):
            o = ["op": "pane_start", "pane": pane, "kind": kind.rawValue]; put(req)
            if let message { o["message"] = message }
            if let agent { o["agent"] = agent }
        case let .paneBrowserUrl(req, pane, url):
            o = ["op": "pane_browser_url", "pane": pane, "url": url]; put(req)
        case let .paneBrowserOpen(req, pane, text):
            o = ["op": "pane_browser_open", "pane": pane, "text": text]; put(req)
        case let .splitRatio(req, split, ratio):
            o = ["op": "split_ratio", "split": split, "ratio": ratio]; put(req)
        case let .sessionConfigSet(req, session, config, value):
            o = ["op": "session_config_set", "session": session, "config": config, "value": value]; put(req)
        case let .sessionNoticeDismiss(req, session):
            o = ["op": "session_notice_dismiss", "session": session]; put(req)
        case let .usage(req, dueOnly):
            o = ["op": "usage"]; put(req)
            if dueOnly { o["due_only"] = true }
        case let .remoteInfo(req):
            o = ["op": "remote_info"]; put(req)
        case let .pairingIssue(req):
            o = ["op": "pairing_issue"]; put(req)
        case let .pairingCancel(req, token):
            o = ["op": "pairing_cancel", "token": token]; put(req)
        case let .deviceList(req):
            o = ["op": "device_list"]; put(req)
        case let .deviceRevoke(req, id):
            o = ["op": "device_revoke", "id": id]; put(req)
        case let .artifactLs(req, space, paneOnly):
            o = ["op": "artifact_ls"]; put(req)
            if let space { o["space"] = space }
            if paneOnly { o["pane"] = true }
        case let .builtinLs(req):
            o = ["op": "builtin_ls"]; put(req)
        case let .builtinInstall(req, name):
            o = ["op": "builtin_install", "name": name]; put(req)
        case let .builtinUpdate(req, name, onEdited):
            o = ["op": "builtin_update", "name": name, "on_edited": onEdited.rawValue]; put(req)
        case let .builtinRemove(req, name):
            o = ["op": "builtin_remove", "name": name]; put(req)
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
        case let .spaceRename(req, space, name):
            o = ["op": "space_rename", "space": space, "name": name]; put(req)
        case .windowOffer:
            o = ["op": "window_offer"]
        case let .windowAnswer(id, data, error):
            o = ["op": "window_answer", "id": id]
            if let data, let value = try? JSONSerialization.jsonObject(with: Data(data.utf8), options: [.fragmentsAllowed]) { o["data"] = value }
            if let error { o["error"] = error }
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

/// One line an app wrote to its own log.
public struct AppLogLine: Sendable, Equatable {
    /// When the app wrote it, in milliseconds since the Unix epoch.
    public var tsMs: UInt64
    public var msg: String
    /// The offset just past this line in the app's log file, which wheeld uses to drop a line it
    /// already took when the batch is sent again.
    public var end: UInt64?

    public init(tsMs: UInt64, msg: String, end: UInt64? = nil) {
        self.tsMs = tsMs
        self.msg = msg
        self.end = end
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
    case starting, ready, switching
    case busy(queued: Int)
    case failed(message: String)
    case gaveUp(tries: Int, message: String)
    case unknown(String)

    public var label: String {
        switch self {
        case .starting: "Starting"
        case .ready: "Ready"
        case .switching: "Switching to the terminal"
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
        case "switching": self = .switching
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
    /// The agent's id for the tool call this request gates; nil for the daemon's own tools.
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
    /// A content type this client does not know yet.
    case other(String)
}

extension ToolContent: Decodable {
    private enum K: String, CodingKey { case type, text, path, old_text, new_text }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .type) {
        case "text": self = .text(try c.decode(String.self, forKey: .text))
        case "diff":
            self = .diff(path: try c.decode(String.self, forKey: .path), oldText: try c.decodeIfPresent(String.self, forKey: .old_text),
                         newText: try c.decode(String.self, forKey: .new_text))
        case let other: self = .other(other)
        }
    }
}

public enum SessionEvent: Sendable, Equatable {
    case user(turn: UInt64, text: String)
    case turnStart(turn: UInt64)
    case text(String)
    case thought(String)
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
    private enum K: String, CodingKey { case kind, turn, text, mode, id, title, tool_kind, status, content, entries, stop_reason, message }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "user": self = .user(turn: try c.decode(UInt64.self, forKey: .turn), text: try c.decode(String.self, forKey: .text))
        case "turn_start": self = .turnStart(turn: try c.decode(UInt64.self, forKey: .turn))
        case "text": self = .text(try c.decode(String.self, forKey: .text))
        case "thought": self = .thought(try c.decode(String.self, forKey: .text))
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
    /// The key belongs to this machine alone: a project's `wheel.toml` cannot set it, so a
    /// project's page leaves it out.
    public let machineOnly: Bool
    /// Why a layer this key could have come from sets nothing: a file wheel cannot read, or a
    /// value in it the key does not take. Shown verbatim, since it is why the value is not the
    /// one a person wrote.
    public let problem: String?

    private enum CodingKeys: String, CodingKey {
        case key, value, origin, `default`, help, machine, project, problem
        case machineOnly = "machine_only"
    }

    public init(key: String, value: String, origin: SettingOrigin, default: String, help: String,
                machine: String? = nil, project: String? = nil, machineOnly: Bool = false,
                problem: String? = nil) {
        self.key = key
        self.value = value
        self.origin = origin
        self.default = `default`
        self.help = help
        self.machine = machine
        self.project = project
        self.machineOnly = machineOnly
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

/// One `[runner]` entry: a name a person chose, the shell command it runs, and the file that set it.
public struct RunnerInfo: Sendable, Equatable, Decodable {
    public let name: String
    public let command: String
    public let origin: SettingOrigin

    public init(name: String, command: String, origin: SettingOrigin) {
        self.name = name
        self.command = command
        self.origin = origin
    }
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

    // Formatters are built once: a listing holds thousands of rows and each one asks for its date.
    private nonisolated(unsafe) static let fractionalDates: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    private nonisolated(unsafe) static let wholeDates = ISO8601DateFormatter()
    private nonisolated(unsafe) static let relativeDates: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .short
        f.locale = Locale(identifier: "en_US_POSIX")
        return f
    }()

    /// `updatedAt` as a date, with or without fractional seconds; nil when absent or unreadable.
    public var updated: Date? {
        guard let updatedAt else { return nil }
        return Self.fractionalDates.date(from: updatedAt) ?? Self.wholeDates.date(from: updatedAt)
    }

    /// How long ago it was last worked in, as a menu row shows it: `5 min. ago`; nil with no date.
    public func age(now: Date = Date()) -> String? {
        guard let updated else { return nil }
        // Under a minute reads "now" rather than counting seconds that are stale by the time the
        // menu opens.
        if now.timeIntervalSince(updated) < 60 { return "just now" }
        return Self.relativeDates.localizedString(for: updated, relativeTo: now)
    }
}

/// What a client says in `renderRefused` (the daemon matches the text).
public enum RenderRefusal {
    /// The client does not show the pane it was asked to draw.
    public static let notShown = "not shown"
}

/// Whether the daemon can list one folder macOS guards.
public struct FolderAccess: Sendable, Equatable, Decodable {
    public var path: String
    public var allowed: Bool
    public var error: String?

    public init(path: String, allowed: Bool, error: String? = nil) {
        self.path = path
        self.allowed = allowed
        self.error = error
    }
}

/// The program the daemon runs from and the guarded folders it can open.
public struct HostAccess: Sendable, Equatable, Decodable {
    public var exe: String
    public var folders: [FolderAccess]

    public init(exe: String, folders: [FolderAccess]) {
        self.exe = exe
        self.folders = folders
    }
}

/// The PATH wheeld runs with and hands every terminal and agent it starts.
public struct LoginPath: Sendable, Equatable, Decodable {
    public var path: String
    /// Where it came from, as a sentence: the login shell, or the environment wheeld started in.
    public var how: String
    /// Why the login shell gave no PATH when wheeld asked it for one; nil when it gave one or
    /// wheeld did not ask.
    public var problem: String?

    public init(path: String, how: String, problem: String? = nil) {
        self.path = path
        self.how = how
        self.problem = problem
    }
}

/// Whether wheeld's log lines reach the log store at `logs.push_url`.
public struct LogShipping: Sendable, Equatable, Decodable {
    /// The push URL in force; empty when shipping is off.
    public var url: String
    /// `off` (no URL), `waiting` (a URL, and no push to it yet), `ok` (the last push was accepted)
    /// or `failing` (the last push failed or was refused; `error` says why).
    public var state: String
    /// When a push was last accepted, in milliseconds since the Unix epoch.
    public var lastOkMs: UInt64?
    /// Bytes written on the daemon's machine that the store does not have yet.
    public var waitingBytes: UInt64
    /// Why the last push failed; nil once one succeeds.
    public var error: String?

    enum CodingKeys: String, CodingKey {
        case url, state, error
        case lastOkMs = "last_ok_ms"
        case waitingBytes = "waiting_bytes"
    }

    public init(url: String, state: String, lastOkMs: UInt64? = nil, waitingBytes: UInt64 = 0, error: String? = nil) {
        self.url = url
        self.state = state
        self.lastOkMs = lastOkMs
        self.waitingBytes = waitingBytes
        self.error = error
    }
}

/// An agent as `status` lists it: its `agent.<name>.command` and `agent.<name>.home` settings and where that program is.
public struct AgentInfo: Sendable, Equatable, Decodable {
    public var name: String
    /// The command as the setting holds it: a program, with any arguments of its own.
    public var command: String
    /// The `agent.<name>.home` setting: the configuration directory the agent's panes run with.
    /// Nil when it is unset and the agent uses its own default.
    public var home: String?
    /// The program's absolute path, when it was found.
    public var path: String?
    public var found: Bool
    /// Why the program was not found.
    public var why: String?
    /// The skills in the agent's home, which a composer offers after the agent's skill prefix.
    public var skills: [SkillInfo]

    public init(name: String, command: String, home: String? = nil, path: String? = nil, found: Bool, why: String? = nil, skills: [SkillInfo] = []) {
        self.name = name
        self.command = command
        self.home = home
        self.path = path
        self.found = found
        self.why = why
        self.skills = skills
    }

    private enum CodingKeys: String, CodingKey { case name, command, home, path, found, why, skills }

    /// The daemon leaves `skills` out when an agent has none.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            name: try c.decode(String.self, forKey: .name),
            command: try c.decode(String.self, forKey: .command),
            home: try c.decodeIfPresent(String.self, forKey: .home),
            path: try c.decodeIfPresent(String.self, forKey: .path),
            found: try c.decode(Bool.self, forKey: .found),
            why: try c.decodeIfPresent(String.self, forKey: .why),
            skills: try c.decodeIfPresent([SkillInfo].self, forKey: .skills) ?? []
        )
    }
}

/// One skill an agent can run.
public struct SkillInfo: Sendable, Equatable, Decodable {
    public var name: String
    public var description: String

    public init(name: String, description: String = "") {
        self.name = name
        self.description = description
    }
}

/// A `wheel window` command the daemon forwards to the Mac app, which owns the windows.
public enum WindowCommand: Sendable, Equatable {
    case ls
    case new(name: String?, space: String?)
    case rename(window: String, name: String)
    case close(window: String)
    case spaceMove(space: String, to: String)
}

/// A device that paired with the daemon.
public struct DeviceInfo: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let name: String
    public let pairedMs: UInt64
    /// When it last connected or disconnected; while connected, now.
    public let lastSeenMs: UInt64
    /// Connections it has open now.
    public let connected: Int

    public init(id: String, name: String, pairedMs: UInt64, lastSeenMs: UInt64, connected: Int) {
        self.id = id
        self.name = name
        self.pairedMs = pairedMs
        self.lastSeenMs = lastSeenMs
        self.connected = connected
    }

    private enum CodingKeys: String, CodingKey {
        case id, name, connected
        case pairedMs = "paired_ms"
        case lastSeenMs = "last_seen_ms"
    }
}

/// One subfolder in a `dirList` reply; `space` is the id of the space open at it, if any.
public struct DirEntry: Decodable, Sendable, Equatable {
    public let name: String
    public let path: String
    public let space: String?

    public init(name: String, path: String, space: String? = nil) {
        self.name = name
        self.path = path
        self.space = space
    }
}

/// A directory a space was made for, and when (unix ms).
public struct RecentDir: Decodable, Sendable, Equatable {
    public let dir: String
    public let openedMs: UInt64

    public init(dir: String, openedMs: UInt64) {
        self.dir = dir
        self.openedMs = openedMs
    }

    private enum CodingKeys: String, CodingKey { case dir, openedMs = "opened_ms" }
}

public enum ServerMsg: Sendable {
    case status(req: UInt64?, version: String, logPath: String, uptimeSecs: UInt64, terms: [TermInfo], perms: [PermInfo],
                spaces: [SpaceInfo], active: String?, agents: [AgentInfo], shellIntegration: String?, loginPath: LoginPath?,
                logs: LogShipping?, host: HostAccess?)
    case termOpened(req: UInt64?, id: String, pid: UInt32?)
    case termAttached(req: UInt64?, id: String, replayBytes: Int)
    case termOutput(id: String, data: Data)
    case termExited(id: String, code: UInt32?)
    case sessionQueued(req: UInt64, session: String, turn: UInt64)
    case sessionAttached(req: UInt64?, session: String, state: SessionState, events: [StampedEvent], config: [ConfigOption], context: ContextUse?, now: Date)
    case session(session: String, event: StampedEvent)
    case sessionStateChanged(session: String, state: SessionState)
    /// A pane's status, or its space's, changed; `pane` is nil when only the space's did.
    case statusChanged(space: String, pane: String?, status: PaneStatus?, spaceStatus: PaneStatus?)
    /// The session's config options changed — a set took, or the agent moved one itself.
    /// Replaces the whole list for that session.
    case sessionConfig(session: String, options: [ConfigOption])
    /// How full `session`'s context window is, as the agent last reported it; nil once the session
    /// moves to another conversation. A client attaching is told the current figure in `sessionAttached`.
    case sessionContext(session: String, context: ContextUse?)
    /// The slash commands `session`'s agent accepts, by name, once it has listed them; replaces the
    /// whole list. A composer offers only the commands in it.
    case sessionCommands(session: String, commands: [String])
    /// A status for the person over `session` — a conversation that would not reopen — that is up
    /// (`notice` set) or has come down (nil). The daemon keeps it in memory only: it is never a
    /// transcript line, and a client attaching while it is up is sent it again.
    case sessionNotice(session: String, notice: String?)
    /// The conversations a session's agent holds for its directory, newest first.
    case conversations(req: UInt64, session: String, conversations: [ConversationInfo])
    /// The conversation a session is in after a new or a resume: the agent's id for it.
    case conversation(req: UInt64, session: String, id: String)
    case permRequested(PermInfo)
    case permResolved(id: String, choice: String)
    /// The usage board, in reply to `usage`.
    case usage(req: UInt64, board: UsageBoard)
    /// The usage board changed: a reading was taken or the agents or pins settings were written;
    /// sent to every connection.
    case usageChanged(board: UsageBoard)
    /// How remote clients reach the daemon, in reply to `remoteInfo`.
    case remote(req: UInt64, info: RemoteInfo)
    /// A one-time pairing token, in reply to `pairingIssue`; it stops working at `expiresMs`.
    case pairing(req: UInt64, token: String, expiresMs: UInt64)
    /// The paired devices, in reply to `deviceList`.
    case devices(req: UInt64, devices: [DeviceInfo])
    /// The paired devices changed; sent to every connection.
    case devicesChanged(devices: [DeviceInfo])
    /// The first frame of a connection that paired: the credential this device uses from now on.
    case paired(device: String, credential: String)
    /// A remote client connected, whether it paired just now or presented its credential.
    case deviceConnected(device: String, name: String, address: String, paired: Bool)
    /// The whole layout, after any change to spaces, panes or focus.
    case spaces(spaces: [SpaceInfo], active: String?)
    /// A file in web pane `pane`'s folder changed: reload it.
    case paneReload(pane: String)
    /// Artifact `name` has no entry any more: its page's browser storage can go. `cwd` is the
    /// space directory of a `project` artifact, nil for a `machine` one.
    case artifactGone(scope: ArtifactScope, name: String, cwd: String?)
    /// Browser pane `pane` is showing `url` now — the one field that changed, because a person
    /// browsing changes it far too often to resend the whole layout for. Apply it to the pane
    /// already held; a client with no such pane ignores it.
    case paneUrl(pane: String, url: String)
    /// Wheel's settings resolved for `space` (nil: no space, the machine-wide view), in reply to
    /// `settingsLs` or `settingSet`, and to every connection after a write. A client showing
    /// another space's settings asks again for that space rather than taking this list for it.
    case settings(req: UInt64?, space: String?, settings: [SettingInfo])
    /// Every artifact `artifactLs` asked for.
    case artifacts(req: UInt64?, artifacts: [ArtifactInfo])
    /// Every built-in as it now stands, in reply to any built-in request.
    case builtins(req: UInt64, builtins: [BuiltinInfo])
    /// `builtinUpdate` with `ask` found files a person edited and changed nothing.
    case builtinEdited(req: UInt64, name: String, edited: [EditedFile])
    case created(req: UInt64, id: String)
    /// The directories spaces were made for, newest first, in reply to `spaceRecent`.
    case spaceRecent(req: UInt64, dirs: [RecentDir])
    /// The subfolders of `path` (expanded, absolute), in reply to `dirList`; `error` says why there are none.
    case dirList(req: UInt64, path: String, dirs: [DirEntry], error: String?)
    /// The browsing roots as absolute paths, in reply to `dirRoots`.
    case dirRoots(req: UInt64, roots: [String])
    /// The `[runner]` commands, in name order, in reply to `runnerLs`.
    case runners(req: UInt64, runners: [RunnerInfo])
    /// `paneStart` finished: the pane, and for an agent its session's name.
    case started(req: UInt64, pane: String, session: String?)
    /// What `paneView` left the pane showing: its view (nil for a pane that is neither) and the
    /// Claude Code conversation it shows.
    case paneView(req: UInt64, pane: String, view: ConversationView?, conversation: String?)
    /// `pane_browser_open` sent `pane` to `url`.
    case opened(req: UInt64, pane: String, url: String)
    /// Draw `pane` as a PNG and answer with `rendered` or `renderRefused` naming `id`. Sent only to
    /// a connection that sent `renderOffer`.
    case renderPane(id: UInt64, pane: String)
    /// Run `command` on the windows this client owns and answer with `windowAnswer` naming `id`.
    /// Sent only to a connection that sent `windowOffer`.
    case windowRequest(id: UInt64, command: WindowCommand)
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
        case ev, req, notice, version, log_path, uptime_secs, terms, perms, id, pid, replay_bytes, data, code, turn, state, events, event, session, perm, choice, message, spaces, active, pane, term, config, options, url, space, settings, now, board, conversations, commands, artifacts, context, view, conversation, agents, shell_integration, login_path, logs, host, dirs, listen, addresses, port, fingerprint, problem, token, expires_ms, devices, device, credential, name, address, paired, command, window, to, status, space_status, path, error, roots, scope, cwd, builtins, edited, runners
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        let req = try c.decodeIfPresent(UInt64.self, forKey: .req)
        switch try c.decode(String.self, forKey: .ev) {
        case "status":
            self = .status(req: req, version: try c.decode(String.self, forKey: .version), logPath: try c.decode(String.self, forKey: .log_path),
                           uptimeSecs: try c.decode(UInt64.self, forKey: .uptime_secs),
                           terms: try c.decode([TermInfo].self, forKey: .terms),
                           perms: try c.decodeIfPresent([PermInfo].self, forKey: .perms) ?? [],
                           spaces: try c.decodeIfPresent([SpaceInfo].self, forKey: .spaces) ?? [], active: try c.decodeIfPresent(String.self, forKey: .active),
                           agents: try c.decode([AgentInfo].self, forKey: .agents),
                           shellIntegration: try c.decodeIfPresent(String.self, forKey: .shell_integration),
                           loginPath: try c.decodeIfPresent(LoginPath.self, forKey: .login_path),
                           logs: try c.decodeIfPresent(LogShipping.self, forKey: .logs),
                           host: try c.decodeIfPresent(HostAccess.self, forKey: .host))
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
                                    context: try c.decodeIfPresent(ContextUse.self, forKey: .context),
                                    now: Date(timeIntervalSince1970: Double(try c.decode(UInt64.self, forKey: .now)) / 1000))
        case "session": self = .session(session: try c.decode(String.self, forKey: .session), event: try c.decode(StampedEvent.self, forKey: .event))
        case "session_state_changed":
            self = .sessionStateChanged(session: try c.decode(String.self, forKey: .session), state: try c.decode(SessionState.self, forKey: .state))
        case "status_changed":
            self = .statusChanged(space: try c.decode(String.self, forKey: .space), pane: try c.decodeIfPresent(String.self, forKey: .pane),
                                  status: try? c.decodeIfPresent(PaneStatus.self, forKey: .status),
                                  spaceStatus: try? c.decodeIfPresent(PaneStatus.self, forKey: .space_status))
        case "session_config":
            self = .sessionConfig(session: try c.decode(String.self, forKey: .session),
                                  options: try c.decodeIfPresent([ConfigOption].self, forKey: .options) ?? [])
        case "session_context":
            self = .sessionContext(session: try c.decode(String.self, forKey: .session), context: try c.decodeIfPresent(ContextUse.self, forKey: .context))
        case "session_commands":
            self = .sessionCommands(session: try c.decode(String.self, forKey: .session), commands: try c.decode([String].self, forKey: .commands))
        case "session_notice":
            self = .sessionNotice(session: try c.decode(String.self, forKey: .session), notice: try c.decodeIfPresent(String.self, forKey: .notice))
        case "conversations":
            self = .conversations(req: try c.decode(UInt64.self, forKey: .req), session: try c.decode(String.self, forKey: .session),
                                  conversations: try c.decode([ConversationInfo].self, forKey: .conversations))
        case "conversation":
            self = .conversation(req: try c.decode(UInt64.self, forKey: .req), session: try c.decode(String.self, forKey: .session),
                                 id: try c.decode(String.self, forKey: .id))
        case "usage": self = .usage(req: try c.decode(UInt64.self, forKey: .req), board: try c.decode(UsageBoard.self, forKey: .board))
        case "usage_changed": self = .usageChanged(board: try c.decode(UsageBoard.self, forKey: .board))
        case "remote":
            self = .remote(req: try c.decode(UInt64.self, forKey: .req), info: RemoteInfo(
                listen: try c.decodeIfPresent(String.self, forKey: .listen),
                addresses: try c.decodeIfPresent([String].self, forKey: .addresses) ?? [],
                port: try c.decodeIfPresent(UInt16.self, forKey: .port) ?? 0,
                fingerprint: try c.decodeIfPresent(String.self, forKey: .fingerprint) ?? "",
                problem: try c.decodeIfPresent(String.self, forKey: .problem)))
        case "perm_requested": self = .permRequested(try c.decode(PermInfo.self, forKey: .perm))
        case "perm_resolved": self = .permResolved(id: try c.decode(String.self, forKey: .id), choice: try c.decode(String.self, forKey: .choice))
        case "pairing":
            self = .pairing(req: try c.decode(UInt64.self, forKey: .req), token: try c.decode(String.self, forKey: .token),
                            expiresMs: try c.decode(UInt64.self, forKey: .expires_ms))
        case "devices": self = .devices(req: try c.decode(UInt64.self, forKey: .req), devices: try c.decode([DeviceInfo].self, forKey: .devices))
        case "devices_changed": self = .devicesChanged(devices: try c.decode([DeviceInfo].self, forKey: .devices))
        case "paired": self = .paired(device: try c.decode(String.self, forKey: .device), credential: try c.decode(String.self, forKey: .credential))
        case "device_connected":
            self = .deviceConnected(device: try c.decode(String.self, forKey: .device), name: try c.decode(String.self, forKey: .name),
                                    address: try c.decode(String.self, forKey: .address), paired: try c.decode(Bool.self, forKey: .paired))
        case "spaces": self = .spaces(spaces: try c.decode([SpaceInfo].self, forKey: .spaces), active: try c.decodeIfPresent(String.self, forKey: .active))
        case "pane_reload": self = .paneReload(pane: try c.decode(String.self, forKey: .pane))
        case "artifact_gone":
            self = .artifactGone(scope: try c.decode(ArtifactScope.self, forKey: .scope), name: try c.decode(String.self, forKey: .name),
                                 cwd: try c.decodeIfPresent(String.self, forKey: .cwd))
        case "pane_url":
            self = .paneUrl(pane: try c.decode(String.self, forKey: .pane), url: try c.decode(String.self, forKey: .url))
        case "settings":
            self = .settings(req: req, space: try c.decodeIfPresent(String.self, forKey: .space),
                             settings: try c.decode([SettingInfo].self, forKey: .settings))
        case "artifacts": self = .artifacts(req: req, artifacts: try c.decode([ArtifactInfo].self, forKey: .artifacts))
        case "builtins": self = .builtins(req: try c.decode(UInt64.self, forKey: .req), builtins: try c.decode([BuiltinInfo].self, forKey: .builtins))
        case "builtin_edited":
            self = .builtinEdited(req: try c.decode(UInt64.self, forKey: .req), name: try c.decode(String.self, forKey: .name),
                                  edited: try c.decode([EditedFile].self, forKey: .edited))
        case "created": self = .created(req: try c.decode(UInt64.self, forKey: .req), id: try c.decode(String.self, forKey: .id))
        case "space_recent": self = .spaceRecent(req: try c.decode(UInt64.self, forKey: .req), dirs: try c.decode([RecentDir].self, forKey: .dirs))
        case "dir_list":
            self = .dirList(req: try c.decode(UInt64.self, forKey: .req), path: try c.decode(String.self, forKey: .path),
                            dirs: try c.decode([DirEntry].self, forKey: .dirs), error: try c.decodeIfPresent(String.self, forKey: .error))
        case "dir_roots": self = .dirRoots(req: try c.decode(UInt64.self, forKey: .req), roots: try c.decode([String].self, forKey: .roots))
        case "runners": self = .runners(req: try c.decode(UInt64.self, forKey: .req), runners: try c.decode([RunnerInfo].self, forKey: .runners))
        case "started": self = .started(req: try c.decode(UInt64.self, forKey: .req), pane: try c.decode(String.self, forKey: .pane), session: try c.decodeIfPresent(String.self, forKey: .session))
        case "pane_view":
            self = .paneView(req: try c.decode(UInt64.self, forKey: .req), pane: try c.decode(String.self, forKey: .pane),
                             view: try c.decodeIfPresent(String.self, forKey: .view).flatMap(ConversationView.init(rawValue:)),
                             conversation: try c.decodeIfPresent(String.self, forKey: .conversation))
        case "opened": self = .opened(req: try c.decode(UInt64.self, forKey: .req), pane: try c.decode(String.self, forKey: .pane), url: try c.decode(String.self, forKey: .url))
        case "window_request":
            let command = try c.decode(String.self, forKey: .command)
            func text(_ key: K) throws -> String { try c.decode(String.self, forKey: key) }
            switch command {
            case "ls": self = .windowRequest(id: try c.decode(UInt64.self, forKey: .id), command: .ls)
            case "new": self = .windowRequest(id: try c.decode(UInt64.self, forKey: .id), command: .new(name: try c.decodeIfPresent(String.self, forKey: .name), space: try c.decodeIfPresent(String.self, forKey: .space)))
            case "rename": self = .windowRequest(id: try c.decode(UInt64.self, forKey: .id), command: .rename(window: try text(.window), name: try text(.name)))
            case "close": self = .windowRequest(id: try c.decode(UInt64.self, forKey: .id), command: .close(window: try text(.window)))
            case "space_move": self = .windowRequest(id: try c.decode(UInt64.self, forKey: .id), command: .spaceMove(space: try text(.space), to: try text(.to)))
            default: self = .unknown("window_request \(command)")
            }
        case "render_pane": self = .renderPane(id: try c.decode(UInt64.self, forKey: .id), pane: try c.decode(String.self, forKey: .pane))
        case "ok": self = .ok(req: try c.decode(UInt64.self, forKey: .req))
        case "error": self = .error(req: req, term: try c.decodeIfPresent(String.self, forKey: .term), message: try c.decode(String.self, forKey: .message))
        case let other: self = .unknown(other)
        }
    }
}
