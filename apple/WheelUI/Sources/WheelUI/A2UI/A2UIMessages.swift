import A2UISwiftCore
import Foundation
import WheelKit

/// A2UI over MCP (a2ui.org's "A2UI over MCP" guide) as a pane reads it: a tool whose `_meta.ui`
/// names an `a2ui://` layout resource, the resource's `createSurface` and `updateComponents`
/// messages, and the `updateDataModel` messages a tool call returns as an embedded resource of the
/// same MIME type.
public enum A2UIMessages {
    /// The MIME types the layout and the data travel as. The guide writes the first; the second is
    /// the order some servers write it in.
    public static let mimeTypes: Set<String> = ["application/a2ui+json", "application/json+a2ui"]

    /// The tool and layout resource a server's tool list links, from the first tool whose
    /// `_meta.ui` names an A2UI resource.
    public struct Link: Equatable, Sendable {
        public let tool: String
        public let uri: String
    }

    public static func link(in tools: [JSONValue]) -> Link? {
        for tool in tools {
            guard case let .object(fields) = tool, case let .string(name)? = fields["name"],
                  case let .object(meta)? = fields["meta"], case let .object(ui)? = meta["ui"],
                  case let .string(uri)? = ui["resourceUri"] else { continue }
            let mime: String? = if case let .string(m)? = ui["mimeType"] { m } else { nil }
            if let mime, mimeTypes.contains(mime) || uri.hasPrefix("a2ui://") { return Link(tool: name, uri: uri) }
            if mime == nil, uri.hasPrefix("a2ui://") { return Link(tool: name, uri: uri) }
        }
        return nil
    }

    /// What the messages of a document or a tool result came to.
    public struct Decoded {
        public var messages: [A2uiMessage]
        /// The names of the components the catalog does not hold; each is drawn as an
        /// "unsupported" placeholder instead of being dropped.
        public var unsupported: [String]
    }

    public enum Failure: Error, Equatable, CustomStringConvertible {
        case notA2UI(String)
        public var description: String { switch self { case let .notA2UI(why): why } }
    }

    /// The messages in `text`: one message object or an array of them. A component whose name is
    /// not in `catalog` becomes an `Unsupported` component carrying that name.
    public static func decode(text: String, catalog: Set<String>) throws -> Decoded {
        guard let data = text.data(using: .utf8), var json = try? JSONSerialization.jsonObject(with: data) else {
            throw Failure.notA2UI("the text is not JSON")
        }
        if let one = json as? [String: Any] { json = [one] }
        guard var list = json as? [[String: Any]] else { throw Failure.notA2UI("the JSON is neither a message nor a list of messages") }
        var unsupported: [String] = []
        for i in list.indices {
            guard var update = list[i]["updateComponents"] as? [String: Any],
                  var components = update["components"] as? [[String: Any]] else { continue }
            for j in components.indices {
                guard let name = components[j]["component"] as? String, !catalog.contains(name) else { continue }
                unsupported.append(name)
                components[j] = ["id": components[j]["id"] ?? "", "component": "Unsupported", "name": name]
            }
            update["components"] = components
            list[i]["updateComponents"] = update
        }
        let rewritten = try JSONSerialization.data(withJSONObject: list)
        do {
            return Decoded(messages: try JSONDecoder().decode([A2uiMessage].self, from: rewritten), unsupported: unsupported)
        } catch {
            throw Failure.notA2UI("not A2UI messages: \(error)")
        }
    }

    /// The text of a resource's first content, when it is an A2UI document.
    public static func text(ofResource contents: [JSONValue]) -> String? {
        guard case let .object(first)? = contents.first, case let .string(text)? = first["text"] else { return nil }
        return text
    }

    /// The A2UI text blocks of a tool result: its embedded resources of an A2UI MIME type. A result
    /// with none falls back to a text block that parses as A2UI.
    public static func texts(ofResult result: JSONValue) -> [String] {
        guard case let .object(fields) = result, case let .array(content)? = fields["content"] else { return [] }
        var embedded: [String] = []
        var plain: [String] = []
        for block in content {
            guard case let .object(b) = block else { continue }
            if case let .object(r)? = b["resource"], case let .string(mime)? = r["mimeType"], mimeTypes.contains(mime),
               case let .string(text)? = r["text"] {
                embedded.append(text)
            } else if case let .string(text)? = b["text"] {
                plain.append(text)
            }
        }
        return embedded.isEmpty ? plain : embedded
    }

    /// Whether the result says the tool failed, and the first text it gave.
    public static func failure(ofResult result: JSONValue) -> String? {
        guard case let .object(fields) = result, case .bool(true)? = fields["is_error"] else { return nil }
        guard case let .array(content)? = fields["content"] else { return "the tool failed" }
        for block in content { if case let .object(b) = block, case let .string(text)? = b["text"] { return text } }
        return "the tool failed"
    }

    /// An action's context as the arguments of the tool it names.
    public static func arguments(_ action: A2uiClientAction) -> String {
        guard let data = try? JSONEncoder().encode(action.context) else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }
}
