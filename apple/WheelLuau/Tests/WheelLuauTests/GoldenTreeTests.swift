import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// Every pane script in the repo run with the feeds recorded under `clients/gpui/luau/tests/scripts`,
/// its tree compared with the `<name>.tree.json` beside them, which the Rust runtime (`wheel-luau`'s
/// `golden` test) must also draw. `WHEEL_LUAU_GOLDEN=write` writes the trees instead.
@Suite struct GoldenTreeTests {
    static let repo = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
    static let scripts = repo.appendingPathComponent("clients/gpui/luau/tests/scripts")

    static func read(_ url: URL) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))
    }

    /// Every `.luau` file under the package's `lib/`, keyed by its path in the package.
    static func modules(_ package: URL) -> [String: String] {
        let lib = package.appendingPathComponent("lib")
        var out: [String: String] = [:]
        guard let walk = FileManager.default.enumerator(at: lib, includingPropertiesForKeys: nil) else { return out }
        for case let url as URL in walk where url.pathExtension == "luau" {
            let rel = url.standardizedFileURL.path.dropFirst(lib.standardizedFileURL.path.count + 1)
            out["lib/\(rel)"] = try? String(contentsOf: url, encoding: .utf8)
        }
        return out
    }

    static func json(_ node: PaneNode) -> JSONValue {
        var fields = node.props
        fields["component"] = .string(node.component)
        if !node.children.isEmpty { fields["children"] = .array(node.children.map(json)) }
        return .object(fields)
    }

    @Test func everyPaneScriptDrawsItsRecordedTree() throws {
        guard case let .array(cases) = try Self.read(Self.scripts.appendingPathComponent("cases.json")) else {
            Issue.record("cases.json is not a list"); return
        }
        let write = ProcessInfo.processInfo.environment["WHEEL_LUAU_GOLDEN"] == "write"
        #expect(cases.count >= 17)
        for case let .object(c) in cases {
            guard case let .string(name)? = c["name"], case let .string(pkg)? = c["package"],
                  case let .string(pane)? = c["pane"] else { Issue.record("a case lacks name, package or pane"); continue }
            let package = Self.repo.appendingPathComponent(pkg)
            let source = try String(contentsOf: package.appendingPathComponent(pane), encoding: .utf8)
            var feeds: [JSONValue] = []
            if case let .array(list)? = try? Self.read(Self.scripts.appendingPathComponent("\(name).feeds.json")) { feeds = list }
            var missing: [String] = []
            let host = PaneRuntime.Host(feed: { feed, params in
                for case let .object(f) in feeds where f["name"] == .string(feed) && f["params"] == params {
                    return PaneRuntime.FeedRead(value: f["value"] ?? .null, stale: f["stale"] == .bool(true))
                }
                missing.append(feed)
                return PaneRuntime.FeedRead(value: .null)
            })
            let runtime = try PaneRuntime(source: source, modules: Self.modules(package), host: host)
            if case let .array(messages)? = c["messages"] { for m in messages { try runtime.send(m) } }
            let tree = Self.json(try runtime.view(c["data"] ?? .object([:])))
            #expect(missing.isEmpty, "\(name) read feeds with no recording: \(missing)")
            let file = Self.scripts.appendingPathComponent("\(name).tree.json")
            if write {
                var out = try JSONSerialization.data(withJSONObject: tree.foundation, options: [.prettyPrinted, .sortedKeys])
                out.append(0x0A)
                try out.write(to: file)
            } else {
                #expect(tree == (try Self.read(file)), "\(name) draws another tree than \(name).tree.json")
            }
        }
    }

    /// Each script in `errors.json`, run under the default limits, fails with the kind and message
    /// in `errors.expected.json`, which the Rust runtime must also give.
    @Test func everyFailureReadsAsRecorded() throws {
        guard case let .array(cases) = try Self.read(Self.scripts.appendingPathComponent("errors.json")) else {
            Issue.record("errors.json is not a list"); return
        }
        var out: [String: JSONValue] = [:]
        for case let .object(c) in cases {
            guard case let .string(name)? = c["name"], case let .string(source)? = c["source"],
                  case let .string(chunk)? = c["chunk"] else { Issue.record("an error case lacks name, source or chunk"); continue }
            do {
                _ = try PaneRuntime(source: source, chunkName: chunk).view()
                Issue.record("\(name) did not fail")
            } catch let failure as PaneRuntime.Failure {
                let kind = switch failure {
                case .syntax: "syntax"
                case .runtime: "runtime"
                case .stepBudget: "step_budget"
                case .memory: "memory"
                case .module: "module"
                case .view: "view"
                }
                out[name] = .object(["kind": .string(kind), "message": .string(failure.description)])
            }
        }
        let file = Self.scripts.appendingPathComponent("errors.expected.json")
        if ProcessInfo.processInfo.environment["WHEEL_LUAU_GOLDEN"] == "write" {
            var data = try JSONSerialization.data(withJSONObject: JSONValue.object(out).foundation, options: [.prettyPrinted, .sortedKeys])
            data.append(0x0A)
            try data.write(to: file)
        } else {
            #expect(JSONValue.object(out) == (try Self.read(file)))
        }
    }
}
