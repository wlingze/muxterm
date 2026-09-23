import AppKit
import MuxtermChrome

enum PaneTitleAction: Equatable {
    case splitHorizontal
    case splitVertical
    case fullscreen
    case cycleLayout
    case moveToNewTab
    case moveToTab(UInt32)
    case close
}

/// Pane 标题悬浮在终端上，显隐不改变 Surface 或 client 的格子。
enum PaneTitleBarGeometry {
    static let height: CGFloat = 22
    static let dividerLength: CGFloat = 6

    static func reservedHeight(showsTitles: Bool) -> CGFloat {
        0
    }

    static func clientContentSize(container: NSSize, showsTitles: Bool) -> NSSize {
        NSSize(
            width: container.width,
            height: max(0, container.height - reservedHeight(showsTitles: showsTitles))
        )
    }

    /// 计划格子没变就不要发 resize；变了只发这一次。
    static func shouldSend(previous: (UInt16, UInt16)?, next: (UInt16, UInt16)) -> Bool {
        guard let previous else { return true }
        return previous != next
    }
}

/// 递归二叉树 Pane 分割布局（对应 CLayoutNode）。
///
/// 全程 Auto Layout，避免 frame/AL 混用导致子视图 bounds=0 → SwiftTerm 黑屏。
/// 每个 Workspace 一份已建 tab 树；切工作区 / 切 tab 只挂已有 host，不拆 SwiftTerm。
final class PaneLayoutView: NSView, TerminalClientContentSizing {
    private struct CachedTabTree {
        var layout: LayoutNode
        var paneIds: Set<UInt32>
        var rootView: NSView
        var hostByPane: [UInt32: PaneHostView]
        var activePaneId: UInt32
    }

    private struct ParkedWorkspace {
        var tabTrees: [UInt32: CachedTabTree]
        var currentTabId: UInt32?
        var lastPanes: [Pane]
        var baseLayout: LayoutNode?
        var fullscreenPaneId: UInt32?
        var lastLayoutBounds: (Int, Int)
    }

    private var terminalManager: TerminalManager
    private var rootView: NSView?
    private var rootConstraints: [NSLayoutConstraint] = []
    private var currentLayout: LayoutNode?
    /// 未全屏时的原始布局（全屏恢复用；tmux 模式由 core zoom 驱动）。
    private var baseLayout: LayoutNode?
    /// 本地 shell 全屏的目标 pane；tmux 模式保持 nil（core 负责 zoom）。
    private var fullscreenPaneId: UInt32?
    private var lastPanes: [Pane] = []
    private var forceRebuild = false
    private var focusedPaneId: UInt32 = 0
    private var hostByPane: [UInt32: PaneHostView] = [:]
    private var currentPaneIds = Set<UInt32>()
    private var currentTabId: UInt32?
    private var tabTrees: [UInt32: CachedTabTree] = [:]
    private var parkedByManager: [ObjectIdentifier: ParkedWorkspace] = [:]
    private var lastLayoutBounds: (Int, Int) = (0, 0)
    private var geometrySyncScheduled = false
    private var pendingGeometryPaneIds: Set<UInt32>?
    private var pendingSyncKind: GeometrySyncKind = .window
    var onActivatePane: ((UInt32) -> Void)?
    var onMovePaneToNewTab: ((UInt32) -> Void)?
    var onSwapPanes: ((UInt32, UInt32) -> Void)?
    var onPaneTitleAction: ((UInt32, PaneTitleAction) -> Void)?
    /// 标题栏「移动」菜单在弹出时询问最新目标。nil tab = 新建。
    var moveDestinationsProvider: (() -> [(tabId: UInt32?, title: String)])?
    var allowsPaneBreak = false {
        didSet {
            for host in hostByPane.values {
                host.setAllowsMoveToNewTab(allowsPaneBreak && currentPaneIds.count > 1)
                host.setTitleDragEnabled(allowsPaneBreak && currentPaneIds.count > 1)
            }
        }
    }
    /// 分隔条释放后提交：pane、横向（宽度）/纵向（高度）、字符格尺寸。
    var onResizeDivider: ((UInt32, Bool, UInt16) -> Void)?
    /// Surface 从 seeding 变为可显示。调用方应把键盘交还给 active pane。
    var onSurfaceBecameReady: ((UInt32, Bool) -> Void)?

    init(terminalManager: TerminalManager) {
        self.terminalManager = terminalManager
        super.init(frame: .zero)
        bindSurfaceReadiness(to: terminalManager)
        wantsLayer = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        setAccessibilityIdentifier("muxterm.paneLayout")
    }

    /// 切 Workspace：把当前 Surface 树停到旧 manager 名下，挂上新 manager 已有的树。
    /// 返回是否成功挂上停驻树（切回已打开过的 Workspace 时为 true）。
    @discardableResult
    func replaceTerminalManager(_ newManager: TerminalManager) -> Bool {
        guard newManager !== terminalManager else { return currentTabId != nil }
        terminalManager.onSurfaceReadinessChanged = nil
        let parkedTabId = currentTabId
        parkCurrentTab()
        parkedByManager[ObjectIdentifier(terminalManager)] = ParkedWorkspace(
            tabTrees: tabTrees,
            currentTabId: parkedTabId,
            lastPanes: lastPanes,
            baseLayout: baseLayout,
            fullscreenPaneId: fullscreenPaneId,
            lastLayoutBounds: lastLayoutBounds
        )
        detachRoot()
        tabTrees.removeAll()
        currentTabId = nil
        currentLayout = nil
        baseLayout = nil
        fullscreenPaneId = nil
        lastPanes = []
        forceRebuild = false
        hostByPane.removeAll()
        currentPaneIds.removeAll()
        lastLayoutBounds = (0, 0)
        terminalManager = newManager
        bindSurfaceReadiness(to: newManager)
        var restored = false
        if let parked = parkedByManager.removeValue(forKey: ObjectIdentifier(newManager)) {
            tabTrees = parked.tabTrees
            lastPanes = parked.lastPanes
            baseLayout = parked.baseLayout
            fullscreenPaneId = parked.fullscreenPaneId
            lastLayoutBounds = parked.lastLayoutBounds
            let tabId = parked.currentTabId ?? parked.tabTrees.keys.first
            if let tabId, let cached = tabTrees[tabId] {
                installCached(cached, tabId: tabId)
                restored = true
            }
        }
        needsLayout = true
        return restored
    }

