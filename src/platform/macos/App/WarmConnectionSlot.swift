import AppKit
import CMuxterm
import MuxtermChrome

/// ConnectionPool 的真实 slot：持有 CoreBridge + 独立 TerminalManager。
///
/// 切换目标时旧 slot 进入 background 继续 poll（保持 warm），不立即
/// shutdown；只有淘汰时才 evict。tmux/ssh 淘汰用 detach 保留远端
/// server/session，local shell 直接 shutdown（前端就是 PTY 模拟器）。
///
/// 后台线程只排空 FFI。`PaneOutput` / `PaneFrame` / `PaneSnapshot` 必须 hop 回主线程
/// 再喂给 **这个** Workspace 自己的 TerminalManager。不能在
/// `muxterm.macos.background-poll` 上改 Swift Dictionary / SwiftTerm。
final class WarmConnectionSlot: ConnectionSlotProtocol {
    var key: ConnectionKey
    var targetConfig: TargetConfig
    let bridge: CoreBridge
    let terminalManager: TerminalManager
    private let stateLock = NSLock()
    /// CoreBridge 不能与同一个 handle 并发访问，但不能把这把锁和
    /// lifecycle/pending 队列锁混用。后台 poll 只短暂持有 bridgeLock，
    /// 主线程切换时不会被等待中的 Surface 队列反向卡住。
    private let bridgeLock = NSLock()
    private var lifecycleValue: ConnectionLifecycle = .background
    private var lastSnapshotValue = FrameSnapshot()
    private var attentionSnapshotValue: AttentionSnapshot?
    private var structuredAgentsValue: [StructuredPaneAgent] = []
    /// Pane → 1-based Tab number for the sidebar. The map is invalidated by
    /// topology changes and rebuilt from the Core snapshot on the next read.
    private var tabNumbersByPaneValue: [UInt32: Int]?
    /// Pane → stable TabId used by sidebar navigation. Kept beside the display
    /// number so building the sidebar never needs a second topology walk.
    private var tabIdsByPaneValue: [UInt32: UInt32]?
    private var workspaceReplicaIDValue: String?
    /// 后台拓扑快照的低频刷新闸门。事件轮询本身保持高频，但完整的
    /// get-tabs/get-panes/get-layout 只在结构变化或到期时执行，避免把
    /// 远端控制流查询变成每次侧栏刷新都要等待的同步工作。
    private var topologyRefreshAt: TimeInterval = -.infinity
    private var topologyRefreshRequested = true
    private var pendingAttentionNotifications: [AttentionNotification] = []
    private var pendingSurfaceEvents: [StateChange] = []
    /// 某个 pane 的后台输出合并超过安全上限后，丢弃不完整 suffix，等
    /// Runtime 发新的权威 baseline；不能把中间截断的 ANSI 当作可解析 VT。
    private var pendingSurfaceOverflowPanes = Set<UInt32>()
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
    /// 第一次进入 warm pool 的顺序。固定 Cmd+Ctrl+N 编号使用它，
    /// 不随最近使用/激活重排；Linux WorkspacePool::opened_order 同语义。
    var openedOrder: UInt64

    /// 最近一次后台 poll 后的快照（只读缓存，避免后台刷新 UI）。
    var lastSnapshot: FrameSnapshot {
        stateLock.lock()
        defer { stateLock.unlock() }
        return lastSnapshotValue
    }

    /// 前台使用该 slot 时由 MainWindow 写入的最新拓扑。后台不再重复做
    /// 全量 snapshot；切回时 refreshUI 仍会读取权威快照。
    func cacheSnapshot(_ snapshot: FrameSnapshot) {
        stateLock.lock()
        lastSnapshotValue = snapshot
        stateLock.unlock()
    }

    /// 后台轮询得到的注意力快照。主线程侧栏只读缓存，不为展示再次触碰
    /// 远端 CoreBridge。
    var cachedAttentionSnapshot: AttentionSnapshot? {
        stateLock.lock()
        defer { stateLock.unlock() }
        return attentionSnapshotValue
    }

