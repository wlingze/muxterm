import AppKit

/// 主窗口：持有 CoreBridge + Timer 轮询 `muxterm_poll_events`，分发到 UI。
final class MainWindowController: NSWindowController, NSWindowDelegate {
    private let bridge: CoreBridge
    private let terminalManager: TerminalManager
    private let content: ContentView
    private var pollTimer: Timer?
    private var lastSnapshot = FrameSnapshot()
    private var needsLayoutReload = true
    private var isClosing = false

    init(bridge: CoreBridge) {
        self.bridge = bridge
        self.terminalManager = TerminalManager(bridge: bridge)
        self.content = ContentView(terminalManager: terminalManager)

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 640),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Muxterm"
        window.titleVisibility = .visible
        window.minSize = NSSize(width: 480, height: 320)
        window.center()
        window.contentView = content
        window.setAccessibilityIdentifier("muxterm.mainWindow")

        super.init(window: window)
        window.delegate = self

        content.tabBar.onSelectTab = { [weak self] tabId in
            self?.bridge.execute(task: MuxTask.switchTab(tabId))
            self?.needsLayoutReload = true
            self?.refreshUI()
        }
        content.tabBar.onNewTab = { [weak self] in
            self?.newTab()
        }
        content.paneLayout.onActivatePane = { [weak self] paneId in
            guard let self else { return }
            self.bridge.execute(task: MuxTask.switchPane(paneId))
            self.refreshUI()
            self.focusActiveTerminal()
        }
        terminalManager.onOutputSnippetChanged = { [weak self] snippet in
            self?.content.statusBar.updateOutputSnippet(snippet)
        }

        installKeyEquivalents()
        startPolling()
        DispatchQueue.main.async { [weak self] in
            self?.refreshUI()
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    deinit {
        pollTimer?.invalidate()
        if !isClosing {
            bridge.shutdown()
        }
    }

    // MARK: - 公开动作（菜单 / 快捷键）

    @objc func newTab() {
        bridge.execute(task: MuxTask.newTab())
        needsLayoutReload = true
        refreshUI()
    }

    @objc func closeActivePane() {
        let pane = lastSnapshot.activePane
        guard pane != 0 else { return }
        // 唯一 pane 时关 pane 会触发后端关 window；UI 侧随后收到 Exited 再关窗口。
        bridge.execute(task: MuxTask.closePane(pane))
        needsLayoutReload = true
        refreshUI()
        maybeCloseIfSessionEnded()
    }

    @objc func closeActiveWindow() {
        closeSessionWindow()
    }

    /// 菜单 Cmd+1..9：tag 为 1-based 序号。
    @objc func switchTabByNumber(_ sender: Any?) {
        let n: Int
        if let item = sender as? NSMenuItem {
            n = item.tag
        } else if let num = sender as? NSNumber {
            n = num.intValue
        } else {
            return
        }
        switchToTabIndex(n)
    }

    /// 1-based 序号切换 tab。
    func switchToTabIndex(_ oneBased: Int) {
        guard oneBased >= 1, oneBased <= lastSnapshot.tabs.count else { return }
        let tabId = lastSnapshot.tabs[oneBased - 1].id
        bridge.execute(task: MuxTask.switchTab(tabId))
        needsLayoutReload = true
        refreshUI()
    }

    @objc func splitHorizontal() {
        splitActivePane(horizontal: true)
    }

    @objc func splitVertical() {
        splitActivePane(horizontal: false)
    }

    @objc func setTabBarTop(_ sender: Any?) {
        TabBarPosition.set(.top)
        content.applyTabBarPosition(.top)
    }

    @objc func setTabBarBottom(_ sender: Any?) {
        TabBarPosition.set(.bottom)
        content.applyTabBarPosition(.bottom)
    }

    @objc func nextPane() {
        bridge.execute(task: MuxTask.nextPane())
        needsLayoutReload = true
        refreshUI()
        focusActiveTerminal()
    }

    @objc func prevPane() {
        bridge.execute(task: MuxTask.prevPane())
        needsLayoutReload = true
        refreshUI()
        focusActiveTerminal()
    }

    private func focusActiveTerminal() {
        let snap = bridge.snapshot()
        guard snap.activePane != 0 else { return }
        let view = terminalManager.view(for: snap.activePane)
        terminalManager.focusTarget = view
        window?.makeFirstResponder(view)
        content.paneLayout.markActivePane(snap.activePane)
    }

    private func splitActivePane(horizontal: Bool) {
        let pane = lastSnapshot.activePane
        guard pane != 0 else { return }
        bridge.execute(task: MuxTask.splitPane(targetPane: pane, horizontal: horizontal))
        needsLayoutReload = true
        refreshUI()
        // 分割后立刻把焦点交回活跃终端，避免「看起来黑且键入无响应」
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let snap = self.bridge.snapshot()
            if snap.activePane != 0 {
                let view = self.terminalManager.view(for: snap.activePane)
                self.window?.makeFirstResponder(view)
                self.terminalManager.focusTarget = view
                _ = view.syncSizeToPty()
                view.forceRedraw()
            }
        }
    }

    // MARK: - 事件循环