    /// 淘汰后丢掉已经没有 slot 的停驻树，避免 SwiftTerm 泄漏。
    func dropParked(except managers: [TerminalManager]) {
        let keep = Set(managers.map { ObjectIdentifier($0) })
        parkedByManager = parkedByManager.filter { keep.contains($0.key) }
    }

    private func bindSurfaceReadiness(to manager: TerminalManager) {
        manager.onSurfaceReadinessChanged = { [weak self] paneId, ready in
            self?.setSurfaceReady(paneId: paneId, ready: ready)
            self?.onSurfaceBecameReady?(paneId, ready)
        }
    }

    private func setSurfaceReady(paneId: UInt32, ready: Bool) {
        if let host = hostByPane[paneId] {
            host.setSurfaceReady(ready)
            return
        }
        for cache in tabTrees.values {
            if let host = cache.hostByPane[paneId] {
                host.setSurfaceReady(ready)
                return
            }
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    /// 根据布局树挂载子视图。同一 tab 第二次进入只显示缓存树。
    ///
    /// 返回 false 表示 layout 与当前 tab 的 pane 快照不一致；调用方应保留
    /// reload 标记等待后端的下一帧，不能把旧 tab 的 pane 树套到新 tab 上。
    @discardableResult
    func apply(layout: LayoutNode?, panes: [Pane], tabId: UInt32) -> Bool {
        lastPanes = panes
        updatePaneTitles(panes, hosts: hostByPane)
        if let cached = tabTrees[tabId] {
            updatePaneTitles(panes, hosts: cached.hostByPane)
        }
        if let layout {
            baseLayout = layout
        }
        let expectedPaneIDs = panes.map(\.id)
        var tree: LayoutNode?
        if panes.isEmpty {
            tree = nil
        } else if let layout {
            let treePaneIDs = layout.leafPaneIDs()
            guard PaneLayoutProjection.accepts(
                treePaneIDs: treePaneIDs,
                paneIDs: expectedPaneIDs
            ) else {
                return false
            }
            tree = layout
        } else if panes.count == 1 {
            tree = .leaf(paneId: panes[0].id)
        } else {
            return false
        }
        // 全屏（本地 shell）：只渲染目标 pane 的叶子。
        if let full = PaneFullscreenPolicy.resolvedFullscreenId(
            fullscreenPaneId: fullscreenPaneId,
            paneIDs: expectedPaneIDs
        ) {
            tree = .leaf(paneId: full)
        }

        // 缓存记录的是实际可见投影；zoom 时完整 pane 快照仍含隐藏兄弟。
        // 用完整列表比较会在每次快照/切 tab 时拆树，反复改变终端 allocation。
        let expectedIds = Set(tree?.leafPaneIDs() ?? [])
        let previousIds = currentTabId == tabId ? currentPaneIds : (tabTrees[tabId]?.paneIds ?? [])
        let previousFocus = currentTabId == tabId
            ? focusedPaneId
            : (tabTrees[tabId]?.activePaneId ?? focusedPaneId)
        let active = PaneFocusStickiness.resolvedActive(
            snapshotActive: panes.first(where: \.isActive)?.id,
            currentActive: previousFocus == 0 ? nil : previousFocus,
            visibleIds: expectedIds,
            previousVisibleIds: previousIds
        ) ?? 0
        if !forceRebuild, tabId == currentTabId, tree == currentLayout, expectedIds == currentPaneIds {
            markActivePane(active)
            return true
        }
        if !forceRebuild, let cached = tabTrees[tabId], cached.layout == tree, cached.paneIds == expectedIds {
            if tabId != currentTabId {
                parkCurrentTab()
                installCached(cached, tabId: tabId)
            }
            markActivePane(active)
            return true
        }
        forceRebuild = false

        parkCurrentTab()
        let reusedHosts = tabTrees[tabId]?.hostByPane ?? [:]
        tabTrees.removeValue(forKey: tabId)
        currentTabId = tabId

        guard let tree else {
            // 不销毁任何 pane 视图：tab 切换 / 布局重建必须保留 SwiftTerm
            // 状态，只有 STATE_PANE_CLOSED 才移除视图。
            return true
        }

        currentLayout = tree
        hostByPane = reusedHosts
        let ids = Set(collectPaneIds(tree))
        let showsTitles = PaneTitleLayoutPolicy.showsTitleBar(visiblePaneCount: ids.count)
        let built = build(node: tree, showsTitles: showsTitles)
        hostByPane = hostByPane.filter { ids.contains($0.key) }
        attachRoot(built)

        currentPaneIds = ids
        for host in hostByPane.values {
            host.setAllowsMoveToNewTab(allowsPaneBreak && ids.count > 1)
            host.setTitleDragEnabled(allowsPaneBreak && ids.count > 1)
        }
        markActivePane(active)
        tabTrees[tabId] = CachedTabTree(
            layout: tree,
            paneIds: ids,
            rootView: built,
            hostByPane: hostByPane,
            activePaneId: active
        )

        needsLayout = true
        // 窗口外框没变可以省略 refresh-client -C（hysteresis 在
        // syncAllVisibleSizes 里）。换了一棵 pane 树仍要按每个 host
        // 的像素重算 SwiftTerm 格子，否则切 tab 后字体/结构会错，
        // 直到用户再点一下 pane。
        if TabGeometrySyncPolicy.needsPaneGridSync(treeChanged: true) {
            scheduleGeometrySync(paneIds: ids, kind: .treeChange)
        }
        return true
    }

    func hasCachedTab(_ tabId: UInt32) -> Bool {
        tabTrees[tabId] != nil || currentTabId == tabId
    }

    /// 乐观切 tab：缓存命中时立刻挂树。返回该 tab 上次的活动 pane。
    @discardableResult
    func revealCachedTab(_ tabId: UInt32) -> UInt32? {
        guard let cached = tabTrees[tabId] else { return nil }
        if tabId == currentTabId {
            return cached.activePaneId
        }
        parkCurrentTab()
        installCached(cached, tabId: tabId)
        return cached.activePaneId
    }

    func pruneTabs(keeping ids: Set<UInt32>) {
        for key in tabTrees.keys where !ids.contains(key) {
            tabTrees.removeValue(forKey: key)
        }
    }

    func testLeafPaneIDs() -> [UInt32] {
        guard let currentLayout else { return [] }
        return collectPaneIds(currentLayout)
    }

    func testHost(for paneId: UInt32) -> PaneHostView? {
        if let host = hostByPane[paneId] {
            return host
        }
        for cache in tabTrees.values {
            if let host = cache.hostByPane[paneId] {
                return host
            }
        }
        return nil
    }

    private func parkCurrentTab() {
        guard let tabId = currentTabId, let root = rootView, let layout = currentLayout else {
            detachRoot()
            return
        }
        tabTrees[tabId] = CachedTabTree(
            layout: layout,
            paneIds: currentPaneIds,
            rootView: root,
            hostByPane: hostByPane,
            activePaneId: currentPaneIds.contains(focusedPaneId)
                ? focusedPaneId
                : (lastPanes.first(where: \.isActive)?.id ?? lastPanes.first?.id ?? 0)
        )
        // 先清空 pane 集合，detach 触发的 layout() 才不会再 schedule
        // geometry sync / refresh-client -C。
        currentPaneIds.removeAll()
        pendingGeometryPaneIds = nil
        pendingSyncKind = .window
        detachRoot()
        currentLayout = nil
        hostByPane.removeAll()
        currentTabId = nil
    }

    private func detachRoot() {
        if let rootView {
            NSLayoutConstraint.deactivate(rootConstraints)
            rootConstraints = []
            rootView.removeFromSuperview()
        }
        rootView = nil
        pendingGeometryPaneIds = nil
    }

    private func attachRoot(_ view: NSView) {
        view.translatesAutoresizingMaskIntoConstraints = false
        if view.superview !== self {
            view.removeFromSuperview()
            addSubview(view)
        }
        rootConstraints = [
            view.leadingAnchor.constraint(equalTo: leadingAnchor),
            view.trailingAnchor.constraint(equalTo: trailingAnchor),
            view.topAnchor.constraint(equalTo: topAnchor),
            view.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]
        NSLayoutConstraint.activate(rootConstraints)
        rootView = view
    }

    private func installCached(_ cached: CachedTabTree, tabId: UInt32) {
        attachRoot(cached.rootView)
        currentTabId = tabId
        currentLayout = cached.layout
        hostByPane = cached.hostByPane
        currentPaneIds = cached.paneIds
        // 不要在这里写 lastLayoutBounds：窗口外框没变时 layout() 会跳过，
        // 但停驻树可能是 0×0 建的，必须按现在的 host 像素重算格子。
        let showsTitles = PaneTitleLayoutPolicy.showsTitleBar(
            visiblePaneCount: currentPaneIds.count
        )
        for host in hostByPane.values {
            host.setAllowsMoveToNewTab(allowsPaneBreak && currentPaneIds.count > 1)
            host.setShowsTitleBar(showsTitles)
            host.setTitleDragEnabled(allowsPaneBreak && currentPaneIds.count > 1)
        }
        markActivePane(cached.activePaneId)
        if TabGeometrySyncPolicy.shouldSyncOnCachedReveal() {
            scheduleGeometrySync(paneIds: cached.paneIds, kind: .cachedReveal)
        }
    }

    func testPaneAllocation(_ paneId: UInt32) -> NSSize {
        hostByPane[paneId]?.bounds.size ?? .zero
    }

    func testPaneSurfaceVisible(_ paneId: UInt32) -> Bool {
        hostByPane[paneId]?.isHidden == false
    }

    func testPaneTitleVisible(_ paneId: UInt32) -> Bool {
        hostByPane[paneId]?.isTitleBarVisibleForTesting == true
    }

    func testPaneTitle(_ paneId: UInt32) -> String? {
        hostByPane[paneId]?.titleForTesting
    }

    func testPaneTerminalHeight(_ paneId: UInt32) -> CGFloat {
        hostByPane[paneId]?.terminalHeightForTesting ?? 0
    }

    var terminalClientContentSize: NSSize {
        PaneTitleBarGeometry.clientContentSize(
            container: bounds.size,
            showsTitles: PaneTitleLayoutPolicy.showsTitleBar(visiblePaneCount: currentPaneIds.count)
        )
    }

    func testPaneTitleActions(_ paneId: UInt32) -> [PaneTitleAction] {
        hostByPane[paneId]?.titleActionsForTesting ?? []
    }

    func testTriggerPaneTitleAction(_ paneId: UInt32, action: PaneTitleAction) {
        hostByPane[paneId]?.triggerTitleAction(action)
    }

    func testMovePaneToNewTab(_ paneId: UInt32) {
        hostByPane[paneId]?.triggerMoveToNewTab()
    }

    func testHandlePaneDrag(_ paneId: UInt32, to destination: UInt32?) {
        handlePaneDrag(source: paneId, destination: destination)
    }

    fileprivate func handlePaneDrag(source: UInt32, destination: UInt32?) {
        switch PaneDragLayoutPolicy.action(source: source, destination: destination) {
        case .swap(let other):
            onSwapPanes?(source, other)
        case .moveToNewTab:
            onMovePaneToNewTab?(source)
        case .cancel:
            break
        }
    }

    func paneId(atWindowPoint point: NSPoint) -> UInt32? {
        for (id, host) in hostByPane {
            let local = host.convert(point, from: nil)
            if host.bounds.contains(local) {
                return id
            }
        }
        return nil
    }

    /// 本地 shell：切换 pane 全屏（再次调用恢复）。tmux 模式走 core zoom，
    /// 不调用这里。
    func toggleFullscreen(paneId: UInt32) {
        guard lastPanes.contains(where: { $0.id == paneId }) else { return }
        fullscreenPaneId = fullscreenPaneId == paneId ? nil : paneId
        forceRebuild = true
        _ = apply(layout: baseLayout, panes: lastPanes, tabId: currentTabId ?? 0)
    }

    /// 本地 shell 在全屏状态下切到另一个 pane；保持全屏，只替换显示目标。
    func setFullscreenPane(paneId: UInt32) {
        guard fullscreenPaneId != nil,
              fullscreenPaneId != paneId,
              lastPanes.contains(where: { $0.id == paneId })
        else {
            return
        }
        fullscreenPaneId = paneId
        forceRebuild = true
        _ = apply(layout: baseLayout, panes: lastPanes, tabId: currentTabId ?? 0)
    }

    var testFullscreenPaneID: UInt32? {
        fullscreenPaneId
    }

    /// bridge 查询在缓存激活阶段暂停后，重新安排当前树的尺寸同步；
    /// `layout()` 可能认为窗口尺寸没有变化，因此不能只依赖 AppKit 再次布局。
    func resumeGeometrySync() {
        guard !currentPaneIds.isEmpty else { return }
        scheduleGeometrySync(paneIds: currentPaneIds)
    }

    /// 全屏 pane 乐观切换后，目标 host 已经放大，但 tmux 的尺寸事件可能
    /// 还没返回。Auto Layout 完成后立即用真实 allocation 更新 Surface。
    func refreshSurfaceGeometry(paneId: UInt32) {
        guard currentPaneIds.contains(paneId) else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.currentPaneIds.contains(paneId) else { return }
            self.layoutSubtreeIfNeeded()
            self.hostByPane[paneId]?.layoutSubtreeIfNeeded()
            self.terminalManager.syncSurfaceToAllocatedSize(paneId: paneId)
        }
    }

    /// 更新活跃 pane 高亮、未聚焦蒙层与 AX（供 Cmd+[ / ] 焦点跟随断言）。
    func markActivePane(_ paneId: UInt32) {
        focusedPaneId = paneId
        let visibleCount = hostByPane.count
        for (id, host) in hostByPane {
            host.setActive(id == paneId, visiblePaneCount: visibleCount)
        }
        if let tabId = currentTabId, var cached = tabTrees[tabId] {
            cached.activePaneId = paneId
            tabTrees[tabId] = cached
        }
    }

    func updatePaneTitle(paneId: UInt32, title: String) {
        lastPanes = lastPanes.map { pane in
            guard pane.id == paneId else { return pane }
            return Pane(
                id: pane.id,
                cols: pane.cols,
                rows: pane.rows,
                isActive: pane.isActive,
                title: title
            )
        }
        hostByPane[paneId]?.setTitle(title)
        for cached in tabTrees.values {
            cached.hostByPane[paneId]?.setTitle(title)
        }
    }

    private func finalizeAfterLayout(
        paneIds: Set<UInt32>,
        kind: GeometrySyncKind,
        attempt: Int
    ) {
        guard paneIds == currentPaneIds else { return }
        layoutSubtreeIfNeeded()
        if (bounds.width < 8 || bounds.height < 8), attempt < 10 {
            DispatchQueue.main.async { [weak self] in
                self?.finalizeAfterLayout(
                    paneIds: paneIds,
                    kind: kind,
                    attempt: attempt + 1
                )
            }
            return
        }
        for host in hostByPane.values {
            host.publishGeometry()
        }
        terminalManager.syncAllVisibleSizes(
            paneIds: paneIds,
            container: self,
            pinToAllocation: GeometrySyncPolicy.pinLocalGrids(kind),
            clientTreeChanged: GeometrySyncPolicy.clientTreeChanged(kind)
        )
        if GeometrySyncPolicy.forceRedraw(kind) {
            terminalManager.forceRedraw(paneIds: paneIds)
        }
    }

    override func layout() {
        super.layout()
        guard !currentPaneIds.isEmpty else { return }
        let token = (Int(bounds.width.rounded()), Int(bounds.height.rounded()))
        guard token != lastLayoutBounds else { return }
        lastLayoutBounds = token
        scheduleGeometrySync(paneIds: currentPaneIds, kind: .window)
    }

    private func scheduleGeometrySync(
        paneIds: Set<UInt32>,
        kind: GeometrySyncKind = .window
    ) {
        guard paneIds == currentPaneIds else { return }
        pendingGeometryPaneIds = paneIds
        pendingSyncKind = GeometrySyncPolicy.merge(pendingSyncKind, kind)
        guard !geometrySyncScheduled else { return }
        geometrySyncScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.geometrySyncScheduled = false
            let latestPaneIds = self.pendingGeometryPaneIds ?? self.currentPaneIds
            let kind = self.pendingSyncKind
            self.pendingGeometryPaneIds = nil
            self.pendingSyncKind = .window
            self.finalizeAfterLayout(
                paneIds: latestPaneIds,
                kind: kind,
                attempt: 0
            )
        }
    }

