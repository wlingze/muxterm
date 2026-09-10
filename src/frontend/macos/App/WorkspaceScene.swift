import AppKit
import CMuxterm
import MuxtermChrome

/// 一个 Workspace scene 的主线程只读状态与 Surface mailbox。
/// EventPump 先把 Core 的 owned DTO 写入这里，SceneStack/Sidebar/Terminal
/// 只消费这个值类型视图，不需要重新查询远端。
final class WorkspaceViewStore {
    var snapshot = FrameSnapshot()
    var attentionSnapshot: AttentionSnapshot?
    var structuredAgents: [StructuredPaneAgent] = []
    var tabNumbersByPane: [UInt32: Int]?
    var tabIdsByPane: [UInt32: UInt32]?
    var workspaceReplicaID: String?
    var topologyRefreshAt: TimeInterval = -.infinity
    var topologyRefreshRequested = true
    var pendingAttentionNotifications: [AttentionNotification] = []
    var pendingSurfaceEvents: [StateChange] = []
    var pendingSurfaceOverflowPanes = Set<UInt32>()
    var pendingDrainedWhileHidden = false
}

/// 常驻 Workspace scene：持有 CoreBridge 路由、TerminalManager 和 ViewStore。
final class WorkspaceScene: SceneProtocol {
    var key: SceneKey
    var targetConfig: TargetConfig
    let bridge: CoreBridge
    /// Stable identity used when several scenes share one Core handle.
    let workspaceID: String?
    /// MainWindow's serialized UI-to-Core command boundary. Shared-event
    /// ingestion uses it for attention mutations without touching CoreBridge.
    var enqueueCoreCommand: ((QueuedMuxCommand) -> Bool)?
    let terminalManager: TerminalManager
    let viewStore: WorkspaceViewStore
    /// SceneStack and the workspace EventPump are both main-thread owned.
    /// Keep this state synchronous so scene activation never waits on a lock.
    private var visibilityValue: SceneVisibility = .hidden
    var visibility: SceneVisibility {
        get { visibilityValue }
        set { visibilityValue = newValue }
    }
    var lastUsedAt: UInt64
    /// 第一次进入 SceneStack 的顺序。固定 Cmd+Ctrl+N 编号使用它，
    /// 不随最近使用/激活重排；Linux WorkspacePool::opened_order 同语义。
    var openedOrder: UInt64

    /// EventPump 最近一次提交的快照（只读 ViewStore）。
    var lastSnapshot: FrameSnapshot {
        return viewStore.snapshot
    }

    /// EventPump 写入的最新拓扑；场景切换时直接使用，不重新读取 Core。
    func cacheSnapshot(_ snapshot: FrameSnapshot) {
        viewStore.snapshot = snapshot
    }

    /// EventPump 提交的注意力快照；主线程侧栏只读 ViewStore。
    var cachedAttentionSnapshot: AttentionSnapshot? {
        return viewStore.attentionSnapshot
    }

    /// 写入当前 scene 的值类型注意力快照。
    func cacheAttentionSnapshot(_ snapshot: AttentionSnapshot) {
        viewStore.attentionSnapshot = snapshot
        if let workspaceID = snapshot.workspaces.first?.workspaceId {
            viewStore.workspaceReplicaID = workspaceID
        }
    }

    /// Core 返回的稳定 Workspace 身份。
    var cachedWorkspaceReplicaID: String? {
        return viewStore.workspaceReplicaID
    }

    func cacheWorkspaceReplicaID(_ workspaceID: String) {
        guard !workspaceID.isEmpty else { return }
        viewStore.workspaceReplicaID = workspaceID
    }

    /// Core 的结构化 agent 快照副本。
    var cachedStructuredAgents: [StructuredPaneAgent] {
        return viewStore.structuredAgents
    }

    func cacheStructuredAgents(_ agents: [StructuredPaneAgent]) {
        viewStore.structuredAgents = agents
    }

    /// Cached Tab numbers are a value snapshot so sidebar refreshes do not
    /// touch the remote bridge on every poll.
    var cachedTabNumbersByPane: [UInt32: Int]? {
        return viewStore.tabNumbersByPane
    }

    var cachedTabIdsByPane: [UInt32: UInt32]? {
        return viewStore.tabIdsByPane
    }

    func cacheTabTargets(
        tabIdsByPane: [UInt32: UInt32],
        tabNumbersByPane: [UInt32: Int]
    ) {
        viewStore.tabIdsByPane = tabIdsByPane
        viewStore.tabNumbersByPane = tabNumbersByPane
    }

