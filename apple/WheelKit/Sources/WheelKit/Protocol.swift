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
    /// `dir` is the folder a `web` pane shows; the daemon refuses it for any other kind.
    case paneSetKind(req: UInt64?, pane: String, kind: NewPaneKind, dir: String? = nil)
    case splitRatio(req: UInt64?, split: String, ratio: Double)

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
        case let .paneSetKind(req, pane, kind, dir):
            o = ["op": "pane_set_kind", "pane": pane, "kind": kind.rawValue]; put(req)
            if let dir { o["dir"] = dir }
        case let .splitRatio(req, split, ratio):
            o = ["op": "split_ratio", "split": split, "ratio": ratio]; put(req)
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
    case toolCall(id: String, title: String, kind: String, status: String, content: [ToolContent])
    /// `content`, when present, replaces the call's whole content.
    case toolUpdate(id: String, title: String?, status: String?, content: [ToolContent]?)
    case plan([PlanEntry])
    case turnEnd(turn: UInt64, stopReason: String)
    case error(turn: UInt64?, message: String)
    case other(String)
}

extension SessionEvent: Decodable {
    private enum K: String, CodingKey { case kind, turn, text, id, title, tool_kind, status, content, entries, stop_reason, message }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "user": self = .user(turn: try c.decode(UInt64.self, forKey: .turn), text: try c.decode(String.self, forKey: .text))
        case "turn_start": self = .turnStart(turn: try c.decode(UInt64.self, forKey: .turn))
        case "text": self = .text(try c.decode(String.self, forKey: .text))
        case "tool_call":
            self = .toolCall(id: try c.decode(String.self, forKey: .id), title: try c.decode(String.self, forKey: .title),
                             kind: try c.decode(String.self, forKey: .tool_kind), status: try c.decode(String.self, forKey: .status),
                             content: try c.decodeIfPresent([ToolContent].self, forKey: .content) ?? [])
        case "tool_update":
            self = .toolUpdate(id: try c.decode(String.self, forKey: .id), title: try c.decodeIfPresent(String.self, forKey: .title),
                               status: try c.decodeIfPresent(String.self, forKey: .status),
                               content: try c.decodeIfPresent([ToolContent].self, forKey: .content))
        case "plan": self = .plan(try c.decode([PlanEntry].self, forKey: .entries))
        case "turn_end": self = .turnEnd(turn: try c.decode(UInt64.self, forKey: .turn), stopReason: try c.decode(String.self, forKey: .stop_reason))
        case "error": self = .error(turn: try c.decodeIfPresent(UInt64.self, forKey: .turn), message: try c.decode(String.self, forKey: .message))
        case let other: self = .other(other)
        }
    }
}

public enum ServerMsg: Sendable {
    case status(req: UInt64?, version: String, logPath: String, terms: [TermInfo], lead: SessionState?, perms: [PermInfo],
                spaces: [SpaceInfo], active: String?)
    case termOpened(req: UInt64?, id: String, pid: UInt32?)
    case termAttached(req: UInt64?, id: String, replayBytes: Int)
    case termOutput(id: String, data: Data)
    case termExited(id: String, code: UInt32?)
    case sessionQueued(req: UInt64, session: String, turn: UInt64)
    case sessionAttached(req: UInt64?, session: String, state: SessionState, events: [SessionEvent])
    case session(session: String, event: SessionEvent)
    case sessionStateChanged(session: String, state: SessionState)
    case permRequested(PermInfo)
    case permResolved(id: String, choice: String)
    /// The whole layout, after any change to spaces, panes or focus.
    case spaces(spaces: [SpaceInfo], active: String?)
    /// A file in web pane `pane`'s folder changed: reload it.
    case paneReload(pane: String)
    case created(req: UInt64, id: String)
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
        case ev, req, version, log_path, terms, lead, perms, id, pid, replay_bytes, data, code, turn, state, events, event, session, perm, choice, message, spaces, active, pane, term
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        let req = try c.decodeIfPresent(UInt64.self, forKey: .req)
        switch try c.decode(String.self, forKey: .ev) {
        case "status":
            self = .status(req: req, version: try c.decode(String.self, forKey: .version), logPath: try c.decode(String.self, forKey: .log_path),
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
                                    events: try c.decode([SessionEvent].self, forKey: .events))
        case "session": self = .session(session: try c.decode(String.self, forKey: .session), event: try c.decode(SessionEvent.self, forKey: .event))
        case "session_state_changed":
            self = .sessionStateChanged(session: try c.decode(String.self, forKey: .session), state: try c.decode(SessionState.self, forKey: .state))
        case "perm_requested": self = .permRequested(try c.decode(PermInfo.self, forKey: .perm))
        case "perm_resolved": self = .permResolved(id: try c.decode(String.self, forKey: .id), choice: try c.decode(String.self, forKey: .choice))
        case "spaces": self = .spaces(spaces: try c.decode([SpaceInfo].self, forKey: .spaces), active: try c.decodeIfPresent(String.self, forKey: .active))
        case "pane_reload": self = .paneReload(pane: try c.decode(String.self, forKey: .pane))
        case "created": self = .created(req: try c.decode(UInt64.self, forKey: .req), id: try c.decode(String.self, forKey: .id))
        case "ok": self = .ok(req: try c.decode(UInt64.self, forKey: .req))
        case "error": self = .error(req: req, term: try c.decodeIfPresent(String.self, forKey: .term), message: try c.decode(String.self, forKey: .message))
        case let other: self = .unknown(other)
        }
    }
}