    private func startPolling() {
        let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
            self?.pollOnce()
        }
        RunLoop.main.add(timer, forMode: .common)
        pollTimer = timer
    }

    private func pollOnce() {
        let events = bridge.pollEvents()
        var outputSeen = false
        var structureChanged = false
        for ev in events {
            if ev.isPaneOutput {
                terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
                outputSeen = true
            } else {
                structureChanged = true
                needsLayoutReload = true
            }
            if ev.isBackendStatus, ev.paneId == 4 {
                // pane_id 复用状态码：4 = exited
                closeSessionWindow()
                return
            }
            if ev.isPaneClosed || ev.isTabClosed {
                structureChanged = true
            }
        }
        if needsLayoutReload || structureChanged {
            refreshUI()
            maybeCloseIfSessionEnded()
        } else if outputSeen {
            content.statusBar.update(snapshot: lastSnapshot)
        }
    }

    private func refreshUI() {
        let snap = bridge.snapshot()
        lastSnapshot = snap
        content.tabBar.update(tabs: snap.tabs)
        if needsLayoutReload {
            content.paneLayout.apply(layout: snap.layout, panes: snap.panes)
            needsLayoutReload = false
        }
        content.statusBar.update(snapshot: snap)
        content.statusBar.updateOutputSnippet(terminalManager.recentOutputSnippet)

        if snap.activePane != 0 {
            let view = terminalManager.view(for: snap.activePane)
            terminalManager.focusTarget = view
            content.paneLayout.markActivePane(snap.activePane)
            if window?.firstResponder !== view {
                window?.makeFirstResponder(view)
            }
        }
    }

    /// session/window 已空时关闭 NSWindow。
    private func maybeCloseIfSessionEnded() {
        let snap = bridge.snapshot()
        if snap.tabs.isEmpty && snap.panes.isEmpty {
            closeSessionWindow()
        }
    }

    private func closeSessionWindow() {
        guard !isClosing else { return }
        isClosing = true
        pollTimer?.invalidate()
        pollTimer = nil
        bridge.shutdown()
        window?.close()
    }

    // MARK: - 快捷键

    private func installKeyEquivalents() {
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.handleKey(event) ? nil : event
        }
    }

    /// 返回 true 表示已消费事件。
    private func handleKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let ch = event.charactersIgnoringModifiers?.lowercased()

        // Cmd+T 新建 tab
        if flags.contains(.command), !flags.contains(.shift), ch == "t" {
            newTab()
            return true
        }
        // Cmd+D 水平分割 / Cmd+Shift+D 竖直分割
        if flags.contains(.command), !flags.contains(.shift), ch == "d" {
            splitActivePane(horizontal: true)
            return true
        }
        if flags.contains(.command), flags.contains(.shift), ch == "d" {
            splitActivePane(horizontal: false)
            return true
        }
        // Cmd+W 关闭 window
        if flags.contains(.command), !flags.contains(.shift), ch == "w" {
            closeActiveWindow()
            return true
        }
        // Cmd+1..9 切 tab（必须在 SwiftTerm interpretKeyEvents 之前吞掉，否则 noop:）
        if flags.contains(.command),
           !flags.contains(.option),
           let raw = event.charactersIgnoringModifiers?.first,
           let n = Int(String(raw)),
           (1...9).contains(n)
        {
            switchToTabIndex(n)
            return true
        }

        // Cmd+[ / Cmd+]：上一个 / 下一个 pane（焦点跟随）
        if flags.contains(.command), !flags.contains(.shift), ch == "[" {
            prevPane()
            return true
        }
        if flags.contains(.command), !flags.contains(.shift), ch == "]" {
            nextPane()
            return true
        }

        // Alt+T 新建 tab（兼容 TUI）
        if flags.contains(.option), !flags.contains(.command), ch == "t" {
            newTab()
            return true
        }
        // Alt+S / Alt+V：与 TUI 一致
        if flags.contains(.option), !flags.contains(.command), ch == "s" {
            splitActivePane(horizontal: true)
            return true
        }
        if flags.contains(.option), !flags.contains(.command), ch == "v" {
            splitActivePane(horizontal: false)
            return true
        }
        // Alt+[ / Alt+]：切 pane
        if flags.contains(.option), !flags.contains(.command), ch == "[" {
            prevPane()
            return true
        }
        if flags.contains(.option), !flags.contains(.command), ch == "]" {
            nextPane()
            return true
        }
        // Alt+1..9 切 tab
        if flags.contains(.option),
           !flags.contains(.command),
           let raw = event.charactersIgnoringModifiers?.first,
           let n = Int(String(raw)),
           (1...9).contains(n)
        {
            switchToTabIndex(n)
            return true
        }
        // Ctrl+Q 退出应用
        if flags.contains(.control), ch == "q" {
            NSApp.terminate(nil)
            return true
        }
        // Ctrl+D：关当前 pane（末 pane 关 tab / 末 tab 关 window）
        if flags.contains(.control), !flags.contains(.command), ch == "d" {
            closeActivePane()
            return true
        }
        return false
    }

    func windowWillClose(_ notification: Notification) {
        if !isClosing {
            isClosing = true
            pollTimer?.invalidate()
            pollTimer = nil
            bridge.shutdown()
        }
    }
}