    private func build(node: LayoutNode, showsTitles: Bool) -> NSView {
        switch node {
        case .leaf(let paneId):
            if let existing = hostByPane[paneId] {
                existing.removeFromSuperview()
                existing.setShowsTitleBar(showsTitles)
                return existing
            }
            let term = terminalManager.view(for: paneId)
            let title = lastPanes.first(where: { $0.id == paneId })?.title ?? ""
            let wrap = PaneHostView(paneId: paneId, title: title, terminal: term)
            // 尚未完成 Runtime seed / 首批 PTY 的 pane 先藏起来，避免白屏。
            wrap.setSurfaceReady(terminalManager.isSurfaceReady(for: paneId))
            wrap.onActivate = { [weak self] id in
                self?.onActivatePane?(id)
            }
            wrap.onMoveToNewTab = { [weak self] id in
                self?.onMovePaneToNewTab?(id)
            }
            wrap.onSwapPanes = { [weak self] a, b in
                self?.onSwapPanes?(a, b)
            }
            wrap.dropTargetProvider = { [weak self] windowPoint in
                self?.paneId(atWindowPoint: windowPoint)
            }
            wrap.onTitleAction = { [weak self] id, action in
                self?.onPaneTitleAction?(id, action)
            }
            wrap.moveDestinationsProvider = { [weak self] in
                self?.moveDestinationsProvider?() ?? []
            }
            wrap.setAllowsMoveToNewTab(false)
            wrap.setShowsTitleBar(showsTitles)
            wrap.setTitleDragEnabled(false)
            hostByPane[paneId] = wrap
            return wrap

        case .split(let horizontal, let ratio, let first, let second):
            let firstPaneID = edgePaneID(in: first)
            return SplitContainerView(
                horizontal: horizontal,
                ratio: CGFloat(ratio) / 1000.0,
                first: build(node: first, showsTitles: showsTitles),
                second: build(node: second, showsTitles: showsTitles),
                firstPaneID: firstPaneID,
                onResize: { [weak self] paneID, isHorizontal, extent in
                    self?.commitDividerResize(
                        paneID: paneID,
                        horizontal: isHorizontal,
                        firstExtent: extent
                    )
                }
            )
        }
    }

