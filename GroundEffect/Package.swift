// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "GroundEffect",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "GroundEffect", targets: ["GroundEffect"])
    ],
    dependencies: [
        // KeychainAccess for easier Keychain operations
        .package(url: "https://github.com/kishikawakatsumi/KeychainAccess.git", from: "4.2.2"),
    ],
    targets: [
        .executableTarget(
            name: "GroundEffect",
            dependencies: [
                "KeychainAccess"
            ],
            path: "Sources"
        )
    ]
)
