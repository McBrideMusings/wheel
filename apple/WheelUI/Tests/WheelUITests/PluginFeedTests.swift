import Foundation
import Testing
import WheelKit
@testable import WheelUI

@MainActor @Suite struct PluginFeedTests {
    static let script = """
    return {
        init = function(previous) return previous or {} end,
        update = function(state, msg) state.more = (msg.type == "more") end,
        view = function(state, data)
            local a = data.feed("x.y", {})
            local rows = {
                ui.Text("a " .. tostring(if a == nil then "nil" else a.n)),
                ui.Button{ child = ui.Text("More"), action = { type = "more" } },
                ui.Button{ child = ui.Text("Less"), action = { type = "less" } },
            }
            if state.more then table.insert(rows, ui.Text("z " .. tostring(data.feed("x.z", { k = 1 }) ~= nil))) end
            return ui.Column(rows)
        end,
    }
    """

    final class Calls { var sub: [String] = []; var unsub: [String] = [] }

    func make(_ calls: Calls) -> PluginPane {
        let pane = PluginPane()
        pane.onFeeds = { s, u in calls.sub += s.map(\.0); calls.unsub += u.map(\.0) }
        pane.load(source: Self.script, chunk: "t.luau")
        return pane
    }

    func texts(_ pane: PluginPane) -> [String] { pane.componentTexts.compactMap { $0["text"] } }

    @Test func aFeedReadSubscribesAndShowsValueThenPatch() {
        let calls = Calls()
        let pane = make(calls)
        #expect(pane.error == nil)
        #expect(calls.sub == ["x.y"])
        #expect(texts(pane).first == "a nil")
        pane.feedUpdate(feed: "x.y", params: [String: Any](), value: ["n": 1], patch: nil)
        #expect(texts(pane).first == "a 1")
        pane.feedUpdate(feed: "x.y", params: [String: Any](), value: nil, patch: [["op": "replace", "path": "/n", "value": 7]])
        #expect(texts(pane).first == "a 7")
        #expect(pane.error == nil)
        #expect(pane.feedJSON == ["x.y {}": "{\"n\":7}"])
    }

    @Test func aStaleUpdateKeepsTheValueAndTheScriptReadsTheFlag() {
        let pane = PluginPane()
        pane.load(source: """
        return { view = function(state, data)
            local v, stale = data.feed("x.y", {})
            return ui.Text(tostring(if v == nil then "nil" else v.n) .. (if stale then " stale" else ""))
        end }
        """, chunk: "s.luau")
        pane.feedUpdate(feed: "x.y", params: [String: Any](), value: ["n": 3], patch: nil)
        #expect(texts(pane) == ["3"])
        pane.feedUpdate(feed: "x.y", params: [String: Any](), value: ["n": 3], patch: nil, stale: true)
        #expect(texts(pane) == ["3 stale"])
        pane.feedUpdate(feed: "x.y", params: [String: Any](), value: nil, patch: [["op": "replace", "path": "/n", "value": 4]])
        #expect(texts(pane) == ["4"], "the next update from the backend is fresh")
    }

    @Test func aBadPatchSetsErrorAndReadsTheFeedAgain() {
        let calls = Calls()
        let pane = make(calls)
        pane.feedUpdate(feed: "x.y", params: [String: Any](), value: ["n": 1], patch: nil)
        calls.sub = []
        pane.feedUpdate(feed: "x.y", params: [String: Any](), value: nil, patch: [["op": "remove", "path": "/nope"]])
        #expect(pane.error != nil)
        #expect(texts(pane).first == "a 1")
        #expect(calls.unsub == ["x.y"] && calls.sub == ["x.y"], "unsubscribe then subscribe, so wheeld sends the whole value")
        #expect(pane.feedJSON == ["x.y {}": "null"], "the value that no longer matches wheeld's is dropped")
        pane.feedUpdate(feed: "x.y", params: [String: Any](), value: ["n": 2], patch: nil)
        #expect(texts(pane).first == "a 2")
    }

    @Test func aRefusedSubscribeIsShownAndSubscribedAgainOnTheNextRender() {
        let calls = Calls()
        let pane = make(calls)
        #expect(calls.sub == ["x.y"])
        pane.feedFailed(feed: "x.y", params: [String: Any](), message: "no such feed")
        #expect(pane.error == "no such feed")
        #expect(pane.feeds.isEmpty)
        calls.sub = []
        // Any render that reads the feed again subscribes it again: pressing a button renders.
        let buttons = pane.surface.components.values.filter { $0.type == "Button" }.map(\.id).sorted()
        #expect(pane.press(component: buttons[1]))
        #expect(calls.sub == ["x.y"])
        #expect(pane.feeds.map(\.0) == ["x.y"])
    }

