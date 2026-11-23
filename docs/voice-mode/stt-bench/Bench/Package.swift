// swift-tools-version: 6.0
import Foundation
import PackageDescription

let xcf = "../whisper.cpp/build-apple/whisper.xcframework"
let hasWhisper = FileManager.default.fileExists(atPath: (#filePath as NSString).deletingLastPathComponent + "/" + xcf)

let package = Package(
    name: "Bench",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [.library(name: "BenchCore", targets: ["BenchCore"])],
    dependencies: [
        .package(url: "https://github.com/argmaxinc/argmax-oss-swift.git", exact: "1.1.1"),
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.18.4"),
    ],
    targets: [
        .target(name: "BenchCore", dependencies: [
            .product(name: "WhisperKit", package: "argmax-oss-swift"),
            .product(name: "FluidAudio", package: "FluidAudio"),
        ] + (hasWhisper ? ["whisper"] : [])),
        .executableTarget(name: "bench", dependencies: ["BenchCore"]),
    ] + (hasWhisper ? [.binaryTarget(name: "whisper", path: xcf)] : [])
)
