# Plugin system: the backend protocol crate and how Luau is built

The plugin system (epic `wheel-pmf2`) needs two pieces of third-party code: a JSON-RPC 2.0 peer for the protocol between `wheeld` and a plugin's backend, and the Luau VM inside both Apple clients. This page names each choice, the candidates checked and the checks that showed it works, and says how the pane runtime built on Luau behaves.

## Backend protocol: `async-lsp` over Content-Length framing

**Crate:** [`async-lsp`](https://docs.rs/async-lsp) 0.2.4, built with `default-features = false, features = ["tokio"]`, in `wheeld` and in the Rust SDK.

**Framing:** LSP's base protocol: each message is a `Content-Length: <bytes>\r\n\r\n` header followed by that many bytes of UTF-8 JSON. The same bytes go over the unix socket `wheeld` assigns a backend and over a backend's stdio.

**Patches:** a `feed/update` carries either `value` (the whole new value) or `patch`, an RFC 6902 JSON Patch array, applied with the [`json-patch`](https://docs.rs/json-patch) crate (5.0.0).

**TypeScript SDK:** [`vscode-jsonrpc`](https://www.npmjs.com/package/vscode-jsonrpc) 10.0.0, which speaks the same framing in both directions.

What the protocol needs from a crate, and how `async-lsp` meets it:

| Need | How `async-lsp` does it |
| --- | --- |
| One connection where each side sends requests and notifications to the other (`wheeld` calls `initialize` and `action/invoke`; the backend calls `wheel/call` and sends `feed/update` and `log`) | `MainLoop::new_client` and `MainLoop::new_server` each return a socket for outbound `request`/`notify`, while a `Router` answers inbound messages on the same stream |
| A request handled while the other side is still waiting on its own request (the backend calls `wheel/call` during `initialize`) | Handlers return futures; the main loop keeps reading while one is pending |
| Wheel's own method names and serde types | A method is a type implementing `lsp_types::request::Request` or `Notification` (three items: `METHOD`, `Params`, `Result`), so no LSP method is required |
| Unix socket and stdio | `MainLoop::run_buffered` takes any `futures` `AsyncRead` and `AsyncWrite`; a tokio stream goes through `tokio_util::compat` |

Its cost: it depends on `lsp-types` 0.95 for the two traits, and has one maintainer.

Candidates checked:

| Crate | Version | Why not |
| --- | --- | --- |
| `jsonrpsee` | 0.26.1 | Its client makes requests but takes none, and its transports are HTTP and WebSocket; no peer that is client and server on one stream |
| `json-rpc-rs` | 0.3.0 | Serves inbound calls only (`call()` returns a response); no outbound requests |
| `jsoncall` | 0.0.3 | Pre-release, 11% of the crate documented |
| `rmcp` | 3.8.0 (in the tree) | MCP's message set only; the protocol replaces MCP |
| Hand-written newline-delimited JSON over `tokio-util`'s `LinesCodec` | — | A request/response correlator and dispatcher that `async-lsp` already ships |

Throwaway check (`async-lsp` 0.2.4, one `tokio::net::UnixStream`): `wheeld`'s side called `initialize`; while answering, the backend called `wheel/call` back and got `{"ok":true}`; then `action/invoke` made the backend send one `feed/update` with `value` and one with `patch`:

```text
wheeld <- wheel/call notify {"title":"toy up"}
wheeld -> initialize: feeds ["build", "log"] actions ["rebuild"]
wheeld -> action/invoke: {"started":"rebuild"}
wheeld <- feed/update {"feed":"build","params":{},"value":{"progress":0.5}}
wheeld <- feed/update {"feed":"log","params":{},"patch":[{"op":"add","path":"/lines/-","value":"compiling wheeld"}]}
feed updates received: 2
```

## Luau: vendored sources as a local Swift package

**Version:** Luau [0.747](https://github.com/luau-lang/luau/releases/tag/0.747), MIT licence. Source tarball `https://github.com/luau-lang/luau/archive/refs/tags/0.747.tar.gz`, SHA-256 `2c72471f91c42af7f2c07cc4cc602b27c215346d491d7ab3d80da63050ad43f2`.

**Packaging:** the package is `apple/WheelLuau`, beside WheelUI. Luau's `Common`, `Ast`, `Bytecode`, `Compiler` and `VM` directories (127 files, 2.6 MB) and its `LICENSE.txt` are copied, unchanged from the tarball above, into its `luau/` directory. `CodeGen` (the JIT, which iOS forbids), `Analysis`, `Config`, `Require`, `Inliner` and `CLI` are left out. The package has three targets:

- `LuauCore`: every `.cpp` of those five directories, compiled as C++17. Its only public header directory is `VM/include` (`lua.h`, `luaconf.h`, `lualib.h`, all plain C); the C++ include directories are `headerSearchPath`s, so Swift's importer never meets a C++ header. It defines `LUA_API` and `LUACODE_API` as `extern "C"` for the C++ build only.
- `CLuau`: the header Swift imports, `Sources/CLuau/include/CLuau.h`, which includes `lua.h`, `lualib.h` and `../../../luau/Compiler/include/luacode.h` (a path relative to the header, since `luacode.h` belongs to no public header directory), and `Sources/CLuau/CLuau.cpp`, the C++ that may raise a Luau error: the memory-limited allocator, the step-budget interrupt callback and the trampoline that calls a host function. A Luau error is a C++ exception, and these keep it from ever unwinding a Swift frame.
- `WheelLuau`: `PaneRuntime`, one pane script in its own VM, and `PaneNode`, the tree its `view` returns (see below).

Candidates checked:

| Candidate | Why not |
| --- | --- |
| [`RadiusDay/SwiftLuau`](https://github.com/RadiusDay/SwiftLuau) (the only Luau package on the Swift Package Index) | Pins Luau 0.701 (tag `0.3.2+0.701`), last updated 9 Dec 2025, its README says it is in early stages, two stars |
| Luau as a git submodule | The repo has no submodules; the five directories would still need a `Package.swift` of ours, and a clone would carry the 14 directories the apps never build |
| Luau's CMake build into an XCFramework | A second build system beside SwiftPM and Xcode, and a binary checked in or downloaded at build time |

## The pane runtime

`PaneRuntime(source:chunkName:limits:host:)` (`apple/WheelLuau/Sources/WheelLuau/PaneRuntime.swift`) runs one pane script. The script returns a module table:

```lua
local function init(previous) return previous or { count = 0 } end
local function update(state, msg)
    if msg.type == "inc" then state.count += 1 end
    return state
end
local function view(state, data)
    return ui.VStack{
        ui.Text("Count " .. state.count),
        ui.Button{ label = "Add", action = { type = "inc" } },
    }
end
return { init = init, update = update, view = view }
```

- `init(previous)` makes the state: `previous` is nil for a new pane and the old state after `reload(source:)`. A module without `init` starts from an empty table.
- `send(msg)` calls `update(state, msg)`. A non-nil result becomes the state; nil keeps the state `update` changed in place. A module without `update` ignores messages.
- `view(data)` calls `view(state, data)` and returns a `PaneNode`. `data` holds the caller's fields (when it is a JSON object) and `data.feed(name, params)`; `wheel.action(name, args)` is a global. Both call the `Host` closures the caller supplies.
- `ui.<Name>{ ... }` builds a node for any component name, so a script names WheelUI's components and modifiers as WheelUI does (ADR-0031, ADR-0038): string keys become properties (`modifiers` among them), the array part becomes `children`, a string in it becomes a `Text`, and `ui.Text("hi")` sets `text`. `PaneNode.a2uiComponents()` flattens a tree into the component list of an A2UI `updateComponents` message, the form WheelUI's surface draws.

The VM is sandboxed:

| Limit | How |
| --- | --- |
| Frozen globals | `luaL_openlibs`, then `os` removed and `ui` and `wheel` set, then `luaL_sandbox`; each module loads in its own thread after `luaL_sandboxthread`. Luau ships no `io` or `require` in these libraries. |
| Steps | `lua_callbacks(L)->interrupt` counts safepoints (loop back edges and calls) and raises an error past `Limits.steps` (default 10,000,000 per entry). The count stays past the budget, so a script that catches the error with `pcall` is stopped again at its next safepoint. |
| Memory | `lua_newstate` with an allocator that counts bytes and refuses growth past `Limits.memory` (default 32 MB) while script code runs; Swift's own pushes run with the limit lifted, so they never raise. An entry that starts above half the limit runs a full collection first, since the allocator cannot collect and garbage from earlier entries would otherwise count against it. |
| Interpreter only | `CodeGen` is not in the package. |
| No script outside the limits | The module's `init`, `update` and `view` are read raw (`lua_rawgetfield`), so a metatable on the module runs nothing. Converting a Lua value to JSON (`view`'s tree, host-call arguments, `state`) visits at most 100,000 values and 200 levels, so a shared or cyclic table is an error, never a hang. |

Every failure is a thrown `PaneRuntime.Failure` (`syntax`, `runtime`, `stepBudget`, `memory`, `module`, `view`) and leaves the last good module in place: a reload whose source fails to compile, or whose `init` raises, keeps the previous module running. The state is one Lua table, not a copy, so it keeps whatever `update` or `init` changed in it before raising.

A Lua table becomes a JSON array when its keys are exactly 1 to n, or it is empty; any other table becomes an object, its number keys written as strings (`{1, 2, nil, 4}` gives `{"1":1,"2":2,"4":4}`). Flattening gives each node an id from its place (`root.0.2`, and `root.1@child` for a node held in a property), and those ids win over a script's own `id` property.

Checks (Swift 6.4.2, Xcode with the macOS 27.2 SDK):

- macOS, `swift test --package-path apple/WheelLuau`: 19 tests pass (counter, reload keeping state, step budget, memory limit, absent `io`/`os`/`require`, frozen libraries, syntax error and failing `init` on reload, bad module, bad view, host feed and action, a host call from a coroutine, flattening and its ids, a module with a hostile metatable, shared and cyclic tables, garbage from an earlier entry, table conversion).
- iOS, `xcodebuild -scheme WheelLuau -destination 'generic/platform=iOS'` and `'generic/platform=iOS Simulator'` from `apple/WheelLuau`: `** BUILD SUCCEEDED **` for both; the device objects are `arm64`, `platform IOS`, `minos 18.0`, with `luau_compile`, `luau_load`, `lua_pcall`, `luaL_sandbox` and `wheel_luau_pcall` defined.
