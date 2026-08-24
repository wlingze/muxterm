import AppKit
import CMuxterm
import MuxtermChrome

/// ConnectionPool 的真实 slot：持有 CoreBridge + 独立 TerminalManager。
///
/// 切换目标时旧 slot 进入 background 继续 poll（保持 warm），不立即
/// shutdown；只有淘汰时才 evict。tmux/ssh 淘汰用 detach 保留远端
/// server/session，local shell 直接 shutdown（前端就是 PTY 模拟器）。
final class WarmConnectionSlot: ConnectionSlotProtocol {
    var key: ConnectionKey
    var targetConfig: TargetConfig
    let bridge: CoreBridge
    let terminalManager: TerminalManager
    private let stateLock = NSLock()
    private var lifecycleValue: ConnectionLifecycle = .background
    private var lastSnapshotValue = FrameSnapshot()
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

    /// 后台继续 poll：已有 Surface 继续吃 PTY；不新建 widget，不 dump Index。
    func pollBackground() {
        // CoreBridge 的 C ABI 句柄不是可并发访问的；同一 slot 的前台切换、
        // 后台 poll、淘汰必须串行。锁只覆盖该 slot，不会阻塞主窗口其它连接。
        stateLock.lock()
        guard lifecycleValue == .background else {
            stateLock.unlock()
            return
        }
        defer { stateLock.unlock() }
        terminalManager.setViewCreationEnabled(false)
        defer { terminalManager.setViewCreationEnabled(true) }
        terminalManager.beginEventBatch()
        defer { terminalManager.endEventBatch() }
        let events = bridge.pollEvents()
        _ = bridge.takeError()
        for ev in events {
            if ev.isPaneClosed {
                terminalManager.removePane(ev.paneId)
            } else if ev.isPaneSnapshot {
                terminalManager.handleSnapshot(paneId: ev.paneId, data: ev.data)
            } else if ev.isPaneOutput {
                terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
            } else if ev.type == STATE_STATUS_SUBSCRIPTION,
                      ev.name.hasPrefix("muxterm.pane-cmd") {
                // 后台 Workspace 也要维护 pane 进程名；否则 Attention 从后台
                // 触发时只能显示 workspace/node，无法标记 Codex/Cursor。
                let value = String(data: ev.data, encoding: .utf8) ?? ""
                _ = bridge.attentionSetProcessName(
                    paneId: ev.paneId,
                    name: value.isEmpty ? nil : value
                )
            } else if ev.isBackendStatus, ev.paneId == 4 {
                // 后台连接退出：不主动关窗口，交给前台下次激活时处理。
                continue
            }
        }
        lastSnapshotValue = bridge.snapshot()
    }

    /// 在 MainWindow 使用该 slot 的 bridge 前先切换生命周期。若后台 poll
    /// 已经开始，这里会等它释放锁，确保 Swift/FFI 不发生并发访问。
    func prepareForForeground() {
        stateLock.lock()
        lifecycleValue = .active
        stateLock.unlock()
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
        bridge.shutdown()
    }
}
