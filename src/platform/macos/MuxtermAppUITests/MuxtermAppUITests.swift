import XCTest

/// Muxterm macOS XCUITest（键盘驱动）。
///
/// ## CLI/TUI 复现结论（写测试前已核对）
/// - **双写**：LocalBackend `WriteRaw` 曾本地回显 + pty 回显（全平台 FFI）；已修。
/// - **Cmd+数字 noop**：仅 macOS/SwiftTerm；TUI 用 Alt+N，无此问题。
/// - **Tab 栏空白**：仅 AppKit `fullSizeContentView`；TUI 无此问题。
/// - **Ctrl+D**：
///   - TUI：`is_quit` 把 Ctrl+D 当**退出 TUI**（不发给 shell）——与「EOF 关 tab」语义不同。
///   - LocalBackend：单 pane 退出关 window；多 tab 时末 pane Exit 应只关该 tab（已补单测）。
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

    /// 启动 → 输入验证输出 → Cmd+Shift+S/V 搭 3 pane → Cmd+T 第二 tab → Cmd+1 回 tab1 验布局。
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

        // 2) Cmd+Shift+S/V → 当前 tab 3 panes
        // （XCUITest 下 Option 修饰键常无法传到 AppKit；人手 Alt+S/V 仍可用）
        app.typeKey("s", modifierFlags: [.command, .shift])
        waitStatusContains(status, "panes: 2", timeout: 8)
        app.typeKey("v", modifierFlags: [.command, .shift])
        waitStatusContains(status, "panes: 3", timeout: 8)

        // 3) Cmd+T → 2 tabs（新 tab 默认 1 pane）
        app.typeKey("t", modifierFlags: .command)
        waitStatusContains(status, "tabs: 2", timeout: 5)

        // 4) Cmd+2 确保在 tab2，再 Cmd+1 切回 tab1，验证 3-pane 布局
        app.typeKey("2", modifierFlags: .command)
        waitStatusContains(status, "panes: 1", timeout: 5)
        app.typeKey("1", modifierFlags: .command)
        waitStatusContains(status, "panes: 3", timeout: 5)
        waitStatusContains(status, "tabs: 2", timeout: 3)

        let tabBar = app.descendants(matching: .any)["muxterm.tabBar"]
        XCTAssertTrue(tabBar.waitForExistence(timeout: 3), "Tab 栏应可见")
    }

    // MARK: - Ctrl+D 退出

    /// 单 tab：Ctrl+D（EOF）退出 shell → 关闭整个窗口。
    func testCtrlDSingleTabClosesWindow() throws {
        let window = waitMainWindow()
        window.click()
        waitStatusContains(statusBar(), "connected", timeout: 5)

        // 发给 shell 的 EOF
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

        // 两个 tab
        app.typeKey("t", modifierFlags: .command)
        waitStatusContains(status, "tabs: 2", timeout: 5)

        // 在当前（多半是 tab2）Ctrl+D
        app.typeKey("d", modifierFlags: .control)

        // 窗口仍在，tabs 回到 1
        XCTAssertTrue(window.waitForExistence(timeout: 3), "多 tab 时窗口应保留")
        waitStatusContains(status, "tabs: 1", timeout: 8)
        waitStatusContains(status, "connected", timeout: 3)
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
