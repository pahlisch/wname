// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "wname",
    platforms: [.macOS(.v14)],
    targets: [
        .target(name: "WNameCore"),
        .executableTarget(name: "wname", dependencies: ["WNameCore"]),
        .executableTarget(name: "check", dependencies: ["WNameCore"]),
        .executableTarget(name: "live"),
    ]
)
