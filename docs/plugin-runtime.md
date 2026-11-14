# Plugin system: the backend protocol crate and how Luau is built

The plugin system (epic `wheel-pmf2`) needs two pieces of third-party code: a JSON-RPC 2.0 peer for the protocol between `wheeld` and a plugin's backend, and the Luau VM inside both Apple clients. This page names each choice, the candidates checked, and the throwaway build that showed it works.

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

**Packaging:** Luau's `Common`, `Ast`, `Bytecode`, `Compiler` and `VM` directories (127 files, 2.6 MB) and its `LICENSE.txt` are copied into a local Swift package beside WheelUI in `apple/`, under a `luau/` directory at the package root, so the package holds `luau/Common`, `luau/Ast`, `luau/Bytecode`, `luau/Compiler`, `luau/VM` and `Sources/CLuau`. `CodeGen` (the JIT, which iOS forbids), `Analysis`, `Config`, `Require`, `Inliner` and `CLI` are left out. The package has two targets:

- `LuauCore`: every `.cpp` of those five directories, compiled as C++17. Its only public header directory is `VM/include` (`lua.h`, `luaconf.h`, `lualib.h`, all plain C); the C++ include directories are `headerSearchPath`s, so Swift's importer never meets a C++ header. It defines `LUA_API` and `LUACODE_API` as `extern "C"` for the C++ build only.
- `CLuau`: one header, `Sources/CLuau/include/CLuau.h`, including `lua.h`, `lualib.h` and `../../../luau/Compiler/include/luacode.h` (a path relative to the header, since `luacode.h` belongs to no public header directory). Swift code imports `CLuau` and calls `luau_compile`, `luau_load`, `lua_pcall`, `luaL_sandbox` and the rest of the C API directly.

Candidates checked:

| Candidate | Why not |
| --- | --- |
| [`RadiusDay/SwiftLuau`](https://github.com/RadiusDay/SwiftLuau) (the only Luau package on the Swift Package Index) | Pins Luau 0.701 (tag `0.3.2+0.701`), last updated 9 Dec 2025, its README says it is in early stages, two stars |
| Luau as a git submodule | The repo has no submodules; the five directories would still need a `Package.swift` of ours, and a clone would carry the 14 directories the apps never build |
| Luau's CMake build into an XCFramework | A second build system beside SwiftPM and Xcode, and a binary checked in or downloaded at build time |

Throwaway check (Swift 6.4.2, Xcode with the macOS 27.2 SDK): the package above, with an executable that compiles and runs Luau source in a state after `luaL_openlibs`, `luaL_sandbox` and `luaL_sandboxthread`:

- macOS, `swift build`: built in 11.9 s and printed `sum: 3`, `io: nil`, `os.execute: nil`, `version: Luau`.
- iOS Simulator, `xcodebuild -destination 'generic/platform=iOS Simulator'`: `** BUILD SUCCEEDED **` for the library and for the executable, a universal `arm64` and `x86_64` Mach-O whose build version reads `platform IOSSIMULATOR`, `minos 18.0`, with `luau_compile`, `luau_load`, `lua_pcall` and `luaL_sandbox` defined in it.

The check created the state with `luaL_newstate` and set no interrupt callback, and built no iOS device slice: the custom allocator (`lua_newstate`), the instruction budget (`lua_callbacks(L)->interrupt`) and the `arm64-iphoneos` build are the runtime's to prove (`wheel-pmf2.2.1`).
