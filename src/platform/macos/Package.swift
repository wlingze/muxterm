// swift-tools-version: 5.9
import Foundation
import PackageDescription

// 包根：src/platform/macos/（与 linux/、tui/ 同级）
let packageRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
// Rust staticlib：通过 scripts/build-macos.sh 生成的 Vendor/libmuxterm.a 软链引用，
// 指向实际的 cargo target 目录（仓库本地 ./target/<profile>），不依赖共享 ../muxterm-target。
let libSearchPath = packageRoot
    .appendingPathComponent("Vendor")
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
            dependencies: [
                "MuxtermChrome",
                .product(name: "SwiftTerm", package: "SwiftTerm"),
            ],
            path: "ChromeTests"
            // XCTest only（Swift Testing 在此包无 @Test，勿依赖其 0-test runner 输出）。
            // 跑：swift test --disable-swift-testing --filter FlatChromeTests
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
                "App/I18n.swift",
                "App/MainWindow.swift",
                "App/ContentView.swift",
                "App/CommandPalette.swift",
                "App/ConnectionDiscovery.swift",
                "App/QuickConnectController.swift",
                "App/TargetConfigWindow.swift",
                "Terminal/TerminalView.swift",
                "Terminal/TerminalManager.swift",
                "UI/TabBar.swift",
                "UI/PaneLayout.swift",
                "UI/StatusBar.swift",
            ],
            resources: [
                .process("Resources"),
            ],
            linkerSettings: [
                .unsafeFlags([
                    "-Xlinker",
                    "-force_load",
                    "-Xlinker",
                    "\(libSearchPath)/libmuxterm.a",
                    "-liconv",
                    "-lresolv",
                ]),
            ]
        ),
    ]
)
