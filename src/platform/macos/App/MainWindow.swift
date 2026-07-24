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
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "Muxterm"
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
            let view = self.terminalManager.view(for: paneId)
            self.window?.makeFirstResponder(view)
            self.terminalManager.focusTarget = view
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
        if flags.contains(.command), ch == "t" {
            newTab()
            return true
        }
        // Cmd+D 关闭 pane
        if flags.contains(.command), ch == "d" {
            closeActivePane()
            return true
        }
        // Cmd+W 关闭 window
        if flags.contains(.command), ch == "w" {
            closeActiveWindow()
            return true
        }

        // Alt+T 新建 tab（兼容 TUI）
        if flags.contains(.option), ch == "t" {
            newTab()
            return true
        }
        // Alt+1..9 切 tab
        if flags.contains(.option),
           let raw = event.charactersIgnoringModifiers?.first,
           let n = Int(String(raw)),
           (1...9).contains(n),
           n <= lastSnapshot.tabs.count
        {
            bridge.execute(task: MuxTask.switchTab(lastSnapshot.tabs[n - 1].id))
            needsLayoutReload = true
            refreshUI()
            return true
        }
        // Ctrl+Q 退出应用
        if flags.contains(.control), ch == "q" {
            NSApp.terminate(nil)
            return true
        }
        // Ctrl+D：交给终端（EOF）；若唯一 pane，后端 Exit 后 UI 关窗。
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
