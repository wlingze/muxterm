import Foundation
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 对标 `linux_prefs_e2e`：Cmd+= 增大字号并通过 Core 事务持久化。
final class PrefsE2ETests: XCTestCase {
    func testCmdEqualIncreasesFontAndPersists() throws {
        let config = try IsolatedMuxtermConfig(label: "prefs-font", toml: """
        config_version = 1

        [font]
        family = "Menlo"
        size = 18.0
        """)
        defer { config.restore() }

        AppE2E.requireTmux()
        let fx = OnePaneCat(label: "prefs")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        let before = app.testTerminalFontSize()
        app.increaseTerminalFontSize(nil)
        AppE2E.pump(80)
        let after = app.testTerminalFontSize()
        XCTAssertEqual(after, before + 1, accuracy: 0.01)

        let persisted = try String(contentsOf: config.configURL, encoding: .utf8)
        XCTAssertEqual(MuxtermTerminalFont.settings(from: persisted).size, after, accuracy: 0.01)
        XCTAssertFalse(
            FileManager.default.fileExists(
                atPath: config.root.appendingPathComponent("muxterm/preferences.toml").path
            ),
            "新实现只写统一 config.toml，不得重建 legacy preferences.toml"
        )
    }
}
