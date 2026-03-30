// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SyncStewardMac",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(
            name: "syncsteward-macos",
            targets: ["SyncStewardMac"]
        ),
    ],
    targets: [
        .executableTarget(
            name: "SyncStewardMac",
            path: "Sources/SyncStewardMac"
        ),
        .testTarget(
            name: "SyncStewardMacTests",
            dependencies: ["SyncStewardMac"],
            path: "Tests/SyncStewardMacTests"
        ),
    ]
)
