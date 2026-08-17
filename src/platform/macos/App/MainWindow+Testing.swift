import AppKit
import Foundation
import MuxtermChrome

/// in-process e2e 钩子（对标 Linux `AppWindow::test_*`）。不要删。
/// 兼容别名：旧测试仍访问 `app.searchPanel` / `app.attentionPanel`，
/// 现在都指向同一个统一面板（面板内靠 tab 区分）。
extension MainWindowController {
    var searchPanel: UnifiedPanelController { unifiedPanel }
    var attentionPanel: UnifiedPanelController { unifiedPanel }

    func testPollOnce() {
        pollOnce()
    }

    func testPollOutputEventCount() -> Int {
        pollOnce()
        return lastPaneOutputEventCount
    }

    func testFlushFeeds() {
        terminalManager.testFlushFeeds()
        content.layoutSubtreeIfNeeded()
        window?.layoutIfNeeded()
    }

    func testTabAndPaneCounts() -> (tabs: Int, panes: Int) {
        (lastSnapshot.tabs.count, lastSnapshot.panes.count)
    }

    func testTabIDs() -> [UInt32] {
        lastSnapshot.tabs.map(\.id)
    }

    func testActiveTabID() -> UInt32 {
        lastSnapshot.tabs.first(where: \.isActive)?.id ?? lastSnapshot.activeTab
    }

    func testActivePaneID() -> UInt32 {
        lastSnapshot.panes.first(where: \.isActive)?.id
            ?? lastSnapshot.panes.first?.id
            ?? 0
    }

    func testLayoutLeafIDs() -> [UInt32] {
        content.paneLayout.testLeafPaneIDs()
    }

    func testPaneAllocation(_ paneId: UInt32) -> NSSize {
        content.paneLayout.testPaneAllocation(paneId)
    }

    func testPaneTerminalText(_ paneId: UInt32) -> String {
        terminalManager.view(for: paneId).visibleScreenText()
    }

    func testActivePaneTerminalText() -> String {
        testPaneTerminalText(testActivePaneID())
    }

    func testAllVisibleTerminalText() -> String {
        testLayoutLeafIDs().map { testPaneTerminalText($0) }.joined(separator: "\n")
    }

    func testSearchAll(_ query: String) -> [(tabId: UInt32, paneId: UInt32, line: String)] {
        guard let json = bridge.searchAllJSON(query: query),
              let data = json.data(using: .utf8),
              let snapshot = SearchSnapshot.decode(data)
        else {
            return []
        }
        return snapshot.hits.map { ($0.tabId, $0.paneId, $0.line) }
    }

    func testOpenSearchPanel() {
        unifiedPanel.present(initial: .search)
    }

    func testSearchPanelOpen() -> Bool {
        unifiedPanel.testIsPresented()
    }

    func testSetSearchQuery(_ query: String) {
        unifiedPanel.testSetQuery(query)
    }

    func testActivateFirstSearchHit() {
        unifiedPanel.testActivateFirstHit()
    }

    func testSearchHitCount() -> Int {
        unifiedPanel.testHitCount()
    }

    func testOpenAttentionPanel() {
        unifiedPanel.present(initial: .attention)
    }

    func testAttentionPanelOpen() -> Bool {
        unifiedPanel.testIsPresented()
    }

    func testAttentionRowCount() -> Int {
        unifiedPanel.testRowCount()
    }

    func testBlockedCount() -> Int {
        guard let json = bridge.attentionSnapshotJSON(),
              let data = json.data(using: .utf8),
              let snapshot = AttentionSnapshot.decode(data)
        else {
            return 0
        }
        return snapshot.blockedCount
    }

    func testDoneCount() -> Int {
        guard let json = bridge.attentionSnapshotJSON(),
              let data = json.data(using: .utf8),
              let snapshot = AttentionSnapshot.decode(data)
        else {
            return 0
        }
        return snapshot.workspaces.reduce(0) { $0 + $1.done }
    }

    func testNotificationsRecorded() -> [String] {
        recordedNotifications
    }

