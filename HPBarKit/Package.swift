// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "HPBarKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "HPBarKit", targets: ["HPBarKit"]),
    ],
    targets: [
        .target(
            name: "HPBarKit",
            resources: [.process("Resources")]
        ),
        .testTarget(
            name: "HPBarKitTests",
            dependencies: ["HPBarKit"]
        ),
    ]
)
