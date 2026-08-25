import AppKit
import MuxtermChrome

/// 递归二叉树 Pane 分割布局（对应 CLayoutNode）。
///
/// 全程 Auto Layout，避免 frame/AL 混用导致子视图 bounds=0 → SwiftTerm 黑屏。
/// 每个 Workspace 一份已建 tab 树；切工作区 / 切 tab 只挂已有 host，不拆 SwiftTerm。
final class PaneLayoutView: NSView {
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
    private var hostByPane: [UInt32: PaneHostView] = [:]
    private var currentPaneIds = Set<UInt32>()
    private var currentTabId: UInt32?
    private var tabTrees: [UInt32: CachedTabTree] = [:]
    private var parkedByManager: [ObjectIdentifier: ParkedWorkspace] = [:]
    private var lastLayoutBounds: (Int, Int) = (0, 0)
    private var geometrySyncScheduled = false
    private var pendingGeometryPaneIds: Set<UInt32>?
    var onActivatePane: ((UInt32) -> Void)?
    var onMovePaneToNewTab: ((UInt32) -> Void)?
    var allowsPaneBreak = false {
        didSet {
            for host in hostByPane.values {
                host.setAllowsMoveToNewTab(allowsPaneBreak && currentPaneIds.count > 1)
            }
        }
    }
    /// 分隔条释放后提交：pane、横向（宽度）/纵向（高度）、字符格尺寸。
    var onResizeDivider: ((UInt32, Bool, UInt16) -> Void)?

    init(terminalManager: TerminalManager) {
        self.terminalManager = terminalManager
        super.init(frame: .zero)
        bindSurfaceReadiness(to: terminalManager)
        wantsLayer = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        setAccessibilityIdentifier("muxterm.paneLayout")
    }

