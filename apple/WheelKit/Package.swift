// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "WheelKit",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "WheelKit", targets: ["WheelKit"]),
    ],
    targets: [
        .target(name: "WheelKit"),
        .testTarget(name: "WheelKitTests", dependencies: ["WheelKit"]),
    ]
)
