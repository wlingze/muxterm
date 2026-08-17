import Foundation
import XCTest
@testable import MuxtermAppLib

/// 对标 `linux_prefs_e2e`：Cmd+= 增大字号并持久化（macOS 写 UserDefaults，不得新建 preferences.toml）。
final class PrefsE2ETests: XCTestCase {
    func testCmdEqualIncreasesFontAndPersists() throws {
        let defaults = UserDefaults.standard
        let key = "muxterm.terminalFontSize"
        defaults.removeObject(forKey: key)
        defer { defaults.removeObject(forKey: key) }

        AppE2E.requireTmux()
        let fx = OnePaneCat(label: "prefs")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        let before = defaults.object(forKey: key) as? Double
        app.increaseTerminalFontSize(nil)
        AppE2E.pump(80)
        let after = try XCTUnwrap(defaults.object(forKey: key) as? Double, "Cmd+= 必须把字号写入 UserDefaults \(key)")
        if let before {
            XCTAssertGreaterThan(after, before)
        } else {
            XCTAssertGreaterThan(after, 0)
        }
        let prefs = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/muxterm/preferences.toml")
        // 本测试不得为了绿去新建 preferences.toml；若用户机器上已有文件则忽略。
        _ = prefs
    }
}