    /// 切 Workspace：把当前 Surface 树停到旧 manager 名下，挂上新 manager 已有的树。
    func replaceTerminalManager(_ newManager: TerminalManager) {
        guard newManager !== terminalManager else { return }
        terminalManager.onSurfaceReadinessChanged = nil
        parkCurrentTab()
        parkedByManager[ObjectIdentifier(terminalManager)] = ParkedWorkspace(
            tabTrees: tabTrees,
            currentTabId: currentTabId,
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
        if let parked = parkedByManager.removeValue(forKey: ObjectIdentifier(newManager)) {
            tabTrees = parked.tabTrees
            lastPanes = parked.lastPanes
            baseLayout = parked.baseLayout
            fullscreenPaneId = parked.fullscreenPaneId
            lastLayoutBounds = parked.lastLayoutBounds
            if let tabId = parked.currentTabId, let cached = tabTrees[tabId] {
                installCached(cached, tabId: tabId)
            }
        }
        needsLayout = true
    }

    /// 淘汰后丢掉已经没有 slot 的停驻树，避免 SwiftTerm 泄漏。
    func dropParked(except managers: [TerminalManager]) {
        let keep = Set(managers.map { ObjectIdentifier($0) })
        parkedByManager = parkedByManager.filter { keep.contains($0.key) }
    }

    private func bindSurfaceReadiness(to manager: TerminalManager) {
        manager.onSurfaceReadinessChanged = { [weak self] paneId, ready in
            self?.setSurfaceReady(paneId: paneId, ready: ready)
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

        let expectedIds = Set(expectedPaneIDs)
        let active = panes.first(where: \.isActive)?.id ?? panes.first?.id ?? 0
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
        tabTrees.removeValue(forKey: tabId)
        currentTabId = tabId

        guard let tree else {
            // 不销毁任何 pane 视图：tab 切换 / 布局重建必须保留 SwiftTerm
            // 状态，只有 STATE_PANE_CLOSED 才移除视图。
            return true
        }

        currentLayout = tree
        hostByPane.removeAll()
        let built = build(node: tree)
        attachRoot(built)

        let ids = Set(collectPaneIds(tree))
        currentPaneIds = ids
        for host in hostByPane.values {
            host.setAllowsMoveToNewTab(allowsPaneBreak && ids.count > 1)
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
        scheduleGeometrySync(paneIds: ids)
        return true
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
            activePaneId: tabTrees[tabId]?.activePaneId
                ?? lastPanes.first(where: \.isActive)?.id
                ?? lastPanes.first?.id
                ?? 0
        )
        detachRoot()
        currentLayout = nil
        hostByPane.removeAll()
        currentPaneIds.removeAll()
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
        for host in hostByPane.values {
            host.setAllowsMoveToNewTab(allowsPaneBreak && currentPaneIds.count > 1)
        }
        markActivePane(cached.activePaneId)
    }

    func testPaneAllocation(_ paneId: UInt32) -> NSSize {
        hostByPane[paneId]?.bounds.size ?? .zero
    }

    func testPaneSurfaceVisible(_ paneId: UInt32) -> Bool {
        hostByPane[paneId]?.isHidden == false
    }

    func testMovePaneToNewTab(_ paneId: UInt32) {
        hostByPane[paneId]?.triggerMoveToNewTab()
    }

    /// 本地 shell：切换 pane 全屏（再次调用恢复）。tmux 模式走 core zoom，
    /// 不调用这里。
    func toggleFullscreen(paneId: UInt32) {
        guard lastPanes.contains(where: { $0.id == paneId }) else { return }
        fullscreenPaneId = fullscreenPaneId == paneId ? nil : paneId
        forceRebuild = true
        _ = apply(layout: baseLayout, panes: lastPanes, tabId: currentTabId ?? 0)
    }

    /// 更新活跃 pane 高亮与 AX（供 Cmd+[ / ] 焦点跟随断言）。
    func markActivePane(_ paneId: UInt32) {
        for (id, host) in hostByPane {
            host.setActive(id == paneId)
        }
        if let tabId = currentTabId, var cached = tabTrees[tabId] {
            cached.activePaneId = paneId
            tabTrees[tabId] = cached
        }
    }

    private func finalizeAfterLayout(paneIds: Set<UInt32>, attempt: Int) {
        guard paneIds == currentPaneIds else { return }
        layoutSubtreeIfNeeded()
        if (bounds.width < 8 || bounds.height < 8), attempt < 10 {
            DispatchQueue.main.async { [weak self] in
                self?.finalizeAfterLayout(paneIds: paneIds, attempt: attempt + 1)
            }
            return
        }
        for host in hostByPane.values {
            host.publishGeometry()
        }
        terminalManager.syncAllVisibleSizes(paneIds: paneIds, container: self)
        terminalManager.forceRedraw(paneIds: paneIds)
    }

    override func layout() {
        super.layout()
        guard !currentPaneIds.isEmpty else { return }
        let token = (Int(bounds.width.rounded()), Int(bounds.height.rounded()))
        guard token != lastLayoutBounds else { return }
        lastLayoutBounds = token
        scheduleGeometrySync(paneIds: currentPaneIds)
    }

    private func scheduleGeometrySync(paneIds: Set<UInt32>) {
        guard paneIds == currentPaneIds else { return }
        pendingGeometryPaneIds = paneIds
        guard !geometrySyncScheduled else { return }
        geometrySyncScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.geometrySyncScheduled = false
            let latestPaneIds = self.pendingGeometryPaneIds ?? self.currentPaneIds
            self.pendingGeometryPaneIds = nil
            self.finalizeAfterLayout(paneIds: latestPaneIds, attempt: 0)
        }
    }

    private func build(node: LayoutNode) -> NSView {
        switch node {
        case .leaf(let paneId):
            let term = terminalManager.view(for: paneId)
            let wrap = PaneHostView(paneId: paneId, terminal: term)
            // 尚未完成 Runtime seed / 首批 PTY 的 pane 先藏起来，避免白屏。
            wrap.setSurfaceReady(terminalManager.isSurfaceReady(for: paneId))
            wrap.onActivate = { [weak self] id in
                self?.onActivatePane?(id)
            }
            wrap.onMoveToNewTab = { [weak self] id in
                self?.onMovePaneToNewTab?(id)
            }
            wrap.setAllowsMoveToNewTab(false)
            hostByPane[paneId] = wrap
            return wrap

        case .split(let horizontal, let ratio, let first, let second):
            let firstPaneID = edgePaneID(in: first)
            return SplitContainerView(
                horizontal: horizontal,
                ratio: CGFloat(ratio) / 1000.0,
                first: build(node: first),
                second: build(node: second),
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
}

/// 承载单个终端；暴露几何 AX 供布局比例测试。
final class PaneHostView: NSView {
    let paneId: UInt32
    var onActivate: ((UInt32) -> Void)?
    var onMoveToNewTab: ((UInt32) -> Void)?
    private var isPaneActive = false
    private let moveToNewTabItem: NSMenuItem
    private let moveSeparator: NSMenuItem

    init(paneId: UInt32, terminal: MuxTerminalView) {
        self.paneId = paneId
        self.moveToNewTabItem = NSMenuItem(
            title: MuxtermI18n.shared.tr(.movePaneToNewTab),
            action: #selector(moveToNewTab(_:)),
            keyEquivalent: ""
        )
        self.moveSeparator = NSMenuItem.separator()
        super.init(frame: .zero)
        wantsLayer = true
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
        NSLayoutConstraint.activate([
            terminal.leadingAnchor.constraint(equalTo: leadingAnchor),
            terminal.trailingAnchor.constraint(equalTo: trailingAnchor),
            terminal.topAnchor.constraint(equalTo: topAnchor),
            terminal.bottomAnchor.constraint(equalTo: bottomAnchor),
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

    func setActive(_ active: Bool) {
        isPaneActive = active
        // 1px 指示，避免厚边框「卡片」感
        layer?.borderWidth = active ? FlatChrome.activePaneBorderWidth : 0
        layer?.borderColor = active ? NSColor.controlAccentColor.cgColor : nil
        layer?.cornerRadius = 0
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

    func setAllowsMoveToNewTab(_ allowed: Bool) {
        moveSeparator.isHidden = !allowed
        moveToNewTabItem.isHidden = !allowed
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
        let text = "pane=@\(paneId) w=\(w) h=\(h) active=\(isPaneActive ? 1 : 0)"
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
        self.dividerLength = 6
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