    /// 返回 first 子树靠近当前 split 外侧分隔线的叶子 pane。
    private func edgePaneID(in node: LayoutNode) -> UInt32 {
        switch node {
        case .leaf(let paneId):
            return paneId
        case .split(_, _, _, let second):
            // first 在横向布局的右边界、纵向布局的下边界与外部分隔线相邻。
            return edgePaneID(in: second)
        }
    }

    private func commitDividerResize(paneID: UInt32, horizontal: Bool, firstExtent: CGFloat) {
        guard let cell = terminalManager.cellSizeInPixels(paneIds: currentPaneIds) else { return }
        let backing = convertToBacking(
            NSRect(x: 0, y: 0, width: firstExtent, height: firstExtent)
        )
        let pixels = horizontal ? backing.width : backing.height
        let cellPixels = horizontal ? cell.width : cell.height
        guard let size = PaneResizeMath.characterCount(
            pixelLength: Double(pixels), cellPixels: cellPixels
        ) else { return }
        onResizeDivider?(paneID, horizontal, size)
    }

    private func collectPaneIds(_ node: LayoutNode) -> [UInt32] {
        switch node {
        case .leaf(let id):
            return [id]
        case .split(_, _, let first, let second):
            return collectPaneIds(first) + collectPaneIds(second)
        }
    }

