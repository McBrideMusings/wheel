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
- `view(data)` calls `view(state, data)` and returns a `PaneNode`. `data` holds the caller's fields (when it is a JSON object) and `data.feed(name, params)`, which returns two values, the feed's value and `stale` (true while the backend that publishes it is down, so the value is the last one it sent); `wheel.action(name, args)` is a global. Both call the `Host` closures the caller supplies.
- `ui.<Name>{ ... }` builds a node for any component name, so a script names WheelUI's components and modifiers as WheelUI does (ADR-0031, ADR-0038): string keys become properties (`modifiers` among them), the array part becomes `children`, a string in it becomes a `Text`, and `ui.Text("hi")` sets `text`. `PaneNode.a2uiComponents()` flattens a tree into the component list of an A2UI `updateComponents` message, the form WheelUI's surface draws.

The VM is sandboxed:

| Limit | How |
| --- | --- |
| Frozen globals | `luaL_openlibs`, then `os` removed and `ui`, `wheel` and `require` set, then `luaL_sandbox`; each module loads in its own thread after `luaL_sandboxthread`. Luau ships no `io` or `require` in these libraries; Wheel's `require` is its own (below). |
| `require` | `require("lib/ui")` runs the package's `lib/ui.luau` (`.luau` may be left off; `.` parts are dropped and `..` climbs one folder, so `lib/../ui` is `ui.luau`) from the modules `plugin_source` carried, once per VM, and returns what it returned. It runs under the entry that called it, so its steps and memory count against that entry. A path starting with `/`, or whose `..` climbs above the package folder, raises `require("../x"): the path leaves the package`; a missing file raises `the package has no lib/x.luau`; a module that is required again before it returns raises `a.luau is already being required (a require cycle)`; a module's syntax error names it (`demo/lib/x.luau:1: ...`). Each error is a runtime failure and shows in the pane header. A reload takes the new modules and forgets what ran and what was loading, so each module runs again on its next `require`; a failed reload keeps the old ones. Requiring across packages is not possible. |
| Steps | `lua_callbacks(L)->interrupt` counts safepoints (loop back edges and calls) and raises an error past `Limits.steps` (default 10,000,000 per entry). The count stays past the budget, so a script that catches the error with `pcall` is stopped again at its next safepoint. |
| Memory | `lua_newstate` with an allocator that counts bytes and refuses growth past `Limits.memory` (default 32 MB) while script code runs; Swift's own pushes run with the limit lifted, so they never raise. An entry that starts above half the limit runs a full collection first, since the allocator cannot collect and garbage from earlier entries would otherwise count against it. |
| Interpreter only | `CodeGen` is not in the package. |
| No script outside the limits | The module's `init`, `update` and `view` are read raw (`lua_rawgetfield`), so a metatable on the module runs nothing. Converting a Lua value to JSON (`view`'s tree, host-call arguments, `state`) visits at most 100,000 values and 200 levels, so a shared or cyclic table is an error, never a hang. |

Every failure is a thrown `PaneRuntime.Failure` (`syntax`, `runtime`, `stepBudget`, `memory`, `module`, `view`) and leaves the last good module in place: a reload whose source fails to compile, or whose `init` raises, keeps the previous module running. The state is one Lua table, not a copy, so it keeps whatever `update` or `init` changed in it before raising.

A Lua table becomes a JSON array when its keys are exactly 1 to n, or it is empty; any other table becomes an object, its number keys written as strings (`{1, 2, nil, 4}` gives `{"1":1,"2":2,"4":4}`). Flattening gives each node an id from its place (`root.0.2`, and `root.1@child` for a node held in a property), and those ids win over a script's own `id` property.

Checks (Swift 6.4.2, Xcode with the macOS 27.2 SDK):

- macOS, `swift test --package-path apple/WheelLuau`: 29 tests pass (counter, reload keeping state, step budget, memory limit, absent `io`/`os`, `require` (a module drawn, run once, rerun after a reload, kept after a failed reload, a path out of the package, a missing module, a cycle, a module syntax error, a `..` that climbs back in), frozen libraries, syntax error and failing `init` on reload, bad module, bad view, host feed and action, a host call from a coroutine, flattening and its ids, a module with a hostile metatable, shared and cyclic tables, garbage from an earlier entry, table conversion).
- iOS, `xcodebuild -scheme WheelLuau -destination 'generic/platform=iOS'` and `'generic/platform=iOS Simulator'` from `apple/WheelLuau`: `** BUILD SUCCEEDED **` for both; the device objects are `arm64`, `platform IOS`, `minos 18.0`, with `luau_compile`, `luau_load`, `lua_pcall`, `luaL_sandbox` and `wheel_luau_pcall` defined.

## Plugin panes

A plugin package is a folder holding `wheel-plugin.toml`: `[plugin]` (`id`, `version`) and any number of `[[pane]]` (`id`, `title`, `script`, `reads`, `actions`). `wheeld` finds packages in `<project>/.wheel/plugins/<id>/`, then `~/.wheel/plugins/<id>/` (beside the machine `wheel.toml`), then its built-ins (`daemon/plugins/<id>/`, compiled in; `sample` has a counter and a text field). The first package with an id is the one a pane runs, and a project's packages show only in its own spaces (`daemon/src/plugins.rs`). A script path may not leave its package, and a script that with the package's other `.luau` files comes to over 1 MB is refused. Those files are the modules `require` reads: every `.luau` under the package folder but the pane's own script, at most 8 folders deep and 10,000 entries, skipping hidden folders and `node_modules`, `target`, `build`, `dist` and `vendor`, without entering a symlinked folder or reading a symlinked file that resolves outside the package. A module that cannot be read (not UTF-8, no permission) is left out and logged, never a refusal of the pane.

