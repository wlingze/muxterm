// swift-tools-version: 5.9
import Foundation
import PackageDescription

// 包根：src/platform/macos/（与 linux/、tui/ 同级）
let packageRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
// Rust staticlib：仓库旁共享 target（.cargo/config.toml → ../muxterm-target）
// 从 src/platform/macos 上溯四级到 self/，再进 muxterm-target/release
let libSearchPath = packageRoot
    .appendingPathComponent("../../../../muxterm-target/release")
    .standardizedFileURL
    .path

let package = Package(
    name: "MuxtermApp",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .library(name: "MuxtermChrome", targets: ["MuxtermChrome"]),
        .executable(name: "MuxtermApp", targets: ["MuxtermApp"]),
    ],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm.git", from: "1.15.0"),
    ],
    targets: [
        // 纯 chrome / 快捷键逻辑（无 AppKit、不链 libmuxterm）
        .target(
            name: "MuxtermChrome",
            path: "Chrome"
        ),
        .testTarget(
            name: "MuxtermChromeTests",
            dependencies: ["MuxtermChrome"],
            path: "ChromeTests"
        ),
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
                "MuxtermChrome",
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
                "MuxtermAppUITests",
                "Chrome",
                "ChromeTests",
                "project.yml",
                ".build",
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