    @Test func aSecondFeedSubscribesAndADroppedOneUnsubscribes() {
        let calls = Calls()
        let pane = make(calls)
        let buttons = pane.surface.components.values.filter { $0.type == "Button" }.map(\.id).sorted()
        #expect(buttons.count == 2)
        // The first button's action is `more`, the second's `less`, in the order the script lists them.
        #expect(pane.press(component: buttons[0]))
        #expect(calls.sub == ["x.y", "x.z"])
        #expect(pane.press(component: buttons[1]))
        #expect(calls.unsub == ["x.z"])
        #expect(pane.feeds.map(\.0) == ["x.y"])
    }

    @Test func closeUnsubscribesEverything() {
        let calls = Calls()
        let pane = make(calls)
        pane.close()
        #expect(calls.unsub == ["x.y"])
        #expect(pane.feeds.isEmpty)
    }

    @Test func anUpdateForAFeedNotReadIsIgnored() {
        let pane = make(Calls())
        pane.feedUpdate(feed: "other.f", params: nil, value: 1, patch: nil)
        #expect(pane.feedJSON.keys.sorted() == ["x.y {}"])
    }
}

@Suite struct JSONPatchTests {
    func j(_ text: String) -> JSONValue { try! JSONDecoder().decode(JSONValue.self, from: Data(text.utf8)) }
    func apply(_ patch: String, _ doc: String) throws -> JSONValue { try JSONPatch.apply(j(patch), to: j(doc)) }

    @Test func add() throws {
        #expect(try apply(#"[{"op":"add","path":"/a/b","value":1}]"#, #"{"a":{}}"#) == j(#"{"a":{"b":1}}"#))
        #expect(try apply(#"[{"op":"add","path":"/l/-","value":3},{"op":"add","path":"/l/0","value":0}]"#, #"{"l":[1,2]}"#) == j(#"{"l":[0,1,2,3]}"#))
        #expect(try apply(#"[{"op":"add","path":"","value":5}]"#, "1") == .number(5))
    }
    @Test func removeAndReplace() throws {
        #expect(try apply(#"[{"op":"remove","path":"/l/1"},{"op":"replace","path":"/a","value":"x"}]"#, #"{"l":[1,2,3],"a":0}"#) == j(#"{"l":[1,3],"a":"x"}"#))
        #expect(throws: JSONPatch.Failure.self) { try apply(#"[{"op":"remove","path":"/zz"}]"#, "{}") }
        #expect(throws: JSONPatch.Failure.self) { try apply(#"[{"op":"replace","path":"/zz","value":1}]"#, "{}") }
    }
    @Test func moveCopyTest() throws {
        #expect(try apply(#"[{"op":"move","from":"/a","path":"/b"}]"#, #"{"a":1}"#) == j(#"{"b":1}"#))
        #expect(try apply(#"[{"op":"copy","from":"/a","path":"/b"}]"#, #"{"a":[1]}"#) == j(#"{"a":[1],"b":[1]}"#))
        #expect(try apply(#"[{"op":"test","path":"/a","value":[1]}]"#, #"{"a":[1]}"#) == j(#"{"a":[1]}"#))
        #expect(throws: JSONPatch.Failure.self) { try apply(#"[{"op":"test","path":"/a","value":2}]"#, #"{"a":1}"#) }
    }
    @Test func pointerEscapes() throws {
        #expect(try apply(#"[{"op":"add","path":"/a~1b/c~0d","value":1}]"#, #"{"a/b":{}}"#) == j(#"{"a/b":{"c~d":1}}"#))
    }
    @Test func badIndexAndUnknownOp() {
        #expect(throws: JSONPatch.Failure.self) { try apply(#"[{"op":"add","path":"/l/5","value":1}]"#, #"{"l":[]}"#) }
        #expect(throws: JSONPatch.Failure.self) { try apply(#"[{"op":"add","path":"/l/01","value":1}]"#, #"{"l":[1,2]}"#) }
        #expect(throws: JSONPatch.Failure.self) { try apply(#"[{"op":"bogus","path":"/a"}]"#, "{}") }
    }
    @Test func failedPatchLeavesDocumentUnchanged() throws {
        var doc = j(#"{"l":[1,2],"a":{"b":1}}"#)
        let patch = j(#"[{"op":"add","path":"/l/-","value":3},{"op":"remove","path":"/a/b"},{"op":"move","from":"/l","path":"/m"},{"op":"remove","path":"/zz"}]"#)
        #expect(throws: JSONPatch.Failure.self) { try JSONPatch.apply(patch, to: &doc) }
        #expect(doc == j(#"{"l":[1,2],"a":{"b":1}}"#))
    }
    @Test func appendToLongArrayIsConstantTime() throws {
        var doc: JSONValue = .object(["lines": .array(Array(repeating: .array([]), count: 200_000))])
        let append = j(#"[{"op":"add","path":"/lines/-","value":[]}]"#)
        let start = Date()
        for _ in 0..<1000 { try JSONPatch.apply(append, to: &doc) }
        let seconds = Date().timeIntervalSince(start)
        guard case let .object(o) = doc, case let .array(a)? = o["lines"] else { Issue.record("shape"); return }
        #expect(a.count == 201_000)
        #expect(seconds < 0.25, "1000 appends to 200000 lines took \(seconds)s")
    }
}
