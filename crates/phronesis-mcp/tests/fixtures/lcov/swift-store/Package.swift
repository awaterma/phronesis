// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "store-kit",
    targets: [
        .target(name: "Store"),
        .testTarget(name: "StoreTests", dependencies: ["Store"]),
    ]
)