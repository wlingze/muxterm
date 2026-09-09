import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 用户切换主题后终端配色必须跟着走（不再固定 1e1e2e 黑底）。
///
/// 默认（未切主题）就是浅色 palette；深色只在用户切到 dark 之后。
final class ThemeToggleE2ETests: XCTestCase {
    func testToggleThemeChangesChromeAndTerminalPalette() throws {
        AppE2E.requireTmux()
        let config = try IsolatedMuxtermConfig(label: "theme", toml: """
        config_version = 1

        [theme]
        name = "white"
        light = "white"
        dark = "black"
        """)
        defer { config.restore() }

        let fx = OnePaneCat(label: "theme")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        let initialOsc = try XCTUnwrap(app.testThemeHexColors(), "启动必须能读到终端色")
        XCTAssertEqual(
            initialOsc.fg.lowercased(),
            MuxtermPalette.light.fg.lowercased(),
            "默认主题必须是浅色终端前景，不能再写死 cdd6f4"
        )
        XCTAssertEqual(
            initialOsc.bg.lowercased(),
            MuxtermPalette.light.bg.lowercased(),
            "默认主题必须是浅色终端背景（白色），不能再写死 1e1e2e"
        )

        let beforeDark = app.testChromeAppearanceIsDark()
        let beforeSaved = app.currentTheme()
        app.testToggleTheme()
        AppE2E.pump(80)

        XCTAssertNotEqual(
            app.currentTheme(),
            beforeSaved,
            "Cmd-Shift-P 主题项必须翻转 light/dark"
        )
        let savedDark = MuxtermTerminalColors.themeName(
            from: try String(contentsOf: config.configURL, encoding: .utf8)
        )
        XCTAssertEqual(MuxtermTheme.from(name: savedDark), app.currentTheme())
        XCTAssertEqual(savedDark, "black")
        XCTAssertNotEqual(
            app.testChromeAppearanceIsDark(),
            beforeDark,
            "窗口/chrome effectiveAppearance 必须跟着主题走"
        )

        let osc = try XCTUnwrap(app.testThemeHexColors(), "必须能读到上报色")
        let palette = app.currentTheme().palette
        XCTAssertEqual(
            osc.fg.lowercased(),
            palette.fg.lowercased(),
            "OSC 10 / SwiftTerm 前景必须等于当前主题 palette，不能再写死 cdd6f4"
        )
        XCTAssertEqual(
            osc.bg.lowercased(),
            palette.bg.lowercased(),
            "OSC 11 / SwiftTerm 背景必须等于当前主题 palette，不能再写死 1e1e2e"
        )

        app.testToggleTheme()
        AppE2E.pump(80)
        let osc2 = try XCTUnwrap(app.testThemeHexColors())
        let palette2 = app.currentTheme().palette
        XCTAssertEqual(osc2.fg.lowercased(), palette2.fg.lowercased())
        XCTAssertEqual(osc2.bg.lowercased(), palette2.bg.lowercased())
        XCTAssertNotEqual(
            osc.bg.lowercased(),
            osc2.bg.lowercased(),
            "两次切换后终端背景必须真的变（light 白 / dark 黑）"
        )

        app.testOpenCommandPalette()
        AppE2E.pump(40)
        let titles = app.testPaletteTitles().joined(separator: " ").lowercased()
        let nextName = app.currentTheme() == .light ? "dark" : "light"
        XCTAssertTrue(
            titles.contains(nextName),
            "面板主题项必须显示下一档。titles=\(app.testPaletteTitles())"
        )
    }

    func testSetThemeColorsUsesProvidedHex() {
        AppE2E.ensureApp()
        MuxtermTerminalColors.activePalette = .light
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 320, height: 180))
        let def = view.themeHexColors()
        XCTAssertEqual(def.fg.lowercased(), MuxtermPalette.light.fg)
        XCTAssertEqual(def.bg.lowercased(), MuxtermPalette.light.bg)

        view.setThemeColors(
            fgHex: MuxtermTerminalColors.lightForegroundHex,
            bgHex: MuxtermTerminalColors.lightBackgroundHex
        )
        let light = view.themeHexColors()
        XCTAssertEqual(light.fg.lowercased(), MuxtermTerminalColors.lightForegroundHex)
        XCTAssertEqual(light.bg.lowercased(), MuxtermTerminalColors.lightBackgroundHex)

        view.setThemeColors(
            fgHex: MuxtermTerminalColors.foregroundHex,
            bgHex: MuxtermTerminalColors.backgroundHex
        )
        let dark = view.themeHexColors()
        XCTAssertEqual(dark.fg.lowercased(), MuxtermTerminalColors.foregroundHex)
        XCTAssertEqual(dark.bg.lowercased(), MuxtermTerminalColors.backgroundHex)
    }
}
