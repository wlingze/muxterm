import AppKit
import MuxtermChrome

/// ConnectionPool 的真实 slot：持有 CoreBridge + 独立 TerminalManager。
///
/// 切换目标时旧 slot 进入 background 继续 poll（保持 warm），不立即
/// shutdown；只有淘汰时才 evict。tmux/ssh 淘汰用 detach 保留远端
/// server/session，local shell 直接 shutdown（前端就是 PTY 模拟器）。
final class WarmConnectionSlot: ConnectionSlotProtocol {
    let key: ConnectionKey
    let bridge: CoreBridge
    let terminalManager: TerminalManager
    var lifecycle: ConnectionLifecycle = .background
    var lastUsedAt: UInt64

    /// 最近一次后台 poll 后的快照（只读缓存，避免后台刷新 UI）。
    private(set) var lastSnapshot = FrameSnapshot()

    init(key: ConnectionKey, bridge: CoreBridge, now: UInt64) {
        self.key = key
        self.bridge = bridge
        self.terminalManager = TerminalManager(bridge: bridge)
        self.lastUsedAt = now
    }

    /// 后台继续 poll：喂事件给本 slot 的 TerminalManager，保持 SwiftTerm
    /// 状态 warm；不做同步 displayIfNeeded（视图可能不在窗口层级）。
    func pollBackground() {
        terminalManager.beginEventBatch()
        defer { terminalManager.endEventBatch() }
        let events = bridge.pollEvents()
        _ = bridge.takeError()
        for ev in events {
            if ev.isPaneClosed {
                terminalManager.removePane(ev.paneId)
            } else if ev.isPaneOutput {
                terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
            } else if ev.isBackendStatus, ev.paneId == 4 {
                // 后台连接退出：不主动关窗口，交给前台下次激活时处理。
                continue
            }
        }
        lastSnapshot = bridge.snapshot()
    }

    /// 淘汰：tmux/ssh 先 detach（保留 server/session），再回收 handle；
    /// local shell 直接 shutdown（无独立 server 可保留）。
    func evict(reason: ConnectionEvictionReason) {
        lifecycle = .evicting
        if terminalManager.usesClientResize {
            _ = bridge.detach()
        }
        bridge.shutdown()
    }

    /// 窗口/应用关闭：直接回收 handle，不保留后台连接。
    func shutdown() {
        lifecycle = .evicting
        bridge.shutdown()
    }
}
