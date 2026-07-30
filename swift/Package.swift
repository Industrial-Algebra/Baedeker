// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Baedeker",
    platforms: [.macOS(.v13), .iOS(.v16)],
    products: [
        .library(name: "Baedeker", targets: ["Baedeker"]),
    ],
    targets: [
        // The Rust staticlib + C header, packaged per-Apple-platform by
        // build-xcframework.sh (gitignored — run the script before opening
        // the package in Xcode).
        .binaryTarget(name: "CBaedeker", path: "Vendor/CBaedeker.xcframework"),
        .target(
            name: "Baedeker",
            dependencies: ["CBaedeker"]
        ),
        .testTarget(
            name: "BaedekerTests",
            dependencies: ["Baedeker"]
        ),
    ]
)
