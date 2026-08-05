import AppKit
import MuxtermChrome

/// Tab 栏位置（UserDefaults: `muxterm.tabBarPosition`）。
enum TabBarPosition: String {
    case top
    case bottom

    static var current: TabBarPosition {
        let raw = UserDefaults.standard.string(forKey: "muxterm.tabBarPosition") ?? "top"
        return TabBarPosition(rawValue: raw) ?? .top
    }

    static func set(_ position: TabBarPosition) {
        UserDefaults.standard.set(position.rawValue, forKey: "muxterm.tabBarPosition")
    }
}

/// iTerm Minimal 风格扁平 Tab 栏：多 tab 等分铺满，最右固定「+」；单 tab 隐藏。
final class TabBarView: NSView {
    var onSelectTab: ((UInt32) -> Void)?
    var onNewTab: (() -> Void)?
    /// 可见性变化（单 tab 隐藏 / 多 tab 显示）时回调，便于 ContentView 收紧约束。
    var onVisibilityChanged: ((Bool) -> Void)?

    private var tabs: [Tab] = []
    private let tabsStack = NSStackView()
    private let newTabButton = NSButton()
    private let edgeLine = CALayer()
    private var heightConstraint: NSLayoutConstraint!
    private(set) var isBarVisible = false

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        // 与终端同色底，避免独立面板感
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        setAccessibilityIdentifier("muxterm.tabBar")
        setAccessibilityElement(true)
        setAccessibilityRole(.tabGroup)
        setAccessibilityLabel(MuxtermI18n.shared.tr(.tabsAccessibility))

        edgeLine.backgroundColor = NSColor.separatorColor.cgColor
        layer?.addSublayer(edgeLine)

        tabsStack.orientation = .horizontal
        tabsStack.alignment = .centerY
        tabsStack.spacing = 0
        tabsStack.distribution = .fillEqually
        tabsStack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(tabsStack)

        newTabButton.title = "+"
        newTabButton.bezelStyle = .shadowlessSquare
        newTabButton.isBordered = false
        newTabButton.font = NSFont.systemFont(ofSize: 13, weight: .regular)
        newTabButton.contentTintColor = NSColor.secondaryLabelColor
        newTabButton.target = self
        newTabButton.action = #selector(newTabClicked)
        newTabButton.toolTip = MuxtermI18n.shared.tr(.newTabTooltip)
        newTabButton.setAccessibilityIdentifier("muxterm.newTabButton")
        newTabButton.translatesAutoresizingMaskIntoConstraints = false
        newTabButton.wantsLayer = true
        addSubview(newTabButton)

        heightConstraint = heightAnchor.constraint(equalToConstant: 0)

        NSLayoutConstraint.activate([
            heightConstraint,
            tabsStack.leadingAnchor.constraint(equalTo: leadingAnchor),
            tabsStack.topAnchor.constraint(equalTo: topAnchor),
            tabsStack.bottomAnchor.constraint(equalTo: bottomAnchor),
            tabsStack.trailingAnchor.constraint(equalTo: newTabButton.leadingAnchor),

            newTabButton.trailingAnchor.constraint(equalTo: trailingAnchor),
            newTabButton.topAnchor.constraint(equalTo: topAnchor),
            newTabButton.bottomAnchor.constraint(equalTo: bottomAnchor),
            newTabButton.widthAnchor.constraint(equalToConstant: FlatChrome.newTabButtonWidth),
        ])

        applyVisibility(false, notify: false)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    /// `edgeAtBottom == true` 时画底部分隔线（tab 在上）；否则画顶部分隔线（tab 在下）。
    func setEdgeLineAtBottom(_ atBottom: Bool) {
        edgeAtBottom = atBottom
        needsLayout = true
    }

    private var edgeAtBottom = true

    override func layout() {
        super.layout()
        let y: CGFloat = edgeAtBottom ? 0 : bounds.height - 1
        edgeLine.frame = CGRect(x: 0, y: y, width: bounds.width, height: 1)
    }

    func update(tabs: [Tab]) {
        self.tabs = tabs
        tabsStack.arrangedSubviews.forEach { $0.removeFromSuperview() }

        // 单 tab：隐藏整条栏（Cmd+T 仍可新建）
        let show = tabs.count >= 2
        applyVisibility(show, notify: true)
        guard show else { return }

        for (index, tab) in tabs.enumerated() {
            let title = tab.name.isEmpty ? "\(index + 1)" : "\(index + 1):\(tab.name)"
            let cell = TabCellButton(title: title, tabId: tab.id, active: tab.isActive)
            cell.target = self
            cell.action = #selector(tabClicked(_:))
            cell.setAccessibilityIdentifier("muxterm.tab.\(index + 1)")
            tabsStack.addArrangedSubview(cell)
        }
        needsDisplay = true
    }

    func refreshLocalization() {
        setAccessibilityLabel(MuxtermI18n.shared.tr(.tabsAccessibility))
        newTabButton.toolTip = MuxtermI18n.shared.tr(.newTabTooltip)
    }

    private func applyVisibility(_ visible: Bool, notify: Bool) {
        isHidden = !visible
        heightConstraint.constant = visible ? FlatChrome.tabBarHeight : 0
        if isBarVisible != visible {
            isBarVisible = visible
            if notify {
                onVisibilityChanged?(visible)
            }
        }
    }

    @objc private func tabClicked(_ sender: NSButton) {
        onSelectTab?(UInt32(sender.tag))
    }

    @objc private func newTabClicked() {
        onNewTab?()
    }
}

/// 扁平 tab 单元：透明底、无圆角/阴影；活跃项字重 + 底边指示线。
private final class TabCellButton: NSButton {
    init(title: String, tabId: UInt32, active: Bool) {
        super.init(frame: .zero)
        self.title = title
        self.tag = Int(tabId)
        bezelStyle = .shadowlessSquare
        isBordered = false
        setButtonType(.momentaryChange)
        font = NSFont.systemFont(ofSize: 11, weight: active ? .semibold : .regular)
        contentTintColor = active ? NSColor.labelColor : NSColor.secondaryLabelColor
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
        layer?.cornerRadius = 0

        if active {
            let underline = CALayer()
            underline.backgroundColor = NSColor.controlAccentColor.cgColor
            underline.frame = CGRect(x: 0, y: 0, width: 2000, height: FlatChrome.activeTabUnderlineHeight)
            underline.name = "activeUnderline"
            layer?.addSublayer(underline)
        }

        let sep = CALayer()
        sep.backgroundColor = NSColor.separatorColor.withAlphaComponent(0.35).cgColor
        sep.name = "sep"
        layer?.addSublayer(sep)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override func layout() {
        super.layout()
        if let underline = layer?.sublayers?.first(where: { $0.name == "activeUnderline" }) {
            underline.frame = CGRect(
                x: 0,
                y: 0,
                width: bounds.width,
                height: FlatChrome.activeTabUnderlineHeight
            )
        }
        if let sep = layer?.sublayers?.first(where: { $0.name == "sep" }) {
            sep.frame = CGRect(
                x: bounds.width - 1,
                y: 4,
                width: 1,
                height: max(0, bounds.height - 8)
            )
        }
    }
}
