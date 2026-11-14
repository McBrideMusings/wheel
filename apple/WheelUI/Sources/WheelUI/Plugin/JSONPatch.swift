import Foundation
import WheelKit

/// An RFC 6902 JSON Patch applied to a `JSONValue`: `add`, `remove`, `replace`, `move`, `copy` and `test`, with
/// RFC 6901 pointers (`~0`, `~1`, `-` for the end of an array). A failing operation throws and leaves nothing applied.
public enum JSONPatch {
    public struct Failure: Error, Equatable, CustomStringConvertible {
        public let description: String
    }

    public static func apply(_ patch: JSONValue, to value: JSONValue) throws -> JSONValue {
        var doc = value
        try apply(patch, to: &doc)
        return doc
    }

    /// Applies the patch to `doc` in place. A caller holding the document's only reference pays nothing per operation:
    /// an append to an array of any length is constant time. A failing operation undoes the ones before it, so `doc`
    /// is unchanged when this throws.
    public static func apply(_ patch: JSONValue, to doc: inout JSONValue) throws {
        guard case let .array(ops) = patch else { throw Failure(description: "a patch is an array of operations") }
        var undo: [Undo] = []
        for (i, op) in ops.enumerated() {
            do { try step(op, in: &doc, undo: &undo) } catch let f as Failure {
                for u in undo.reversed() { u.run(on: &doc) }
                throw Failure(description: "patch operation \(i): \(f.description)")
            }
        }
    }

    /// What puts a document back after one primitive edit.
    private enum Undo {
        case remove([String])
        case insert([String], JSONValue)
        case replace([String], JSONValue)

        func run(on doc: inout JSONValue) {
            var ignored: [Undo] = []
            switch self {
            case let .remove(path): _ = try? JSONPatch.remove(at: path[...], in: &doc, undo: &ignored)
            case let .insert(path, value): try? JSONPatch.insert(value, at: path[...], in: &doc, undo: &ignored)
            case let .replace(path, value): try? JSONPatch.replace(value, at: path[...], in: &doc, undo: &ignored)
            }
        }
    }

    private static func step(_ op: JSONValue, in doc: inout JSONValue, undo: inout [Undo]) throws {
        guard case let .object(fields) = op, case let .string(name)? = fields["op"], case let .string(path)? = fields["path"] else {
            throw Failure(description: "an operation needs op and path")
        }
        let tokens = try parse(path)
        func need(_ key: String) throws -> JSONValue {
            guard let v = fields[key] else { throw Failure(description: "\(name) needs \(key)") }
            return v
        }
        func from() throws -> [String] {
            guard case let .string(p)? = fields["from"] else { throw Failure(description: "\(name) needs from") }
            return try parse(p)
        }
        switch name {
        case "add": try insert(try need("value"), at: tokens[...], in: &doc, undo: &undo)
        case "remove": _ = try remove(at: tokens[...], in: &doc, undo: &undo)
        case "replace":
            _ = try get(tokens[...], in: doc)
            try replace(try need("value"), at: tokens[...], in: &doc, undo: &undo)
        case "move":
            let source = try from()
            if tokens.starts(with: source), tokens.count > source.count { throw Failure(description: "cannot move into itself") }
            let removed = try remove(at: source[...], in: &doc, undo: &undo)
            try insert(removed, at: tokens[...], in: &doc, undo: &undo)
        case "copy": try insert(try get(try from()[...], in: doc), at: tokens[...], in: &doc, undo: &undo)
        case "test":
            guard try get(tokens[...], in: doc) == (try need("value")) else { throw Failure(description: "test failed at \(path)") }
        default: throw Failure(description: "unknown operation \(name)")
        }
    }

    private static func parse(_ pointer: String) throws -> [String] {
        if pointer.isEmpty { return [] }
        guard pointer.hasPrefix("/") else { throw Failure(description: "pointer \(pointer) does not start with /") }
        return pointer.dropFirst().split(separator: "/", omittingEmptySubsequences: false).map {
            $0.replacingOccurrences(of: "~1", with: "/").replacingOccurrences(of: "~0", with: "~")
        }
    }

    private static func index(_ token: String, count: Int, allowEnd: Bool) throws -> Int {
        if allowEnd, token == "-" { return count }
        guard !token.isEmpty, token.allSatisfy(\.isASCII), token.allSatisfy(\.isNumber), token == "0" || !token.hasPrefix("0"),
              let i = Int(token), i < count || (allowEnd && i == count)
        else { throw Failure(description: "no array index \(token)") }
        return i
    }

