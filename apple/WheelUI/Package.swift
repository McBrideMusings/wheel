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
        .package(path: "../WheelLuau"),
        .package(url: "https://github.com/gonzalezreal/textual.git", exact: "0.5.0"),
        // Dictation's speech-to-text: Parakeet TDT 0.6b v3 on Core ML (docs/voice-mode/stt-engines.md).
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.18.4"),
    ],
    targets: [
        .target(
            name: "WheelUI",
            dependencies: [
                "WheelKit",
                "WheelLuau",
                .product(name: "Textual", package: "textual"),
                .product(name: "FluidAudio", package: "FluidAudio"),
            ]
        ),
        .testTarget(name: "WheelUITests", dependencies: ["WheelUI"]),
    ]
)