    private func updatePaneTitles(_ panes: [Pane], hosts: [UInt32: PaneHostView]) {
        for pane in panes {
            hosts[pane.id]?.setTitle(pane.title)
        }
    }
}

/// 点穿的黑色蒙层：视觉上把未聚焦 pane 向黑色插值，不截获鼠标。
private final class InactivePaneDimView: NSView {
    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.withAlphaComponent(
            InactivePaneDimmingPolicy.amount
        ).cgColor
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }
}

/// 承载单个终端；暴露几何 AX 供布局比例测试。
final class PaneHostView: NSView {
    let paneId: UInt32
    var onActivate: ((UInt32) -> Void)?
    var onMoveToNewTab: ((UInt32) -> Void)?
    var onSwapPanes: ((UInt32, UInt32) -> Void)?
    var dropTargetProvider: ((NSPoint) -> UInt32?)?
    var onTitleAction: ((UInt32, PaneTitleAction) -> Void)?
    var moveDestinationsProvider: (() -> [(tabId: UInt32?, title: String)])? {
        didSet { titleBar.moveDestinationsProvider = moveDestinationsProvider }
    }
    private var isPaneActive = false
    private var allowsTitleBar = false
    private var visiblePaneCount = 1
    private let moveToNewTabItem: NSMenuItem
    private let moveSeparator: NSMenuItem
    private let titleBar: PaneTitleBarView
    private let terminal: MuxTerminalView
    private let dimOverlay = InactivePaneDimView()
    private var titleBarHeightConstraint: NSLayoutConstraint!
    private var titleTrackingArea: NSTrackingArea?

