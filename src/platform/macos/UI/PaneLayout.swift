import AppKit

/// 递归二叉树 Pane 分割布局（对应 CLayoutNode），不用 NSSplitView。
final class PaneLayoutView: NSView {
    private let terminalManager: TerminalManager
    private var rootView: NSView?
    var onActivatePane: ((UInt32) -> Void)?

    init(terminalManager: TerminalManager) {
        self.terminalManager = terminalManager
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// 根据布局树重建子视图。
    func apply(layout: LayoutNode?, panes: [Pane]) {
        rootView?.removeFromSuperview()
        rootView = nil

        let tree = layout ?? panes.first.map { .leaf(paneId: $0.id) }
        guard let tree else { return }

        let built = build(node: tree)
        built.translatesAutoresizingMaskIntoConstraints = false
        addSubview(built)
        NSLayoutConstraint.activate([
            built.leadingAnchor.constraint(equalTo: leadingAnchor),
            built.trailingAnchor.constraint(equalTo: trailingAnchor),
            built.topAnchor.constraint(equalTo: topAnchor),
            built.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        rootView = built

        let ids = Set(collectPaneIds(tree))
        terminalManager.retainOnly(paneIds: ids)
    }

    private func build(node: LayoutNode) -> NSView {
        switch node {
        case .leaf(let paneId):
            let term = terminalManager.view(for: paneId)
            term.translatesAutoresizingMaskIntoConstraints = false
            let wrap = PaneHostView(paneId: paneId, terminal: term)
            wrap.onActivate = { [weak self] id in
                self?.onActivatePane?(id)
            }
            return wrap

        case .split(let horizontal, let ratio, let first, let second):
            let container = SplitContainerView(
                horizontal: horizontal,
                ratio: CGFloat(ratio) / 1000.0,
                first: build(node: first),
                second: build(node: second)
            )
            return container
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

/// 承载单个终端，点击时激活 pane。
private final class PaneHostView: NSView {
    let paneId: UInt32
    var onActivate: ((UInt32) -> Void)?

    init(paneId: UInt32, terminal: MuxTerminalView) {
        self.paneId = paneId
        super.init(frame: .zero)
        wantsLayer = true
        terminal.translatesAutoresizingMaskIntoConstraints = false
        addSubview(terminal)
        NSLayoutConstraint.activate([
            terminal.leadingAnchor.constraint(equalTo: leadingAnchor),
            terminal.trailingAnchor.constraint(equalTo: trailingAnchor),
            terminal.topAnchor.constraint(equalTo: topAnchor),
            terminal.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func mouseDown(with event: NSEvent) {
        onActivate?(paneId)
        super.mouseDown(with: event)
    }
}

/// 简单二分容器：按 ratio 分配 first/second。
private final class SplitContainerView: NSView {
    private let horizontal: Bool
    private let ratio: CGFloat
    private let firstView: NSView
    private let secondView: NSView
    private let divider = NSView()

    init(horizontal: Bool, ratio: CGFloat, first: NSView, second: NSView) {
        self.horizontal = horizontal
        self.ratio = min(max(ratio, 0.05), 0.95)
        self.firstView = first
        self.secondView = second
        super.init(frame: .zero)

        first.translatesAutoresizingMaskIntoConstraints = false
        second.translatesAutoresizingMaskIntoConstraints = false
        divider.translatesAutoresizingMaskIntoConstraints = false
        divider.wantsLayer = true
        divider.layer?.backgroundColor = NSColor.separatorColor.cgColor

        addSubview(first)
        addSubview(divider)
        addSubview(second)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func layout() {
        super.layout()
        let div: CGFloat = 4
        let bounds = self.bounds
        if horizontal {
            // 水平分割 = 左右排布
            let leftW = (bounds.width - div) * ratio
            firstView.frame = NSRect(x: 0, y: 0, width: leftW, height: bounds.height)
            divider.frame = NSRect(x: leftW, y: 0, width: div, height: bounds.height)
            secondView.frame = NSRect(
                x: leftW + div,
                y: 0,
                width: bounds.width - leftW - div,
                height: bounds.height
            )
        } else {
            // 垂直分割 = 上下排布
            let topH = (bounds.height - div) * ratio
            firstView.frame = NSRect(x: 0, y: bounds.height - topH, width: bounds.width, height: topH)
            divider.frame = NSRect(x: 0, y: bounds.height - topH - div, width: bounds.width, height: div)
            secondView.frame = NSRect(
                x: 0,
                y: 0,
                width: bounds.width,
                height: bounds.height - topH - div
            )
        }
    }
}
