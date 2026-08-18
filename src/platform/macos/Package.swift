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

let muxtermForceLoad: [LinkerSetting] = [
    .unsafeFlags([
        "-Xlinker",
        "-force_load",
        "-Xlinker",
        "\(libSearchPath)/libmuxterm.a",
        "-liconv",
        "-lresolv",
    ]),
]

let appLibSources = [
    "CoreBridge/CoreBridge.swift",
    "App/AppDelegate.swift",
    "App/I18n.swift",
    "App/MainWindow.swift",
    "App/MainWindow+Testing.swift",
    "App/ContentView.swift",
    "App/CommandPalette.swift",
    "App/ConnectionDiscovery.swift",
    "App/WarmConnectionSlot.swift",
    "App/QuickConnectController.swift",
    "App/SearchPanelController.swift",
    "App/AttentionPanelController.swift",
    "App/UnifiedPanelController.swift",
    "App/TargetConfigWindow.swift",
    "App/SettingsWindow.swift",
    "Terminal/TerminalView.swift",
    "Terminal/TerminalManager.swift",
    "UI/TabBar.swift",
    "UI/PaneLayout.swift",
    "UI/StatusBarView.swift",
]

let package = Package(
    name: "MuxtermApp",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .library(name: "MuxtermChrome", targets: ["MuxtermChrome"]),
        .library(name: "MuxtermAppLib", targets: ["MuxtermAppLib"]),
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
        // AppKit 会话层：给 in-process e2e 用（对标 Linux AppWindow）。
        .target(
            name: "MuxtermAppLib",
            dependencies: [
                "CMuxterm",
                "MuxtermChrome",
                .product(name: "SwiftTerm", package: "SwiftTerm"),
            ],
            path: ".",
            exclude: [
                "main.swift",
                "Package.swift",
                "Info.plist",
                "Vendor",
                "CoreBridge/include",
                "CoreBridge/shim.c",
                "CoreBridge/muxterm.h",
                "MuxtermAppUITests",
                "Chrome",
                "ChromeTests",
                "AppE2ETests",
                "project.yml",
                ".build",
                "UI/ConnectionStatusView.swift",
            ],
            sources: appLibSources,
            resources: [
                .process("Resources"),
            ]
        ),
        .executableTarget(
            name: "MuxtermApp",
            dependencies: [
                "MuxtermAppLib",
            ],
            path: ".",
            exclude: [
                "Package.swift",
                "Info.plist",
                "Vendor",
                "App",
                "Terminal",
                "UI",
                "CoreBridge",
                "Resources",
                "MuxtermAppUITests",
                "Chrome",
                "ChromeTests",
                "AppE2ETests",
                "project.yml",
                ".build",
            ],
            sources: [
                "main.swift",
            ],
            linkerSettings: muxtermForceLoad
        ),
        // in-process AppKit e2e（对标 tests/linux_*_e2e.rs）。必须链 libmuxterm.a。
        .testTarget(
            name: "MuxtermAppE2ETests",
            dependencies: [
                "MuxtermAppLib",
            ],
            path: "AppE2ETests",
            linkerSettings: muxtermForceLoad
        ),
    ]
)
