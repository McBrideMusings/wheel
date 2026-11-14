import Foundation
import WheelKit

/// An RFC 6902 JSON Patch applied to a `JSONValue`: `add`, `remove`, `replace`, `move`, `copy` and `test`, with
/// RFC 6901 pointers (`~0`, `~1`, `-` for the end of an array). A failing operation throws and leaves nothing applied.
public enum JSONPatch {
    public struct Failure: Error, Equatable, CustomStringConvertible {
        public let description: String
    }

    public static func apply(_ patch: JSONValue, to value: JSONValue) throws -> JSONValue {
        guard case let .array(ops) = patch else { throw Failure(description: "a patch is an array of operations") }
        var doc = value
        for (i, op) in ops.enumerated() {
            do { doc = try step(op, to: doc) } catch let f as Failure {
                throw Failure(description: "patch operation \(i): \(f.description)")
            }
        }
        return doc
    }

    private static func step(_ op: JSONValue, to doc: JSONValue) throws -> JSONValue {
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
        case "add": return try insert(try need("value"), at: tokens[...], in: doc)
        case "remove": return try remove(at: tokens[...], in: doc).doc
        case "replace":
            _ = try get(tokens[...], in: doc)
            return try replace(try need("value"), at: tokens[...], in: doc)
        case "move":
            let source = try from()
            if tokens.starts(with: source), tokens.count > source.count { throw Failure(description: "cannot move into itself") }
            let removed = try remove(at: source[...], in: doc)
            return try insert(removed.value, at: tokens[...], in: removed.doc)
        case "copy": return try insert(try get(try from()[...], in: doc), at: tokens[...], in: doc)
        case "test":
            guard try get(tokens[...], in: doc) == (try need("value")) else { throw Failure(description: "test failed at \(path)") }
            return doc
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

    private static func replace(_ new: JSONValue, at path: ArraySlice<String>, in doc: JSONValue) throws -> JSONValue {
        guard let head = path.first else { return new }
        switch doc {
        case var .object(o):
            o[head] = try replace(new, at: path.dropFirst(), in: o[head] ?? .null)
            return .object(o)
        case var .array(a):
            let i = try index(head, count: a.count, allowEnd: false)
            a[i] = try replace(new, at: path.dropFirst(), in: a[i])
            return .array(a)
        default: throw Failure(description: "cannot step into a scalar at \(head)")
        }
    }

    private static func insert(_ new: JSONValue, at path: ArraySlice<String>, in doc: JSONValue) throws -> JSONValue {
        guard let head = path.first else { return new }
        let last = path.count == 1
        switch doc {
        case var .object(o):
            if last { o[head] = new } else {
                guard let child = o[head] else { throw Failure(description: "no member \(head)") }
                o[head] = try insert(new, at: path.dropFirst(), in: child)
            }
            return .object(o)
        case var .array(a):
            if last { a.insert(new, at: try index(head, count: a.count, allowEnd: true)) } else {
                let i = try index(head, count: a.count, allowEnd: false)
                a[i] = try insert(new, at: path.dropFirst(), in: a[i])
            }
            return .array(a)
        default: throw Failure(description: "cannot step into a scalar at \(head)")
        }
    }

    private static func remove(at path: ArraySlice<String>, in doc: JSONValue) throws -> (doc: JSONValue, value: JSONValue) {
        guard let head = path.first else { throw Failure(description: "cannot remove the whole document") }
        let last = path.count == 1
        switch doc {
        case var .object(o):
            guard let child = o[head] else { throw Failure(description: "no member \(head)") }
            if last { o[head] = nil; return (.object(o), child) }
            let r = try remove(at: path.dropFirst(), in: child)
            o[head] = r.doc
            return (.object(o), r.value)
        case var .array(a):
            let i = try index(head, count: a.count, allowEnd: false)
            if last { let removed = a.remove(at: i); return (.array(a), removed) }
            let r = try remove(at: path.dropFirst(), in: a[i])
            a[i] = r.doc
            return (.array(a), r.value)
        default: throw Failure(description: "cannot step into a scalar at \(head)")
        }
    }
}