    func invalidateTabNumbers() {
        viewStore.tabIdsByPane = nil
        viewStore.tabNumbersByPane = nil
        viewStore.topologyRefreshRequested = true
    }

    /// 主线程消费后台已经取走的通知；通知的 FFI 查询不再发生在 UI 切换路径。
    func takePendingAttentionNotifications() -> [AttentionNotification] {
        dispatchPrecondition(condition: .onQueue(.main))
        let result = viewStore.pendingAttentionNotifications
        viewStore.pendingAttentionNotifications.removeAll()
        return result
    }

    init(
        key: SceneKey,
        bridge: CoreBridge,
        workspaceID: String? = nil,
        terminalManager: TerminalManager? = nil,
        targetConfig: TargetConfig? = nil,
        now: UInt64,
        openedOrder: UInt64 = 0
    ) {
        self.key = key
        self.targetConfig = targetConfig ?? key.targetConfig
        self.bridge = bridge
        self.workspaceID = workspaceID
        self.viewStore = WorkspaceViewStore()
        self.terminalManager = terminalManager ?? TerminalManager(
            bridge: bridge,
            workspaceID: workspaceID
        )
        // 首帧拓扑和 agent 状态由 MainWindow 的 EventPump 提交；构造 scene
        // 不得绕过 ViewStore 直接查询 Core。首轮 poll 会处理 needsLayoutReload。
        self.viewStore.workspaceReplicaID = workspaceID
        self.lastUsedAt = now
        self.openedOrder = openedOrder
    }

    /// Consume events already copied by the main-thread workspace EventPump.
    /// Shared-core scenes never poll their handle here; this method only
    /// updates the scene-owned topology/cache and queues render data.
    func ingestSharedEvents(_ events: [StateChange]) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard workspaceID != nil else { return }
        guard visibility != .closed else { return }

        let topologyChanged = events.contains(where: { Self.changesTopology($0.type) })
        if topologyChanged {
            invalidateTabNumbers()
        }
        let topology = topologyChanged
            ? Self.captureTopology(from: bridge, workspaceID: workspaceID)
            : nil
        var surface: [StateChange] = []
        for event in events {
            if event.type == STATE_STATUS_SUBSCRIPTION,
               event.name.hasPrefix("muxterm.pane-cmd")
            {
                let value = String(data: event.data, encoding: .utf8) ?? ""
                if let workspaceID, let enqueueCoreCommand {
                    _ = enqueueCoreCommand(.attention(
                        workspaceID: workspaceID,
                        .setProcessName(
                            paneID: event.paneId,
                            name: value.isEmpty ? nil : value
                        ),
                        failureMessage: ""
                    ))
                }
            } else if event.isPaneOutput
                || event.isPaneFrame
                || event.isPaneSnapshot
                || event.isPaneHistory
                || event.isPaneClosed
            {
                surface.append(event)
            }
        }