    /// 写入当前 slot 的值类型注意力快照；激活 slot 时也用它减少重复 JSON 查询。
    func cacheAttentionSnapshot(_ snapshot: AttentionSnapshot) {
        stateLock.lock()
        attentionSnapshotValue = snapshot
        if let workspaceID = snapshot.workspaces.first?.workspaceId {
            workspaceReplicaIDValue = workspaceID
        }
        stateLock.unlock()
    }

    /// 后台轮询得到的稳定 Workspace 身份。
    var cachedWorkspaceReplicaID: String? {
        stateLock.lock()
        defer { stateLock.unlock() }
        return workspaceReplicaIDValue
    }

    func cacheWorkspaceReplicaID(_ workspaceID: String) {
        guard !workspaceID.isEmpty else { return }
        stateLock.lock()
        workspaceReplicaIDValue = workspaceID
        stateLock.unlock()
    }

    /// Core 的结构化 agent 快照副本。
    var cachedStructuredAgents: [StructuredPaneAgent] {
        stateLock.lock()
        defer { stateLock.unlock() }
        return structuredAgentsValue
    }

    func cacheStructuredAgents(_ agents: [StructuredPaneAgent]) {
        stateLock.lock()
        structuredAgentsValue = agents
        stateLock.unlock()
    }

    /// Cached Tab numbers are a value snapshot so sidebar refreshes do not
    /// touch the remote bridge on every poll.
    var cachedTabNumbersByPane: [UInt32: Int]? {
        stateLock.lock()
        defer { stateLock.unlock() }
        return tabNumbersByPaneValue
    }

    var cachedTabIdsByPane: [UInt32: UInt32]? {
        stateLock.lock()
        defer { stateLock.unlock() }
        return tabIdsByPaneValue
    }

    func cacheTabTargets(
        tabIdsByPane: [UInt32: UInt32],
        tabNumbersByPane: [UInt32: Int]
    ) {
        stateLock.lock()
        tabIdsByPaneValue = tabIdsByPane
        tabNumbersByPaneValue = tabNumbersByPane
        stateLock.unlock()
    }

    func invalidateTabNumbers() {
        stateLock.lock()
        tabIdsByPaneValue = nil
        tabNumbersByPaneValue = nil
        topologyRefreshRequested = true
        stateLock.unlock()
    }

    /// 主线程消费后台已经取走的通知；通知的 FFI 查询不再发生在 UI 切换路径。
    func takePendingAttentionNotifications() -> [AttentionNotification] {
        dispatchPrecondition(condition: .onQueue(.main))
        stateLock.lock()
        defer { stateLock.unlock() }
        let result = pendingAttentionNotifications
        pendingAttentionNotifications.removeAll()
        return result
    }

    init(
        key: ConnectionKey,
        bridge: CoreBridge,
        terminalManager: TerminalManager? = nil,
        targetConfig: TargetConfig? = nil,
        now: UInt64,
        openedOrder: UInt64 = 0
    ) {
        self.key = key
        self.targetConfig = targetConfig ?? key.targetConfig
        self.bridge = bridge
        self.terminalManager = terminalManager ?? TerminalManager(bridge: bridge)
        // CoreBridge 在 connect 后已完成有限 bootstrap；把这份首帧状态直接
        // 放进 warm cache，侧栏首次渲染不必等下一次后台 poll。
        self.structuredAgentsValue = bridge.structuredAgentSnapshot()
        self.lastUsedAt = now
        self.openedOrder = openedOrder
    }

    /// ConnectionPool 协议入口：后台只排空 FFI，不碰 Surface。
    func pollBackground() {
        _ = drainBackgroundEvents()
    }

