// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "VectorComputerHelper",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "vector-computer-helper", targets: ["VectorComputerHelper"])],
    targets: [.executableTarget(name: "VectorComputerHelper")]
)

