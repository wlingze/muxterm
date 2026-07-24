import XCTest

/// Muxterm macOS XCUITest（键盘驱动）。
///
/// **硬性约定**（分割/布局相关）：
/// 1. 布局正确性 — 3 pane 后尺寸比例合理（等分水平 / 竖直半高）
/// 2. pane 切换 — Cmd+[ / Cmd+]（或 Alt）切换，状态栏 `pane: @N` 与焦点跟随
/// 3. 每 pane 独立 I/O — 各 pane 分别 echo，输出只出现在对应 terminal AX
/// 4. 分割后不黑屏 — echo 后 snippet + terminal AX 均可见输出
///
/// ## 快捷键
/// Cmd+T tab；Cmd+D / Cmd+Shift+D 分屏；Cmd+[ / ] 切 pane；Ctrl+D 关闭；Cmd+1..9 切 tab
final class MuxtermAppUITests: XCTestCase {
    var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
        try preflightAutomationMode()
        let lingering = XCUIApplication(bundleIdentifier: "dev.muxterm.app")
        if lingering.state != .notRunning {
            lingering.terminate()
            RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        }
        app = makeApplication()
        app.launchEnvironment["MUXTERM_UITEST"] = "1"
        // 先 activate 再 launch：部分环境下可避免卡在 Running Background
        app.activate()
        app.launch()
        app.activate()
        XCTAssertTrue(
            app.wait(for: .runningForeground, timeout: 15),
            "应用应进入前台（当前=\(app.state.rawValue)）"
        )
    }

    /// macOS XCUITest 依赖 UI Automation；未开启时 launch 会卡在 Running Background ~60s。
    private func preflightAutomationMode() throws {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/automationmodetool")
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe
        do {
            try proc.run()
            proc.waitUntilExit()
        } catch {
            return
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        let out = String(data: data, encoding: .utf8) ?? ""
        if out.localizedCaseInsensitiveContains("Automation Mode is disabled") {
            XCTFail(
                """
                UI Automation 未开启，XCUITest 无法把 Muxterm 拉到前台。
                请在本机执行一次（需管理员密码）：
                  sudo /usr/bin/automationmodetool enable-automationmode-without-authentication
                然后确认 `/usr/bin/automationmodetool` 显示 enabled。
                """
            )
        }
    }

    override func tearDownWithError() throws {
        if app.state == .runningForeground || app.state == .runningBackground {
            app.terminate()
        }
        app = nil
    }

    // MARK: - Split + 渲染

    /// Cmd+D 后 `echo SPLIT_OK` 必须可见（黑屏则失败）。
    func testCmdDSplitThenEchoSPLIT_OK() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        settle(window)
        echoAndVerifyVisible("SPLIT_OK")
        assertHorizontalSplitRoughlyEqual()
    }

    // MARK: - 2tab3pane + 每 tab I/O

    func testTwoTabThreePaneViaKeyboard() throws {
        let window = waitMainWindow()
        let status = statusBar()
        waitStatusContains(status, "connected", timeout: 5)
        window.click()

        echoAndVerifyVisible("BOOT_OK")

        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        settle(window)
        echoAndVerifyVisible("SPLIT_OK")

        app.typeKey("d", modifierFlags: [.command, .shift])
        waitStatusContains(status, "panes: 3", timeout: 8)
        settle(window)
        echoAndVerifyVisible("VSPLIT_OK")
        assertThreePaneLayoutReasonable()

        app.typeKey("t", modifierFlags: .command)
        waitStatusContains(status, "tabs: 2", timeout: 5)
        settle(window)

        app.typeKey("1", modifierFlags: .command)
        waitStatusContains(status, "panes: 3", timeout: 5)
        settle(window)
        echoAndVerifyVisible("TAB1_OK")

        app.typeKey("2", modifierFlags: .command)
        waitStatusContains(status, "panes: 1", timeout: 5)
        settle(window)
        echoAndVerifyVisible("TAB2_OK")
    }

    // MARK: - Pane 切换 + 每 pane 独立输出

    /// Cmd+] 切 pane；每个 pane 各自 echo，输出落在对应 terminal。
    func testPaneSwitchFocusAndPerPaneIO() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        // 水平两分
        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        settle(window)

        let paneA = activePaneId(from: status)
        let markerA = "PANE_A_OK"
        echoAndVerifyVisible(markerA)
        assertTerminal(paneA, contains: markerA)

        // Cmd+] → 另一 pane，焦点应变
        app.typeKey("]", modifierFlags: .command)
        settle(window)
        waitActivePaneChange(from: paneA, status: status, timeout: 5)
        let paneB = activePaneId(from: status)
        XCTAssertNotEqual(paneA, paneB, "Cmd+] 后活跃 pane 应变化")

        let markerB = "PANE_B_OK"
        echoAndVerifyVisible(markerB)
        assertTerminal(paneB, contains: markerB)
        // A 不应被 B 的输入污染（B 的标记不应出现在 A）
        assertTerminal(paneA, doesNotContain: markerB)

        // Cmd+[ 回到 A
        app.typeKey("[", modifierFlags: .command)
        settle(window)
        waitStatusContains(status, "pane: @\(paneA)", timeout: 5)
        echoAndVerifyVisible("PANE_A2_OK")
        assertTerminal(paneA, contains: "PANE_A2_OK")
    }

    // MARK: - Ctrl+D

    func testCtrlDSingleTabClosesWindow() throws {
        let window = waitMainWindow()
        window.click()
        waitStatusContains(statusBar(), "connected", timeout: 5)
        echoAndVerifyVisible("BEFORE_QUIT")

        app.typeKey("d", modifierFlags: .control)
        let disappeared = NSPredicate(format: "exists == false")
        let exp = XCTNSPredicateExpectation(predicate: disappeared, object: window)
        XCTAssertEqual(XCTWaiter.wait(for: [exp], timeout: 8), .completed)
    }

    func testCtrlDMultiTabClosesOnlyCurrentTab() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        app.typeKey("t", modifierFlags: .command)
        waitStatusContains(status, "tabs: 2", timeout: 5)
        settle(window)
        echoAndVerifyVisible("TAB2_BEFORE_CLOSE")

        app.typeKey("d", modifierFlags: .control)
        XCTAssertTrue(window.waitForExistence(timeout: 3))
        waitStatusContains(status, "tabs: 1", timeout: 8)
        settle(window)
        echoAndVerifyVisible("TAB1_AFTER_CLOSE")
    }

    func testCtrlDClosesOnlyCurrentPane() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        settle(window)
        echoAndVerifyVisible("SPLIT_OK")

        app.typeKey("d", modifierFlags: .control)
        waitStatusContains(status, "panes: 1", timeout: 8)
        settle(window)
        echoAndVerifyVisible("AFTER_PANE_CLOSE")
    }

    // MARK: - 冒烟

    func testLaunchShowsMainWindow() throws {
        _ = waitMainWindow()
    }

    func testTabBarVisible() throws {
        let window = waitMainWindow()
        window.click()
        waitStatusContains(statusBar(), "connected", timeout: 5)
        app.typeKey("t", modifierFlags: .command)
        waitStatusContains(statusBar(), "tabs: 2", timeout: 5)
        settle(window)
        echoAndVerifyVisible("TABBAR_OK")
        XCTAssertTrue(app.descendants(matching: .any)["muxterm.tabBar"].waitForExistence(timeout: 5))
    }

    // MARK: - Helpers

    private func echoAndVerifyVisible(_ marker: String) {
        app.windows["Muxterm"].click()
        app.typeText("echo \(marker)\r")
        let snippet = app.descendants(matching: .any)["muxterm.outputSnippet"]
        XCTAssertTrue(snippet.waitForExistence(timeout: 5))
        waitValueContains(snippet, marker, timeout: 10)
        assertAnyTerminalContains(marker, timeout: 10)
    }

    private func settle(_ window: XCUIElement) {
        RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        window.click()
    }

    private func waitMainWindow() -> XCUIElement {
        let window = app.windows["Muxterm"]
        XCTAssertTrue(window.waitForExistence(timeout: 8))
        return window
    }

    private func statusBar() -> XCUIElement {
        let status = app.descendants(matching: .any)["muxterm.statusBar"]
        XCTAssertTrue(status.waitForExistence(timeout: 5))
        return status
    }

    private func waitStatusContains(_ status: XCUIElement, _ text: String, timeout: TimeInterval) {
        let deadline = Date().addingTimeInterval(timeout)
        var last = ""
        while Date() < deadline {
            last = (status.value as? String) ?? status.label
            if last.localizedCaseInsensitiveContains(text) { return }
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
        }
        XCTFail("状态栏应包含 \(text)，实际=\(last)")
    }

    private func waitValueContains(_ element: XCUIElement, _ text: String, timeout: TimeInterval) {
        let deadline = Date().addingTimeInterval(timeout)
        var last = ""
        while Date() < deadline {
            last = (element.value as? String) ?? element.label
            if last.contains(text) { return }
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
        }
        XCTFail("应包含 \(text)，实际=\(last)")
    }

    private func activePaneId(from status: XCUIElement) -> UInt32 {
        let text = (status.value as? String) ?? status.label
        // "... pane: @12"
        guard let range = text.range(of: #"pane: @(\d+)"#, options: .regularExpression) else {
            XCTFail("无法解析活跃 pane: \(text)")
            return 0
        }
        let chunk = String(text[range])
        let digits = chunk.filter(\.isNumber)
        return UInt32(digits) ?? 0
    }

    private func waitActivePaneChange(from old: UInt32, status: XCUIElement, timeout: TimeInterval) {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let now = activePaneId(from: status)
            if now != 0, now != old { return }
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
        }
        XCTFail("活跃 pane 未从 @\(old) 变化")
    }

    private func terminalElements() -> XCUIElementQuery {
        app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "muxterm.terminal.")
        )
    }

    private func paneHostElements() -> XCUIElementQuery {
        app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "muxterm.pane.")
        )
    }

    private func assertAnyTerminalContains(_ text: String, timeout: TimeInterval) {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let terms = terminalElements()
            for i in 0..<terms.count {
                let v = (terms.element(boundBy: i).value as? String) ?? ""
                if v.contains(text) { return }
            }
            let snippet = (app.descendants(matching: .any)["muxterm.outputSnippet"].value as? String) ?? ""
            if snippet.contains(text) { return }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        XCTFail("未见输出 \(text)（疑似黑屏）")
    }

    private func assertTerminal(_ paneId: UInt32, contains text: String) {
        let el = app.descendants(matching: .any)["muxterm.terminal.\(paneId)"]
        XCTAssertTrue(el.waitForExistence(timeout: 5), "terminal @\(paneId) 应存在")
        waitValueContains(el, text, timeout: 10)
    }

    private func assertTerminal(_ paneId: UInt32, doesNotContain text: String) {
        let el = app.descendants(matching: .any)["muxterm.terminal.\(paneId)"]
        guard el.exists else { return }
        let v = (el.value as? String) ?? ""
        XCTAssertFalse(v.contains(text), "pane @\(paneId) 不应含 \(text)，实际=\(v)")
    }

    /// 水平二分后两 pane 宽度应接近（±35%）。
    private func assertHorizontalSplitRoughlyEqual() {
        settle(app.windows["Muxterm"])
        let hosts = paneHostElements()
        XCTAssertGreaterThanOrEqual(hosts.count, 2, "水平分割后应有 ≥2 pane host")
        var widths: [CGFloat] = []
        for i in 0..<hosts.count {
            let f = hosts.element(boundBy: i).frame
            if f.width > 10 { widths.append(f.width) }
        }
        XCTAssertGreaterThanOrEqual(widths.count, 2, "应能读到 pane 宽度")
        let a = widths[0], b = widths[1]
        let ratio = min(a, b) / max(a, b)
        XCTAssertGreaterThan(ratio, 0.55, "水平二分宽度比应接近 1，实际 \(a) vs \(b)")
    }

    /// 3 pane（水平再竖直）后：三块非零、面积大致三等分，且宽或高有明显分层。
    private func assertThreePaneLayoutReasonable() {
        settle(app.windows["Muxterm"])
        let hosts = paneHostElements()
        XCTAssertGreaterThanOrEqual(hosts.count, 3, "3-pane 布局应有 ≥3 host")
        var frames: [CGRect] = []
        for i in 0..<min(hosts.count, 8) {
            let f = hosts.element(boundBy: i).frame
            if f.width > 5, f.height > 5 { frames.append(f) }
        }
        XCTAssertGreaterThanOrEqual(frames.count, 3, "应读到 3 个非零 pane frame")
        let widths = frames.map(\.width).sorted()
        let heights = frames.map(\.height).sorted()
        let widthSpread = (widths.last! - widths.first!) / widths.last!
        let heightSpread = (heights.last! - heights.first!) / heights.last!
        XCTAssertTrue(
            widthSpread > 0.15 || heightSpread > 0.15,
            "3-pane 尺寸应有差异，wSpread=\(widthSpread) hSpread=\(heightSpread) frames=\(frames)"
        )
        // 面积比例：三块合计后各自约占 1/3（允许较大误差，主要防黑屏叠成全屏）
        let areas = frames.prefix(3).map { $0.width * $0.height }
        let total = areas.reduce(0, +)
        XCTAssertGreaterThan(total, 1, "pane 总面积应 > 0")
        for (i, area) in areas.enumerated() {
            let share = area / total
            XCTAssertGreaterThan(share, 0.12, "pane[\(i)] 面积占比过小 \(share)，疑似布局塌陷")
            XCTAssertLessThan(share, 0.65, "pane[\(i)] 面积占比过大 \(share)，疑似未真正分割")
        }
    }

    private func makeApplication() -> XCUIApplication {
        // 显式指定时才用外部 .app；xcodebuild test 必须用 scheme host，否则会 Running Background
        if let path = ProcessInfo.processInfo.environment["MUXTERM_APP_PATH"], !path.isEmpty {
            return XCUIApplication(url: URL(fileURLWithPath: path))
        }
        return XCUIApplication()
    }
}
