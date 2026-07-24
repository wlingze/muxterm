// swift-tools-version: 5.9
import Foundation
import PackageDescription

let packageRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let libSearchPath = packageRoot + "/Vendor"

let package = Package(
    name: "MuxtermApp",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .executable(name: "MuxtermApp", targets: ["MuxtermApp"]),
    ],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm.git", from: "1.15.0"),
    ],
    targets: [
        // C ABI 头文件模块（对应 CoreBridge/muxterm.h）
        .target(
            name: "CMuxterm",
            path: "CoreBridge",
            exclude: ["CoreBridge.swift", "muxterm.h"],
            publicHeadersPath: "include"
        ),
        .executableTarget(
            name: "MuxtermApp",
            dependencies: [
                "CMuxterm",
                .product(name: "SwiftTerm", package: "SwiftTerm"),
            ],
            path: ".",
            exclude: [
                "Package.swift",
                "Info.plist",
                "Vendor",
                "CoreBridge/include",
                "CoreBridge/shim.c",
                "CoreBridge/muxterm.h",
            ],
            sources: [
                "main.swift",
                "CoreBridge/CoreBridge.swift",
                "App/AppDelegate.swift",
                "App/MainWindow.swift",
                "App/ContentView.swift",
                "Terminal/TerminalView.swift",
                "Terminal/TerminalManager.swift",
                "UI/TabBar.swift",
                "UI/PaneLayout.swift",
                "UI/StatusBar.swift",
            ],
            linkerSettings: [
                .unsafeFlags([
                    "-L\(libSearchPath)",
                    "-lmuxterm",
                    "-liconv",
                    "-lresolv",
                ]),
            ]
        ),
    ]
)
