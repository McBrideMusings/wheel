// swift-tools-version: 6.2
// The Luau runtime a plugin pane's script runs in (ADR-0041, docs/plugin-runtime.md).
import PackageDescription

let package = Package(
    name: "WheelLuau",
    platforms: [.macOS(.v15), .iOS(.v18)],
    products: [
        .library(name: "WheelLuau", targets: ["WheelLuau"]),
    ],
    dependencies: [
        .package(path: "../WheelKit"),
    ],
    targets: [
        // Luau 0.747's compiler and interpreter, vendored under luau/ and compiled as C++. Only
        // VM/include (lua.h, luaconf.h, lualib.h: plain C) is public, so Swift's importer never
        // meets a C++ header.
        .target(
            name: "LuauCore",
            path: "luau",
            exclude: ["LICENSE.txt"],
            sources: ["Common/src", "Ast/src", "Bytecode/src", "Compiler/src", "VM/src"],
            publicHeadersPath: "VM/include",
            cxxSettings: [
                .headerSearchPath("Common/include"), .headerSearchPath("Ast/include"),
                .headerSearchPath("Bytecode/include"), .headerSearchPath("Compiler/include"),
                .headerSearchPath("VM/src"),
                .define("LUA_API", to: "extern \"C\""), .define("LUACODE_API", to: "extern \"C\""),
            ]
        ),
        // The C surface Swift imports (lua.h, lualib.h, luacode.h) and the C++ pieces that may
        // raise a Luau error: the memory-limited allocator, the step budget and the host call
        // trampoline. A Luau error is a C++ exception, so it must never unwind a Swift frame.
        .target(
            name: "CLuau",
            dependencies: ["LuauCore"],
            path: "Sources/CLuau",
            cxxSettings: [.define("LUA_API", to: "extern \"C\""), .define("LUACODE_API", to: "extern \"C\"")]
        ),
        .target(name: "WheelLuau", dependencies: ["CLuau", "WheelKit"]),
        .testTarget(name: "WheelLuauTests", dependencies: ["WheelLuau", "WheelKit"]),
    ],
    cxxLanguageStandard: .cxx17
)
