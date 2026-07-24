import AppKit

/// 递归二叉树 Pane 分割布局（对应 CLayoutNode）。
///
/// 全程 Auto Layout，避免 frame/AL 混用导致子视图 bounds=0 → SwiftTerm 黑屏。
final class PaneLayoutView: NSView {
    private let terminalManager: TerminalManager
    private var rootView: NSView?
    private var rootConstraints: [NSLayoutConstraint] = []
    private var hostByPane: [UInt32: PaneHostView] = [:]
    var onActivatePane: ((UInt32) -> Void)?

    init(terminalManager: TerminalManager) {
        self.terminalManager = terminalManager
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        setAccessibilityIdentifier("muxterm.paneLayout")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// 根据布局树重建子视图，并在布局完成后同步 PTY 尺寸 + 强制重绘。
    func apply(layout: LayoutNode?, panes: [Pane]) {
        if let rootView {
            NSLayoutConstraint.deactivate(rootConstraints)
            rootConstraints = []
            rootView.removeFromSuperview()
        }
        rootView = nil
        hostByPane.removeAll()

        let tree = layout ?? panes.first.map { .leaf(paneId: $0.id) }
        guard let tree else { return }

        let built = build(node: tree)
        built.translatesAutoresizingMaskIntoConstraints = false
        addSubview(built)
        rootConstraints = [
            built.leadingAnchor.constraint(equalTo: leadingAnchor),
            built.trailingAnchor.constraint(equalTo: trailingAnchor),
            built.topAnchor.constraint(equalTo: topAnchor),
            built.bottomAnchor.constraint(equalTo: bottomAnchor),
        ]
        NSLayoutConstraint.activate(rootConstraints)
        rootView = built

        let ids = Set(collectPaneIds(tree))
        terminalManager.retainOnly(paneIds: ids)

        let active = panes.first(where: \.isActive)?.id ?? panes.first?.id ?? 0
        markActivePane(active)

        needsLayout = true
        DispatchQueue.main.async { [weak self] in
            self?.finalizeAfterLayout(paneIds: ids, attempt: 0)
        }
    }

    /// 更新活跃 pane 高亮与 AX（供 Cmd+[ / ] 焦点跟随断言）。
    func markActivePane(_ paneId: UInt32) {
        for (id, host) in hostByPane {
            host.setActive(id == paneId)
        }
    }

    private func finalizeAfterLayout(paneIds: Set<UInt32>, attempt: Int) {
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
        terminalManager.syncAllVisibleSizes(paneIds: paneIds)
        terminalManager.forceRedraw(paneIds: paneIds)
    }

    private func build(node: LayoutNode) -> NSView {
        switch node {
        case .leaf(let paneId):
            let term = terminalManager.view(for: paneId)
            let wrap = PaneHostView(paneId: paneId, terminal: term)
            wrap.onActivate = { [weak self] id in
                self?.onActivatePane?(id)
            }
            hostByPane[paneId] = wrap
            return wrap

        case .split(let horizontal, let ratio, let first, let second):
            return SplitContainerView(
                horizontal: horizontal,
                ratio: CGFloat(ratio) / 1000.0,
                first: build(node: first),
                second: build(node: second)
            )
        }
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
    private var isPaneActive = false

    init(paneId: UInt32, terminal: MuxTerminalView) {
        self.paneId = paneId
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        translatesAutoresizingMaskIntoConstraints = false
        setAccessibilityIdentifier("muxterm.pane.\(paneId)")
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel("Pane \(paneId)")

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
        publishGeometry()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func setActive(_ active: Bool) {
        isPaneActive = active
        layer?.borderWidth = active ? 2 : 0
        layer?.borderColor = active ? NSColor.controlAccentColor.cgColor : nil
        publishGeometry()
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

/// 二分容器：纯 Auto Layout，按 ratio 分配 first/second。
private final class SplitContainerView: NSView {
    init(horizontal: Bool, ratio: CGFloat, first: NSView, second: NSView) {
        super.init(frame: .zero)
        wantsLayer = true
        translatesAutoresizingMaskIntoConstraints = false

        let r = min(max(ratio, 0.05), 0.95)
        let divider = NSView()
        divider.translatesAutoresizingMaskIntoConstraints = false
        divider.wantsLayer = true
        divider.layer?.backgroundColor = NSColor.separatorColor.cgColor

        first.translatesAutoresizingMaskIntoConstraints = false
        second.translatesAutoresizingMaskIntoConstraints = false

        addSubview(first)
        addSubview(divider)
        addSubview(second)

        let divThickness: CGFloat = 4
        let multiplier = r / (1.0 - r)

        if horizontal {
            NSLayoutConstraint.activate([
                first.leadingAnchor.constraint(equalTo: leadingAnchor),
                first.topAnchor.constraint(equalTo: topAnchor),
                first.bottomAnchor.constraint(equalTo: bottomAnchor),

                divider.leadingAnchor.constraint(equalTo: first.trailingAnchor),
                divider.topAnchor.constraint(equalTo: topAnchor),
                divider.bottomAnchor.constraint(equalTo: bottomAnchor),
                divider.widthAnchor.constraint(equalToConstant: divThickness),

                second.leadingAnchor.constraint(equalTo: divider.trailingAnchor),
                second.trailingAnchor.constraint(equalTo: trailingAnchor),
                second.topAnchor.constraint(equalTo: topAnchor),
                second.bottomAnchor.constraint(equalTo: bottomAnchor),

                first.widthAnchor.constraint(equalTo: second.widthAnchor, multiplier: multiplier)
                    .withPriority(.defaultHigh),
            ])
        } else {
            NSLayoutConstraint.activate([
                first.leadingAnchor.constraint(equalTo: leadingAnchor),
                first.trailingAnchor.constraint(equalTo: trailingAnchor),
                first.topAnchor.constraint(equalTo: topAnchor),

                divider.leadingAnchor.constraint(equalTo: leadingAnchor),
                divider.trailingAnchor.constraint(equalTo: trailingAnchor),
                divider.topAnchor.constraint(equalTo: first.bottomAnchor),
                divider.heightAnchor.constraint(equalToConstant: divThickness),

                second.leadingAnchor.constraint(equalTo: leadingAnchor),
                second.trailingAnchor.constraint(equalTo: trailingAnchor),
                second.topAnchor.constraint(equalTo: divider.bottomAnchor),
                second.bottomAnchor.constraint(equalTo: bottomAnchor),

                first.heightAnchor.constraint(equalTo: second.heightAnchor, multiplier: multiplier)
                    .withPriority(.defaultHigh),
            ])
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}

private extension NSLayoutConstraint {
    func withPriority(_ priority: NSLayoutConstraint.Priority) -> NSLayoutConstraint {
        self.priority = priority
        return self
    }
}