A plugin pane is the pane kind `{"kind":"plugin","plugin":"<id>","pane":"<pane id>"}`, set with `pane_set_kind` kind `plugin` and `"plugin":{"plugin":..,"pane":..}` (`wheel pane kind <pane> plugin --plugin <id> --plugin-pane <pane id>`). `plugin_ls` lists the packages a space can use (`wheel plugin ls`); `plugin_source` returns a pane's script and, in `modules`, every other `.luau` file of its package keyed by its path in the package (`lib/ui.luau`; `PROTOCOL_VERSION` 39), and `wheeld` sends `plugin_source` unasked to every connection when a shown pane's script or any of those modules changes on disk (one message per save, after a 100 ms debounce). `PROTOCOL_VERSION` 35.

On the Mac, the GPUI client opens each plugin pane in the bridge (`clients/gpui/native/Bridge.swift`), which fetches the script over its own `wheeld` socket and runs it in WheelUI's `PluginPane` (`apple/WheelUI/Sources/WheelUI/Plugin/PluginPane.swift`). `view(state, data)` gets the pane's place in `data`: `pane` (its id), `space`, `cwd` (the space's directory), `plugin` and `plugin_pane` (the manifest's ids), so a script can pass its project directory as a feed param.

Each render flattens `view`'s tree into an A2UI surface. Every table the script puts where a component raises an action becomes the message `update` gets when it does: a component property named `action` or ending in `Action` (a `Button`'s `action`, a `Table`'s `rowAction`, `sortAction`, `foldAction` and `selectAction`, a `Modal`'s `dismissAction`), a `Canvas` hit's `action` and `doubleAction`, a `ConfirmationDialog` button's `action` and a `Table` row's `menu[].action`. A table anywhere else (a hit's `context`, a row's cells) is data and reaches the component as written. A `Button` `action = { copy = { text = ... } }` copies the text, as in A2UI, and sends nothing. The component's own fields for the event are added where the table has no key of that name:

| Raised by | Fields added |
| --- | --- |
| `Table` `rowAction` | `id` (the row's) |
| `Table` `sortAction` | `key`, `asc` |
| `Table` `foldAction` | `path` |
| `Table` `selectAction` | `ids` (every selected row) |
| `Canvas` hit `action`, `doubleAction` | `id` (the hit's), plus the hit's `context` / `doubleContext` |
| `Button`, row `menu`, `Modal` `dismissAction` | none |

A control with an `on_change` table (`TextField`, `CheckBox`, `ChoicePicker`, `Slider`, `Stepper`, `DateTimeInput`, ...) has its `value` bound to the data model and sends the table with `value` set to what the control wrote (text, a boolean, the array of picked values, a number). A `TextField` is bound with or without `on_change`. The control shows what the person last wrote until the script's `value` changes: a render writes the script's `value` only when it differs from the one the script gave the render before, so a script that does not echo an edit back never undoes it, and one that sets a new value (a reset) shows it. A failure keeps the last good view and puts the error in the pane header; a good load clears it.

## Plugin backends

A package's `[backend]` is a process `wheeld` runs and talks to in JSON-RPC 2.0 with Content-Length framing, through `async-lsp` (`daemon/src/backends.rs`). Keys: `command` (required, run with `/bin/sh -c` in the package folder), `transport` (`"stdio"`, the default, or `"unix"`), `restart` (`"always"`, `"on-failure"`, the default, or `"never"`), `watch` and `needs` (string arrays). `wheel plugin ls` and `plugin_ls` report the backend per package with its `state`: `stopped` (never started), `starting`, `running` or `exited`, plus `pid`, `feeds`, `actions`, `log` and `error` (a spawn failure, a failed `initialize`, or how the process ended: `exited with status 1`, `ended by signal 9`). Every process after the first is sent `feed/subscribe` for every feed of the package a pane still reads, once it answers `initialize`; feeds reach their backend through the package, never a stored process, so an exited process is never sent `feed/unsubscribe`.

**Restart.** One supervisor task per package starts the process, starts it again, and never lets two run at once. When a process ends or fails to start without `wheeld` asking, `restart` decides: `always` starts it again, `on-failure` starts it again unless it exited with status 0 (a signal, a failed `initialize` and a spawn failure all count as failures), `never` leaves it `exited`. A restart waits 1 s, doubling per consecutive failure up to 30 s; a process that stayed up 10 s starts the count over. It happens whether or not a pane reads a feed, runs `initialize` again and replays `feed/subscribe` for the feeds panes read. The package's space directory (`WHEEL_SPACE_DIR`) is the one it last started with. While the wait runs the state is `exited` and `error` reads `ended by signal 9; restarting in 1 s`; when no restart follows it reads `ended by signal 9; not restarted (restart = never)` or `exited with status 0; not restarted (restart = on-failure)`.

The wait is never skipped by a read. A `feed_subscribe` or `plugin_action` that arrives during it returns at once with the error `backend of <plugin> is restarting in <n> s: <why>` (`<n>` the seconds left, rounded up, at least 1; `<why>` how the process ended, or `<file> changed`). One that arrives after an end the policy does not restart (`never`, or status 0 under `on-failure`) starts a fresh process: `never` means `wheeld` does not restart the backend on its own. `plugin_restart {plugin, space}` and `wheel plugin restart <id> [--space s]` restart at once, also during a wait, and also start a backend that never ran or is `exited`; they reply once the new process answered `initialize`. When `wheeld` stops no restart happens, and a read that arrives then is refused with `wheeld is stopping`.

Every automatic launch (after a wait, or on a watched change) reads the package's `wheel-plugin.toml` again and starts from its `[backend]` as it is then: `command`, `transport`, `restart` and `watch`. When the manifest cannot be read, does not parse or has no `[backend]`, nothing is launched: the reason is written to the log as `not restarted: <reason>` and kept in `error`, a process that runs is left running, and the next change or request tries again. A built-in package has no folder and keeps the spec it started with.

`watch` lists paths relative to the package folder, matched with the same `*` as `reads` (it matches `/` too) against each changed file's path under the folder. Once the backend has started, a matching change restarts it 300 ms after the last matching change, whether the process runs, waits to restart or exited and is not restarted by the policy (shutdown, process group killed after 1 s, then start). A change within 10 s of the previous launch counts as a failure for the backoff: the restart waits `restarting in <n> s: <file> changed` (1 s, 2 s, 4 s, up to 30 s), so a backend that writes its own watched file is slowed to that schedule; a change after the process stood 10 s restarts at once and starts the count over, and a change while a restart already waits restarts at once. The folder is watched recursively, from the first start until `wheeld` stops; built-in packages are never watched. Restarting a process that is still `starting` kills its process group at once, since it was never sent `shutdown`; a process that ended or was restarted sends no more `feed/update` (an update from an instance that is no longer the package's current one is dropped). Each restart writes `restart requested`, `restarting: <file> changed`, `restarting in <n> s: <file> changed` or `restarting in <n> s after: <why>` to the log.

**Start.** One instance runs per package (the package folder, or `builtin:<id>`), whichever spaces read it. It starts on the first `feed_subscribe` to one of its feeds and the request waits for `initialize`. The process gets the environment an MCP server gets, so the `wheel` command reaches this `wheeld` (`WHEEL_ENV`, `WHEEL_URL`, `WHEEL_TOKEN_FILE`, and a `PATH` that starts with Wheel's bin directory, where `wheel` resolves, then `wheeld`'s login PATH), then `WHEEL_PLUGIN_ID`, `WHEEL_SPACE_DIR` (the directory of the space that started it) and, for `unix`, `WHEEL_PLUGIN_SOCKET`: a socket `wheeld` binds at `<support dir>/b/<hash>.sock` (under the temp directory when that path would pass 104 bytes), accepts one connection on within 10 s, and unlinks once connected. Over `unix`, the process's stdin is closed and its stdout goes to the log. Each process is its own process group, which `wheeld` kills when the process ends, when `shutdown` is not answered within 1 s and when `wheeld` stops.

**Log.** `<log dir>/plugins/<plugin id>.log` (the folder of `wheeld.log`), appended to: the process's stderr (and stdout over `unix`) line by line, and each `log` notification as `HH:MM:SS UTC <level> <message>`. `plugin_logs` / `wheel plugin logs <id> [--space s] [--tail n]` reads it.

| Method | Direction | Kind | Params and result |
| --- | --- | --- | --- |
| `initialize` | wheeld to backend | request | `{wheel_version, plugin_id, config: {}}` gives `{feeds, actions}`, each by its own name; 10 s to answer |
| `feed/subscribe` | wheeld to backend | notification | `{feed, params}`: start sending `feed/update` for it; sent when the first pane reads the feed |
| `feed/unsubscribe` | wheeld to backend | notification | `{feed, params}`: stop; sent when the last pane reading it leaves |
| `action/invoke` | wheeld to backend | request | `{action, args, caller: {pane, space, plugin, plugin_pane}}` gives any JSON value, or a JSON-RPC error; `action` is the name the backend listed (`echo`, not `clock.echo`); `caller` is the invoking pane, its space and its own plugin and pane id; 30 s to answer |
| `shutdown` | wheeld to backend | notification | no params: exit now |
| `feed/update` | backend to wheeld | notification | `{feed, params, value}` (the whole value) or `{feed, params, patch}` (RFC 6902); `params` is echoed as subscribed |
| `log` | backend to wheeld | notification | `{level, message}` |
| `wheel/call` | backend to wheeld | request | answered with the JSON-RPC error -32601 until `needs` is enforced |

A malformed `feed/update`, or a patch that does not apply, is written to the log and dropped; it never ends the connection.

**Feed names.** A pane names a feed `<plugin>.<name>` (`clock.now`); the plugin is resolved in the pane's space as a pane's plugin is, and the backend sees `now`. A feed not in the backend's `initialize` reply is refused.

**Reads and actions.** `wheeld` refuses a feed or action the calling pane's `[[pane]]` entry does not list, for `wheel.*` feeds too. `feed_subscribe` finds the pane's plugin and pane id from its kind, resolves the package in the pane's space and refuses the feed unless one `reads` entry matches it: `pane p2 (clock.main) may not read clock.secret; its reads: clock.now, clock.lines, wheel.spaces` (`none` when the list is empty). In `reads` and `actions`, `*` matches any run of characters, none or dots included, and nothing else is special (`clock.*` matches `clock.now`; `*` matches everything). Any pane may read another plugin's feeds when listed. Actions are named `<plugin>.<name>` like feeds: `wheeld` refuses one the pane's `actions` do not match (`pane p2 (clock.main) may not invoke clock.hidden; its actions: clock.echo, clock.fail`), then one the backend did not name in `initialize` (`plugin clock has no action x; its actions: ...`), and there is no `wheel.*` action (`wheeld has no actions; wheel.x does not exist`). A backend's JSON-RPC error, or no answer in 30 s, is returned as the error `<action> failed: <message>`. `wheel.spaces`, `wheel.panes` and `wheel.agents` are `wheeld`'s:

| Feed | Value |
| --- | --- |
| `wheel.spaces` | `[{id, name, cwd, session}]`, every space |
| `wheel.panes` | `[{id, space, kind}]`, every pane, `kind` its wire name (`terminal`, `plugin`, ...) |
| `wheel.agents` | `[{pane, space, kind, agent, status}]`, every pane running an agent: an agent pane (`agent` is its harness) or a terminal running one; `status` is `working`, `idle`, `needs_you`, ... or null |

`wheeld` publishes the three whenever the layout or a status changes, and only when a pane reads them and the value differs from the last one.

**Grants.** A pane's `reads` are checked at `feed_subscribe` and again for every live feed subscription whenever a pane closes or changes kind, and whenever a file under a watched plugin folder changes (the same watch that sends `plugin_source`). Each subscription's task waits on one broadcast `wheeld` fires on those events; a subscription whose pane is gone, no longer a plugin pane, or no longer allowed the feed by the manifest on disk ends, on every connection, and its client is sent `error` with the subscription's `req` and the refusal (`pane ... may not read <feed>; its reads: ...`, or `... is not a plugin pane`).

**Stale feeds.** When a backend's process ends, every feed of that package that a pane reads keeps its last value and is marked stale: each reader is sent `feed_update` with that `value` and `stale: true`. The mark clears with the next `feed/update` the backend sends for the feed, which reaches readers as a `feed_update` with `stale` omitted. A backend must therefore send each feed's value (`feed/update` with `value`) on every `feed/subscribe`, the replayed ones after a restart included: a feed the new process says nothing about stays stale. A `feed_subscribe` reply, and the whole value a lagging reader is resent, carry `stale: true` while the mark is set.

**Hub.** Each feed is keyed by (backend or `wheel`, feed, params; no params and `{}` are one feed). It holds the current value, kept current by applying each patch, and a broadcast of changes, under one mutex, so a reader gets the value and joins the broadcast with no change missed or repeated. When the last reader drops, the feed is forgotten and the backend gets `feed/unsubscribe`; a feed read again starts at null until the backend sends a value. A backend gets one deadline of 10 s to connect its socket and answer `initialize`, which `feed_subscribe` waits on too; when a process ends or fails to start, its whole process group is killed. A backend's log is `<id>.log`, rotated to `<id>.log.1` past 4 MB, and `plugin_logs` reads its tail by seeking from the end (200 lines when `tail` is not given). A `feed_subscribe` runs in its own task, so a backend that is slow to start does not hold the connection's other messages.

**Ops.** `feed_subscribe {pane, feed, params}` (the pane must be a plugin pane) replies `feed_update` with `value` (null until the backend sends one), then sends `feed_update` with `value` or `patch` on the connection per change (`value` and `stale: true` when the backend is down), until `feed_unsubscribe` (replies `ok`), the connection closes or the pane's grant ends; a connection that falls behind is sent the whole current value and continues from that value. A second `feed_subscribe` for the same pane, feed and params replaces the first. `plugin_action {pane, action, args}` runs in its own task, starts the plugin's backend if needed (up to 10 s) and waits up to 30 s for the backend's answer, then replies `plugin_action {pane, action, result}` with the backend's result, or `error`; a `plugin_action` sent with no `req` runs and its result is not sent back (an error still is). The pane's `actions` are checked before the backend starts and again right before `action/invoke` is sent. `plugin_logs {plugin, space, tail}` replies `plugin_logs {plugin, path, text}`. `plugin_restart {plugin, space}` runs in its own task and replies `ok`, or `error` (`plugin <id> has no [backend]`, or why the new process did not start). `PROTOCOL_VERSION` 38.

**Checks.** `wheel plugin feed <pane> <feed> [--params json] [--count n]` subscribes and prints each `feed_update` as one JSON line. `wheel plugin action <pane> <action> [--args json]` invokes an action and prints the result as one JSON line; a refusal or backend error exits non-zero with the message. `wheel plugin restart <id> [--space s]` prints `restarted <id> (pid <n>)`, or exits non-zero with the error. `wheel plugin ls` prints each backend's state, pid and `error` line. `tools/plugin-backend.sh` starts a throwaway `wheeld` and runs the fixture `tools/fixtures/plugins/clock/` (a Python backend with feeds `clock.now` and `clock.lines`, actions `clock.echo`, `clock.fail` and `clock.hidden` (listed by the backend, not the manifest), and a pane script reading the feeds with `wheel.spaces` and calling the actions) over stdio and over a unix socket, with `tools/fixtures/plugins/reader/` (no backend, `reads = ["clock.*"]`) for the glob. The clock fixture's manifest has `watch = ["backend.py"]`; copies with `restart = "never"`, with `watch = ["*"]` and a command that prints a marker line, a backend that runs `exit 0` and one that runs `exit 1` cover the policies, the watch and the manifest re-read. It runs 59 checks, including that a backend's log shows `WHEEL_URL`, `WHEEL_TOKEN_FILE` and `wheel` on its `PATH`, that a read during a crash-looping backend's backoff is refused with `restarting in`, that touching the watched file starts an exited `restart = never` backend, that a restart after a watched edit or a crash runs the manifest's command as it is now, that a manifest that does not parse is not launched, that a pane changing kind and a manifest narrowing its `reads` each end a live feed subscription, that `kill -9` on the clock backend sends its reader a stale value and then fresh ones while `wheeld` and a terminal pane's process keep running, that a change to the watched file and `wheel plugin restart clock` each give a new backend pid, and that `restart = never` and an exit with status 0 under `on-failure` leave the backend down with `not restarted` in `plugin ls`.

## Wheelhouse plugin

`plugins/wheelhouse/` is a plugin package (`wheel-plugin.toml`, id `wheelhouse`) whose backend is the `wheelhouse` binary, `wheelhouse backend`: Wheel's JSON-RPC over stdio, one process for every project (`plugins/wheelhouse/src/backend.rs`). Its panes are `beads` (`panes/beads.luau`), which reads `wheelhouse.beads`, `wheelhouse.beads_graph` (for its Focus and Graph views, drawn on a Canvas) and `wheel.panes` and invokes `wheelhouse.press` and `wheelhouse.run_list`, and `repo` (`panes/repo.luau`), which reads `wheelhouse.repo` and invokes `wheelhouse.branch_delete` and `wheelhouse.run_list`, and `diff` (`panes/diff.luau`), which reads `wheelhouse.diff` and invokes no action, and `prs` (`panes/prs.luau`), which reads `wheelhouse.prs` and `wheelhouse.pr_detail` and invokes the `pr_*` actions and `wheelhouse.run_list`, and `issues` (`panes/issues.luau`), which reads `wheelhouse.issues` and invokes `wheelhouse.issue_create`, `wheelhouse.issue_close` and `wheelhouse.run_list`. The `project` pane reads `wheelhouse.project` and invokes `wheelhouse.vision_save`; the `implement` and `swarm` panes read `wheelhouse.work` and invoke `wheelhouse.agent_start`. There are no MCP Beads, Repo, Diff, PRs, Issues, Project, Implement or Swarm pages (`wheelhouse issues` still lists, creates and closes issues, `wheelhouse prs` still lists the open PRs, `wheelhouse project` prints the project's status, and `wheelhouse agents` lists and starts agents). The `diff` feed's value is a checkout's `Snapshot`: `{name, top, branch, head, default, error, scopes: [{key, label, base, fork, commits, files: [{path, old, status, add, del, binary, truncated, hunks: [{header, lines: [{kind, old, new, text}]}]}]}]}`, `scopes[0]` being the uncommitted work. It is read again after file events (no polling): any change under the checkout's top except `.git` and the paths git ignores (`git ls-files --others --ignored --exclude-standard --directory`, read again on every refresh), and a change to HEAD, the index, its log, `packed-refs`, `info/exclude` or anything under the common git dir's `refs`. The checkout is watched recursively; a path that several subscriptions name is watched once, recursively when any of them needs it so (a `beads` subscription's watch of a project directory is non-recursive, and is upgraded while a `diff` subscription on the same checkout runs and downgraded when it leaves). A subscription's matching paths are in place before its first read, so a change made during that read is an event. The `diff` pane draws at most 200 files in its stacked sections and tables (the view tree stays under the runtime's 100,000 values) and names the rest in a note; its wide layout has a fixed height of 600 pt because the client wraps a plugin pane in a scroll view (wheel-pmf2.10). Reads key on the feed and its whole params, so a project read by two panes is read once per feed, and two subscriptions that differ in any param (`pr_detail` with `number` 3 and 5) are two values, each read, ordered and patched on its own. An action or the 120 s clock names a feed and a project, and reads every subscription of that feed whose `dir` is that project.

**Install.** The manifest's backend command is `../../target/debug/wheelhouse backend`, run by `/bin/sh` in the package folder (the physical folder, so `../..` is the repo), so the package works from a symlink to the repo and from nowhere else:

```sh
cargo build -p wheelhouse
ln -s <repo>/plugins/wheelhouse ~/.wheel/plugins/wheelhouse
wheel plugin restart wheelhouse     # after each rebuild; the manifest has no watch
```

`wheeld` follows a symlink at the package root (`daemon/src/plugins.rs` canonicalizes it for the script and the module scan; modules still may not leave the resolved package). Every project's spaces see it.

**Feed `beads`**, params `{dir}` (the absolute project directory, echoed as subscribed). Each subscribe sends the whole value, later changes send an RFC 6902 patch, and a value equal to the last one sends nothing. The backend reads again, 150 ms after the last event that changes a project it serves (and at most 1 s after the first, however long events keep coming), when `<dir>/.beads/last-touched`, `<dir>/wheel.toml` or `~/.wheel/bead-panes.json` changes (a file watcher, no polling), and right after a press claims or releases a bead. The value, from a fixture project:

```json
{"now": 1763110000, "name": "proj-a", "path": "/private/tmp/claude/wheel/wh/proj-a", "has_repo": true, "error": null, "problems": [],
 "buttons": {"task": {"start": {"label": "Implement", "run": "..."}, "resume": {"label": "Continue", "run": "..."}, "focus": {"label": "Focus", "run": null}},
             "epic": {"start": {"label": "Plan", "run": "..."}, "resume": {"label": null, "run": null}, "focus": {"label": null, "run": null}},
             "closed": null},
 "scripts": [{"name": "mark", "label": "Mark list"}],
 "records": {"a-gyh.2": {"pane": "p2", "conversation": ""}},
 "pressing": [],
 "beads": [{"id": "a-lm3", "title": "Leaf to press", "status": "open", "kind": "task", "priority": 2, "parent": null, "labels": [],
            "owner": "someone@example.com", "updated": "2026-11-14T08:43:33Z", "blocked_by": [], "description": "", "ready": true, "epic": false}]}
```

`now` is the epoch second (an integer) the snapshot was read, for a pane that has no clock; a snapshot that differs only in `now` sends nothing, and a patch for any other change carries the new `now`. `buttons` is `[beads.task]`, `[beads.epic]` and `[beads.disabled] closed` of the project's `wheel.toml` (a group is null when the project wrote none); `scripts` are its `[beads.list.<name>]` buttons with their label; `records` is the pane and conversation each press recorded; `pressing` lists the beads a press holds now. With no `.beads` folder `has_repo` is false and `beads` is empty; a failing `bd` is in `error`. The backend sends data only: which button a bead shows, from `buttons`, `records` and `wheel.panes`, is the pane's decision.

**Feed `beads_graph`**, params `{dir, view, sel, depth, width, more, list}` (the Beads pane's Focus and Graph views): `view` is `"graph"` or anything else for Focus; `sel` is the selected bead (`""` for none); `depth` the hops Focus shows on each side, 1 to 3, and `width` the canvas width in points, 300 to 4000, both JSON numbers (`2` or `2.0`); `more` the key of an open `+ N more` node (`""` for none); `list` is `{q, f?, not}`, the pane's search text, its filters (`f` maps a filter to the values ticked, and is left out when none is, since an empty table travels as an array) and the filters read as "is not", which dim the beads they drop. Every distinct params is its own subscription, read on the same events as `beads` (a file watcher on `<dir>/.beads/last-touched`, `<dir>/wheel.toml` and `~/.wheel/bead-panes.json`, 150 ms after the last event) and right after a press, so a bead made with `bd` while a pane draws arrives as a patch. A params that was not subscribed before reads null until the backend answers, so a pane draws the last value it had meanwhile. The value:

```json
{"view": "focus", "n": 31, "center": "fx-t4f",
 "caption": "x: dependency direction, blocked by on the left and unlocks on the right, by hops. y: grouping, parent above, children below. 15 blockers, 0 unlocks.", "captionBad": false, "cyc": false, "empty": null,
 "scene": {"ops": [...], "hits": [{"id": "g-9ab", "rect": [24, 80, 214, 28], "label": "g-9ab, ready, Blocker 1",
           "tip": ["g-9ab  ready\nBlocker 1"], "action": {"type": "focus_on"}, "doubleAction": {"type": "open_bead"}}, ...],
           "bounds": [0, 0, 900, 612], "controls": false, "wheel": "pan", "focus": {...}},
 "more": null}
```

`scene` is the Canvas's props (ADR-0029) without the fields that are the pane's to choose (`escapeAction`, `fitKey`, `sizeAction`, `sizeKnown`, `height`, `modifiers`); it is null when `empty` is a string (`No beads yet`, or what Focus says when no bead is picked). Every action a hit, `emptyAction` or `enterAction` names is a table (`{"type": "focus_on"}`, `open_bead`, `graph_more`, `graph_select`) that the pane's `update` receives with the hit's `id` and `context` added, never a bare name. In Focus `center` is the bead drawn in the middle, and the pane pushes it on its history before a `focus_on`; in Graph it is null, `scene.controls` is true and the zoom layers (lane bars, one dot per bead, one box per bead) show at the zooms the ops and hits name. `more` is null, or when `params.more` names a `+ N more` node of the drawn Focus, `{title, rows: [{id, title, word, tone}]}` for the sheet behind it. `caption` is the one line above the drawing and `captionBad` and `cyc` flag a dependency cycle. A 1,500-bead project's Graph value is about 73,000 values by the runtime's count, so the pane stays under the 100,000 limit because it sends no table in Graph and Focus.

**Feed `repo`**, params `{dir}`: the project's local branches (all but the default), stashes and the last 15 commits on the default branch, from `repo::collect`. It is read again when `HEAD`, `logs/HEAD`, `packed-refs` or `refs/stash` of the checkout's git directories change, or anything under `refs/heads`, `refs/remotes/origin` or `worktrees` (watched recursively), and every 120 s for every subscription, since open PRs and their checks come from `gh` and have no event. A tree that does not exist yet (`refs/remotes/origin` before the first fetch, `worktrees` before the first linked worktree) is watched through its nearest existing ancestor, whose creation of the tree reads the feed again. Reads are ordered per (feed, project): a slow `gh` in one project delays no other feed or project, two reads of the same feed and project never swap order, and a press re-reads only `beads`, a `branch_delete` only `repo`. The value, from a fixture project:

```json
{"now": 1794647860, "name": "proj-r", "top": "/private/tmp/claude/wheel/wh/proj-r", "default": "origin/main", "head": "main",
 "branches": [{"branch": "merged-b", "ts": 1794647859, "subject": "first commit", "author": "t", "ahead": 0, "behind": 0, "upstream": null,
               "upstream_gone": false, "remote": null, "unpushed": null, "worktree": null, "pr": null, "pr_merged": false, "local_only": 0,
               "dead": "merged", "blocked": null}],
 "stashes": [{"ref": "stash@{0}", "subject": "WIP on main: 9d68eb1 first commit", "when": "0 seconds ago"}],
 "commits": [{"sha": "9d68eb1", "subject": "first commit", "author": "t", "ts": 1794647859}],
 "pr_error": null, "error": null, "scripts": [{"name": "mark", "label": "Mark branches"}]}
```

A branch's `worktree` is `{path, current, primary, status: {staged, unstaged, untracked, conflicted}, op}` and its `pr` is `{number, title, draft, url, review, mergeable, ci}` (`ci` is `pass`, `fail`, `pending` or null). `dead` names why the branch is safe to delete (`upstream deleted`, `PR merged`, `merged`, `on origin`) and `blocked` why it still cannot be (its worktree is the primary or the pane's own checkout, holds a paused operation or has uncommitted changes). A directory outside a git checkout has `error` set and nothing else filled; a failing `gh` puts the last line it printed in `pr_error`. `scripts` are the project's `[repo.list.<name>]` buttons.

**Feed `prs`**, params `{dir}`: the project's open pull requests with their review standing, from `prs::collect` (one `gh api graphql` read, shared by every pane and project of the same repo for 30 s). It is read again when `HEAD`, `logs/HEAD`, `packed-refs` or `refs/stash` change, or anything under `refs/heads`, `refs/remotes/origin` or `worktrees` (the `repo` feed's watches, since a fetch or push moves `refs/remotes/origin`), and every 120 s for every subscription. `fetched` changes only when `gh` is really asked, so a read answered from the 30 s share sends nothing. The value, from a canned snapshot:

```json
{"now": 1794647860, "name": "proj", "repo": "o/proj", "me": "me", "error": null,
 "fetched": 1794647800, "fetched_clock": "09:16", "next_clock": "09:18",
 "prs": [{"number": 7, "title": "Fix the thing", "author": "you", "branch": "fix-thing", "draft": false, "mergeable": "MERGEABLE",
          "checks": "passing", "review": "in-review", "bucket": "needs-me", "my": "not-reviewed", "standing": "review", "updated": 1794640000,
          "url": "https://github.com/o/proj/pull/7", "fix": false, "additions": 3, "deletions": 1, "files": 1, "commits": 1,
          "labels": ["bug"], "reviewers": ["me"], "summary": "The start of the description"}],
 "buttons": [{"name": "fix", "label": "Make mergeable", "on": ["fix"], "worktree": true}],
 "scripts": [{"name": "mark", "label": "Mark PRs"}]}
```

`fetched_clock` and `next_clock` are the local `HH:MM` of `fetched` and of `fetched` plus 120 s (empty strings while `fetched` is 0). `bucket` is `mine`, `needs-me`, `waiting` or `other`; `standing` is one of `review`, `re-review`, `reply`, `changes-requested`, `comments`, `no-reviewer`, `waiting-reviewer`, `waiting-author`, `not-asked`, `approved`. `buttons` are the project's `[prs.buttons.<name>]` (`on` is a list of `any`, a bucket, `fix`, `draft` or `ready`; `label` falls back to the name) and `scripts` its `[prs.list.<name>]` buttons. A directory with no GitHub repository, or a failed `gh`, sets `error` and leaves `prs` empty.

**Feed `issues`**, params `{dir}`: the project's GitHub issues, open and closed (newest 500), from `issues::collect` (`gh issue list`). It watches no file: it is read on subscribe, every 120 s for every subscription, and after each `issue_create`, `issue_close` and `run_list` with view `issues`. The value:

```json
{"now": 1793491200, "name": "wheel", "error": null, "problems": [],
 "issues": [{"number": 12, "title": "Fix the thing", "state": "open", "labels": ["bug"], "assignees": ["you"], "author": "me",
             "body": "first 3000 characters", "url": "https://github.com/o/r/issues/12", "updated": 1793490000}],
 "scripts": [{"name": "triage", "label": "Triage"}]}
```

`state` is `open` or `closed`; `updated` is Unix seconds. `scripts` are the project's `[issues.list.<name>]` buttons; `problems` lists what is wrong with the `wheel.toml`. A directory with no GitHub repository, or a failed `gh`, sets `error` (`<dir>: gh issue list failed: ...`) and leaves `issues` empty.

**Feed `pr_detail`**, params `{dir, number}` (`number` a JSON integer): one pull request in full, digested to data from `gh api graphql`, shared by every pane for 30 s. It is read on the same events and clock as `prs`; every `pr_detail` subscription of the project is read again, each as its own value. Each open `pr_detail` subscription asks `gh` once per 120 s tick and once after each `pr_*` action (one GraphQL query for one PR, with its last 30 commits and first 60 files). The repo-hosted images in the description and comments are downloaded with `gh` into a cache folder before the value is made, so an image block's `url` is the cached file's URL. The value:

```json
{"now": 1794647860, "number": 7, "error": null, "state": "OPEN", "draft": false, "mergeable": "MERGEABLE", "merge_state": "CLEAN",
 "base": "main", "cross": false, "created": 1794600000, "updated": 1794640000, "url": "https://github.com/o/proj/pull/7",
 "assignees": ["me"], "reviewers": ["you (requested)", "zed (approved)"], "labels": ["bug"],
 "body": [{"kind": "text", "text": "Fixes it", "heading": 0, "quote": false, "indent": 0}, {"kind": "image", "url": "https://x/a.png", "alt": "shot", "indent": 0}],
 "checks": [{"name": "ci", "state": "SUCCESS"}],
 "talk": [{"at": 1794602000, "who": "zed", "what": "review: approved", "blocks": [{"kind": "text", "text": "ok", "heading": 0, "quote": false, "indent": 0}]}],
 "commits_total": 31, "commits": [{"sha": "bbb1234", "headline": "second", "at": 1794601000}],
 "files_total": 61, "files": [{"path": "a.rs", "additions": 3, "deletions": 1}],
 "agents": ["claude", "codex"]}
```

`state` is `OPEN`, `CLOSED` or `MERGED`. `checks[].state` is a check run's conclusion (its status while it has none) or a status context's state. `talk` is the reviews that say something (an empty `COMMENTED` review is left out) and the comments, oldest first; `commits` are the last 30, newest first, with `commits_total` the whole count; `files` the first 60 with `files_total`. A block is `{kind: "text", text, heading, quote, indent}` (`text` is inline Markdown including its list bullet or task marker; `heading` 0 for none, 1 to 3 for H1 to H3, 4 for deeper; `indent` in points), `{kind: "image", url, alt, indent}`, `{kind: "code", text, indent}` or `{kind: "rule"}`. A PR that does not exist, or a failed `gh`, sets `error` (`no PR #9`, or the `gh` error) and leaves the other fields empty. `agents` are the agents `wheel status` finds, for a pane's agent choice.

**Feed `project`**, params `{dir}`: the project's status against its vision, from `status::collect`. It is read again when `<dir>/.beads/last-touched`, `<dir>/wheel.toml` or `<dir>/docs/CONTEXT.md` changes: the backend watches the project directory, its `.beads` and its `docs` directory, none recursively, and a `docs` directory created later is an event in the project directory, after which the next read watches it. `CONTEXT.md` and `docs` events matter to `project` subscriptions only. There is no clock. The value:

```json
{"name": "proj", "has_tracker": true, "vision": "Why this exists.", "error": null,
 "counts": {"done": 12, "progress": 1, "ready": 3, "blocked": 2, "open": 4, "deferred": 0},
 "epics": [{"id": "proj-a", "title": "An epic", "done": 3, "all": 7, "ready": 2, "closed": false}]}
```

`counts` hold the tracker's items by state (epics count in none); `epics` list open epics first, each with its children's progress and how many are ready. With no `.beads` folder `has_tracker` is false and `counts` are zero and `epics` empty; `vision` is the first paragraph of `docs/CONTEXT.md` after its title, or null; a failing `bd` is in `error`.

**Feed `work`**, params `{dir}`: the agent panes of the project (those whose space is the project's directory or one of its worktrees under `~/.worktrees/<project name>/`), one row per item an agent declared with `work_set` (a pane that declared nothing is one row with a null `item`). It reads no file. The backend follows `wheel agent watch` (the `wheel` command on its PATH, with `WHEEL_URL` and `WHEEL_TOKEN_FILE` in its environment) from the first `work` subscription until the last one leaves, and every change of the list reads each subscribed project again. The value:

```json
{"name": "proj", "loaded": true, "error": null,
 "agents": [{"pane": "p3", "space": "proj", "agent": "w1", "model": "sonnet", "status": "working", "item": "bead proj-a.1", "stage": "edit", "note": "adding the feed"}]}
```

`loaded` is false until `wheel agent watch` has delivered a list (the agents are then empty because none is known, not because none runs), or has failed to start: `loaded` is then true, `agents` is empty and `error` holds the spawn error until a list arrives (null otherwise). `status` is `idle`, `working`, `needs_you`, `stopped`, `done` (a turn ended since a client last showed the pane) or `unknown` (the daemon reported none); `stage` is `plan`, `edit`, `build`, `verify`, `review`, `gate` or null.

**Actions**, each returning `{"said": "<words for the pane>"}` or an error that reaches the caller as `wheelhouse.<action> failed: <message>`:

- `press {dir, bead}`: decides the bead's act again from the records and the panes open now (`wheel pane ls`). A recorded pane that is open is focused (`wheel pane focus`); otherwise the `[beads.task]` or `[beads.epic]` `start` or `resume` command runs in `dir` with `BEAD_ID`, `BEAD_TITLE`, `BEAD_TYPE`, `BEAD_REPO`, `BEAD_PATH` (and `CONVERSATION` for resume) and `WHEEL_PANE` set to the pressing pane, and the last line it prints is recorded as the bead's pane. Errors: `no bead <id>`, a disabled button's own help (`Starting; wait for the pane`, `[beads.task] has no start.run`, `This bead is closed`), `<id> has no button to press`, `<id> is already starting`, the command's own failure.
- `run_list {dir, script, ids, view}`: with `view` `prs`, runs `[prs.list.<script>]` with `PR_NUMBERS` (the PR numbers space separated), `PR_REPO` and `PR_PATH`; with `view` `issues`, runs `[issues.list.<script>]` with `ISSUE_NUMBERS` (the issue numbers space separated), `ISSUE_REPO` (the project's name) and `ISSUE_PATH`; with `view` absent or `beads`, runs `[beads.list.<script>]` in `dir` with `BEAD_IDS` (the ids space separated), `BEAD_REPO` and `BEAD_PATH`; with `view` `repo`, runs `[repo.list.<script>]` with `BRANCHES` and `REPO_PATH` (the checkout's top). The pane sends the ids it lists. Errors: `no list button <script>`, `the list is empty`, `no view <view>` (views: `beads`, `repo`, `prs`, `issues`).
- `issue_create {dir, title}`: `gh issue create` with an empty body; says `Opened #12`. A blank title fails with `an issue needs a title` (the client adds the `issue_create failed: ` prefix, as it does for the `pr_*` actions). Reads the `issues` feed again whether or not it went through.
- `issue_close {dir, number}`: `gh issue close`; says `Closed #12`. Reads the `issues` feed again whether or not it went through.
- `branch_delete {dir, branch}`: deletes a local branch, its worktree first through `admin worktree rm -y` when it has one, only while the branch is dead and nothing blocks it; returns what it did (`removed worktree <path>; deleted branch <name>`) and reads the project again either way. Errors arrive as `<branch>: <why>` (`not deletable: N commit(s) live on no origin ref`, `blocked: <why>`, `no such local branch`).
- `vision_save {dir, text}`: writes `text` as the vision paragraph of `<dir>/docs/CONTEXT.md` (the rest of the file is kept; the file is created with a title when it is missing); says `Vision saved`, reads the `project` feed again whether or not the write worked. Errors: `vision_save needs args.text`, `the vision cannot be empty`.
- `agent_start {dir, brief}`: splits the calling pane, starts an agent in the new pane (the `agent.default` setting) and sends it `brief`; says `Started the agent in pane <id>`. Errors: `agent_start needs args.brief`, `starting an agent needs the asking pane` (the call names no pane), the failing `wheel` command's own message (the new pane is closed again).
- `pr_merge {dir, number, method}` (`method` `squash`, `merge` or `rebase`, default `squash`): `gh pr merge`, the branch kept; says `Merged #7 (squash).`
- `pr_close {dir, number}`: `gh pr close`; says `Closed #7.`
- `pr_draft {dir, number, draft}`: `draft` true puts the PR into draft (`#7 is a draft.`), false takes it out (`#7 is ready for review.`).
- `pr_checkout {dir, number}`: `gh pr checkout` in the project's checkout, then a terminal pane beside the calling pane; says `Checked out <branch> here; terminal <pane> opened.`
- `pr_worktree {dir, number, agent, open}` (`agent` `default` or an agent's name, `open` `pane` or `space`): a worktree on the PR's branch (the one that holds it, else `~/.worktrees/<project>/<branch>`), then an agent in it, in a pane beside the caller or in the space on the worktree; says `<branch> is in <worktree>; agent pane <pane> started.`
- `pr_button {dir, number, name}`: runs the project's `[prs.buttons.<name>]` command with `PR_NUMBER`, `PR_TITLE`, `PR_BRANCH`, `PR_URL`, `PR_REPO`, `PR_PATH` (and `PR_WORKTREE` when the button asks for one); says `<label> started for #7. See the new pane.`
- `pr_refresh {dir}`: drops what was read of the project's repo, so the next read asks `gh`; says an empty string.

Every `pr_*` action finds the PR in the project's open PRs (`no open PR #N` when it is not there), and, whether it went through or failed, drops the repo's cached PR list and that PR's cached detail and reads every `prs` and `pr_detail` subscription of the project again. The action's errors arrive as the `gh` or `git` message.

**`dir` is any absolute directory.** Every feed and action takes whatever directory the pane names: a pane granted `wheelhouse.beads` reads, and one granted `wheelhouse.press` or `wheelhouse.run_list` runs the buttons of, any project on the machine, not only its own space's. The backend is trusted local code and a grant names a feed or an action, never a directory.

`WHEEL_DATA_DIR`, which the MCP page hands a button from the asking pane's data directory, is not set: no `[beads.*]` or `[repo.list.*]` command in `~/.wheel/wheel.toml` or in any project's `wheel.toml` under `~/Projects` reads it (only artifact refresh commands do; no `[repo.list]` table exists in any of them), and a plugin pane has no data directory.

`tools/wheelhouse-plugin.sh` checks the package against a throwaway `wheeld` (32 checks): the symlinked package, the feed's shape, a `bd update` arriving as a patch, no message for an unchanged value, a second project directory, press for a fresh, resumed, focused and epic bead, `run_list` with `BEAD_IDS`, the errors, a `wheel.toml` edit and a record file write reaching the feed, and `kill -9` of the backend giving a stale value and then a fresh one; and for `repo`: the feed's shape, a new branch arriving as a patch, `branch_delete` of a merged branch and its removal from the feed, the refusal for a branch with local-only commits (its branch named once in the error), `run_list` with `BRANCHES` and `REPO_PATH`, an unknown view, a checkout with no origin whose `git remote add origin` and first fetch reach the feed within 3 s, and closing a repo feed on a directory leaving the beads feed on it updating.