    init(paneId: UInt32, title: String = "", terminal: MuxTerminalView) {
        self.paneId = paneId
        self.terminal = terminal
        self.moveToNewTabItem = NSMenuItem(
            title: MuxtermI18n.shared.tr(.movePaneToNewTab),
            action: #selector(moveToNewTab(_:)),
            keyEquivalent: ""
        )
        self.moveSeparator = NSMenuItem.separator()
        self.titleBar = PaneTitleBarView(paneId: paneId, title: title)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.masksToBounds = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        translatesAutoresizingMaskIntoConstraints = false
        setAccessibilityIdentifier("muxterm.pane.\(paneId)")
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel(
            MuxtermI18n.shared.tr(.paneAccessibility, arguments: ["id": "\(paneId)"])
        )

        terminal.translatesAutoresizingMaskIntoConstraints = false
        if terminal.superview !== self {
            terminal.removeFromSuperview()
            addSubview(terminal)
        } else if !subviews.contains(terminal) {
            addSubview(terminal)
        }
        terminal.onActivatePane = { [weak self] id in
            self?.onActivate?(id)
        }
        titleBar.translatesAutoresizingMaskIntoConstraints = false
        titleBar.isHidden = true
        titleBar.onActivate = { [weak self] in
            guard let self else { return }
            self.onActivate?(self.paneId)
        }
        titleBar.onDragOut = { [weak self] windowPoint in
            guard let self else { return }
            let destination = self.dropTargetProvider?(windowPoint)
            switch PaneDragLayoutPolicy.action(source: self.paneId, destination: destination) {
            case .swap(let other):
                self.onActivate?(self.paneId)
                self.onSwapPanes?(self.paneId, other)
            case .moveToNewTab:
                self.triggerMoveToNewTab()
            case .cancel:
                break
            }
        }
        titleBar.onAction = { [weak self] action in
            guard let self else { return }
            self.onTitleAction?(self.paneId, action)
        }
        titleBar.moveDestinationsProvider = moveDestinationsProvider
        dimOverlay.translatesAutoresizingMaskIntoConstraints = false
        dimOverlay.isHidden = true
        addSubview(dimOverlay)
        // 标题在蒙层之上，终端从 host 顶部延伸到底部，不为 hover 重新布局。
        addSubview(titleBar)
        titleBarHeightConstraint = titleBar.heightAnchor.constraint(
            equalToConstant: PaneTitleBarGeometry.height
        )
        NSLayoutConstraint.activate([
            titleBar.leadingAnchor.constraint(equalTo: leadingAnchor),
            titleBar.trailingAnchor.constraint(equalTo: trailingAnchor),
            titleBar.topAnchor.constraint(equalTo: topAnchor),
            titleBarHeightConstraint,
            terminal.leadingAnchor.constraint(equalTo: leadingAnchor),
            terminal.trailingAnchor.constraint(equalTo: trailingAnchor),
            terminal.topAnchor.constraint(equalTo: topAnchor),
            terminal.bottomAnchor.constraint(equalTo: bottomAnchor),
            dimOverlay.leadingAnchor.constraint(equalTo: terminal.leadingAnchor),
            dimOverlay.trailingAnchor.constraint(equalTo: terminal.trailingAnchor),
            dimOverlay.topAnchor.constraint(equalTo: terminal.topAnchor),
            dimOverlay.bottomAnchor.constraint(equalTo: terminal.bottomAnchor),
        ])

        let contextMenu = NSMenu()
        contextMenu.addItem(
            withTitle: MuxtermI18n.shared.tr(.menuCopy),
            action: #selector(NSText.copy(_:)),
            keyEquivalent: ""
        )
        contextMenu.addItem(
            withTitle: MuxtermI18n.shared.tr(.menuPaste),
            action: #selector(NSText.paste(_:)),
            keyEquivalent: ""
        )
        contextMenu.addItem(
            withTitle: MuxtermI18n.shared.tr(.menuSelectAll),
            action: #selector(NSText.selectAll(_:)),
            keyEquivalent: ""
        )
        contextMenu.addItem(moveSeparator)
        moveToNewTabItem.target = self
        contextMenu.addItem(moveToNewTabItem)
        menu = contextMenu
        terminal.menu = contextMenu
        publishGeometry()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    func setActive(_ active: Bool, visiblePaneCount: Int = 1) {
        isPaneActive = active
        self.visiblePaneCount = visiblePaneCount
        // 1px 指示，避免厚边框「卡片」感
        layer?.borderWidth = active ? FlatChrome.activePaneBorderWidth : 0
        layer?.borderColor = active ? NSColor.controlAccentColor.cgColor : nil
        layer?.cornerRadius = 0
        titleBar.setActive(active)
        dimOverlay.isHidden = !InactivePaneDimmingPolicy.shouldDim(
            isActive: active,
            visiblePaneCount: visiblePaneCount
        )
        publishGeometry()
    }

    /// 首帧 seed 期间隐藏整个 host，而不是让 SwiftTerm 绘制半截 Surface。
    /// Auto Layout 仍保留 host 的几何尺寸，seed 完成后只需切换可见性。
    func setSurfaceReady(_ ready: Bool) {
        isHidden = !ready
        if ready {
            needsDisplay = true
        }
    }

    override var acceptsFirstResponder: Bool {
        PaneHostFocusPolicy.acceptsFirstResponder
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let titleTrackingArea { removeTrackingArea(titleTrackingArea) }
        let area = NSTrackingArea(
            rect: .zero,
            options: [.mouseEnteredAndExited, .mouseMoved, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        titleTrackingArea = area
    }

    override func mouseEntered(with event: NSEvent) {
        updateTitleHover(at: convert(event.locationInWindow, from: nil))
    }

    override func mouseMoved(with event: NSEvent) {
        updateTitleHover(at: convert(event.locationInWindow, from: nil))
    }

    override func mouseExited(with event: NSEvent) {
        titleBar.isHidden = true
    }

    private func updateTitleHover(at point: NSPoint) {
        titleBar.isHidden = !(allowsTitleBar && bounds.contains(point)
            && point.y >= bounds.maxY - PaneTitleBarGeometry.height)
    }

    func setAllowsMoveToNewTab(_ allowed: Bool) {
        moveSeparator.isHidden = !allowed
        moveToNewTabItem.isHidden = !allowed
    }

    func setShowsTitleBar(_ visible: Bool) {
        allowsTitleBar = visible
        titleBar.isHidden = true
    }

    func setTitle(_ title: String) {
        titleBar.setTitle(title)
    }

    func setTitleDragEnabled(_ enabled: Bool) {
        titleBar.dragEnabled = enabled
    }

    var isTitleBarVisibleForTesting: Bool { !titleBar.isHidden }
    var titleForTesting: String { titleBar.titleForTesting }
    var terminalHeightForTesting: CGFloat { terminal.frame.height }
    var titleActionsForTesting: [PaneTitleAction] { titleBar.actionsForTesting }
    var isContentDimmedForTesting: Bool { !dimOverlay.isHidden }
    var dimOverlayFrameForTesting: NSRect { dimOverlay.frame }
    var titleBarFrameForTesting: NSRect { titleBar.frame }

    func setTitleHoveredForTesting(_ hovered: Bool) {
        titleBar.isHidden = !(allowsTitleBar && hovered)
    }

    func dimOverlayBlocksHitsForTesting(at point: NSPoint) -> Bool {
        dimOverlay.hitTest(point) != nil
    }

    func triggerTitleAction(_ action: PaneTitleAction) {
        guard !titleBar.isHidden else { return }
        onActivate?(paneId)
        onTitleAction?(paneId, action)
    }

    func triggerMoveToNewTab() {
        guard !moveToNewTabItem.isHidden else { return }
        onActivate?(paneId)
        onMoveToNewTab?(paneId)
    }

    @objc private func moveToNewTab(_ sender: Any?) {
        triggerMoveToNewTab()
    }

    func publishGeometry() {
        let w = Int(bounds.width.rounded())
        let h = Int(bounds.height.rounded())
        let dimmed = InactivePaneDimmingPolicy.shouldDim(
            isActive: isPaneActive,
            visiblePaneCount: visiblePaneCount
        )
        let text = "pane=@\(paneId) w=\(w) h=\(h) active=\(isPaneActive ? 1 : 0) dim=\(dimmed ? 1 : 0)"
        setAccessibilityValue(text)
        setAccessibilityHelp(text)
    }

    override func layout() {
        super.layout()
        publishGeometry()
    }

    override func mouseDown(with event: NSEvent) {
        onActivate?(paneId)
        super.mouseDown(with: event)
    }
}

/// 多 Pane 时位于 Surface 上方的固定标题行；单 Pane 高度为 0。
/// 标题行参与 Auto Layout，终端字符格按剩余真实高度同步给 Runtime。
private final class PaneTitleBarView: NSView {
    var onActivate: (() -> Void)?
    var onDragOut: ((NSPoint) -> Void)?
    var onAction: ((PaneTitleAction) -> Void)?
    var moveDestinationsProvider: (() -> [(tabId: UInt32?, title: String)])?
    var dragEnabled = false
    private let label = NSTextField(labelWithString: "")
    private let actionButton = NSButton()
    private let actionMenu = NSMenu()
    private let moveMenu = NSMenu()
    private let moveItem: NSMenuItem
    private let paneId: UInt32

    init(paneId: UInt32, title: String) {
        self.paneId = paneId
        self.moveItem = NSMenuItem(
            title: MuxtermI18n.shared.tr(.movePane),
            action: nil,
            keyEquivalent: ""
        )
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        layer?.borderColor = NSColor.separatorColor.withAlphaComponent(0.65).cgColor
        layer?.borderWidth = 0.5

        label.translatesAutoresizingMaskIntoConstraints = false
        label.font = NSFont.systemFont(ofSize: 11, weight: .medium)
        label.textColor = .secondaryLabelColor
        label.lineBreakMode = .byTruncatingTail
        addSubview(label)
        setTitle(title)

        configureActionMenu()
        actionButton.translatesAutoresizingMaskIntoConstraints = false
        actionButton.image = NSImage(
            systemSymbolName: "ellipsis.circle",
            accessibilityDescription: MuxtermI18n.shared.tr(.paneActions)
        )
        actionButton.imagePosition = .imageOnly
        actionButton.isBordered = false
        actionButton.bezelStyle = .inline
        actionButton.focusRingType = .none
        actionButton.contentTintColor = .secondaryLabelColor
        actionButton.target = self
        actionButton.action = #selector(showActionMenu(_:))
        actionButton.toolTip = MuxtermI18n.shared.tr(.paneActions)
        actionButton.setAccessibilityIdentifier("muxterm.paneActions.\(paneId)")
        addSubview(actionButton)

        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 7),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            label.trailingAnchor.constraint(lessThanOrEqualTo: actionButton.leadingAnchor, constant: -6),
            actionButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            actionButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            actionButton.widthAnchor.constraint(equalToConstant: 18),
            actionButton.heightAnchor.constraint(equalToConstant: 18),
        ])
        setAccessibilityRole(.group)
        setAccessibilityLabel(label.stringValue)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    func setActive(_ active: Bool) {
        label.textColor = active ? .controlAccentColor : .secondaryLabelColor
        layer?.backgroundColor = (active
            ? NSColor.controlAccentColor.withAlphaComponent(0.10)
            : NSColor.windowBackgroundColor
        ).cgColor
    }

    func setTitle(_ title: String) {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        label.stringValue = trimmed.isEmpty ? "Pane @\(paneId)" : trimmed
        label.toolTip = label.stringValue
        setAccessibilityLabel(label.stringValue)
    }

    var titleForTesting: String { label.stringValue }
    var actionsForTesting: [PaneTitleAction] {
        var items: [PaneTitleAction] = [.splitHorizontal, .splitVertical, .fullscreen, .cycleLayout]
        for destination in moveDestinationsProvider?() ?? [] {
            if let tabId = destination.tabId {
                items.append(.moveToTab(tabId))
            } else {
                items.append(.moveToNewTab)
            }
        }
        items.append(.close)
        return items
    }

    private func configureActionMenu() {
        let horizontal = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuSplitHorizontal),
            action: #selector(splitHorizontal(_:)),
            keyEquivalent: ""
        )
        horizontal.target = self
        actionMenu.addItem(horizontal)

