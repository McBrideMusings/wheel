// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "WheelKit",
    platforms: [.macOS(.v15), .iOS(.v18)],
    products: [
        .library(name: "WheelKit", targets: ["WheelKit"]),
    ],
    targets: [
        .target(name: "WheelKit"),
        .testTarget(name: "WheelKitTests", dependencies: ["WheelKit"]),
    ]
)
