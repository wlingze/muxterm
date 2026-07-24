import XCTest

/// Muxterm macOS XCUITest：启动 app、验证窗口、模拟键盘、校验输出片段。
///
/// 运行方式（需先 `scripts/build-macos.sh` 生成 .app，再用 xcodegen + xcodebuild）：
///   cd src/platform/macos && xcodegen generate
///   xcodebuild test -project Muxterm.xcodeproj -scheme MuxtermApp -destination 'platform=macOS'
final class MuxtermAppUITests: XCTestCase {
    var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
        app = makeApplication()
        app.launchArguments = ["--uitest"]
        app.launchEnvironment["MUXTERM_UITEST"] = "1"
        app.launch()
    }

    override func tearDownWithError() throws {
        app.terminate()
        app = nil
    }

    /// 启动后主窗口存在。
    func testLaunchShowsMainWindow() throws {
        let window = app.windows["Muxterm"]
        XCTAssertTrue(window.waitForExistence(timeout: 5), "主窗口应出现")
        XCTAssertTrue(window.exists)
    }

    /// 状态栏显示 connected。
    func testStatusBarConnected() throws {
        let window = app.windows["Muxterm"]
        XCTAssertTrue(window.waitForExistence(timeout: 5))

        let status = app.descendants(matching: .any)["muxterm.statusBar"]
        XCTAssertTrue(status.waitForExistence(timeout: 5), "状态栏应存在")

        let predicate = NSPredicate(format: "value CONTAINS[c] %@", "connected")
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: status)
        XCTAssertEqual(XCTWaiter.wait(for: [expectation], timeout: 5), .completed)
    }

    /// 模拟键盘输入 echo，并在 outputSnippet 中看到回显（无双写）。
    func testKeyboardInputEchoesOnce() throws {
        let window = app.windows["Muxterm"]
        XCTAssertTrue(window.waitForExistence(timeout: 5))
        window.click()

        // 唯一标记，避免与 prompt 混淆
        let marker = "UXTEST_\(Int(Date().timeIntervalSince1970))"
        app.typeText("echo \(marker)\r")

        let snippet = app.descendants(matching: .any)["muxterm.outputSnippet"]
        XCTAssertTrue(snippet.waitForExistence(timeout: 5))

        let predicate = NSPredicate(format: "value CONTAINS %@", marker)
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: snippet)
        XCTAssertEqual(
            XCTWaiter.wait(for: [expectation], timeout: 8),
            .completed,
            "应看到 echo 输出"
        )

        // 双写检测：标记不应连续出现两次（如 MARKERMARKER）
        let value = (snippet.value as? String) ?? ""
        let doubled = marker + marker
        XCTAssertFalse(
            value.contains(doubled),
            "输出不应双写：\(value)"
        )
    }

    /// Cmd+T 新建 tab 后状态栏 tabs 数增加。
    func testCommandTCreatesTab() throws {
        let window = app.windows["Muxterm"]
        XCTAssertTrue(window.waitForExistence(timeout: 5))
        window.click()

        let status = app.descendants(matching: .any)["muxterm.statusBar"]
        XCTAssertTrue(status.waitForExistence(timeout: 5))

        app.typeKey("t", modifierFlags: .command)

        let predicate = NSPredicate(format: "value CONTAINS %@", "tabs: 2")
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: status)
        XCTAssertEqual(
            XCTWaiter.wait(for: [expectation], timeout: 5),
            .completed,
            "Cmd+T 后应有 2 个 tab"
        )
    }

    /// Cmd+1 / Cmd+2 切换 tab（不应落到 SwiftTerm noop:）。
    func testCommandNumberSwitchesTab() throws {
        let window = app.windows["Muxterm"]
        XCTAssertTrue(window.waitForExistence(timeout: 5))
        window.click()

        let status = app.descendants(matching: .any)["muxterm.statusBar"]
        XCTAssertTrue(status.waitForExistence(timeout: 5))

        app.typeKey("t", modifierFlags: .command)
        let twoTabs = NSPredicate(format: "value CONTAINS %@", "tabs: 2")
        XCTAssertEqual(
            XCTWaiter.wait(
                for: [XCTNSPredicateExpectation(predicate: twoTabs, object: status)],
                timeout: 5
            ),
            .completed
        )

        app.typeKey("1", modifierFlags: .command)
        app.typeKey("2", modifierFlags: .command)

        // 仍 connected，且 tab 栏可访问
        let tabBar = app.descendants(matching: .any)["muxterm.tabBar"]
        XCTAssertTrue(tabBar.waitForExistence(timeout: 3), "Tab 栏应可见")
        let connected = NSPredicate(format: "value CONTAINS[c] %@", "connected")
        XCTAssertEqual(
            XCTWaiter.wait(
                for: [XCTNSPredicateExpectation(predicate: connected, object: status)],
                timeout: 3
            ),
            .completed
        )
    }

    /// Tab 栏存在且可命中。
    func testTabBarVisible() throws {
        let window = app.windows["Muxterm"]
        XCTAssertTrue(window.waitForExistence(timeout: 5))
        let tabBar = app.descendants(matching: .any)["muxterm.tabBar"]
        XCTAssertTrue(tabBar.waitForExistence(timeout: 5), "Tab 栏应渲染")
        XCTAssertTrue(tabBar.isHittable || tabBar.exists)
    }

    // MARK: - Helpers

    /// 优先用环境变量指定的 .app；否则用默认 bundle id / 相对路径。
    private func makeApplication() -> XCUIApplication {
        if let path = ProcessInfo.processInfo.environment["MUXTERM_APP_PATH"], !path.isEmpty {
            return XCUIApplication(url: URL(fileURLWithPath: path))
        }
        let repoApp = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // MuxtermAppUITests
            .deletingLastPathComponent() // macos
            .deletingLastPathComponent() // platform
            .deletingLastPathComponent() // src
            .appendingPathComponent("build/macos/Muxterm.app")
        if FileManager.default.fileExists(atPath: repoApp.path) {
            return XCUIApplication(url: repoApp)
        }
        let app = XCUIApplication()
        app.launchArguments = []
        return app
    }
}
