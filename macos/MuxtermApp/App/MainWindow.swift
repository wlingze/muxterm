import AppKit

/// 主窗口：持有 CoreBridge + Timer 轮询 `muxterm_poll_events`，分发到 UI。
final class MainWindowController: NSWindowController, NSWindowDelegate {
    private let bridge: CoreBridge
    private let terminalManager: TerminalManager
    private let content: ContentView
    private var pollTimer: Timer?
    private var lastSnapshot = FrameSnapshot()
    private var needsLayoutReload = true

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

        super.init(window: window)
        window.delegate = self

        content.tabBar.onSelectTab = { [weak self] tabId in
            self?.bridge.execute(task: MuxTask.switchTab(tabId))
            self?.needsLayoutReload = true
            self?.refreshUI()
        }
        content.tabBar.onNewTab = { [weak self] in
            self?.bridge.execute(task: MuxTask.newTab())
            self?.needsLayoutReload = true
            self?.refreshUI()
        }
        content.paneLayout.onActivatePane = { [weak self] paneId in
            guard let self else { return }
            // 点击激活：聚焦对应终端视图
            let view = self.terminalManager.view(for: paneId)
            self.window?.makeFirstResponder(view)
            self.terminalManager.focusTarget = view
        }

        installKeyEquivalents()
        startPolling()
        // 初次快照
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
        bridge.shutdown()
    }

    // MARK: - 事件循环

    private func startPolling() {
        // ~10Hz 轮询 FFI 事件（与 TUI 的 100ms poll 对齐）
        let timer = Timer(timeInterval: 0.1, repeats: true) { [weak self] _ in
            self?.pollOnce()
        }
        RunLoop.main.add(timer, forMode: .common)
        pollTimer = timer
    }

    private func pollOnce() {
        let events = bridge.pollEvents()
        var outputSeen = false
        for ev in events {
            if ev.isPaneOutput {
                terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
                outputSeen = true
            } else {
                needsLayoutReload = true
            }
        }
        if needsLayoutReload || (!events.isEmpty && !outputSeen) {
            refreshUI()
        } else if outputSeen {
            // 仅更新状态栏（pane 数等可能不变）
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

        if snap.activePane != 0 {
            let view = terminalManager.view(for: snap.activePane)
            terminalManager.focusTarget = view
            if window?.firstResponder !== view {
                window?.makeFirstResponder(view)
            }
        }
    }

    // MARK: - 快捷键（窗口级）

    private func installKeyEquivalents() {
        // 菜单栏快捷键在 AppDelegate 安装；这里处理 Alt+数字等本地键。
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.handleKey(event) ? nil : event
        }
    }

    /// 返回 true 表示已消费事件。
    private func handleKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        // Alt+T 新建 tab
        if flags.contains(.option), event.charactersIgnoringModifiers == "t" {
            bridge.execute(task: MuxTask.newTab())
            needsLayoutReload = true
            refreshUI()
            return true
        }
        // Alt+1..9 切 tab
        if flags.contains(.option),
           let ch = event.charactersIgnoringModifiers?.first,
           let n = Int(String(ch)),
           (1...9).contains(n),
           n <= lastSnapshot.tabs.count
        {
            bridge.execute(task: MuxTask.switchTab(lastSnapshot.tabs[n - 1].id))
            needsLayoutReload = true
            refreshUI()
            return true
        }
        // Ctrl+Q 退出
        if flags.contains(.control), event.charactersIgnoringModifiers == "q" {
            NSApp.terminate(nil)
            return true
        }
        return false
    }

    func windowWillClose(_ notification: Notification) {
        pollTimer?.invalidate()
        pollTimer = nil
        bridge.shutdown()
    }
}