        if !surface.isEmpty {
            for event in surface {
                enqueueSurfaceEvent(event)
            }
            viewStore.pendingDrainedWhileHidden = true
        }
        if let topology {
            viewStore.snapshot = topology.snapshot
            viewStore.tabIdsByPane = topology.tabIdsByPane
            viewStore.tabNumbersByPane = topology.tabNumbersByPane
        }
    }

    /// 是否还有需要 hop 回主线程的 Surface 工作。
    var hasPendingSurfaceWork: Bool {
        return !viewStore.pendingSurfaceEvents.isEmpty || !viewStore.pendingSurfaceOverflowPanes.isEmpty
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

    private static func captureTopology(
        from bridge: CoreBridge,
        workspaceID: String? = nil
    ) -> CachedTopology? {
        let snapshot = workspaceID.map { bridge.snapshot(workspaceID: $0) }
            ?? bridge.snapshot()
        guard !snapshot.tabs.isEmpty else { return nil }
        var tabIdsByPane: [UInt32: UInt32] = [:]
        var tabNumbersByPane: [UInt32: Int] = [:]
        for (index, tab) in snapshot.tabs.enumerated() {
            let panes = workspaceID.map { bridge.getPanes(workspaceID: $0, tabId: tab.id) }
                ?? bridge.getPanes(tabId: tab.id)
            for pane in panes {
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
            viewStore.pendingSurfaceOverflowPanes.remove(paneId)
            viewStore.pendingSurfaceEvents.removeAll { candidate in
                candidate.paneId == paneId
                    && (candidate.isPaneOutput
                        || candidate.isPaneSnapshot
                        || candidate.isPaneFrame)
            }
            viewStore.pendingSurfaceEvents.append(event)
            return
        }

        if event.isPaneOutput {
            guard !viewStore.pendingSurfaceOverflowPanes.contains(paneId) else { return }
            if let index = viewStore.pendingSurfaceEvents.lastIndex(where: {
                $0.paneId == paneId
            }) {
                let previous = viewStore.pendingSurfaceEvents[index]
                guard previous.isPaneOutput else {
                    viewStore.pendingSurfaceEvents.append(event)
                    return
                }
                let combinedSize = previous.data.count.addingReportingOverflow(event.data.count)
                guard !combinedSize.overflow,
                      combinedSize.partialValue <= SurfaceEventBatchPolicy.maxCoalescedOutputBytes
                else {
                    viewStore.pendingSurfaceEvents.removeAll { candidate in
                        candidate.paneId == paneId && candidate.isPaneOutput
                    }
                    viewStore.pendingSurfaceOverflowPanes.insert(paneId)
                    return
                }
                viewStore.pendingSurfaceEvents[index] = StateChange(
                    type: previous.type,
                    paneId: previous.paneId,
                    tabId: previous.tabId,
                    windowId: previous.windowId,
                    data: Self.appendedData(previous.data, event.data),
                    name: previous.name
                )
                return
            }
            viewStore.pendingSurfaceEvents.append(event)
            return
        }

        if event.isPaneClosed {
            viewStore.pendingSurfaceOverflowPanes.remove(paneId)
            viewStore.pendingSurfaceEvents.removeAll { $0.paneId == paneId }
        }
        viewStore.pendingSurfaceEvents.append(event)
    }

    /// 主线程：把 ViewStore mailbox 中的 PTY 事件喂给本 scene 的 Surface 树。
    ///
    /// 隐藏 scene 的批次即使后来变成可见，也不得为从未打开的 pane 新建 widget。
    @discardableResult
    func applyPendingSurfaceEvents(
        maxEvents: Int = SurfaceEventBatchPolicy.maxEventsPerPass,
        timeBudget: TimeInterval = SurfaceEventBatchPolicy.timeBudget
    ) -> Bool {
        dispatchPrecondition(condition: .onQueue(.main))
        let overflowPanes = viewStore.pendingSurfaceOverflowPanes
        viewStore.pendingSurfaceOverflowPanes.removeAll()
        let fromHidden = viewStore.pendingDrainedWhileHidden
        viewStore.pendingDrainedWhileHidden = false
        let alive = visibilityValue != .closed
        let nowVisible = visibilityValue == .visible
        guard alive else { return false }

        if fromHidden {
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
            let next = viewStore.pendingSurfaceEvents.first
            if next != nil {
                viewStore.pendingSurfaceEvents.removeFirst()
            }
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
        if nowVisible {
            terminalManager.setViewCreationEnabled(true)
        }

        let hasPending = !viewStore.pendingSurfaceEvents.isEmpty || !viewStore.pendingSurfaceOverflowPanes.isEmpty
        return hasPending
    }

    private static func appendedData(_ first: Data, _ second: Data) -> Data {
        var combined = first
        combined.append(second)
        return combined
    }

    /// 关闭场景：通知 Core 销毁对应 Workspace；共享 handle 由窗口统一回收。
    func evict(reason: SceneEvictionReason) {
        dispatchPrecondition(condition: .onQueue(.main))
        visibilityValue = .closed
        viewStore.pendingSurfaceEvents.removeAll()
        viewStore.pendingSurfaceOverflowPanes.removeAll()
        viewStore.pendingDrainedWhileHidden = false
        viewStore.pendingAttentionNotifications.removeAll()

        if let workspaceID {
            _ = bridge.closeWorkspace(workspaceID: workspaceID)
        }
    }

    /// 窗口/应用关闭：清理 scene 状态；共享 handle 由窗口统一回收。
    func shutdown() {
        dispatchPrecondition(condition: .onQueue(.main))
        visibilityValue = .closed
        viewStore.pendingSurfaceEvents.removeAll()
        viewStore.pendingSurfaceOverflowPanes.removeAll()
        viewStore.pendingDrainedWhileHidden = false
        viewStore.pendingAttentionNotifications.removeAll()

    }
}
