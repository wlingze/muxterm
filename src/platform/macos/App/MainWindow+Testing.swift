import AppKit
import CoreGraphics
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

    func testSetSidebarOpen(_ open: Bool) {
        setWorkspaceSidebarOpenForTest(open)
    }

    func testSidebarOpen() -> Bool {
        workspaceSidebarOpenForTest()
    }

    func testSidebarWorkspaceNames() -> [String] {
        workspaceSidebar.testWorkspaceNames()
    }

    func testSidebarAgentCount() -> Int {
        workspaceSidebar.testAgentCount()
    }

    func testSidebarAgentIndicators() -> [AgentSidebarIndicator] {
        workspaceSidebar.testAgentIndicators()
    }

    func testPollOutputEventCount() -> Int {
        pollOnce()
        return lastPaneOutputEventCount
    }

    func testStatusSubscriptionActive() -> Bool {
        bridge.statusSubscriptionActive()
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

    func testPaneSurfaceReady(_ paneId: UInt32) -> Bool {
        terminalManager.isSurfaceReady(for: paneId)
            && content.paneLayout.testPaneSurfaceVisible(paneId)
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
        unifiedPanel.present(initial: .search, scope: .workspace)
    }

    func testOpenGlobalSearchPanel() {
        unifiedPanel.present(initial: .search, scope: .all)
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

    /// 通过生产 Core task 创建 tab；测试不得直接操作 tmux socket。
    func testNewTab() {
        newTab()
        pollOnce()
    }

    func testSplitHorizontal() {
        splitHorizontal()
        pollOnce()
    }

    /// 键盘所在的 SwiftTerm pane。host 边框高亮不算。
    func testFocusedTerminalPaneID() -> UInt32? {
        (window?.firstResponder as? MuxTerminalView)?.paneId
    }

    func testFocusTargetPaneID() -> UInt32? {
        terminalManager.focusTarget?.paneId
    }

    func testFirstResponderIsTerminalView() -> Bool {
        window?.firstResponder is MuxTerminalView
    }

    func testFirstResponderIsPaneHost() -> Bool {
        window?.firstResponder is PaneHostView
    }

    func testRestoreTerminalFocus() {
        restoreTerminalFocusIfAllowed()
    }

    func testMakeActiveTerminalFirstResponder() {
        let pane = testActivePaneID()
        let view = terminalManager.view(for: pane)
        window?.makeFirstResponder(view)
    }

    func testActiveTerminalView() -> MuxTerminalView {
        terminalManager.view(for: testActivePaneID())
    }

    func testRouteMonitoredKeyEvent(_ event: NSEvent) -> NSEvent? {
        routeMonitoredKeyEvent(event)
    }

    /// 通过生产 Core task 关闭 tab；测试不得直接 `kill-window`。
    func testCloseTab(_ tabId: UInt32) {
        closeTab(tabId)
        pollOnce()
    }

    func testHasCachedTab(_ tabId: UInt32) -> Bool {
        content.paneLayout.hasCachedTab(tabId)
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

    /// 触控板/native scrollback 生产路径：模拟 TerminalView 的滚轮。
    func testScrollHistory(deltaLines: Int) {
        let pane = testActivePaneID()
        pollOnce()
        terminalManager.scrollPaneHistory(paneId: pane, deltaLines: deltaLines)
        content.setJumpLatestVisible(bridge.paneViewport(paneId: pane) > 0)
    }

    /// 通过真实 NSWindow/AppKit 事件分发滚轮；与 `testScrollHistory` 不同，
    /// 这里必须经过 hit-test 和 SwiftTerm 的 `scrollWheel(with:)`。
    /// CGEvent 使用左上角原点，AppKit screen point 使用左下角原点，
    /// 因此发送前要做一次 y 轴转换。
    @discardableResult
    func testDispatchScrollWheel(deltaLines: Int32) -> Bool {
        guard let window else { return false }
        let pane = testActivePaneID()
        let terminal = terminalManager.view(for: pane)
        window.layoutIfNeeded()
        terminal.layoutSubtreeIfNeeded()
        guard terminal.bounds.width > 1, terminal.bounds.height > 1 else { return false }

        let screenPoint = terminal.convert(
            NSPoint(x: terminal.bounds.midX, y: terminal.bounds.midY),
            to: nil
        )
        guard let event = CGEvent(
            scrollWheelEvent2Source: CGEventSource(stateID: .hidSystemState),
            units: .line,
            wheelCount: 1,
            wheel1: deltaLines,
            wheel2: 0,
            wheel3: 0
        ) else {
            return false
        }
        event.setIntegerValueField(
            .mouseEventWindowUnderMousePointer,
            value: Int64(window.windowNumber)
        )
        event.setIntegerValueField(
            .mouseEventWindowUnderMousePointerThatCanHandleThisEvent,
            value: Int64(window.windowNumber)
        )
        let displayMaxY = NSScreen.screens
            .first(where: { $0.frame.contains(screenPoint) })?
            .frame
            .maxY ?? NSScreen.main?.frame.maxY ?? 0
        event.location = CGPoint(x: screenPoint.x, y: displayMaxY - screenPoint.y)
        guard let nsEvent = NSEvent(cgEvent: event) else { return false }
        window.sendEvent(nsEvent)
        return true
    }

    func testNativeScrollPosition() -> Double {
        terminalManager.view(for: testActivePaneID()).scrollPosition
    }

    func testNativeCanScroll() -> Bool {
        terminalManager.view(for: testActivePaneID()).canScroll
    }

    func testNativeHistoryCapacity() -> Int {
        terminalManager.view(for: testActivePaneID()).historyCapacity
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

    func testJumpLatestTitle() -> String {
        content.jumpLatestButton.title
    }

    func testLastSeenVisible() -> Bool {
        !content.lastSeenButton.isHidden
    }

    func testClickLastSeen() {
        content.lastSeenButton.performClick(nil)
    }

    func testCommandMarkVisible() -> (ok: Bool, fail: Bool) {
        (!content.commandMarkOKButton.isHidden, !content.commandMarkFailButton.isHidden)
    }

    /// 测试命令时间线的前后跳转（对应 Cmd+Option+↑/↓ 生产快捷键）。
    func testPreviousCommand() {
        jumpToPreviousCommand()
        pollOnce()
    }

    func testNextCommand() {
        jumpToNextCommand()
        pollOnce()
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

    /// 走 Unified Panel Existing Connections 的生产 attach-only 路径。
    /// 测试必须传入隔离 tmux socket，禁止落到用户默认 server。
    func testAttachExistingConnection(_ choice: ExistingConnectionChoice) {
        unifiedPanel.onAttachExistingConnection?(choice)
    }

    func testActiveWorkspaceSession() -> String? {
        bridge.session
    }

    func testWorkspaceCount() -> Int {
        workspaceSidebar.testWorkspaceCount()
    }

    func testWorkspaceIDs() -> [String] {
        workspaceSidebar.testWorkspaceIDs()
    }

    func testWorkspaceNames() -> [String] {
        workspaceSidebar.testWorkspaceNames()
    }

    func refreshWorkspaceSidebarForTest() {
        refreshWorkspaceSidebar(force: true)
    }

    func testCloseWorkspace(workspaceId: String) {
        closeWorkspace(workspaceId)
    }

    func testSwitchBackToFirstWorkspace() {
        switchToWorkspaceAtFixedIndex(1)
    }

    func testSwitchToWorkspaceAtFixedIndex(_ oneBased: Int) {
        switchToWorkspaceAtFixedIndex(oneBased)
    }

    func testPerformWhenForegroundReady(_ action: @escaping () -> Void) {
        performWhenForegroundReady(action)
    }

    /// 在指定固定顺序 Workspace 的 bridge 上制造可控的后台锁竞争。
    /// 返回信号量只会在后台线程已经持锁后 signal，E2E 可以精确验证
    /// 点击切换没有等待这段 FFI。
    @discardableResult
    func testHoldWorkspaceBridgeAtFixedIndex(
        _ oneBased: Int,
        duration: TimeInterval
    ) -> DispatchSemaphore? {
        let ordered = workspaceSidebarFixedSlots()
        guard ordered.indices.contains(oneBased - 1) else { return nil }
        return ordered[oneBased - 1].holdBridgeForTesting(duration)
    }

    func testForegroundActivationPending() -> Bool {
        foregroundActivationIsPending
    }

    func testWindowClosing() -> Bool {
        isClosing
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

    /// 构造真实命令时间线快捷键事件；生产路径仍由 `handleKey` 解析
    /// keyCode + Cmd/Option，而不是测试直接调用跳转方法。
    func testMakeCommandTimelineEvent(up: Bool) -> NSEvent? {
        guard let window else { return nil }
        let keyCode: UInt16 = up ? 126 : 125
        let arrow = up ? "\u{F700}" : "\u{F701}"
        return NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [.command, .option],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber,
            context: nil,
            characters: arrow,
            charactersIgnoringModifiers: arrow,
            isARepeat: false,
            keyCode: keyCode
        )
    }

    /// 构造带修饰键的普通字符事件，走生产 `handleKey`。
    func testMakeKeyEvent(
        key: String,
        keyCode: UInt16,
        command: Bool = false,
        control: Bool = false,
        option: Bool = false,
        shift: Bool = false
    ) -> NSEvent? {
        guard let window else { return nil }
        var flags: NSEvent.ModifierFlags = []
        if command { flags.insert(.command) }
        if control { flags.insert(.control) }
        if option { flags.insert(.option) }
        if shift { flags.insert(.shift) }
        return NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: flags,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber,
            context: nil,
            characters: key,
            charactersIgnoringModifiers: key,
            isARepeat: false,
            keyCode: keyCode
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

    func testLastPaletteError() -> String? {
        lastPaletteError
    }

    func testLastPaletteSelection() -> String? {
        lastPaletteSelection
    }

    func testSelectPaletteTitle(_ needle: String) {
        commandPalette.testSelect(matching: needle)
    }

    func testToggleTheme() {
        toggleTheme()
    }

    func testTerminalFontSize() -> CGFloat {
        terminalManager.view(for: testActivePaneID()).fontSize
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
    /// E2E 必须走与生产 UI 相同的 FFI/Core Task，禁止测试直调 tmux。
    func testReorderTab(from fromId: UInt32, target targetId: UInt32, before: Bool) {
        content.statusBar.testMoveTab(from: fromId, target: targetId, before: before)
        pollOnce()
    }

    /// 把 pane 拆成新 tab = tmux `break-pane`（iTerm2 breakOutWindowPane）。
    func testBreakActivePaneToNewTab() {
        let pane = testActivePaneID()
        content.paneLayout.testMovePaneToNewTab(pane)
        pollOnce()
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
