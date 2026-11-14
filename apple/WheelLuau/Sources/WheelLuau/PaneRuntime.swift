import CLuau
import Foundation
import WheelKit

/// One plugin pane's script in its own sandboxed Luau VM (ADR-0041).
///
/// The script returns a module `{ init, update, view }`. `init(previous)` makes the state (nil
/// for a new pane, the old state after a reload), `update(state, msg)` changes it (returning the
/// new state, or nil to keep the one it changed in place), and `view(state, data)` returns a tree
/// built with `ui.<Component>{ ... }`. `data.feed(name, params)` and `wheel.action(name, args)`
/// call the caller's `Host`.
///
/// The VM is interpreter only, its globals are frozen by `luaL_sandbox`, it has no `os`, `io` or
/// `require`, every entry into script code runs under a step budget, and the state's memory is
/// capped while script code runs. Every failure comes back as a thrown `Failure` and leaves the
/// last good module in place; the state keeps whatever the script changed in it before failing.
/// The module's fields are read raw, so a metatable on it runs no script code outside the limits.
///
/// Not thread-safe: the owner calls it from one thread or actor.
public final class PaneRuntime {
    public struct Limits: Sendable {
        /// Bytes the VM may hold while script code runs.
        public var memory: Int
        /// Interrupt safepoints (loop back edges and calls) one entry may pass.
        public var steps: UInt64
        public init(memory: Int = 32 << 20, steps: UInt64 = 10_000_000) {
            self.memory = memory
            self.steps = steps
        }
    }

    /// A feed as the script reads it: its value, and whether the backend that publishes it is
    /// down, so the value is the last one it sent.
    public struct FeedRead: Equatable {
        public var value: JSONValue
        public var stale: Bool
        public init(value: JSONValue, stale: Bool = false) {
            self.value = value
            self.stale = stale
        }
    }

    /// What the script reaches outside its VM.
    public struct Host {
        /// A feed now, for `data.feed(name, params)`, which returns `value, stale`.
        public var feed: (String, JSONValue) -> FeedRead
        /// `wheel.action(name, args)`.
        public var action: (String, JSONValue) -> Void
        public init(feed: @escaping (String, JSONValue) -> FeedRead = { _, _ in FeedRead(value: .null) },
                    action: @escaping (String, JSONValue) -> Void = { _, _ in }) {
            self.feed = feed
            self.action = action
        }
    }

    public enum Failure: Error, Equatable, CustomStringConvertible {
        /// The source did not compile.
        case syntax(String)
        /// The script raised an error.
        case runtime(String)
        /// The script ran past its step budget.
        case stepBudget(String)
        /// The script asked for memory past its limit.
        case memory(String)
        /// The module is not a table with a `view` function.
        case module(String)
        /// `view` returned something that is not a component tree.
        case view(String)

        public var description: String {
            switch self {
            case let .syntax(m), let .runtime(m), let .stepBudget(m), let .memory(m), let .module(m), let .view(m): m
            }
        }
    }

    /// The VM's main thread.
    private let main: OpaquePointer
    /// The thread whose stack the helpers below work on: `main`, or the coroutine a host call
    /// arrived on while it runs.
    private var L: OpaquePointer
    private let limits: UnsafeMutablePointer<WheelLuauLimits>
    private let host: Host
    private let chunkName: String
    private var moduleRef: Int32 = 0
    private var stateRef: Int32 = 0

    private enum HostKind: Int32 { case feed = 0, action = 1 }

    public init(source: String, chunkName: String = "pane", limits: Limits = Limits(), host: Host = Host()) throws {
        self.limits = .allocate(capacity: 1)
        self.limits.initialize(to: WheelLuauLimits(
            memoryLimit: limits.memory, used: 0, stepBudget: limits.steps, steps: 0,
            enforcing: 0, hitBudget: 0, hitMemory: 0, host: nil, hostCall: nil))
        guard let L = wheel_luau_newstate(self.limits) else {
            self.limits.deallocate()
            throw Failure.memory("the VM could not be created")
        }
        self.main = L
        self.L = L
        self.host = host
        self.chunkName = "=" + chunkName
        self.limits.pointee.host = Unmanaged.passUnretained(self).toOpaque()
        self.limits.pointee.hostCall = { host, L, kind in
            let runtime = Unmanaged<PaneRuntime>.fromOpaque(host!).takeUnretainedValue()
            return runtime.hostCall(kind: kind, on: L!)
        }
        try prepareGlobals()
        let module = try load(source)
        do {
            stateRef = try initState(module: module, previous: nil)
        } catch {
            lua_unref(L, module)
            throw error
        }
        moduleRef = module
    }

