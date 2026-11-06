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
    ],
    targets: [
        .target(
            name: "WheelUI",
            dependencies: [
                "WheelKit",
                .product(name: "Textual", package: "textual"),
            ]
        ),
        .testTarget(name: "WheelUITests", dependencies: ["WheelUI"]),
    ]
)