    func testSwitchPane(_ paneId: UInt32) {
        _ = bridge.execute(task: MuxTask.switchPane(paneId))
        needsLayoutReloadForTest()
        // 等 STATE_ACTIVE_PANE_CHANGED 到达并反映到快照，避免 testSendInput
        // 打到旧 active pane。
        let deadline = Date().addingTimeInterval(5)
        while Date() < deadline {
            pollOnce()
            if testActivePaneID() == paneId {
                break
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.03))
        }
    }

    func testSwitchTab(_ tabId: UInt32) {
        requestSwitchTab(tabId)
        pollOnce()
    }

    func testSendInput(_ data: Data) {
        // W19-E：reply overlay 可见时输入走 overlay 的 pane，否则走 active pane。
        if !content.replyOverlayContainer.isHidden, let paneId = replyOverlayPaneId {
            _ = bridge.sendInputQuiet(paneId: paneId, data: data)
        } else {
            _ = bridge.sendInput(paneId: testActivePaneID(), data: data)
        }
        pollOnce()
    }

    func testSetPaneViewport(_ offset: UInt32) {
        let pane = testActivePaneID()
        _ = bridge.setPaneViewport(paneId: pane, offset: offset)
        // 先 poll 同步布局/尺寸，再喂滚动 ANSI，避免布局重建清掉内容。
        pollOnce()
        applyPaneViewport(paneId: pane, offset: offset)
        content.setJumpLatestVisible(bridge.paneViewport(paneId: pane) > 0)
    }

    func testPaneViewport() -> UInt32 {
        UInt32(max(0, bridge.paneViewport(paneId: testActivePaneID())))
    }

    func testClickJumpLatest() {
        jumpToLatest()
        pollOnce()
    }

    func testJumpLatestVisible() -> Bool {
        !content.jumpLatestButton.isHidden
    }

    func testDisconnectOverlayVisible() -> Bool {
        !content.disconnectOverlay.isHidden
    }

    func testWindowVisible() -> Bool {
        window?.isVisible == true && !isClosing
    }

    func testClickStatusTab(_ tabId: UInt32) {
        content.statusBar.testClickTab(tabId)
        pollOnce()
    }

    func testClickStatusDot() {
        content.statusBar.testClickStatusDot()
    }

    func testStatusDotSize() -> NSSize {
        content.statusBar.testStatusDotSize()
    }

    func testPopoverVisible() -> Bool {
        content.statusBar.testPopoverVisible()
    }

    func testPopoverText() -> String {
        content.statusBar.testPopoverText()
    }

    func testTabTitle(_ tabId: UInt32) -> String {
        content.statusBar.testTabTitle(tabId)
    }

    func testConnectTarget(_ config: TargetConfig) {
        connect(config: config)
    }

    func testTogglePaneFullscreen() {
        toggleActivePaneFullscreen()
        pollOnce()
    }

    /// 走真实 `handleKey`（Cmd-Enter 等），不是直接调 fullscreen 函数。
    @discardableResult
    func testDispatchKeyEvent(_ event: NSEvent) -> Bool {
        handleKey(event)
    }

    func testMakeCmdEnterEvent() -> NSEvent? {
        guard let window else { return nil }
        return NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: .command,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber,
            context: nil,
            characters: "\r",
            charactersIgnoringModifiers: "\r",
            isARepeat: false,
            keyCode: 36
        )
    }

    func testMakeTabEvent(shift: Bool) -> NSEvent? {
        guard let window else { return nil }
        var flags: NSEvent.ModifierFlags = []
        if shift { flags.insert(.shift) }
        return NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: flags,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber,
            context: nil,
            characters: "\t",
            charactersIgnoringModifiers: "\t",
            isARepeat: false,
            keyCode: 48
        )
    }

    func testAttentionProcessNames() -> [UInt32: String] {
        guard let json = bridge.attentionSnapshotJSON(),
              let data = json.data(using: .utf8),
              let snapshot = AttentionSnapshot.decode(data)
        else {
            return [:]
        }
        var out: [UInt32: String] = [:]
        for ws in snapshot.workspaces {
            for pane in ws.panes {
                if let name = pane.processName, !name.isEmpty {
                    out[pane.paneId] = name
                }
            }
        }
        return out
    }

    func testThemeHexColors() -> (fg: String, bg: String)? {
        terminalManager.themeHexColors()
    }

    func testActiveCaretFrame() -> CGRect {
        terminalManager.view(for: testActivePaneID()).caretFrame
    }

    func testBecameVisible(_ paneId: UInt32) {
        bridge.attentionOnBecameVisible(paneId: paneId)
        pollOnce()
    }

    func testOpenCommandPalette() {
        openCommandPalette()
    }

    func testPaletteIsPresented() -> Bool {
        commandPalette.testIsPresented()
    }

    func testPaletteTitles() -> [String] {
        commandPalette.testVisibleTitles()
    }

    func testSelectPaletteTitle(_ needle: String) {
        commandPalette.testSelect(matching: needle)
    }

    func testToggleTheme() {
        toggleTheme()
    }

    func testSavedTheme() -> String? {
        UserDefaults.standard.string(forKey: "muxterm.theme")
    }

    func testChromeAppearanceIsDark() -> Bool {
        let appearance = window?.effectiveAppearance ?? NSApp.effectiveAppearance
        return appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    }

    func testShowLocalSessions() {
        showSessions(for: .local)
    }

    func testView(identifier: String) -> NSView? {
        for item in NSApp.windows where item.isVisible {
            if let found = Self.findView(item.contentView, identifier: identifier) {
                return found
            }
        }
        return nil
    }

    func testConnectProgressVisible() -> Bool {
        guard let view = testView(identifier: ConnectProgress.identifier) else {
            return false
        }
        return !view.isHidden && view.alphaValue > 0
    }

    func testConnectProgressValue() -> String {
        let view = testView(identifier: ConnectProgress.identifier)
        return (view?.accessibilityValue() as? String) ?? view?.toolTip ?? ""
    }

    func testReplyOverlayVisible() -> Bool {
        guard let view = testView(identifier: CmdEnterRouting.overlayIdentifier) else {
            return false
        }
        return !view.isHidden && view.alphaValue > 0
    }

    func testReplyOverlayText() -> String {
        guard let term = testView(identifier: CmdEnterRouting.overlayIdentifier) as? MuxTerminalView else {
            return ""
        }
        return term.visibleScreenText()
    }

    func testMakeReturnEvent() -> NSEvent? {
        guard let window else { return nil }
        return NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [],
            timestamp: 0,
            windowNumber: window.windowNumber,
            context: nil,
            characters: "\r",
            charactersIgnoringModifiers: "\r",
            isARepeat: false,
            keyCode: 36
        )
    }

    func testMakeArrowEvent(down: Bool) -> NSEvent? {
        guard let window else { return nil }
        return NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [],
            timestamp: 0,
            windowNumber: window.windowNumber,
            context: nil,
            characters: "",
            charactersIgnoringModifiers: "",
            isARepeat: false,
            keyCode: down ? 125 : 126
        )
    }

    func testStatusRightWidth() -> CGFloat {
        content.statusBar.testStatusRightWidth()
    }

    func testTabButtonWidths() -> [CGFloat] {
        content.statusBar.testTabButtonWidths()
    }

    func testChromeMinX() -> CGFloat {
        content.statusBar.testChromeMinX()
    }

    /// 重排 tab = tmux `move-window`（iTerm2 moveTabAtIndex）。
    /// 直接对隔离 socket 调 tmux：控制 client 对 window 创建类命令有
    /// 已知问题（命令返回成功但外部看不到新 window），子进程路径与
    /// core discovery 一致且可验证 tmux 状态。
    func testReorderTab(from fromId: UInt32, toIndex: Int) {
        guard let socket = bridge.socket else { return }
        let args = TmuxWindowCommands.moveWindowArgs(
            fromIndex: Int(fromId),
            toIndex: toIndex
        )
        _ = runIsolatedTmux(socket: socket, args: args)
        // 控制 client 感知外部变更：强制重查 window/pane 列表。
        _ = bridge.execute(task: MuxTask.refreshTabs())
        pollOnce()
    }

    /// 把 pane 拆成新 tab = tmux `break-pane`（iTerm2 breakOutWindowPane）。
    func testBreakActivePaneToNewTab() {
        let pane = testActivePaneID()
        guard let socket = bridge.socket else { return }
        let args = TmuxWindowCommands.breakPaneArgs(pane: "%\(pane)")
        _ = runIsolatedTmux(socket: socket, args: args)
        _ = bridge.execute(task: MuxTask.refreshTabs())
        pollOnce()
    }

    /// 只对隔离 `-L muxterm-test-*` socket 执行 tmux（与夹具同一纪律）。
    @discardableResult
    private func runIsolatedTmux(socket: String, args: [String]) -> Int32 {
        let candidates = [
            "/opt/homebrew/bin/tmux",
            "/usr/local/bin/tmux",
            "/usr/bin/tmux",
        ]
        let bin = candidates.first {
            FileManager.default.isExecutableFile(atPath: $0)
        } ?? "/usr/bin/tmux"
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: bin)
        proc.arguments = ["-L", socket] + args
        proc.standardOutput = FileHandle.nullDevice
        proc.standardError = FileHandle.nullDevice
        do {
            try proc.run()
            proc.waitUntilExit()
        } catch {
            return -1
        }
        return proc.terminationStatus
    }

    private static func findView(_ root: NSView?, identifier: String) -> NSView? {
        guard let root else { return nil }
        if root.accessibilityIdentifier() == identifier {
            return root
        }
        for child in root.subviews {
            if let found = findView(child, identifier: identifier) {
                return found
            }
        }
        return nil
    }

    private func needsLayoutReloadForTest() {
        // switchPane 后下一轮 poll 会 refreshUI；这里先把快照里的 active 标上。
    }
}
