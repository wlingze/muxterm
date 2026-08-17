import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 主题切换必须改 chrome 外观并持久化；终端 OSC 10/11 仍固定深色。
final class ThemeToggleE2ETests: XCTestCase {
    func testToggleThemeChangesChromeButKeepsDarkOsc() throws {
        AppE2E.requireTmux()
        let defaults = UserDefaults.standard
        let key = "muxterm.theme"
        let previous = defaults.string(forKey: key)
        defaults.removeObject(forKey: key)
        defer {
            if let previous {
                defaults.set(previous, forKey: key)
            } else {
                defaults.removeObject(forKey: key)
            }
        }

        let fx = OnePaneCat(label: "theme")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        let beforeDark = app.testChromeAppearanceIsDark()
        let beforeSaved = app.currentTheme()
        app.testToggleTheme()
        AppE2E.pump(80)

        XCTAssertNotEqual(
            app.currentTheme(),
            beforeSaved,
            "Cmd-Shift-P 主题项必须翻转 light/dark"
        )
        XCTAssertEqual(app.testSavedTheme(), app.currentTheme().rawValue)
        XCTAssertNotEqual(
            app.testChromeAppearanceIsDark(),
            beforeDark,
            "窗口/chrome effectiveAppearance 必须跟着主题走，不能只写 UserDefaults"
        )

        let osc = try XCTUnwrap(app.testThemeHexColors(), "必须能读到上报色")
        XCTAssertEqual(osc.fg.lowercased(), MuxtermTerminalColors.foregroundHex)
        XCTAssertEqual(osc.bg.lowercased(), MuxtermTerminalColors.backgroundHex)

        app.testOpenCommandPalette()
        AppE2E.pump(40)
        let titles = app.testPaletteTitles().joined(separator: " ").lowercased()
        let nextName = app.currentTheme() == .light ? "dark" : "light"
        XCTAssertTrue(
            titles.contains(nextName),
            "面板主题项必须显示下一档。titles=\(app.testPaletteTitles())"
        )
    }
}