    deinit {
        lua_close(main)
        limits.deallocate()
    }

    /// Delivers `msg` to `update`; a module without `update` ignores it.
    public func send(_ msg: JSONValue) throws {
        guard push(function: "update") else { return }
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, stateRef)
        push(msg)
        try call(args: 2, results: 1)
        if lua_type(L, -1) != LUA_TNIL.rawValue {
            lua_unref(L, stateRef)
            stateRef = lua_ref(L, -1)
        }
        pop(1)
    }

    /// The tree `view(state, data)` returns. `data`'s fields reach the script as a table, beside
    /// `data.feed`.
    public func view(_ data: JSONValue = .object([:])) throws -> PaneNode {
        guard push(function: "view") else { throw Failure.module("the module has no view function") }
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, stateRef)
        if case .object = data { push(data) } else { lua_createtable(L, 0, 1) }
        wheel_luau_pushhost(L, HostKind.feed.rawValue)
        lua_setfield(L, -2, "feed")
        try call(args: 2, results: 1)
        let value: JSONValue
        do {
            value = try json(at: -1)
        } catch {
            pop(1)
            throw Failure.view("view returned \(error)")
        }
        pop(1)
        guard let node = PaneNode(value) else {
            throw Failure.view("view returned something that is not a component tree")
        }
        return node
    }

    /// Runs `source` as the pane's module and hands the current state to its `init`. A failure
    /// keeps the previous module and state.
    public func reload(source: String) throws {
        let module = try load(source)
        let state: Int32
        do {
            state = try initState(module: module, previous: stateRef)
        } catch {
            lua_unref(L, module)
            throw error
        }
        lua_unref(L, moduleRef)
        lua_unref(L, stateRef)
        moduleRef = module
        stateRef = state
    }

    /// The state, as JSON.
    public var state: JSONValue {
        get throws {
            lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, stateRef)
            defer { pop(1) }
            return try json(at: -1)
        }
    }

    /// Bytes the VM holds.
    public var memoryUsed: Int { limits.pointee.used }

    // MARK: - Loading

    /// `ui`, whose fields build components under the names they are read by, and `wheel`, then
    /// the sandbox that freezes them with every library.
    private static let prelude = """
    local function node(name, arg)
        local n = { component = name }
        if type(arg) == "string" then
            n.text = arg
            return n
        end
        if type(arg) ~= "table" then return n end
        for k, v in arg do
            if type(k) ~= "number" then n[k] = v end
        end
        local children = {}
        for i, v in ipairs(arg) do
            children[i] = if type(v) == "string" then { component = "Text", text = v } else v
        end
        if #children > 0 then n.children = children end
        return n
    end
    return setmetatable({}, {
        __index = function(_, name)
            return function(arg) return node(name, arg) end
        end,
        __metatable = false,
    })
    """

    private func prepareGlobals() throws {
        luaL_openlibs(L)
        lua_pushnil(L)
        lua_setfield(L, WHEEL_LUA_GLOBALSINDEX, "os")
        try compile(Self.prelude, chunkName: "=ui")
        try call(args: 0, results: 1)
        lua_setfield(L, WHEEL_LUA_GLOBALSINDEX, "ui")
        lua_createtable(L, 0, 1)
        wheel_luau_pushhost(L, HostKind.action.rawValue)
        lua_setfield(L, -2, "action")
        lua_setfield(L, WHEEL_LUA_GLOBALSINDEX, "wheel")
        luaL_sandbox(L)
    }

    /// Compiles `source` into a function on top of the stack, in a new thread whose globals
    /// proxy the frozen ones.
    private func compile(_ source: String, chunkName: String) throws {
        var size = 0
        var source = source
        let bytecode = source.withUTF8 { bytes in
            bytes.withMemoryRebound(to: CChar.self) { luau_compile($0.baseAddress, $0.count, nil, &size) }
        }!
        defer { free(bytecode) }
        let thread = lua_newthread(L)!
        luaL_sandboxthread(thread)
        let status = luau_load(thread, chunkName, bytecode, size, 0)
        if status != 0 {
            let message = string(at: -1, in: thread) ?? "the script did not compile"
            pop(1)
            throw Failure.syntax(message)
        }
        lua_xmove(thread, L, 1)
        lua_remove(L, -2)
    }

    /// Runs `source` and returns a registry ref to the module it returns.
    private func load(_ source: String) throws -> Int32 {
        try compile(source, chunkName: chunkName)
        try call(args: 0, results: 1)
        guard lua_type(L, -1) == LUA_TTABLE.rawValue else {
            pop(1)
            throw Failure.module("the script must return a table with init, update and view")
        }
        let fieldType = lua_rawgetfield(L, -1, "view")
        pop(1)
        guard fieldType == LUA_TFUNCTION.rawValue else {
            pop(1)
            throw Failure.module("the module has no view function")
        }
        return popRef()
    }

    /// Calls the module's `init` with the state `previous` refers to, or nil, and returns a ref
    /// to the new state. A module without `init` starts from an empty table.
    private func initState(module: Int32, previous: Int32?) throws -> Int32 {
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, module)
        let fieldType = lua_rawgetfield(L, -1, "init")
        lua_remove(L, -2)
        if fieldType != LUA_TFUNCTION.rawValue {
            pop(1)
            if let previous {
                lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, previous)
            } else {
                lua_createtable(L, 0, 0)
            }
            return popRef()
        }
        if let previous { lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, previous) } else { lua_pushnil(L) }
        try call(args: 1, results: 1)
        if lua_type(L, -1) == LUA_TNIL.rawValue {
            pop(1)
            lua_createtable(L, 0, 0)
        }
        return popRef()
    }

    /// Pushes the module's function `name`; false, pushing nothing, when the module has none.
    private func push(function name: String) -> Bool {
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, moduleRef)
        let fieldType = lua_rawgetfield(L, -1, name)
        lua_remove(L, -2)
        if fieldType == LUA_TFUNCTION.rawValue { return true }
        pop(1)
        return false
    }

    /// Calls the function below `args` arguments with the limits on; on failure pops the error
    /// and throws it.
    private func call(args: Int32, results: Int32) throws {
        let status = wheel_luau_pcall(L, args, results)
        guard status != 0 else { return }
        let message = string(at: -1, in: L) ?? "the script raised a non-string error"
        pop(1)
        let limits = limits.pointee
        // The budget stays spent once hit, so a script that caught the budget's error is stopped
        // by it again; a memory error caught by the script and replaced is the script's own error.
        if limits.hitBudget != 0 { throw Failure.stepBudget(message) }
        if status == Int32(LUA_ERRMEM.rawValue) {
            lua_gc(L, Int32(LUA_GCCOLLECT.rawValue), 0)
            throw Failure.memory("the script ran past its memory limit of \(limits.memoryLimit) bytes")
        }
        throw Failure.runtime(message)
    }

    // MARK: - Host calls

    /// Runs inside the trampoline: reads `(name, params)`, pushes results, raises nothing.
    private func hostCall(kind: Int32, on thread: OpaquePointer) -> Int32 {
        let caller = L
        L = thread
        defer { L = caller }
        guard let name = lua_type(L, 1) == LUA_TSTRING.rawValue ? string(at: 1, in: L) : nil else {
            lua_pushstring(L, "the first argument must be a name")
            return -1
        }
        let args: JSONValue
        do {
            args = lua_gettop(L) >= 2 ? try json(at: 2) : .null
        } catch {
            lua_pushstring(L, "the arguments are \(error)")
            return -1
        }
        switch HostKind(rawValue: kind) {
        case .feed:
            let read = host.feed(name, args)
            push(read.value)
            lua_pushboolean(L, read.stale ? 1 : 0)
            return 2
        case .action:
            host.action(name, args)
            return 0
        case nil:
            lua_pushstring(L, "unknown host call")
            return -1
        }
    }

    // MARK: - Values

    private static let maxDepth = 200

    private func push(_ value: JSONValue, depth: Int = 0) {
        lua_checkstack(L, 3)
        switch value {
        case .null: lua_pushnil(L)
        case let .bool(b): lua_pushboolean(L, b ? 1 : 0)
        case let .number(n): lua_pushnumber(L, n)
        case let .string(s):
            var s = s
            s.withUTF8 { bytes in
                bytes.withMemoryRebound(to: CChar.self) { lua_pushlstring(L, $0.baseAddress, $0.count) }
            }
        case let .array(list):
            lua_createtable(L, Int32(clamping: list.count), 0)
            guard depth < Self.maxDepth else { return }
            for (i, item) in list.enumerated() {
                push(item, depth: depth + 1)
                lua_rawseti(L, -2, Int32(clamping: i + 1))
            }
        case let .object(fields):
            lua_createtable(L, 0, Int32(clamping: fields.count))
            guard depth < Self.maxDepth else { return }
            for (key, item) in fields {
                push(item, depth: depth + 1)
                lua_setfield(L, -2, key)
            }
        }
    }

    /// Values one conversion to JSON may visit. A table reached twice is converted twice, so a
    /// shared or cyclic table would otherwise grow without bound; the conversion runs outside the
    /// step budget.
    private static let maxNodes = 100_000

    /// What stopped a conversion to JSON.
    struct TooLarge: Error, CustomStringConvertible {
        var description: String { "more than \(PaneRuntime.maxNodes) values (a shared or cyclic table?)" }
    }

    /// The Lua value at `index` as JSON. A table whose keys are exactly 1...n is an array, as is an
    /// empty table; any other table is an object, its number keys written as strings. Functions
    /// and other values are null.
    private func json(at index: Int32) throws -> JSONValue {
        var visited = 0
        return try json(at: index, depth: 0, visited: &visited)
    }

    private func json(at index: Int32, depth: Int, visited: inout Int) throws -> JSONValue {
        visited += 1
        guard visited <= Self.maxNodes else { throw TooLarge() }
        let index = index < 0 ? lua_gettop(L) + index + 1 : index
        switch UInt32(lua_type(L, index)) {
        case LUA_TBOOLEAN.rawValue: return .bool(lua_toboolean(L, index) != 0)
        case LUA_TNUMBER.rawValue: return .number(lua_tonumberx(L, index, nil))
        case LUA_TSTRING.rawValue: return string(at: index, in: L).map(JSONValue.string) ?? .null
        case LUA_TTABLE.rawValue:
            guard depth < Self.maxDepth else { throw TooLarge() }
            lua_checkstack(L, 3)
            var byIndex: [Int: JSONValue] = [:]
            var fields: [String: JSONValue] = [:]
            lua_pushnil(L)
            while lua_next(L, index) != 0 {
                let value: JSONValue
                do {
                    value = try json(at: -1, depth: depth + 1, visited: &visited)
                } catch {
                    pop(2)
                    throw error
                }
                switch UInt32(lua_type(L, -2)) {
                case LUA_TNUMBER.rawValue:
                    let n = lua_tonumberx(L, -2, nil)
                    if n >= 1, n == n.rounded(), n <= Double(Int32.max) { byIndex[Int(n)] = value }
                    fields[Self.key(n)] = value
                case LUA_TSTRING.rawValue:
                    if let key = string(at: -2, in: L) { fields[key] = value }
                default: break
                }
                pop(1)
            }
            if byIndex.count == fields.count, (0..<byIndex.count).allSatisfy({ byIndex[$0 + 1] != nil }) {
                return .array((0..<byIndex.count).map { byIndex[$0 + 1]! })
            }
            return .object(fields)
        default: return .null
        }
    }

    /// A number key as an object key: `3`, not `3.0`.
    private static func key(_ n: Double) -> String {
        n == n.rounded() && abs(n) < 1e15 ? String(Int64(n)) : String(n)
    }

    private func string(at index: Int32, in state: OpaquePointer) -> String? {
        var length = 0
        guard let chars = lua_tolstring(state, index, &length) else { return nil }
        return String(decoding: UnsafeRawBufferPointer(start: chars, count: length), as: UTF8.self)
    }

    private func pop(_ n: Int32) { lua_settop(L, -n - 1) }

    /// A registry ref to the value on top of the stack, which it pops.
    private func popRef() -> Int32 {
        let ref = lua_ref(L, -1)
        pop(1)
        return ref
    }
}