    private static func get(_ path: ArraySlice<String>, in doc: JSONValue) throws -> JSONValue {
        guard let head = path.first else { return doc }
        switch doc {
        case let .object(o):
            guard let child = o[head] else { throw Failure(description: "no member \(head)") }
            return try get(path.dropFirst(), in: child)
        case let .array(a):
            return try get(path.dropFirst(), in: a[try index(head, count: a.count, allowEnd: false)])
        default: throw Failure(description: "cannot step into a scalar at \(head)")
        }
    }

    /// Walks to the container holding the last token and calls `leaf` on it. Each container is lifted out of its
    /// parent while it is edited, so its storage has one reference and an edit never copies it.
    private static func walk<R>(_ path: ArraySlice<String>, _ doc: inout JSONValue, _ leaf: (inout JSONValue, String) throws -> R) throws -> R {
        guard let head = path.first else { throw Failure(description: "an empty pointer names the whole document") }
        if path.count == 1 { return try leaf(&doc, head) }
        switch doc.take() {
        case var .object(o):
            defer { doc = .object(o) }
            guard o.index(forKey: head) != nil else { throw Failure(description: "no member \(head)") }
            return try walk(path.dropFirst(), &o[head]!, leaf)
        case var .array(a):
            defer { doc = .array(a) }
            let i = try index(head, count: a.count, allowEnd: false)
            return try walk(path.dropFirst(), &a[i], leaf)
        case .scalar: throw Failure(description: "cannot step into a scalar at \(head)")
        }
    }

    private static func replace(_ new: JSONValue, at path: ArraySlice<String>, in doc: inout JSONValue, undo: inout [Undo]) throws {
        if path.isEmpty { undo.append(.replace([], doc)); doc = new; return }
        let full = Array(path)
        let old: JSONValue = try walk(path, &doc) { c, head in
            switch c.take() {
            case var .object(o):
                defer { c = .object(o) }
                guard let old = o.updateValue(new, forKey: head) else { o[head] = nil; throw Failure(description: "no member \(head)") }
                return old
            case var .array(a):
                defer { c = .array(a) }
                let i = try index(head, count: a.count, allowEnd: false)
                let old = a[i]
                a[i] = new
                return old
            case .scalar: throw Failure(description: "cannot step into a scalar at \(head)")
            }
        }
        undo.append(.replace(full, old))
    }

    private static func insert(_ new: JSONValue, at path: ArraySlice<String>, in doc: inout JSONValue, undo: inout [Undo]) throws {
        if path.isEmpty { undo.append(.replace([], doc)); doc = new; return }
        let parent = Array(path.dropLast())
        let reverse: Undo = try walk(path, &doc) { c, head in
            switch c.take() {
            case var .object(o):
                defer { c = .object(o) }
                if let old = o.updateValue(new, forKey: head) { return .replace(parent + [head], old) }
                return .remove(parent + [head])
            case var .array(a):
                defer { c = .array(a) }
                let i = try index(head, count: a.count, allowEnd: true)
                a.insert(new, at: i)
                return .remove(parent + [String(i)])
            case .scalar: throw Failure(description: "cannot step into a scalar at \(head)")
            }
        }
        undo.append(reverse)
    }

    private static func remove(at path: ArraySlice<String>, in doc: inout JSONValue, undo: inout [Undo]) throws -> JSONValue {
        if path.isEmpty { throw Failure(description: "cannot remove the whole document") }
        let full = Array(path)
        let removed: JSONValue = try walk(path, &doc) { c, head in
            switch c.take() {
            case var .object(o):
                defer { c = .object(o) }
                guard let old = o.removeValue(forKey: head) else { throw Failure(description: "no member \(head)") }
                return old
            case var .array(a):
                defer { c = .array(a) }
                return a.remove(at: try index(head, count: a.count, allowEnd: false))
            case .scalar: throw Failure(description: "cannot step into a scalar at \(head)")
            }
        }
        undo.append(.insert(full, removed))
        return removed
    }
}

private extension JSONValue {
    mutating func take() -> Taken {
        switch self {
        case let .object(o): self = .null; return .object(o)
        case let .array(a): self = .null; return .array(a)
        default: return .scalar
        }
    }
}

/// A container's payload moved out of its enum, which is left `.null`, so the payload has one reference and
/// editing it copies nothing. A scalar stays where it is.
private enum Taken {
    case object([String: JSONValue])
    case array([JSONValue])
    case scalar
}