        let vertical = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuSplitVertical),
            action: #selector(splitVertical(_:)),
            keyEquivalent: ""
        )
        vertical.target = self
        actionMenu.addItem(vertical)
        actionMenu.addItem(.separator())

        moveItem.submenu = moveMenu
        actionMenu.addItem(moveItem)
        actionMenu.addItem(.separator())

        let fullscreen = NSMenuItem(
            title: MuxtermI18n.shared.tr(.togglePaneFullscreen),
            action: #selector(toggleFullscreen(_:)),
            keyEquivalent: ""
        )
        fullscreen.target = self
        actionMenu.addItem(fullscreen)

        let cycle = NSMenuItem(
            title: MuxtermI18n.shared.tr(.cycleLayout),
            action: #selector(cycleLayout(_:)),
            keyEquivalent: ""
        )
        cycle.target = self
        actionMenu.addItem(cycle)
        actionMenu.addItem(.separator())

        let close = NSMenuItem(
            title: MuxtermI18n.shared.tr(.menuClosePane),
            action: #selector(closePane(_:)),
            keyEquivalent: ""
        )
        close.target = self
        actionMenu.addItem(close)
    }

    @objc private func showActionMenu(_ sender: NSButton) {
        onActivate?()
        rebuildMoveMenu()
        actionMenu.popUp(
            positioning: nil,
            at: NSPoint(x: sender.bounds.maxX, y: sender.bounds.minY),
            in: sender
        )
    }

    private func rebuildMoveMenu() {
        moveMenu.removeAllItems()
        let destinations = moveDestinationsProvider?() ?? []
        moveItem.isHidden = destinations.isEmpty
        for destination in destinations {
            let item = NSMenuItem(
                title: destination.title,
                action: #selector(movePane(_:)),
                keyEquivalent: ""
            )
            item.target = self
            if let tabId = destination.tabId {
                item.representedObject = NSNumber(value: tabId)
            }
            moveMenu.addItem(item)
        }
    }

    @objc private func movePane(_ sender: NSMenuItem) {
        if let tabId = (sender.representedObject as? NSNumber)?.uint32Value {
            onAction?(.moveToTab(tabId))
        } else {
            onAction?(.moveToNewTab)
        }
    }

    @objc private func splitHorizontal(_ sender: Any?) {
        onAction?(.splitHorizontal)
    }

    @objc private func splitVertical(_ sender: Any?) {
        onAction?(.splitVertical)
    }

    @objc private func toggleFullscreen(_ sender: Any?) {
        onAction?(.fullscreen)
    }

    @objc private func cycleLayout(_ sender: Any?) {
        onAction?(.cycleLayout)
    }

    @objc private func closePane(_ sender: Any?) {
        onAction?(.close)
    }

    override func mouseDown(with event: NSEvent) {
        onActivate?()
        guard dragEnabled, let window else { return }
        let start = event.locationInWindow
        var dragged = false
        while let next = window.nextEvent(
            matching: [.leftMouseDragged, .leftMouseUp],
            until: .distantFuture,
            inMode: .eventTracking,
            dequeue: true
        ) {
            if next.type == .leftMouseDragged {
                let dx = next.locationInWindow.x - start.x
                let dy = next.locationInWindow.y - start.y
                if hypot(dx, dy) >= 5 {
                    dragged = true
                    alphaValue = 0.65
                }
            } else if next.type == .leftMouseUp {
                alphaValue = 1
                if dragged { onDragOut?(next.locationInWindow) }
                return
            }
        }
    }
}

/// 二分容器：纯 Auto Layout，按 ratio 分配 first/second；分隔线同时是可拖动手柄。
private final class SplitContainerView: NSView {
    private let horizontal: Bool
    private let dividerLength: CGFloat
    private let first: NSView
    private let second: NSView
    private var currentRatio: CGFloat
    private var ratioConstraint: NSLayoutConstraint!
    private var dragStartPosition: CGFloat = 0
    private var dragStartRatio: CGFloat = 0.5
    private let firstPaneID: UInt32
    private let onResize: (UInt32, Bool, CGFloat) -> Void