    /// 是否还有需要 hop 回主线程的 Surface 工作。
    var hasPendingSurfaceWork: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return !pendingSurfaceEvents.isEmpty || !pendingSurfaceOverflowPanes.isEmpty
    }

    /// 在后台队列排空该 Workspace 的 core 事件。
    ///
    /// 只做 FFI：poll、Index/attention 进程名、缓存 snapshot。
    /// 返回是否有待主线程投递的 Surface 事件。
    @discardableResult
    func drainBackgroundEvents() -> Bool {
        stateLock.lock()
        guard lifecycleValue == .background else {
            stateLock.unlock()
            return false
        }
        stateLock.unlock()

        // 不要在 stateLock 内进入 FFI。激活/关闭只需改变 lifecycle，
        // bridgeLock 负责让同一 handle 的一次短 FFI 批次安全完成。
        bridgeLock.lock()
        stateLock.lock()
        let shouldPoll = lifecycleValue == .background
        stateLock.unlock()
        guard shouldPoll else {
            // 激活可能发生在上面的 lifecycle 检查之后、拿到 bridgeLock
            // 之前。此时不要再启动一批后台 FFI；已经在 lock 内开始的
            // 那一批仍会在下面入队并由前台 catch-up 消费。
            bridgeLock.unlock()
            return false
        }

        let events = bridge.pollEvents(maxCount: 32)
        _ = bridge.takeError()
        let topologyChanged = events.contains(where: { Self.changesTopology($0.type) })
        if topologyChanged {
            invalidateTabNumbers()
        }
        let now = ProcessInfo.processInfo.systemUptime
        stateLock.lock()
        let shouldRefreshTopology = topologyChanged
            || topologyRefreshRequested
            || lastSnapshotValue.tabs.isEmpty
            || now - topologyRefreshAt >= 0.5
        if shouldRefreshTopology {
            topologyRefreshAt = now
            topologyRefreshRequested = false
        }
        stateLock.unlock()
        let topology = shouldRefreshTopology
            ? Self.captureTopology(from: bridge)
            : nil
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
            } else if ev.isPaneOutput || ev.isPaneFrame || ev.isPaneSnapshot || ev.isPaneHistory || ev.isPaneClosed {
                surface.append(ev)
            }
        }

        // 先把 Surface 事件挂入 slot 队列，再释放 bridgeLock。这样如果主线程
        // 恰好开始激活，prepareForForeground 等到的就是完整的这一批事件。
        stateLock.lock()
        guard lifecycleValue != .evicting else {
            stateLock.unlock()
            bridgeLock.unlock()
            return false
        }
        if !surface.isEmpty {
            for event in surface {
                enqueueSurfaceEvent(event)
            }
            pendingDrainedWhileBackground = true
        }
        if let topology {
            lastSnapshotValue = topology.snapshot
            tabIdsByPaneValue = topology.tabIdsByPane
            tabNumbersByPaneValue = topology.tabNumbersByPane
        } else if shouldRefreshTopology {
            // 连接暂时没有可用拓扑时，下一轮仍需重试；不要把一个空的
            // 过渡结果当成稳定 Workspace 列表。
            topologyRefreshRequested = true
        }
        let continueBackgroundMetadata = lifecycleValue == .background
        stateLock.unlock()
        bridgeLock.unlock()

        // 输出洪水只做小批量 poll；拓扑 snapshot 已在后台低频缓存，前台
        // 切回先使用缓存，权威结果也不会在主线程同步读取。注意力快照很小
        // 且只在 utility 线程读取，保持每轮 poll
        // 更新，确保 BEL/agent 状态不会等下一轮 UI 展示才出现。
        var attentionJSON: String?
        var notificationJSON: String?
        var agents: [StructuredPaneAgent]?
        if continueBackgroundMetadata {
            // 生命周期可能在上一个短批次之后已经切换；再次确认后才做
            // metadata FFI，优先把主线程切换让出来。
            bridgeLock.lock()
            stateLock.lock()
            let shouldReadMetadata = lifecycleValue == .background
            stateLock.unlock()
            if shouldReadMetadata {
                attentionJSON = bridge.attentionSnapshotJSON()
                notificationJSON = bridge.attentionTakeNotificationsJSON()
                agents = bridge.structuredAgentSnapshot()
            } else {
                attentionJSON = nil
                notificationJSON = nil
                agents = nil
            }
            bridgeLock.unlock()
        } else {
            attentionJSON = nil
            notificationJSON = nil
            agents = nil
        }
        let attention = attentionJSON.flatMap { json in
            AttentionSnapshot.decode(Data(json.utf8))
        }
        let notifications = notificationJSON.flatMap { json in
            AttentionNotifications.decode(Data(json.utf8))?.notifications
        } ?? []
        stateLock.lock()
        guard lifecycleValue != .evicting else {
            stateLock.unlock()
            return false
        }
        let shouldCommitBackgroundMetadata = continueBackgroundMetadata
            && lifecycleValue == .background
        if shouldCommitBackgroundMetadata {
            if let attention {
                attentionSnapshotValue = attention
                if let workspaceID = attention.workspaces.first?.workspaceId {
                    workspaceReplicaIDValue = workspaceID
                }
            }
        }
        if continueBackgroundMetadata {
            if !notifications.isEmpty {
                pendingAttentionNotifications.append(contentsOf: notifications)
            }
        }
        if shouldCommitBackgroundMetadata, let agents {
            structuredAgentsValue = agents
        }
        let hasPending = !pendingSurfaceEvents.isEmpty || !pendingSurfaceOverflowPanes.isEmpty
        stateLock.unlock()
        return hasPending
    }

    private struct CachedTopology {
        let snapshot: FrameSnapshot
        let tabIdsByPane: [UInt32: UInt32]
        let tabNumbersByPane: [UInt32: Int]
    }

    private static func changesTopology(_ type: UInt32) -> Bool {
        type == STATE_LAYOUT_CHANGED
            || type == STATE_TAB_ADDED
            || type == STATE_TAB_CLOSED
            || type == STATE_TAB_ORDER_CHANGED
            || type == STATE_PANE_ADDED
            || type == STATE_PANE_CLOSED
            || type == STATE_ACTIVE_TAB_CHANGED
            || type == STATE_ACTIVE_PANE_CHANGED
            || type == STATE_PANE_RESIZED
            || type == STATE_TAB_RENAMED
    }

    private static func captureTopology(from bridge: CoreBridge) -> CachedTopology? {
        let snapshot = bridge.snapshot()
        guard !snapshot.tabs.isEmpty else { return nil }
        var tabIdsByPane: [UInt32: UInt32] = [:]
        var tabNumbersByPane: [UInt32: Int] = [:]
        for (index, tab) in snapshot.tabs.enumerated() {
            for pane in bridge.getPanes(tabId: tab.id) {
                tabIdsByPane[pane.id] = tab.id
                tabNumbersByPane[pane.id] = index + 1
            }
        }
        return CachedTopology(
            snapshot: snapshot,
            tabIdsByPane: tabIdsByPane,
            tabNumbersByPane: tabNumbersByPane
        )
    }

    /// 规范化后台 Surface 队列。
    ///
    /// 不同 pane 的输出可以独立合并；同一 pane 只合并 baseline 之后的
    /// 连续输出。新的 snapshot/frame 会淘汰它之前尚未交付的旧 Surface，
    /// 但保留按行历史，因为历史属于 native scrollback 的独立 seed。
    private func enqueueSurfaceEvent(_ event: StateChange) {
        let paneId = event.paneId
        if event.isPaneSnapshot || event.isPaneFrame {
            pendingSurfaceOverflowPanes.remove(paneId)
            pendingSurfaceEvents.removeAll { candidate in
                candidate.paneId == paneId
                    && (candidate.isPaneOutput
                        || candidate.isPaneSnapshot
                        || candidate.isPaneFrame)
            }
            pendingSurfaceEvents.append(event)
            return
        }

        if event.isPaneOutput {
            guard !pendingSurfaceOverflowPanes.contains(paneId) else { return }
            if let index = pendingSurfaceEvents.lastIndex(where: {
                $0.paneId == paneId
            }) {
                let previous = pendingSurfaceEvents[index]
                guard previous.isPaneOutput else {
                    pendingSurfaceEvents.append(event)
                    return
                }
                let combinedSize = previous.data.count.addingReportingOverflow(event.data.count)
                guard !combinedSize.overflow,
                      combinedSize.partialValue <= SurfaceEventBatchPolicy.maxCoalescedOutputBytes
                else {
                    pendingSurfaceEvents.removeAll { candidate in
                        candidate.paneId == paneId && candidate.isPaneOutput
                    }
                    pendingSurfaceOverflowPanes.insert(paneId)
                    return
                }
                pendingSurfaceEvents[index] = StateChange(
                    type: previous.type,
                    paneId: previous.paneId,
                    tabId: previous.tabId,
                    windowId: previous.windowId,
                    data: Self.appendedData(previous.data, event.data),
                    name: previous.name
                )
                return
            }
            pendingSurfaceEvents.append(event)
            return
        }

        if event.isPaneClosed {
            pendingSurfaceOverflowPanes.remove(paneId)
            pendingSurfaceEvents.removeAll { $0.paneId == paneId }
        }
        pendingSurfaceEvents.append(event)
    }

    /// 主线程：把后台排空的 PTY 喂给本 slot 的 Surface 树。
    ///
    /// 后台批次即使后来变成前台，也不得为从未打开的 pane 新建 widget。
    @discardableResult
    func applyPendingSurfaceEvents(
        maxEvents: Int = SurfaceEventBatchPolicy.maxEventsPerPass,
        timeBudget: TimeInterval = SurfaceEventBatchPolicy.timeBudget
    ) -> Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        stateLock.lock()
        let overflowPanes = pendingSurfaceOverflowPanes
        pendingSurfaceOverflowPanes.removeAll()
        let fromBackground = pendingDrainedWhileBackground
        pendingDrainedWhileBackground = false
        let alive = lifecycleValue != .evicting
        let nowActive = lifecycleValue == .active
        stateLock.unlock()
        guard alive else { return false }

        if fromBackground {
            terminalManager.setViewCreationEnabled(false)
        }

        for paneId in overflowPanes {
            terminalManager.markNeedsAuthoritativeSnapshot(paneId: paneId)
        }

        let limit = max(1, maxEvents)
        let started = ProcessInfo.processInfo.systemUptime
        var events: [StateChange] = []
        while events.count < limit {
            if !events.isEmpty,
               SurfaceEventBatchPolicy.shouldYield(
                   processedEvents: events.count,
                   elapsed: ProcessInfo.processInfo.systemUptime - started,
                   maxEvents: limit,
                   timeBudget: timeBudget
               )
            {
                break
            }
            stateLock.lock()
            let next = pendingSurfaceEvents.first
            if next != nil {
                pendingSurfaceEvents.removeFirst()
            }
            stateLock.unlock()
            guard let next else { break }
            events.append(next)
        }

        if !events.isEmpty {
            terminalManager.beginEventBatch()
            for ev in events {
                if ev.isPaneClosed {
                    terminalManager.removePane(ev.paneId)
                } else if ev.isPaneSnapshot {
                    terminalManager.handleSnapshot(paneId: ev.paneId, data: ev.data)
                } else if ev.isPaneFrame {
                    terminalManager.handleFrame(paneId: ev.paneId, data: ev.data)
                } else if ev.isPaneHistory {
                    terminalManager.handleHistory(paneId: ev.paneId, data: ev.data)
                } else if ev.isPaneOutput {
                    terminalManager.handleOutput(paneId: ev.paneId, data: ev.data)
                }
            }
            terminalManager.endEventBatch()
        }
        if nowActive {
            terminalManager.setViewCreationEnabled(true)
        }

        stateLock.lock()
        let hasPending = !pendingSurfaceEvents.isEmpty || !pendingSurfaceOverflowPanes.isEmpty
        stateLock.unlock()
        return hasPending
    }

    private static func appendedData(_ first: Data, _ second: Data) -> Data {
        var combined = first
        combined.append(second)
        return combined
    }

    /// 在 MainWindow 使用该 slot 的 bridge 前先切换生命周期。
    ///
    /// 激活路径不能在主线程等待后台 SSH/FFI。若后台当前正持有 bridgeLock，
    /// 返回 false；调用方先用缓存完成视觉切换，再通过
    /// `tryPrepareActiveBridge()` 重试。生命周期已经变成 active 后，后台
    /// poll 不会再开启下一批 FFI。
    @discardableResult
    func prepareForForeground() -> Bool {
        stateLock.lock()
        guard lifecycleValue != .evicting else {
            stateLock.unlock()
            return false
        }
        lifecycleValue = .active
        stateLock.unlock()

        return tryPrepareActiveBridge()
    }

    /// 非阻塞地确认 active slot 已经可以安全访问 CoreBridge。
    @discardableResult
    func tryPrepareActiveBridge() -> Bool {
        stateLock.lock()
        guard lifecycleValue == .active else {
            stateLock.unlock()
            return false
        }
        stateLock.unlock()

        guard bridgeLock.try() else { return false }
        bridgeLock.unlock()
        // 先把已经排队的 baseline/增量按序交付，再请求可能需要的恢复
        // snapshot；否则旧缺口可能在激活这一拍抢先发出网络任务。
        terminalManager.setViewCreationEnabled(
            true,
            requestRecoverySnapshots: false
        )
        return true
    }

    /// 在后台 slot 上安全访问 CoreBridge。后台 poll、前台激活和淘汰共用
    /// 同一把锁；不得把 bridge 引用带出闭包，否则 C ABI handle 可能并发。
    @discardableResult
    func withBridge<T>(_ body: (CoreBridge) -> T) -> T? {
        bridgeLock.lock()
        defer { bridgeLock.unlock() }
        stateLock.lock()
        let alive = lifecycleValue != .evicting
        stateLock.unlock()
        guard alive else { return nil }
        return body(bridge)
    }

    /// 尝试安全访问 CoreBridge；UI 线程使用它时绝不能因为后台 SSH/FFI
    /// 正在运行而等待。后台/非 UI 的强一致查询仍使用 `withBridge`。
    @discardableResult
    func tryWithBridge<T>(_ body: (CoreBridge) -> T) -> T? {
        guard bridgeLock.try() else { return nil }
        defer { bridgeLock.unlock() }
        stateLock.lock()
        let alive = lifecycleValue != .evicting
        stateLock.unlock()
        guard alive else { return nil }
        return body(bridge)
    }

    /// 测试辅助：让同一后台执行上下文短暂持有 bridgeLock，复现远端
    /// metadata/poll 与主线程 Workspace 激活竞争。返回的信号量在已拿锁后
    /// signal，测试因此不需要靠 sleep 猜时序。
    @discardableResult
    func holdBridgeForTesting(_ duration: TimeInterval) -> DispatchSemaphore {
        let acquired = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self else { return }
            self.bridgeLock.lock()
            acquired.signal()
            if duration > 0 {
                Thread.sleep(forTimeInterval: duration)
            }
            self.bridgeLock.unlock()
        }
        return acquired
    }

    /// 淘汰：tmux/ssh 先 detach（保留 server/session），再回收 handle；
    /// local shell 直接 shutdown（无独立 server 可保留）。
    func evict(reason: ConnectionEvictionReason) {
        stateLock.lock()
        lifecycleValue = .evicting
        pendingSurfaceEvents.removeAll()
        pendingSurfaceOverflowPanes.removeAll()
        pendingDrainedWhileBackground = false
        pendingAttentionNotifications.removeAll()
        stateLock.unlock()

        bridgeLock.lock()
        if terminalManager.usesClientResize {
            _ = bridge.detach()
        }
        bridge.shutdown()
        bridgeLock.unlock()
    }

    /// 窗口/应用关闭：直接回收 handle，不保留后台连接。
    func shutdown() {
        stateLock.lock()
        lifecycleValue = .evicting
        pendingSurfaceEvents.removeAll()
        pendingSurfaceOverflowPanes.removeAll()
        pendingDrainedWhileBackground = false
        pendingAttentionNotifications.removeAll()
        stateLock.unlock()

        bridgeLock.lock()
        bridge.shutdown()
        bridgeLock.unlock()
    }
}
