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
/// Cmd+T tab；Cmd+D / Cmd+Shift+D 分屏；Cmd+[ / ] 切 pane；Ctrl+D 发送 EOF；Cmd+1..9 切 tab
final class MuxtermAppUITests: XCTestCase {
    var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
        try preflightUITestEnvironment()
        let lingering = XCUIApplication(bundleIdentifier: "dev.muxterm.app")
        if lingering.state != .notRunning {
            lingering.terminate()
            RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        }
        app = makeApplication()
        app.launchEnvironment["MUXTERM_UITEST"] = "1"
        app.launch()
        app.activate()
        XCTAssertTrue(
            app.wait(for: .runningForeground, timeout: 15),
            "应用应进入前台（当前=\(app.state.rawValue)）"
        )
    }

    /// SSH / 非 Aqua 会话无法 activate App；缺 UI Automation 认证时失败。
    private func preflightUITestEnvironment() throws {
        let env = ProcessInfo.processInfo.environment
        // SSH 无 GUI 登录会话：跳过（本地 GUI / CI macos runner 继续跑）
        if env["SSH_CONNECTION"] != nil || env["SSH_CLIENT"] != nil || env["SSH_TTY"] != nil {
            throw XCTSkip("XCUITest 需要 GUI 登录会话；SSH 环境跳过（由 CI 跑）")
        }
        if env["MUXTERM_SKIP_UITEST"] == "1" {
            throw XCTSkip("MUXTERM_SKIP_UITEST=1")
        }
        // Cursor agent / launchd Background 域同样无法 activate
        if launchctlManagerName() == "Background" {
            throw XCTSkip("XCUITest 需要 Aqua GUI 会话（当前 launchctl manager=Background）")
        }

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
        // “disabled + 不需认证” 正常——xcodebuild 会自行 flip on
        let needsAuth = out.localizedCaseInsensitiveContains("requires user authentication")
        if needsAuth {
            XCTFail(
                """
                本机开启 UI Automation 仍需密码，XCUITest 无法把 Muxterm 拉到前台。
                请执行一次：
                  sudo /usr/bin/automationmodetool enable-automationmode-without-authentication
                确认输出含 “DOES NOT REQUIRE user authentication”。
                """
            )
        }
    }

    private func launchctlManagerName() -> String? {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        proc.arguments = ["managername"]
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = FileHandle.nullDevice
        do {
            try proc.run()
            proc.waitUntilExit()
        } catch {
            return nil
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        let out = String(data: data, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return out?.isEmpty == false ? out : nil
    }

    override func tearDownWithError() throws {
        if app.state == .runningForeground || app.state == .runningBackground {
            app.terminate()
        }
        app = nil
    }

    // MARK: - 输入去重 + login shell 环境

    func testTypedCommandIsEchoedAndPrintedExactlyOnce() throws {
        let window = waitMainWindow()
        window.click()
        waitStatusContains(statusBar(), "connected", timeout: 5)

        let marker = "MUXTERM_SINGLE_INPUT_74291"
        app.typeText("printf '\(marker)\\n'\r")
        let terminal = terminalElements().firstMatch
        XCTAssertTrue(terminal.waitForExistence(timeout: 5))
        waitValueContains(terminal, marker, timeout: 10)

        let value = (terminal.value as? String) ?? ""
        let count = value.components(separatedBy: marker).count - 1
        XCTAssertEqual(
            count,
            2,
            "marker 应仅出现于命令回显和 printf 输出，各一次；实际=\(count)，内容=\(value)"
        )
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

    /// 上下 pane 的分隔条必须可拖动；拖动后仍保持 3 个非零 pane，随后
    /// 终端输入/输出也必须继续工作，防止 divider 事件导致视图树失焦。
    func testVerticalDividerCanBeDraggedWithoutBreakingLayout() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        app.typeKey("d", modifierFlags: [.command, .shift])
        waitStatusContains(status, "panes: 3", timeout: 8)
        settle(window)

        let dividers = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "muxterm.divider.")
        )
        var verticalDivider: XCUIElement?
        for i in 0..<dividers.count {
            let candidate = dividers.element(boundBy: i)
            let frame = candidate.frame
            // 上下 pane 的 divider 是横线：宽远大于高。
            if frame.width > frame.height * 2, frame.width > 40, frame.height > 0 {
                verticalDivider = candidate
                break
            }
        }
        guard let verticalDivider else {
            XCTFail("应找到上下 pane 的横向 divider")
            return
        }

        let start = verticalDivider.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        let end = verticalDivider.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.75))
        start.press(forDuration: 0.1, thenDragTo: end)
        settle(window)

        waitStatusContains(status, "panes: 3", timeout: 5)
        assertThreePaneLayoutReasonable()
        echoAndVerifyVisible("VERTICAL_DRAG_IO_OK")
    }

    /// Ctrl-C / Ctrl-L 走真实终端控制字节，不应变成字面 x03/x0c，也不能
    /// 让 GUI 失去连接；这是 shell 基础输入回归。
    func testControlKeysReachShellWithoutLiteralEscapes() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        app.typeText("cat\r")
        RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        app.typeKey("c", modifierFlags: .control)
        assertAnyTerminalContains("^C", timeout: 8)

        app.typeKey("l", modifierFlags: .control)
        waitStatusContains(status, "connected", timeout: 5)
        echoAndVerifyVisible("CONTROL_KEYS_OK")
        let output = (app.descendants(matching: .any)["muxterm.outputSnippet"].value as? String) ?? ""
        XCTAssertFalse(output.contains("x03"), "Ctrl-C 不应显示成字面 x03")
        XCTAssertFalse(output.contains("x0c"), "Ctrl-L 不应显示成字面 x0c")
    }

    /// Backspace 必须真的进入 shell 的行编辑器；只做 Swift 侧字节映射
    /// 测试不够，因为 SwiftTerm/AppKit 可能在窗口事件层吞掉 Delete。
    func testBackspaceDeletesCharacterInShell() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        let marker = "BACKSPACE_REAL_74291"
        // 先输入 BAD，再用真实 Delete 删除最后三个字符，最后补 OK。
        app.typeText("printf '\(marker)_BAD")
        for _ in 0..<3 {
            app.typeKey(XCUIKeyboardKey.delete.rawValue, modifierFlags: [])
        }
        app.typeText("OK\\n'")
        app.typeKey(XCUIKeyboardKey.return.rawValue, modifierFlags: [])

        assertAnyTerminalContains("\(marker)_OK", timeout: 10)
    }

    /// 真实 GUI 操作延迟预算：Cmd+D / Cmd+T 都应在 2 秒内反映到状态栏。
    func testSplitAndNewTabFeedbackLatencyBudget() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        let splitStarted = Date()
        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 2)
        XCTAssertLessThan(Date().timeIntervalSince(splitStarted), 2.0)

        let tabStarted = Date()
        app.typeKey("t", modifierFlags: .command)
        waitStatusContains(status, "tabs: 2", timeout: 2)
        XCTAssertLessThan(Date().timeIntervalSince(tabStarted), 2.0)
    }

    // MARK: - Ctrl+D / EOF

    /// Ctrl+D 只能结束当前前台程序，不能直接 kill pane。cat 收到 EOF
    /// 后退出，但同一个 shell 仍在，因此 pane 数和后续 shell I/O 都应保持。
    func testCtrlDExitsForegroundProcessButKeepsPaneAlive() throws {
        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 5)

        app.typeKey("d", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        settle(window)

        app.typeText("cat\r")
        RunLoop.current.run(until: Date().addingTimeInterval(0.4))
        app.typeKey("d", modifierFlags: .control)

        waitStatusContains(status, "panes: 2", timeout: 5)
        echoAndVerifyVisible("FOREGROUND_EOF_SHELL_ALIVE")
    }

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

    /// Cmd+Shift+P 必须打开 VSCode 风格命令面板；local shell 不应伪造
    /// tmux detach 命令，避免用户误以为当前 local pane 可被分离。
    func testCommandPaletteOpensAndLocalHidesTmuxDetach() throws {
        let window = waitMainWindow()
        window.click()
        waitStatusContains(statusBar(), "connected", timeout: 5)

        app.typeKey("p", modifierFlags: [.command, .shift])
        // SearchField 的 accessibility tree 还会把内部 Search 按钮暴露成
        // 同一个 identifier；限定类型，避免 `descendants(.any)` 产生多匹配。
        let input = app.dialogs["command_palette"].searchFields["muxterm.commandPalette.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        input.typeText("detach")

        let list = app.descendants(matching: .any)["muxterm.commandPalette.list"]
        XCTAssertTrue(list.waitForExistence(timeout: 5))
        XCTAssertEqual(list.cells.count, 0, "local 模式不应显示 tmux detach")
        app.typeKey(XCUIKeyboardKey.escape.rawValue, modifierFlags: [])
    }

    /// 真实 tmux control-client 场景：
    ///
    /// 1. attach 到预先创建的 2-tab（首 tab 2-pane）session；
    /// 2. 命令面板搜索并执行 detach；
    /// 3. GUI 窗口退出，但 session、tab/pane 拓扑和 shell 输出仍在；
    /// 4. 再次启动 GUI attach，验证布局和历史画面恢复。
    ///
    /// 这个用例覆盖 local 冒烟测试覆盖不到的完整链路：Swift 命令面板 →
    /// CoreBridge FFI → Task::Detach → tmux detach-client → 再 attach/render。
    func testTmuxCommandPaletteDetachKeepsLayoutAndReattaches() throws {
        try XCTSkipUnless(tmuxAvailable(), "tmux 不可用，跳过真实 tmux UI 测试")

        let suffix = "\(ProcessInfo.processInfo.processIdentifier)-\(Int(Date().timeIntervalSince1970))"
        let socket = "muxterm-ui-detach-\(suffix)"
        let session = "ui_detach"
        let marker = "UI_DETACH_REATTACH_74291"
        try createTmuxScenario(socket: socket, session: session, marker: marker)
        defer { _ = runTmux(socket: socket, args: ["kill-server"]) }

        // setUp 已启动 local shell；切换到明确的 tmux -L/-s 启动参数。
        app.terminate()
        app = makeApplication()
        app.launchArguments = ["-L", socket, "-s", session]
        app.launchEnvironment["MUXTERM_UITEST"] = "1"
        app.launch()
        app.activate()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 15))

        let window = waitMainWindow()
        let status = statusBar()
        window.click()
        waitStatusContains(status, "connected", timeout: 8)
        waitStatusContains(status, "tabs: 2", timeout: 8)

        // Cmd+1 回到预置的双 pane tab，验证 attach 后不只显示 active 的单 pane tab。
        app.typeKey("1", modifierFlags: .command)
        waitStatusContains(status, "panes: 2", timeout: 8)
        assertAnyTerminalContains(marker, timeout: 10)

        app.typeKey("p", modifierFlags: [.command, .shift])
        let palette = app.dialogs["command_palette"]
        let input = palette.searchFields["muxterm.commandPalette.input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        input.typeText("detach")
        let list = palette.tables["muxterm.commandPalette.list"]
        XCTAssertTrue(list.waitForExistence(timeout: 5))
        XCTAssertEqual(list.cells.count, 1, "tmux 模式搜索 detach 应恰好命中一个命令")
        app.typeKey(XCUIKeyboardKey.return.rawValue, modifierFlags: [])

        let disappeared = NSPredicate(format: "exists == false")
        let closed = XCTNSPredicateExpectation(predicate: disappeared, object: window)
        XCTAssertEqual(XCTWaiter.wait(for: [closed], timeout: 8), .completed)

        // detach 只能断开 GUI control client；原生 tmux 仍必须可见同一个布局和 marker。
        XCTAssertTrue(
            waitUntil(timeout: 5) {
                self.tmuxHasSession(socket: socket, session: session)
                    && self.tmuxPaneCount(
                        socket: socket,
                        target: self.tmuxFirstWindowTarget(socket: socket, session: session)
                    ) == 2
                    && self.tmuxCapture(
                        socket: socket,
                        target: self.tmuxFirstWindowTarget(socket: socket, session: session)
                    ).contains(marker)
            },
            "detach 后 tmux session/layout/shell 输出必须保留"
        )

        // 第二次启动是真实 re-attach，不依赖旧 GUI 的内存快照。
        app = makeApplication()
        app.launchArguments = ["-L", socket, "-s", session]
        app.launchEnvironment["MUXTERM_UITEST"] = "1"
        app.launch()
        app.activate()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 15))
        let reattachedWindow = waitMainWindow()
        let reattachedStatus = statusBar()
        reattachedWindow.click()
        waitStatusContains(reattachedStatus, "connected", timeout: 8)
        app.typeKey("1", modifierFlags: .command)
        waitStatusContains(reattachedStatus, "panes: 2", timeout: 8)
        assertAnyTerminalContains(marker, timeout: 10)
    }

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
            let matches = last.localizedCaseInsensitiveContains(text)
                || (text == "connected" && last.contains("已连接"))
            if matches { return }
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

    // MARK: - 真实 tmux UI 场景辅助

    private func tmuxAvailable() -> Bool {
        runProcess(executable: tmuxExecutable(), arguments: ["-V"]).status == 0
    }

    @discardableResult
    private func runTmux(socket: String, args: [String]) -> ProcessResult {
        runProcess(executable: tmuxExecutable(), arguments: ["-L", socket] + args)
    }

    private func tmuxExecutable() -> String {
        let candidates = [
            "/opt/homebrew/bin/tmux",
            "/usr/local/bin/tmux",
            "/usr/bin/tmux",
        ]
        return candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) ?? "tmux"
    }

    private func createTmuxScenario(socket: String, session: String, marker: String) throws {
        let created = runTmux(
            socket: socket,
            args: ["new-session", "-d", "-s", session, "-x", "100", "-y", "30"]
        )
        guard created.status == 0 else {
            throw XCTSkip("无法创建独立 tmux session: \(created.output)")
        }
        let split = runTmux(socket: socket, args: ["split-window", "-h", "-t", session])
        XCTAssertEqual(split.status, 0, "预置 tmux 首 tab 双 pane 失败: \(split.output)")
        let output = runTmux(
            socket: socket,
            args: [
                "send-keys",
                "-t",
                session,
                "printf '\(marker)\\n'",
                "Enter",
            ]
        )
        XCTAssertEqual(output.status, 0, "预置 shell 输出失败: \(output.output)")
        let newWindow = runTmux(socket: socket, args: ["new-window", "-t", session])
        XCTAssertEqual(newWindow.status, 0, "预置第二个 tab 失败: \(newWindow.output)")
    }

    private func tmuxHasSession(socket: String, session: String) -> Bool {
        runTmux(socket: socket, args: ["has-session", "-t", session]).status == 0
    }

    private func tmuxFirstWindowTarget(socket: String, session: String) -> String {
        let result = runTmux(
            socket: socket,
            args: ["list-windows", "-t", session, "-F", "#{window_id}"]
        )
        return result.output.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
    }

    private func tmuxPaneCount(socket: String, target: String) -> Int {
        guard !target.isEmpty else { return 0 }
        let result = runTmux(
            socket: socket,
            args: ["list-panes", "-t", target, "-F", "#{pane_id}"]
        )
        guard result.status == 0 else { return 0 }
        return result.output.split(whereSeparator: \.isNewline).count
    }

    private func tmuxCapture(socket: String, target: String) -> String {
        runTmux(socket: socket, args: ["capture-pane", "-p", "-t", target]).output
    }

    private func waitUntil(timeout: TimeInterval, condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
        }
        return condition()
    }

    private struct ProcessResult {
        let status: Int32
        let output: String
    }

    private func runProcess(executable: String, arguments: [String]) -> ProcessResult {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return ProcessResult(status: -1, output: error.localizedDescription)
        }
        let output = String(
            data: pipe.fileHandleForReading.readDataToEndOfFile(),
            encoding: .utf8
        ) ?? ""
        return ProcessResult(status: process.terminationStatus, output: output)
    }

    private func makeApplication() -> XCUIApplication {
        // 显式指定时才用外部 .app；xcodebuild test 必须用 scheme host，否则会 Running Background
        if let path = ProcessInfo.processInfo.environment["MUXTERM_APP_PATH"], !path.isEmpty {
            return XCUIApplication(url: URL(fileURLWithPath: path))
        }
        return XCUIApplication()
    }
}