    init(
        horizontal: Bool,
        ratio: CGFloat,
        first: NSView,
        second: NSView,
        firstPaneID: UInt32,
        onResize: @escaping (UInt32, Bool, CGFloat) -> Void
    ) {
        self.horizontal = horizontal
        self.first = first
        self.second = second
        self.currentRatio = CGFloat(PaneResizeMath.clampedRatio(Double(ratio)))
        self.dividerLength = PaneTitleBarGeometry.dividerLength
        self.firstPaneID = firstPaneID
        self.onResize = onResize
        super.init(frame: .zero)
        wantsLayer = true
        translatesAutoresizingMaskIntoConstraints = false

        let divider = DividerHandleView(horizontal: horizontal)
        divider.translatesAutoresizingMaskIntoConstraints = false
        divider.onMouseDown = { [weak self] event in self?.beginDrag(event) }
        divider.onMouseDragged = { [weak self] event in self?.drag(event) }
        divider.onMouseUp = { [weak self] _ in self?.endDrag() }

        first.translatesAutoresizingMaskIntoConstraints = false
        second.translatesAutoresizingMaskIntoConstraints = false

        addSubview(first)
        addSubview(divider)
        addSubview(second)

        // 6pt 的命中区域保证鼠标容易抓住，视觉仍只画 1pt 分隔线。
        let multiplier = currentRatio / (1.0 - currentRatio)
        divider.setAccessibilityIdentifier("muxterm.divider.\(firstPaneID)")
        divider.setAccessibilityElement(true)
        divider.setAccessibilityRole(.splitter)

        if horizontal {
            NSLayoutConstraint.activate([
                first.leadingAnchor.constraint(equalTo: leadingAnchor),
                first.topAnchor.constraint(equalTo: topAnchor),
                first.bottomAnchor.constraint(equalTo: bottomAnchor),

                divider.leadingAnchor.constraint(equalTo: first.trailingAnchor),
                divider.topAnchor.constraint(equalTo: topAnchor),
                divider.bottomAnchor.constraint(equalTo: bottomAnchor),
                divider.widthAnchor.constraint(equalToConstant: dividerLength),

                second.leadingAnchor.constraint(equalTo: divider.trailingAnchor),
                second.trailingAnchor.constraint(equalTo: trailingAnchor),
                second.topAnchor.constraint(equalTo: topAnchor),
                second.bottomAnchor.constraint(equalTo: bottomAnchor),

            ])
            ratioConstraint = first.widthAnchor
                .constraint(equalTo: second.widthAnchor, multiplier: multiplier)
                .withPriority(.defaultHigh)
        } else {
            NSLayoutConstraint.activate([
                first.leadingAnchor.constraint(equalTo: leadingAnchor),
                first.trailingAnchor.constraint(equalTo: trailingAnchor),
                first.topAnchor.constraint(equalTo: topAnchor),

                divider.leadingAnchor.constraint(equalTo: leadingAnchor),
                divider.trailingAnchor.constraint(equalTo: trailingAnchor),
                divider.topAnchor.constraint(equalTo: first.bottomAnchor),
                divider.heightAnchor.constraint(equalToConstant: dividerLength),

                second.leadingAnchor.constraint(equalTo: leadingAnchor),
                second.trailingAnchor.constraint(equalTo: trailingAnchor),
                second.topAnchor.constraint(equalTo: divider.bottomAnchor),
                second.bottomAnchor.constraint(equalTo: bottomAnchor),

            ])
            ratioConstraint = first.heightAnchor
                .constraint(equalTo: second.heightAnchor, multiplier: multiplier)
                .withPriority(.defaultHigh)
        }
        NSLayoutConstraint.activate([ratioConstraint])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    private func beginDrag(_ event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        // AppKit 默认坐标原点在左下角；布局树的 vertical 轴从上到下
        // 计算，所以先把 y 转成“距顶部”的坐标，拖动方向才与视觉一致。
        dragStartPosition = horizontal ? point.x : bounds.height - point.y
        dragStartRatio = currentRatio
    }

    private func drag(_ event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let position = horizontal ? point.x : bounds.height - point.y
        let total = horizontal ? bounds.width : bounds.height
        let next = PaneResizeMath.ratioAfterDrag(
            startRatio: Double(dragStartRatio),
            delta: Double(position - dragStartPosition),
            totalLength: Double(total),
            dividerLength: Double(dividerLength)
        )
        currentRatio = CGFloat(next)
        NSLayoutConstraint.deactivate([ratioConstraint])
        let multiplier = currentRatio / (1 - currentRatio)
        ratioConstraint = (horizontal
            ? first.widthAnchor.constraint(equalTo: second.widthAnchor, multiplier: multiplier)
            : first.heightAnchor.constraint(equalTo: second.heightAnchor, multiplier: multiplier)
        ).withPriority(.defaultHigh)
        NSLayoutConstraint.activate([ratioConstraint])
        needsLayout = true
        layoutSubtreeIfNeeded()
    }

    private func endDrag() {
        let extent = horizontal ? first.bounds.width : first.bounds.height
        onResize(firstPaneID, horizontal, extent)
    }
}

/// 分隔条的宽命中区域；mouseDown/dragged 都交给父 split。
private final class DividerHandleView: NSView {
    var onMouseDown: ((NSEvent) -> Void)?
    var onMouseDragged: ((NSEvent) -> Void)?
    var onMouseUp: ((NSEvent) -> Void)?

    init(horizontal: Bool) {
        self.horizontal = horizontal
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.separatorColor.withAlphaComponent(0.35).cgColor
    }

    private let horizontal: Bool

    override init(frame frameRect: NSRect) {
        self.horizontal = true
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.separatorColor.withAlphaComponent(0.35).cgColor
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: horizontal ? .resizeLeftRight : .resizeUpDown)
    }

    override func mouseDown(with event: NSEvent) {
        onMouseDown?(event)

        // AppKit 通常会把 mouseDragged 继续发给当前 view，但在嵌套
        // Auto Layout + live window resize 时，vertical divider 偶尔会丢掉
        // 后续事件。这里在 mouseDown 内显式抓取 left-drag/up，保证上下、
        // 左右分隔条走同一条可靠路径。
        guard let window else { return }
        while true {
            guard let next = window.nextEvent(
                matching: [.leftMouseDragged, .leftMouseUp],
                until: .distantFuture,
                inMode: .eventTracking,
                dequeue: true
            ) else {
                return
            }
            switch next.type {
            case .leftMouseDragged:
                onMouseDragged?(next)
            case .leftMouseUp:
                onMouseUp?(next)
                return
            default:
                continue
            }
        }
    }

    override func mouseDragged(with event: NSEvent) {
        onMouseDragged?(event)
    }

    override func mouseUp(with event: NSEvent) {
        onMouseUp?(event)
    }
}

private extension NSLayoutConstraint {
    func withPriority(_ priority: NSLayoutConstraint.Priority) -> NSLayoutConstraint {
        self.priority = priority
        return self
    }
}
