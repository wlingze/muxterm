import AppKit
import MuxtermChrome

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
        guard let raw = event.charactersIgnoringModifiers, let key = raw.first.map(String.init) else {
            return false
        }
        let chord = KeyChord(
            command: flags.contains(.command),
            shift: flags.contains(.shift),
            option: flags.contains(.option),
            control: flags.contains(.control),
            key: key
        )
        guard let action = KeyBindings.action(for: chord) else { return false }
        switch action {
        case .newTab:
            newTab()
        case .splitHorizontal:
            splitActivePane(horizontal: true)
        case .splitVertical:
            splitActivePane(horizontal: false)
        case .closeWindow:
            closeActiveWindow()
        case .closePane:
            closeActivePane()
        case .switchTab(let n):
            switchToTabIndex(n)
        case .nextPane:
            nextPane()
        case .prevPane:
            prevPane()
        case .quit:
            NSApp.terminate(nil)
        }
        return true
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
