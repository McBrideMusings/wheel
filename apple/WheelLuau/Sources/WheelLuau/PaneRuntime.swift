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
/// `require("lib/ui")` runs the package's `lib/ui.luau` from `modules` (paths in the package; the
/// `.luau` may be left off) once per VM and returns what it returned. A path that leaves the
/// package, a missing file and a module that requires itself again before it returns each raise
/// an error. A reload takes the new modules and runs each again on its next `require`.
///
/// The VM is interpreter only, its globals are frozen by `luaL_sandbox`, it has no `os` or `io`,
/// every entry into script code (a required module included) runs under a step budget, and the state's memory is
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
        /// The feed's name and params as one string: a mounted view re-runs what read it when `flush(changed: "feed:<key>")`.
        public var key: String?
        public init(value: JSONValue, stale: Bool = false, key: String? = nil) {
            self.value = value
            self.stale = stale
            self.key = key
        }
    }

    /// What a flush of a mounted view wrote, and the feed keys (`FeedRead.key`) its live computations read.
    public struct Changes: Equatable {
        public var ops: [ViewOp]
        public var feeds: [String]
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
    /// What a module's chunk name starts with: the pane chunk's plugin, `<plugin>/`.
    private let modulePrefix: String
    private var modules: [String: String]
    private var moduleRef: Int32 = 0
    private var stateRef: Int32 = 0
    /// `require`'s table: `loaded` (path to `{ value = <what the module returned> }`) and
    /// `loading` (the paths whose module is running now).
    private var requireRef: Int32 = 0
    /// The canvas sizes the client reported, `key` to `{ w, h }`; the `ui` prelude reads it.
    private var sizesRef: Int32 = 0
    /// A weak-keyed set of the command lists the `ui` prelude recorded; `json(at:)` counts each
    /// one's values apart from the view's.
    private var canvasesRef: Int32 = 0
    /// The prelude's own functions (`mount`, `unmount`, `flush`, `feed`) and `records`, the weak map from each `ui.state`
    /// and `ui.computed` to its record, which `json(at:)` reads a value from.
    private var internalsRef: Int32 = 0
    private var recordsRef: Int32 = 0
    /// Whether the last `view` bound a state, computed or each: it ran once, and `flush` writes what changes.
    public private(set) var mounted = false
    /// Times `view` ran.
    public private(set) var viewRuns = 0
    /// The feed keys the mounted view's live computations read, after the last `view` or `flush`.
    public private(set) var feeds: [String] = []
    /// Entries the measure cache holds before it is cleared.
    private static let maxMeasureCache = 5_000
    /// Steps one uncached `measureText` call charges to the budget.
    private static let measureCost: UInt64 = 1_000
    private var measureCache: [String: (width: Double, ascent: Double, descent: Double)] = [:]

    private enum HostKind: Int32 { case feed = 0, action = 1, module = 2, measure = 3 }

    public init(source: String, modules: [String: String] = [:], chunkName: String = "pane",
                limits: Limits = Limits(), host: Host = Host()) throws {
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
        self.modulePrefix = chunkName.split(separator: "/", maxSplits: 1).count == 2
            ? String(chunkName.prefix { $0 != "/" }) + "/" : ""
        self.modules = modules
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

    /// Delivers `msg` to `update`; a module without `update` ignores it. In a mounted view the states `update` set write
    /// nothing until `flush`.
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
    /// A view that binds a `ui.state`, `ui.computed` or `ui.each` is mounted: it runs once, every node it returned carries a
    /// handle, and later changes come from `flush`. Calling `view` again mounts it again from the top.
    public func view(_ data: JSONValue = .object([:])) throws -> PaneNode {
        pushInternal("mount")
        guard push(function: "view") else {
            pop(1)
            throw Failure.module("the module has no view function")
        }
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, stateRef)
        if case .object = data { push(data) } else { lua_createtable(L, 0, 1) }
        pushInternal("feed")
        lua_setfield(L, -2, "feed")
        mounted = false
        viewRuns += 1
        try call(args: 3, results: 2)
        let bound = lua_toboolean(L, -1) != 0
        pop(1)
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
        mounted = bound
        feeds = []
        if bound { feeds = try flush().feeds }
        return node
    }

    /// Runs every effect a state change made stale in the mounted view and returns the node writes, in order. `changed`
    /// names an outside source first marked changed: `feed:<FeedRead.key>` or `size:<canvas key>`.
    public func flush(changed source: String? = nil) throws -> Changes {
        guard mounted else { return Changes(ops: [], feeds: []) }
        pushInternal("flush")
        if let source { push(.string(source)) } else { lua_pushnil(L) }
        try call(args: 1, results: 2)
        defer { pop(2) }
        let ops: JSONValue
        let live: JSONValue
        do {
            ops = try json(at: -2)
            live = try json(at: -1)
        } catch {
            throw Failure.view("flush returned \(error)")
        }
        var out: [ViewOp] = []
        for item in ops.arrayItems {
            guard case let .object(op) = item, case let .number(node)? = op["node"] else { continue }
            switch op["op"] {
            case .string("set")?:
                guard case let .string(prop)? = op["prop"] else { continue }
                out.append(.set(node: Int(node), prop: prop, value: op["value"] ?? .null))
            case .string("children")?:
                var children: [PaneNode] = []
                for c in op["children"]?.arrayItems ?? [] {
                    guard let child = PaneNode(c) else { throw Failure.view("a ui.each item is not a component tree") }
                    children.append(child)
                }
                out.append(.children(node: Int(node), children: children))
            default: continue
            }
        }
        feeds = live.arrayItems.compactMap { if case let .string(k) = $0 { k } else { nil } }
        return Changes(ops: out, feeds: feeds)
    }

    /// Runs `source` as the pane's module, with `modules` for its `require`, and hands the current
    /// state to its `init`. A failure keeps the previous module, modules and state.
    public func reload(source: String, modules: [String: String] = [:]) throws {
        let oldModules = self.modules
        let oldLoaded = swapLoaded()
        // A module that yielded while loading and was never resumed must not read as a cycle.
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, requireRef)
        lua_rawgetfield(L, -1, "loading")
        lua_cleartable(L, -1)
        pop(2)
        self.modules = modules
        let module: Int32
        let state: Int32
        do {
            module = try load(source)
            do {
                state = try initState(module: module, previous: stateRef)
            } catch {
                lua_unref(L, module)
                throw error
            }
        } catch {
            self.modules = oldModules
            lua_unref(L, swapLoaded(to: oldLoaded))
            lua_unref(L, oldLoaded)
            throw error
        }
        lua_unref(L, oldLoaded)
        if mounted {
            // The new module's view mounts from the top; the old one's effects stop.
            pushInternal("unmount")
            try? call(args: 0, results: 0)
            mounted = false
            feeds = []
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

    /// Stores the size the client measured for the Canvas named `key`, which the next `view` hands
    /// to its `draw`. Returns whether it changed by 0.5 pt or more in either direction.
    public func setCanvasSize(key: String, width: Double, height: Double) -> Bool {
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, sizesRef)
        defer { pop(1) }
        var changed = true
        if lua_rawgetfield(L, -1, key) == LUA_TTABLE.rawValue {
            lua_rawgetfield(L, -1, "w")
            let w = lua_tonumberx(L, -1, nil)
            pop(1)
            lua_rawgetfield(L, -1, "h")
            let h = lua_tonumberx(L, -1, nil)
            pop(1)
            changed = abs(w - width) >= 0.5 || abs(h - height) >= 0.5
        }
        pop(1)
        guard changed else { return false }
        lua_createtable(L, 0, 2)
        lua_pushnumber(L, width)
        lua_rawsetfield(L, -2, "w")
        lua_pushnumber(L, height)
        lua_rawsetfield(L, -2, "h")
        lua_rawsetfield(L, -2, key)
        return true
    }

    /// Bytes the VM holds.
    public var memoryUsed: Int { limits.pointee.used }

    // MARK: - Loading

    /// `ui`, whose fields build components under the names they are read by, and `wheel`, then
    /// the sandbox that freezes them with every library.
    private static let prelude = """
    local measure_host, sizes, canvases, feed_host = ...
    setmetatable(canvases, { __mode = 'k' })
    local buffer_len, type_of = buffer.len, typeof
    -- ── fine-grained views: ui.state, ui.computed, ui.each ──
    -- A record is a state, a computed, an effect (a write to a mounted node) or a scope (an owner of other records).
    local records = setmetatable({}, { __mode = "k" })
    local meta = setmetatable({}, { __mode = "k" })
    local nodeOwner = setmetatable({}, { __mode = "k" })
    -- A Canvas node's draw: a mounted view draws it again when its size changes.
    local canvasDraw = setmetatable({}, { __mode = "k" })
    local eachMarks = setmetatable({}, { __mode = "k" })
    local current, owner = nil, nil
    local queue, sources = {}, {}
    local nextId, nextHandle = 0, 0
    local sawBinding = false
    local NONE = {}
    local pendingSet, setOrder, pendingChildren = {}, {}, {}
    local root = nil

    local function record(kind, fn)
        nextId += 1
        local r = { kind = kind, id = nextId, fn = fn, subs = {}, deps = {}, versions = {}, depSet = {}, owned = {}, version = 0, stale = false, ran = false }
        if kind ~= "state" and owner ~= nil then table.insert(owner.owned, r) end
        return r
    end

    local function track(r)
        local c = current
        if c == nil or c.depSet[r] then return end
        c.depSet[r] = true
        table.insert(c.deps, r)
        table.insert(c.versions, r.version)
        r.subs[c] = true
    end

    local function unlink(c)
        for _, r in c.deps do r.subs[c] = nil end
        c.deps, c.versions, c.depSet = {}, {}, {}
    end

    local dispose
    local function disposeOwned(r)
        local owned = r.owned
        r.owned = {}
        for _, c in owned do dispose(c) end
    end
    dispose = function(r)
        if r.disposed then return end
        r.disposed = true
        disposeOwned(r)
        unlink(r)
    end

    local function markStale(r)
        for c in r.subs do
            if not c.stale then
                c.stale = true
                if c.kind == "effect" then table.insert(queue, c) end
                markStale(c)
            end
        end
    end

    local function run(c)
        disposeOwned(c)
        unlink(c)
        local prevC, prevO = current, owner
        current, owner = c, c
        local ok, value = pcall(c.fn)
        current, owner = prevC, prevO
        if not ok then
            -- It ran with no dependencies kept: the next refresh runs it again.
            c.stale, c.failed = true, true
            error(value, 0)
        end
        c.failed = false
        if c.kind == "computed" then
            if not c.ran or value ~= c.value or type(value) == "table" then
                c.value = value
                c.version += 1
            end
        else
            if c.ran and c.changed and (c.always or value ~= c.value or type(value) == "table") then c.changed(value) end
            c.value = value
        end
        c.ran = true
    end

    local function refresh(c)
        if c.disposed or (c.ran and not c.stale) then return end
        local changed = not c.ran or c.failed
        if not changed then
            for i, r in c.deps do
                if r.kind == "computed" then refresh(r) end
                if r.version ~= c.versions[i] then changed = true break end
            end
        end
        c.stale = false
        if changed then run(c) end
    end

    local function untracked(fn, ...)
        local prev = current
        current = nil
        local ok, a, b = pcall(fn, ...)
        current = prev
        if not ok then error(a, 0) end
        return a, b
    end

    local function source(key)
        local r = sources[key]
        if r == nil then
            local o = owner
            owner = nil
            r = record("state")
            owner = o
            sources[key] = r
        end
        return r
    end
    local function sizeOf(key) return source("size:" .. key) end

    local State = { __metatable = false }
    State.__index = State
    function State.get(self)
        local r = records[self]
        track(r)
        return r.value
    end
    function State.set(self, value)
        local r = records[self]
        if r == nil or r.kind ~= "state" then error("set is only for a ui.state", 2) end
        if rawequal(r.value, value) and type(value) ~= "table" then return end
        r.value = value
        r.version += 1
        markStale(r)
    end

    local Computed = { __metatable = false }
    Computed.__index = Computed
    function Computed.get(self)
        local r = records[self]
        local ok, err = pcall(refresh, r)
        -- Tracked even when it failed, so its reader runs again once it succeeds.
        track(r)
        if not ok then error(err, 0) end
        return r.value
    end

    local function state(initial)
        local obj = setmetatable({}, State)
        local r = record("state")
        r.value = initial
        records[obj] = r
        return obj
    end

    local function computed(fn)
        if type(fn) ~= "function" then error("ui.computed takes a function", 2) end
        local obj = setmetatable({}, Computed)
        records[obj] = record("computed", fn)
        return obj
    end

    local function isNode(v)
        return type(v) == "table" and type(rawget(v, "component")) == "string"
    end

    -- Whether `v` is or holds a state or computed, outside any node it holds.
    local function bound(v, depth)
        if type(v) ~= "table" then return false end
        if records[v] then return true end
        if depth > 16 or isNode(v) or canvases[v] then return false end
        for _, x in v do
            if bound(x, depth + 1) then return true end
        end
        return false
    end

    -- `v` with every state and computed in it replaced by its value, read inside the running effect so a change re-runs it.
    local function resolve(v, depth)
        if type(v) ~= "table" then return v end
        local r = records[v]
        if r then
            local x = if r.kind == "state" then State.get(v) else Computed.get(v)
            if x == v or depth > 16 then return x end
            return resolve(x, depth + 1)
        end
        if isNode(v) or canvases[v] or eachMarks[v] or depth > 16 then return v end
        local out = nil
        for k, x in v do
            local y = resolve(x, depth + 1)
            if not rawequal(y, x) then
                out = out or table.clone(v)
                out[k] = y
            end
        end
        return out or v
    end

    local snapshot, ensureEach

    local function snapValue(v, keep, depth)
        if type(v) ~= "table" or canvases[v] or depth > 200 then return v end
        if isNode(v) then return snapshot(v, keep) end
        local out = nil
        for k, x in v do
            local y = snapValue(x, keep, depth + 1)
            if not rawequal(y, x) then
                out = out or table.clone(v)
                out[k] = y
            end
        end
        return out or v
    end

    local function queueSet(n, prop, value)
        local props = pendingSet[n]
        if props == nil then
            props = {}
            pendingSet[n] = props
            table.insert(setOrder, n)
        end
        props[prop] = if value == nil then NONE else value
    end

    -- The node's handle and an effect per bound property, made the first time the node is drawn, owned by the scope or
    -- computation that built the node.
    local function register(n)
        nextHandle += 1
        local m = { h = nextHandle, props = {} }
        meta[n] = m
        local prevO = owner
        owner = nodeOwner[n]
        for k, v in n do
            if k ~= "children" and k ~= "component" and bound(v, 0) then
                local e = record("effect", function() return resolve(v, 0) end)
                e.changed = function(value) queueSet(n, k, value) end
                m.props[k] = e
                local ok, err = pcall(run, e)
                if not ok then owner = prevO error(err, 0) end
            end
        end
        local draw = canvasDraw[n]
        if draw and m.props.commands == nil then
            -- The first run reads the size the commands were drawn at; a later one draws again.
            local first = true
            local e = record("effect", function()
                if first then
                    first = false
                    track(sizeOf(n.canvasKey))
                    return n.commands
                end
                return draw()
            end)
            e.changed = function(value) queueSet(n, "commands", value) end
            e.always = true
            m.props.commands = e
            local ok, err = pcall(run, e)
            if not ok then owner = prevO error(err, 0) end
        end
        owner = prevO
        return m
    end

    local function childList(n, keep)
        local list = {}
        local children = rawget(n, "children")
        if children == nil then return list end
        for _, c in children do
            if eachMarks[c] then
                local each = ensureEach(c, n)
                for _, k in each.order do table.insert(list, snapshot(each.items[k].node, keep)) end
            else
                table.insert(list, snapshot(c, keep))
            end
        end
        return list
    end

    snapshot = function(n, keep)
        local m = meta[n]
        if m and m.drawn and keep then return { component = n.component, key = rawget(n, "key"), __node = m.h, __keep = true } end
        m = m or register(n)
        m.drawn = true
        local s = { component = n.component, __node = m.h }
        for k, v in n do
            if k == "children" then
                s.children = childList(n, keep)
            elseif k ~= "component" then
                local e = m.props[k]
                s[k] = snapValue(if e then e.value else v, keep, 0)
            end
        end
        return s
    end

    local function keyOf(each, item, i)
        local key = each.key
        local k = if type(key) == "function" then key(item, i)
                  elseif type(key) == "string" and type(item) == "table" then item[key]
                  else item
        if type(k) ~= "string" and type(k) ~= "number" then
            error("ui.each: the key of item " .. i .. " is a " .. type(k) .. ", not a string or a number", 0)
        end
        return k
    end

    -- The keyed list of `each`, kept by an effect: an item whose key stays keeps its node, its scope and its local states and
    -- gets the new item through its state; a new key runs `fn`; a key that left is disposed.
    ensureEach = function(each, parent)
        if each.effect then return each end
        each.parent = parent
        local prevO = owner
        owner = each.owner
        each.holder = record("scope")
        local e = record("effect", function()
            local list = resolve(each.source, 0)
            if type(list) ~= "table" then list = {} end
            local keys, seen = {}, {}
            untracked(function()
                for i, item in list do
                    local k = keyOf(each, item, i)
                    if seen[k] then error("ui.each: two items have the key " .. tostring(k), 0) end
                    seen[k] = true
                    keys[i] = k
                end
            end)
            local changed = #keys ~= #each.order
            for i, item in list do
                local k = keys[i]
                local it = each.items[k]
                if it then
                    State.set(it.state, item)
                else
                    local scope
                    do
                        local o = owner
                        owner = each.holder
                        scope = record("scope")
                        owner = o
                    end
                    local s = state(item)
                    local o = owner
                    owner = scope
                    local ok, node = pcall(untracked, each.fn, s, k)
                    owner = o
                    if not ok then dispose(scope) error(node, 0) end
                    if not isNode(node) then
                        dispose(scope)
                        error("ui.each: the function must return a component, it returned a " .. type(node), 0)
                    end
                    if rawget(node, "key") == nil then node.key = k end
                    each.items[k] = { scope = scope, state = s, node = node }
                    changed = true
                end
                if not changed and each.order[i] ~= k then changed = true end
            end
            for k, it in each.items do
                if not seen[k] then
                    dispose(it.scope)
                    each.items[k] = nil
                    changed = true
                end
            end
            each.order = keys
            return changed
        end)
        e.changed = function(changed) if changed then pendingChildren[parent] = true end end
        e.always = true
        each.effect = e
        local ok, err = pcall(run, e)
        owner = prevO
        if not ok then error(err, 0) end
        return each
    end

    local function each(source, key, fn)
        if type(fn) ~= "function" then error("ui.each(source, key, fn): fn must be a function", 2) end
        if type(key) ~= "string" and type(key) ~= "function" and key ~= nil then
            error("ui.each(source, key, fn): key must be a field name or a function", 2)
        end
        sawBinding = true
        local mark = { source = source, key = key, fn = fn, items = {}, order = {}, owner = owner }
        eachMarks[mark] = true
        return mark
    end


    local function feed(name, params)
        local value, stale, key = feed_host(name, params)
        if current ~= nil and type(key) == "string" then track(source("feed:" .. key)) end
        return value, stale
    end

    local function liveFeeds()
        local out = {}
        for key, r in sources do
            if string.sub(key, 1, 5) == "feed:" and next(r.subs) ~= nil then table.insert(out, string.sub(key, 6)) end
        end
        table.sort(out)
        return out
    end

    local internals = { records = records, feed = feed }

    -- Runs `view` once. A view that binds a state, a computed or an each is mounted: it returns its tree with a handle on
    -- every node and keeps its effects; any other view is the whole-view model and returns its tree as it built it.
    function internals.mount(view, st, data)
        if root then dispose(root) end
        root = nil
        sawBinding = false
        queue, pendingSet, setOrder, pendingChildren = {}, {}, {}, {}
        local o = owner
        owner = record("scope")
        local scope = owner
        local ok, tree = pcall(view, st, data)
        owner = o
        if not ok then dispose(scope) error(tree, 0) end
        if not sawBinding then
            dispose(scope)
            return tree, false
        end
        root = scope
        if not isNode(tree) then error("view returned something that is not a component tree", 0) end
        local snap = snapshot(tree, false)
        queue, pendingSet, setOrder, pendingChildren = {}, {}, {}, {}
        return snap, true
    end

    function internals.unmount()
        if root then dispose(root) end
        root = nil
        queue, pendingSet, setOrder, pendingChildren = {}, {}, {}, {}
    end

    -- Bumps the source `bump` names (`feed:<key>`, `size:<key>`), if any, then runs every stale effect and returns the
    -- writes they made, and the feeds the mounted view reads now.
    function internals.flush(bump)
        if bump ~= nil and sources[bump] then
            local r = sources[bump]
            r.version += 1
            markStale(r)
        end
        while #queue > 0 do
            local batch = queue
            queue = {}
            table.sort(batch, function(a, b) return a.id < b.id end)
            for i, e in batch do
                local ok, err = pcall(refresh, e)
                if not ok then
                    for j = i + 1, #batch do table.insert(queue, batch[j]) end
                    if e.stale then table.insert(queue, e) end
                    error(err, 0)
                end
            end
        end
        local ops = {}
        local order, sets, kids = setOrder, pendingSet, pendingChildren
        pendingSet, setOrder, pendingChildren = {}, {}, {}
        for _, n in order do
            local m = meta[n]
            if m and m.drawn then
                for prop, v in sets[n] do
                    table.insert(ops, { op = "set", node = m.h, prop = prop, value = if v == NONE then nil else snapValue(v, true, 0) })
                end
            end
        end
        local parents = {}
        for n in kids do
            local m = meta[n]
            if m and m.drawn then table.insert(parents, n) end
        end
        table.sort(parents, function(a, b) return meta[a].h < meta[b].h end)
        for _, n in parents do
            table.insert(ops, { op = "children", node = meta[n].h, children = childList(n, true) })
        end
        return ops, liveFeeds()
    end


    local function node(name, arg)
        local n = { component = name }
        if owner ~= nil then nodeOwner[n] = owner end
        if type(arg) == "string" then
            n.text = arg
            return n
        end
        if type(arg) ~= "table" then return n end
        -- Only a VM that made a state or computed can hold a binding, so a whole-view script pays nothing for the check.
        local check = not sawBinding and next(records) ~= nil
        for k, v in arg do
            if type(k) ~= "number" then
                n[k] = v
                if check and bound(v, 0) then sawBinding, check = true, false end
            end
        end
        local children = {}
        for i, v in ipairs(arg) do
            children[i] = if type(v) == "string" then { component = "Text", text = v } else v
        end
        if #children > 0 then n.children = children end
        return n
    end

    local styleTypes = {
        fillStyle = "string", strokeStyle = "string", font = "string",
        lineWidth = "number", globalAlpha = "number",
    }
    local function numbers(method, ...)
        for i = 1, select("#", ...) do
            if type(select(i, ...)) ~= "number" then
                error("ctx:" .. method .. " expects numbers", 3)
            end
        end
    end
    local function recorder(commands)
        local style = { fillStyle = "fg", strokeStyle = "fg", lineWidth = 1, font = "10px sans-serif", globalAlpha = 1 }
        local stack = {}
        local methods = {}
        local count = 0
        local function add(command)
            count += 1
            if count > 250000 then
                error("a Canvas draw recorded " .. count .. " commands; the limit is 250,000", 3)
            end
            commands[count] = command
        end
        local function plain(method, count)
            methods[method] = function(_, ...)
                numbers(method, ...)
                if select("#", ...) ~= count then error("ctx:" .. method .. " expects " .. count .. " numbers", 2) end
                add({ method, ... })
            end
        end
        local function bare(method)
            methods[method] = function() add({ method }) end
        end
        for _, m in { "beginPath", "closePath", "fill", "stroke" } do bare(m) end
        for m, count in { moveTo = 2, lineTo = 2, bezierCurveTo = 6, quadraticCurveTo = 4, rect = 4,
                          roundRect = 5, translate = 2, scale = 2, rotate = 1 } do
            plain(m, count)
        end
        methods.arc = function(_, x, y, r, a0, a1, ccw)
            numbers("arc", x, y, r, a0, a1)
            if ccw ~= nil and type(ccw) ~= "boolean" then error("ctx:arc expects a boolean for counterclockwise", 2) end
            add({ "arc", x, y, r, a0, a1, ccw == true })
        end
        methods.save = function()
            local saved = {}
            for k, v in style do saved[k] = v end
            stack[#stack + 1] = saved
            add({ "save" })
        end
        methods.restore = function()
            local saved = stack[#stack]
            if saved then
                stack[#stack] = nil
                for k, v in saved do style[k] = v end
            end
            add({ "restore" })
        end
        methods.fillText = function(_, text, x, y)
            if type(text) ~= "string" then error("ctx:fillText expects text", 2) end
            numbers("fillText", x, y)
            add({ "fillText", text, x, y })
        end
        methods.drawImage = function(_, img, x, y, w, h)
            if type(img) ~= "table" or type(img.width) ~= "number" or type(img.height) ~= "number"
                or type_of(img.data) ~= "buffer" then
                error("ctx:drawImage expects { width, height, data = buffer }", 2)
            end
            numbers("drawImage", x, y)
            if w ~= nil or h ~= nil then numbers("drawImage", w, h) end
            for _, side in { img.width, img.height } do
                if side % 1 ~= 0 or side < 1 or side > 2048 then
                    error("ctx:drawImage: an image is 1 to 2048 whole pixels on each side (2048x2048 at most), this one is "
                        .. img.width .. "x" .. img.height, 2)
                end
            end
            local expected = img.width * img.height * 4
            if buffer_len(img.data) ~= expected then
                error("ctx:drawImage: a " .. img.width .. "x" .. img.height .. " image needs " .. expected
                    .. " bytes of RGBA, the buffer has " .. buffer_len(img.data), 2)
            end
            local copy = buffer.create(expected)
            buffer.copy(copy, 0, img.data)
            add({ "drawImage", copy, img.width, img.height, x, y, w or img.width, h or img.height })
        end
        methods.measureText = function(_, text)
            if type(text) ~= "string" then error("ctx:measureText expects text", 2) end
            if #text > 10000 then error("ctx:measureText takes at most 10,000 bytes of text", 2) end
            local width, ascent, descent = measure_host(text, style.font)
            return { width = width, fontBoundingBoxAscent = ascent, fontBoundingBoxDescent = descent }
        end
        return setmetatable({}, {
            __index = function(_, k)
                local v = style[k]
                if v ~= nil then return v end
                local method = methods[k]
                if method == nil then
                    error("ctx has no field or method '" .. tostring(k) .. "' (see docs/plugin-runtime.md for the Canvas 2D calls ctx has)", 2)
                end
                return method
            end,
            __newindex = function(_, k, v)
                local want = styleTypes[k]
                if want == nil then error("ctx has no field " .. tostring(k), 2) end
                if type(v) ~= want then error("ctx." .. k .. " expects a " .. want, 2) end
                style[k] = v
                add({ "set", k, v })
            end,
            __metatable = false,
        })
    end

    local function canvas(arg)
        if type(arg) ~= "table" or type(arg.draw) ~= "function" then return node("Canvas", arg) end
        local key = if arg.key == nil then "canvas" else arg.key
        local height = if arg.height == nil then 240 else arg.height
        local function draw()
            if current ~= nil then track(sizeOf(key)) end
            local size = sizes[key]
            local commands = {}
            arg.draw(recorder(commands), if size then size.w else 0, if size then size.h else height)
            canvases[commands] = true
            return commands
        end
        local n = node("Canvas", arg)
        n.draw, n.key, n.on_pointer = nil, nil, nil
        n.height, n.canvasKey, n.pointerAction = height, key, arg.on_pointer
        n.commands = untracked(draw)
        canvasDraw[n] = draw
        return n
    end

    return setmetatable({}, {
        __index = function(_, name)
            if name == "Canvas" then return canvas end
            if name == "state" then return state end
            if name == "computed" then return computed end
            if name == "each" then return each end
            return function(arg) return node(name, arg) end
        end,
        __metatable = false,
    }), internals
    """

    /// `require(path)`, called with the host's module loader and `require`'s table.
    private static let requirePrelude = """
    local load_module, modules = ...
    local function resolve(path)
        if type(path) ~= "string" then
            error('require takes a path in the package, like "lib/ui"', 3)
        end
        if string.sub(path, 1, 1) == "/" then
            error('require("' .. path .. '"): the path leaves the package', 3)
        end
        local parts = {}
        for part in string.gmatch(path, "[^/]+") do
            if part == ".." then
                if #parts == 0 then
                    error('require("' .. path .. '"): the path leaves the package', 3)
                end
                table.remove(parts)
            elseif part ~= "." then
                table.insert(parts, part)
            end
        end
        local key = table.concat(parts, "/")
        if string.sub(key, -5) ~= ".luau" then key ..= ".luau" end
        return key
    end
    return function(path)
        local key = resolve(path)
        local hit = modules.loaded[key]
        if hit ~= nil then return hit.value end
        if modules.loading[key] then
            error('require("' .. path .. '"): ' .. key .. ' is already being required (a require cycle)', 2)
        end
        local fn, message = load_module(key)
        if fn == nil then
            error(message or ('require("' .. path .. '"): the package has no ' .. key), 2)
        end
        modules.loading[key] = true
        local ok, value = pcall(fn)
        modules.loading[key] = nil
        if not ok then error(value, 0) end
        modules.loaded[key] = { value = value }
        return value
    end
    """

    private func prepareGlobals() throws {
        luaL_openlibs(L)
        lua_pushnil(L)
        lua_setfield(L, WHEEL_LUA_GLOBALSINDEX, "os")
        lua_createtable(L, 0, 0)
        sizesRef = lua_ref(L, -1)
        pop(1)
        try compile(Self.prelude, chunkName: "=ui")
        wheel_luau_pushhost(L, HostKind.measure.rawValue)
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, sizesRef)
        lua_createtable(L, 0, 0)
        canvasesRef = lua_ref(L, -1)
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, canvasesRef)
        lua_remove(L, -2)
        wheel_luau_pushhost(L, HostKind.feed.rawValue)
        try call(args: 4, results: 2)
        lua_rawgetfield(L, -1, "records")
        recordsRef = popRef()
        internalsRef = popRef()
        lua_setfield(L, WHEEL_LUA_GLOBALSINDEX, "ui")
        lua_createtable(L, 0, 2)
        lua_createtable(L, 0, 0)
        lua_setfield(L, -2, "loaded")
        lua_createtable(L, 0, 0)
        lua_setfield(L, -2, "loading")
        requireRef = lua_ref(L, -1)
        pop(1)
        try compile(Self.requirePrelude, chunkName: "=require")
        wheel_luau_pushhost(L, HostKind.module.rawValue)
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, requireRef)
        try call(args: 2, results: 1)
        lua_setfield(L, WHEEL_LUA_GLOBALSINDEX, "require")
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

    /// Pushes the prelude's own function `name`.
    private func pushInternal(_ name: String) {
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, internalsRef)
        lua_rawgetfield(L, -1, name)
        lua_remove(L, -2)
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

    /// Puts a new `loaded` table in `require`'s table, or `ref`'s table, and returns a ref to the
    /// one it replaced.
    private func swapLoaded(to ref: Int32? = nil) -> Int32 {
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, requireRef)
        lua_rawgetfield(L, -1, "loaded")
        let old = lua_ref(L, -1)
        pop(1)
        if let ref { lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, ref) } else { lua_createtable(L, 0, 0) }
        lua_rawsetfield(L, -2, "loaded")
        pop(1)
        return old
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
        case .module:
            guard let source = modules[name] else {
                lua_pushnil(L)
                return 1
            }
            do {
                try compile(source, chunkName: "=" + modulePrefix + name)
                return 1
            } catch {
                lua_pushnil(L)
                lua_pushstring(L, "\(error)")
                return 2
            }
        case .feed:
            let read = host.feed(name, args)
            push(read.value)
            lua_pushboolean(L, read.stale ? 1 : 0)
            if let key = read.key { push(.string(key)) } else { lua_pushnil(L) }
            return 3
        case .action:
            host.action(name, args)
            return 0
        case .measure:
            guard case let .string(font) = args else {
                lua_pushstring(L, "ctx:measureText expects text")
                return -1
            }
            let cacheKey = font + "\u{0}" + name
            let m: (width: Double, ascent: Double, descent: Double)
            if let hit = measureCache[cacheKey] {
                m = hit
            } else {
                limits.pointee.steps &+= Self.measureCost
                m = CanvasFont(font).measure(name)
                if measureCache.count >= Self.maxMeasureCache { measureCache.removeAll() }
                measureCache[cacheKey] = m
            }
            lua_pushnumber(L, m.width)
            lua_pushnumber(L, m.ascent)
            lua_pushnumber(L, m.descent)
            return 3
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
        var canvas = false
        var description: String {
            canvas ? "a Canvas holds more than 2,000,000 values in its commands"
                : "more than \(PaneRuntime.maxNodes) values (a shared or cyclic table?)"
        }
    }

    /// The Lua value at `index` as JSON. A table whose keys are exactly 1...n is an array, as is an
    /// empty table; any other table is an object, its number keys written as strings. Functions
    /// and other values are null.
    private func json(at index: Int32) throws -> JSONValue {
        var visited = 0
        return try json(at: index, depth: 0, visited: &visited, limit: Self.maxNodes, inCanvas: false)
    }

    /// Values one Canvas's command list may hold, counted apart from the view's: 250,000 commands of at most 8 values each always
    /// fit, so only a list the script changed after `ui.Canvas` returned (a shared or cyclic table appended) reaches it.
    private static let maxCanvasValues = 2_000_001

    /// Whether the table at `index` is a command list the prelude recorded.
    private func isCanvasList(at index: Int32) -> Bool {
        lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, canvasesRef)
        lua_pushvalue(L, index)
        lua_rawget(L, -2)
        let found = lua_toboolean(L, -1) != 0
        pop(2)
        return found
    }

    private func json(at index: Int32, depth: Int, visited: inout Int, limit: Int, inCanvas: Bool) throws -> JSONValue {
        visited += 1
        guard visited <= limit else { throw inCanvas ? TooLarge(canvas: true) : TooLarge() }
        let index = index < 0 ? lua_gettop(L) + index + 1 : index
        switch UInt32(lua_type(L, index)) {
        case LUA_TBOOLEAN.rawValue: return .bool(lua_toboolean(L, index) != 0)
        case LUA_TNUMBER.rawValue: return .number(lua_tonumberx(L, index, nil))
        case LUA_TSTRING.rawValue: return string(at: index, in: L).map(JSONValue.string) ?? .null
        case LUA_TBUFFER.rawValue:
            var length = 0
            guard let bytes = lua_tobuffer(L, index, &length) else { return .null }
            return .string(Data(bytes: bytes, count: length).base64EncodedString())
        case LUA_TTABLE.rawValue:
            guard depth < Self.maxDepth else { throw TooLarge() }
            lua_checkstack(L, 4)
            if !inCanvas, isCanvasList(at: index) {
                var own = 0
                return try json(at: index, depth: depth, visited: &own, limit: Self.maxCanvasValues, inCanvas: true)
            }
            // A `ui.state` or `ui.computed` is its value.
            lua_rawgeti(L, WHEEL_LUA_REGISTRYINDEX, recordsRef)
            lua_pushvalue(L, index)
            lua_rawget(L, -2)
            if lua_type(L, -1) == LUA_TTABLE.rawValue {
                lua_rawgetfield(L, -1, "value")
                defer { pop(3) }
                return try json(at: -1, depth: depth + 1, visited: &visited, limit: limit, inCanvas: inCanvas)
            }
            pop(2)
            var byIndex: [Int: JSONValue] = [:]
            var fields: [String: JSONValue] = [:]
            lua_pushnil(L)
            while lua_next(L, index) != 0 {
                let value: JSONValue
                do {
                    value = try json(at: -1, depth: depth + 1, visited: &visited, limit: limit, inCanvas: inCanvas)
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

private extension JSONValue {
    var arrayItems: [JSONValue] { if case let .array(a) = self { a } else { [] } }
}
