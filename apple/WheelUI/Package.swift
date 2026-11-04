// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "WheelUI",
    platforms: [.macOS(.v15), .iOS(.v18)],
    products: [
        .library(name: "WheelUI", targets: ["WheelUI"]),
    ],
    dependencies: [
        .package(path: "../WheelKit"),
        .package(url: "https://github.com/gonzalezreal/textual.git", exact: "0.5.0"),
        // The community A2UI renderer, pinned to a commit (MIT, spec v0.9.1) and used only behind
        // A2UIPaneView, so Google's own Swift package can replace it.
        .package(url: "https://github.com/BBC6BAE9/a2ui-swift.git", revision: "16476ba2cb3fcb4c4bcb141dfa4c2804adbdedb0"),
    ],
    targets: [
        .target(
            name: "WheelUI",
            dependencies: [
                "WheelKit",
                .product(name: "Textual", package: "textual"),
                .product(name: "A2UISwiftCore", package: "a2ui-swift"),
                .product(name: "A2UISwiftUI", package: "a2ui-swift"),
            ]
        ),
        .testTarget(name: "WheelUITests", dependencies: ["WheelUI"]),
    ]
)
