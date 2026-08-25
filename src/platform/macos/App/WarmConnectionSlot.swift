import AppKit
import CMuxterm
import MuxtermChrome

/// ConnectionPool 的真实 slot：持有 CoreBridge + 独立 TerminalManager。
///
/// 切换目标时旧 slot 进入 background 继续 poll（保持 warm），不立即
/// shutdown；只有淘汰时才 evict。tmux/ssh 淘汰用 detach 保留远端
/// server/session，local shell 直接 shutdown（前端就是 PTY 模拟器）。
///
/// 后台线程只排空 FFI。`PaneOutput` / `PaneSnapshot` 必须 hop 回主线程
/// 再喂给 **这个** Workspace 自己的 TerminalManager。不能在
/// `muxterm.macos.background-poll` 上改 Swift Dictionary / SwiftTerm。
final class WarmConnectionSlot: ConnectionSlotProtocol {
    var key: ConnectionKey
    var targetConfig: TargetConfig
    let bridge: CoreBridge
    let terminalManager: TerminalManager
    private let stateLock = NSLock()
    private var lifecycleValue: ConnectionLifecycle = .background
    private var lastSnapshotValue = FrameSnapshot()
    private var pendingSurfaceEvents: [StateChange] = []
    private var pendingDrainedWhileBackground = false
    var lifecycle: ConnectionLifecycle {
        get {
            stateLock.lock()
            defer { stateLock.unlock() }
            return lifecycleValue
        }
        set {
            stateLock.lock()
            lifecycleValue = newValue
            stateLock.unlock()
        }
    }
    var lastUsedAt: UInt64

    /// 最近一次后台 poll 后的快照（只读缓存，避免后台刷新 UI）。
    var lastSnapshot: FrameSnapshot {
        stateLock.lock()
        defer { stateLock.unlock() }
        return lastSnapshotValue
    }

    init(
        key: ConnectionKey,
        bridge: CoreBridge,
        terminalManager: TerminalManager? = nil,
        now: UInt64
    ) {
        self.key = key
        self.targetConfig = key.targetConfig
        self.bridge = bridge
        self.terminalManager = terminalManager ?? TerminalManager(bridge: bridge)
        self.lastUsedAt = now
    }

    /// ConnectionPool 协议入口：后台只排空 FFI，不碰 Surface。
    func pollBackground() {
        _ = drainBackgroundEvents()
    }

    /// 在后台队列排空该 Workspace 的 core 事件。
    ///
    /// 只做 FFI：poll、Index/attention 进程名、缓存 snapshot。
    /// 返回是否有待主线程投递的 Surface 事件。
    @discardableResult
    func drainBackgroundEvents() -> Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard lifecycleValue == .background else { return false }
        let events = bridge.pollEvents()
        _ = bridge.takeError()
        var surface: [StateChange] = []
        for ev in events {
            if ev.type == STATE_STATUS_SUBSCRIPTION,
               ev.name.hasPrefix("muxterm.pane-cmd")
            {
                let value = String(data: ev.data, encoding: .utf8) ?? ""
                _ = bridge.attentionSetProcessName(
                    paneId: ev.paneId,
                    name: value.isEmpty ? nil : value
                )
            } else if ev.isPaneOutput || ev.isPaneSnapshot || ev.isPaneClosed {
                surface.append(ev)
            }
        }
        if !surface.isEmpty {
            pendingSurfaceEvents.append(contentsOf: surface)
            pendingDrainedWhileBackground = true
        }
        lastSnapshotValue = bridge.snapshot()
        return !pendingSurfaceEvents.isEmpty
    }

    /// 主线程：把后台排空的 PTY 喂给本 slot 的 Surface 树。
    ///
    /// 后台批次即使后来变成前台，也不得为从未打开的 pane 新建 widget。
    func applyPendingSurfaceEvents() {
        dispatchPrecondition(condition: .onQueue(.main))
        stateLock.lock()
        let events = pendingSurfaceEvents
        pendingSurfaceEvents.removeAll()
        let fromBackground = pendingDrainedWhileBackground
        pendingDrainedWhileBackground = false
        let alive = lifecycleValue != .evicting
        let nowActive = lifecycleValue == .active
        stateLock.unlock()
        guard alive else { return }

        if fromBackground {
            terminalManager.setViewCreationEnabled(false)
        }
        if !events.isEmpty {
            terminalManager.beginEventBatch()
            for ev in events {
                if ev.isPaneClosed {
                    terminalManager.removePane(ev.paneId)
                } else if ev.isPaneSnapshot {
                    terminalManager.handleSnapshot(paneId: ev.paneId, data: ev.data)
                } else if ev.isPaneOutput {
                    terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
                }
            }
            terminalManager.endEventBatch()
        }
        if nowActive {
            terminalManager.setViewCreationEnabled(true)
        }
    }

    /// 在 MainWindow 使用该 slot 的 bridge 前先切换生命周期。若后台 poll
    /// 已经开始，这里会等它释放锁，确保 Swift/FFI 不发生并发访问。
    func prepareForForeground() {
        stateLock.lock()
        lifecycleValue = .active
        stateLock.unlock()
        terminalManager.setViewCreationEnabled(true)
    }

    /// 在后台 slot 上安全访问 CoreBridge。后台 poll、前台激活和淘汰共用
    /// 同一把锁；不得把 bridge 引用带出闭包，否则 C ABI handle 可能并发。
    @discardableResult
    func withBridge<T>(_ body: (CoreBridge) -> T) -> T? {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard lifecycleValue != .evicting else { return nil }
        return body(bridge)
    }

    /// 淘汰：tmux/ssh 先 detach（保留 server/session），再回收 handle；
    /// local shell 直接 shutdown（无独立 server 可保留）。
    func evict(reason: ConnectionEvictionReason) {
        stateLock.lock()
        defer { stateLock.unlock() }
        lifecycleValue = .evicting
        pendingSurfaceEvents.removeAll()
        pendingDrainedWhileBackground = false
        if terminalManager.usesClientResize {
            _ = bridge.detach()
        }
        bridge.shutdown()
    }

    /// 窗口/应用关闭：直接回收 handle，不保留后台连接。
    func shutdown() {
        stateLock.lock()
        defer { stateLock.unlock() }
        lifecycleValue = .evicting
        pendingSurfaceEvents.removeAll()
        pendingDrainedWhileBackground = false
        bridge.shutdown()
    }
}
