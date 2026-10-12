import Foundation

/// The wheeld wire protocol (proto/src/lib.rs): one JSON object per WebSocket text frame.
/// Client messages are tagged by `op`, daemon messages by `ev`.

public enum ClientMsg: Sendable, Equatable {
    case status(req: UInt64?)
    case termOpen(req: UInt64?, cwd: String?, cols: Int, rows: Int)
    case termAttach(req: UInt64?, id: String)
    case termDetach(id: String)
    case termInput(req: UInt64?, id: String, data: Data)
    case termResize(req: UInt64?, id: String, cols: Int, rows: Int)
    case leadAttach(req: UInt64?)
    case leadSend(req: UInt64?, text: String)
    case leadCancel(req: UInt64?)
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
        case let .termInput(req, id, data):
            o = ["op": "term_input", "id": id, "data": data.base64EncodedString()]; put(req)
        case let .termResize(req, id, cols, rows):
            o = ["op": "term_resize", "id": id, "cols": cols, "rows": rows]; put(req)
        case let .leadAttach(req):
            o = ["op": "lead_attach"]; put(req)
        case let .leadSend(req, text):
            o = ["op": "lead_send", "text": text]; put(req)
        case let .leadCancel(req):
            o = ["op": "lead_cancel"]; put(req)
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
    public let bufferedBytes: Int

    private enum CodingKeys: String, CodingKey {
        case id, pid, cwd, cols, rows, alive
        case bufferedBytes = "buffered_bytes"
    }
}

public enum LeadState: Sendable, Equatable {
    case starting, ready
    case busy(queued: Int)
    case failed(message: String)
    case unknown(String)

    public var label: String {
        switch self {
        case .starting: "Starting"
        case .ready: "Ready"
        case let .busy(queued): queued > 0 ? "Working · \(queued) queued" : "Working"
        case let .failed(message): "Stopped: \(message)"
        case let .unknown(phase): phase
        }
    }
}

extension LeadState: Decodable {
    private enum K: String, CodingKey { case phase, queued, message }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .phase) {
        case "starting": self = .starting
        case "ready": self = .ready
        case "busy": self = .busy(queued: try c.decodeIfPresent(Int.self, forKey: .queued) ?? 0)
        case "failed": self = .failed(message: try c.decodeIfPresent(String.self, forKey: .message) ?? "")
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
}

public struct PlanEntry: Codable, Sendable, Equatable {
    public let content: String
    public let status: String

    public init(content: String, status: String) {
        self.content = content
        self.status = status
    }
}

public enum LeadEvent: Sendable, Equatable {
    case user(turn: UInt64, text: String)
    case text(String)
    case toolCall(id: String, title: String, kind: String, status: String)
    case toolUpdate(id: String, title: String?, status: String?)
    case plan([PlanEntry])
    case turnEnd(turn: UInt64, stopReason: String)
    case error(turn: UInt64?, message: String)
    case other(String)
}

extension LeadEvent: Decodable {
    private enum K: String, CodingKey { case kind, turn, text, id, title, tool_kind, status, entries, stop_reason, message }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "user": self = .user(turn: try c.decode(UInt64.self, forKey: .turn), text: try c.decode(String.self, forKey: .text))
        case "text": self = .text(try c.decode(String.self, forKey: .text))
        case "tool_call":
            self = .toolCall(id: try c.decode(String.self, forKey: .id), title: try c.decode(String.self, forKey: .title),
                             kind: try c.decode(String.self, forKey: .tool_kind), status: try c.decode(String.self, forKey: .status))
        case "tool_update":
            self = .toolUpdate(id: try c.decode(String.self, forKey: .id), title: try c.decodeIfPresent(String.self, forKey: .title),
                               status: try c.decodeIfPresent(String.self, forKey: .status))
        case "plan": self = .plan(try c.decode([PlanEntry].self, forKey: .entries))
        case "turn_end": self = .turnEnd(turn: try c.decode(UInt64.self, forKey: .turn), stopReason: try c.decode(String.self, forKey: .stop_reason))
        case "error": self = .error(turn: try c.decodeIfPresent(UInt64.self, forKey: .turn), message: try c.decode(String.self, forKey: .message))
        case let other: self = .other(other)
        }
    }
}

public enum ServerMsg: Sendable {
    case status(req: UInt64?, version: String, logPath: String, terms: [TermInfo], lead: LeadState?, perms: [PermInfo],
                spaces: [SpaceInfo], active: String?)
    case termOpened(req: UInt64?, id: String, pid: UInt32?)
    case termAttached(req: UInt64?, id: String, replayBytes: Int)
    case termOutput(id: String, data: Data)
    case termExited(id: String, code: UInt32?)
    case leadQueued(req: UInt64, turn: UInt64)
    case leadAttached(req: UInt64?, state: LeadState, events: [LeadEvent])
    case lead(LeadEvent)
    case leadStateChanged(LeadState)
    case permRequested(PermInfo)
    case permResolved(id: String, choice: String)
    /// The whole layout, after any change to spaces, panes or focus.
    case spaces(spaces: [SpaceInfo], active: String?)
    /// A file in web pane `pane`'s folder changed: reload it.
    case paneReload(pane: String)
    case created(req: UInt64, id: String)
    case ok(req: UInt64)
    case error(req: UInt64?, message: String)
    /// A message this client does not know yet.
    case unknown(String)

    public static func decode(_ text: String) throws -> ServerMsg {
        try JSONDecoder().decode(ServerMsg.self, from: Data(text.utf8))
    }
}

extension ServerMsg: Decodable {
    private enum K: String, CodingKey {
        case ev, req, version, log_path, terms, lead, perms, id, pid, replay_bytes, data, code, turn, state, events, event, perm, choice, message, spaces, active, pane
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        let req = try c.decodeIfPresent(UInt64.self, forKey: .req)
        switch try c.decode(String.self, forKey: .ev) {
        case "status":
            self = .status(req: req, version: try c.decode(String.self, forKey: .version), logPath: try c.decode(String.self, forKey: .log_path),
                           terms: try c.decode([TermInfo].self, forKey: .terms), lead: try c.decodeIfPresent(LeadState.self, forKey: .lead),
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
        case "lead_queued": self = .leadQueued(req: try c.decode(UInt64.self, forKey: .req), turn: try c.decode(UInt64.self, forKey: .turn))
        case "lead_attached": self = .leadAttached(req: req, state: try c.decode(LeadState.self, forKey: .state), events: try c.decode([LeadEvent].self, forKey: .events))
        case "lead": self = .lead(try c.decode(LeadEvent.self, forKey: .event))
        case "lead_state_changed": self = .leadStateChanged(try c.decode(LeadState.self, forKey: .state))
        case "perm_requested": self = .permRequested(try c.decode(PermInfo.self, forKey: .perm))
        case "perm_resolved": self = .permResolved(id: try c.decode(String.self, forKey: .id), choice: try c.decode(String.self, forKey: .choice))
        case "spaces": self = .spaces(spaces: try c.decode([SpaceInfo].self, forKey: .spaces), active: try c.decodeIfPresent(String.self, forKey: .active))
        case "pane_reload": self = .paneReload(pane: try c.decode(String.self, forKey: .pane))
        case "created": self = .created(req: try c.decode(UInt64.self, forKey: .req), id: try c.decode(String.self, forKey: .id))
        case "ok": self = .ok(req: try c.decode(UInt64.self, forKey: .req))
        case "error": self = .error(req: req, message: try c.decode(String.self, forKey: .message))
        case let other: self = .unknown(other)
        }
    }
}
