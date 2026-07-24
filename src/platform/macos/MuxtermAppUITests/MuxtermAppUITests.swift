import XCTest

/// Muxterm macOS XCUITest（键盘驱动）。
///
/// ## 快捷键（macOS）
/// - **Cmd+T**：新建 tab
/// - **Cmd+D** / **Cmd+Shift+D**：水平 / 竖直分割 pane
/// - **Ctrl+D**：关闭当前 pane（末 pane 关 tab；末 tab 关 window）
/// - **Cmd+1..9**：切 tab
///
/// ## CLI/TUI 复现结论
/// - **双写**：LocalBackend `WriteRaw` 曾本地回显 + pty 回显；已修。
/// - **Cmd+数字 noop**：仅 macOS/SwiftTerm；TUI 用 Alt+N。
/// - **Ctrl+D**：macOS 显式关 pane/tab/window；TUI 仍把 Ctrl+D 当退出应用。
///
/// ## 运行
/// ```
/// ./scripts/build-macos.sh
/// cd src/platform/macos && xcodegen generate
/// xcodebuild test -project Muxterm.xcodeproj -scheme MuxtermApp -destination 'platform=macOS'
/// ```
final class MuxtermAppUITests: XCTestCase {
    var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
        app = makeApplication()
        app.launchEnvironment["MUXTERM_UITEST"] = "1"
        app.launch()
    }

    override func tearDownWithError() throws {
        if app.state == .runningForeground || app.state == .runningBackground {
            app.terminate()
        }
        app = nil
    }

    // MARK: - 2tab3pane（全键盘）

    /// 启动 → 输入验证输出 → Cmd+D / Cmd+Shift+D 搭 3 pane → Cmd+T → Cmd+1 验布局。
    func testTwoTabThreePaneViaKeyboard() throws {
        let window = waitMainWindow()
        let status = statusBar()
        waitStatusContains(status, "connected", timeout: 5)

        // 1) 输入命令，验证输出（无双写）
        window.click()
        let marker = "UXTEST_\(Int(Date().timeIntervalSince1970))"
        app.typeText("echo \(marker)\r")
        let snippet = app.descendants(matching: .any)["muxterm.outputSnippet"]
        XCTAssertTrue(snippet.waitForExistence(timeout: 5))
        waitValueContains(snippet, marker, timeout: 8)
        let echoed = (snippet.value as? String) ?? ""
        XCTAssertFalse(echoed.contains(marker + marker), "不应双写: \(echoed)")

        // 2) Cmd+D 水平 + Cmd+Shift+D 竖直 → 3 panes
        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        app.typeKey("d", modifierFlags: [.command, .shift])
        waitStatusContains(status, "panes: 3", timeout: 8)

        // 3) Cmd+T → 2 tabs（新 tab 默认 1 pane）
        app.typeKey("t", modifierFlags: .command)
        waitStatusContains(status, "tabs: 2", timeout: 5)

        // 4) Cmd+2 → tab2，再 Cmd+1 回 tab1 验 3-pane
        app.typeKey("2", modifierFlags: .command)
        waitStatusContains(status, "panes: 1", timeout: 5)
        app.typeKey("1", modifierFlags: .command)
        waitStatusContains(status, "panes: 3", timeout: 5)
        waitStatusContains(status, "tabs: 2", timeout: 3)

        let tabBar = app.descendants(matching: .any)["muxterm.tabBar"]
        XCTAssertTrue(tabBar.waitForExistence(timeout: 3), "Tab 栏应可见")
    }

    // MARK: - Ctrl+D 关闭 pane / tab / window

    /// 单 tab 单 pane：Ctrl+D → 关闭整个窗口。
    func testCtrlDSingleTabClosesWindow() throws {
        let window = waitMainWindow()
        window.click()
        waitStatusContains(statusBar(), "connected", timeout: 5)

        app.typeKey("d", modifierFlags: .control)

        let disappeared = NSPredicate(format: "exists == false")
        let exp = XCTNSPredicateExpectation(predicate: disappeared, object: window)
        XCTAssertEqual(
            XCTWaiter.wait(for: [exp], timeout: 8),
            .completed,
            "单 tab Ctrl+D 后主窗口应关闭"
        )
    }

    /// 多 tab：Ctrl+D 只关掉当前 tab，窗口与另一 tab 保留。
    func testCtrlDMultiTabClosesOnlyCurrentTab() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        app.typeKey("t", modifierFlags: .command)
        waitStatusContains(status, "tabs: 2", timeout: 5)

        app.typeKey("d", modifierFlags: .control)

        XCTAssertTrue(window.waitForExistence(timeout: 3), "多 tab 时窗口应保留")
        waitStatusContains(status, "tabs: 1", timeout: 8)
        waitStatusContains(status, "connected", timeout: 3)
    }

    /// 多 pane：Ctrl+D 只关当前 pane，窗口与剩余 pane 保留。
    func testCtrlDClosesOnlyCurrentPane() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)

        app.typeKey("d", modifierFlags: .control)
        waitStatusContains(status, "panes: 1", timeout: 8)
        waitStatusContains(status, "tabs: 1", timeout: 3)
        waitStatusContains(status, "connected", timeout: 3)
        XCTAssertTrue(window.waitForExistence(timeout: 2), "多 pane 时 Ctrl+D 不应关窗")

        // 再搭 3 panes，Ctrl+D → 2
        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        app.typeKey("d", modifierFlags: [.command, .shift])
        waitStatusContains(status, "panes: 3", timeout: 8)
        app.typeKey("d", modifierFlags: .control)
        waitStatusContains(status, "panes: 2", timeout: 8)
        XCTAssertTrue(window.waitForExistence(timeout: 2), "3→2 pane 时窗口应保留")
    }

    // MARK: - 基础冒烟

    func testLaunchShowsMainWindow() throws {
        _ = waitMainWindow()
    }

    func testTabBarVisible() throws {
        _ = waitMainWindow()
        let tabBar = app.descendants(matching: .any)["muxterm.tabBar"]
        XCTAssertTrue(tabBar.waitForExistence(timeout: 5), "Tab 栏应渲染")
    }

    // MARK: - Helpers

    private func waitMainWindow() -> XCUIElement {
        let window = app.windows["Muxterm"]
        XCTAssertTrue(window.waitForExistence(timeout: 5), "主窗口应出现")
        return window
    }

    private func statusBar() -> XCUIElement {
        let status = app.descendants(matching: .any)["muxterm.statusBar"]
        XCTAssertTrue(status.waitForExistence(timeout: 5), "状态栏应存在")
        return status
    }

    /// 在测试线程轮询 AX value（XCTNSPredicateExpectation 读自定义 AX 常拿到陈旧快照）。
    private func waitStatusContains(_ status: XCUIElement, _ text: String, timeout: TimeInterval) {
        let deadline = Date().addingTimeInterval(timeout)
        var last = ""
        while Date() < deadline {
            last = (status.value as? String) ?? status.label
            if last.localizedCaseInsensitiveContains(text) {
                return
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
        }
        XCTFail("状态栏应包含 \(text)，实际=\(last)")
    }

    private func waitValueContains(_ element: XCUIElement, _ text: String, timeout: TimeInterval) {
        let deadline = Date().addingTimeInterval(timeout)
        var last = ""
        while Date() < deadline {
            last = (element.value as? String) ?? element.label
            if last.contains(text) {
                return
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
        }
        XCTFail("应包含 \(text)，实际=\(last)")
    }

    private func makeApplication() -> XCUIApplication {
        if let path = ProcessInfo.processInfo.environment["MUXTERM_APP_PATH"], !path.isEmpty {
            return XCUIApplication(url: URL(fileURLWithPath: path))
        }
        let repoApp = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("build/macos/Muxterm.app")
        if FileManager.default.fileExists(atPath: repoApp.path) {
            return XCUIApplication(url: repoApp)
        }
        return XCUIApplication()
    }
}
