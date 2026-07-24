import AppKit

/// 自绘 Tab 栏：从 `getTabs()` 渲染按钮，点击执行 SwitchTab。
final class TabBarView: NSView {
    var onSelectTab: ((UInt32) -> Void)?
    var onNewTab: (() -> Void)?

    private var tabs: [Tab] = []
    private var buttons: [NSButton] = []
    private let stack = NSStackView()
    private let newTabButton = NSButton()
    private let bottomBorder = CALayer()

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        // 与窗口背景区分，避免「顶部空白」观感
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        setAccessibilityIdentifier("muxterm.tabBar")
        setAccessibilityElement(true)
        setAccessibilityRole(.tabGroup)
        setAccessibilityLabel("Tabs")

        bottomBorder.backgroundColor = NSColor.separatorColor.cgColor
        layer?.addSublayer(bottomBorder)

        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 6, left: 10, bottom: 6, right: 10)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)

        newTabButton.title = "+"
        newTabButton.bezelStyle = .rounded
        newTabButton.isBordered = true
        newTabButton.target = self
        newTabButton.action = #selector(newTabClicked)
        newTabButton.toolTip = "新建 Tab（Cmd+T）"
        newTabButton.setAccessibilityIdentifier("muxterm.newTabButton")

        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            heightAnchor.constraint(equalToConstant: 40),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func layout() {
        super.layout()
        bottomBorder.frame = CGRect(x: 0, y: 0, width: bounds.width, height: 1)
    }

    func update(tabs: [Tab]) {
        self.tabs = tabs
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        buttons.removeAll()

        if tabs.isEmpty {
            let placeholder = NSTextField(labelWithString: "无标签页")
            placeholder.font = NSFont.systemFont(ofSize: 12)
            placeholder.textColor = NSColor.secondaryLabelColor
            stack.addArrangedSubview(placeholder)
        } else {
            for (index, tab) in tabs.enumerated() {
                let title = tab.name.isEmpty ? "Tab \(index + 1)" : tab.name
                let btn = NSButton(title: title, target: self, action: #selector(tabClicked(_:)))
                btn.bezelStyle = .rounded
                btn.isBordered = true
                btn.tag = Int(tab.id)
                btn.setAccessibilityIdentifier("muxterm.tab.\(index + 1)")
                if tab.isActive {
                    btn.contentTintColor = NSColor.controlAccentColor
                    btn.state = .on
                } else {
                    btn.contentTintColor = NSColor.labelColor
                    btn.state = .off
                }
                buttons.append(btn)
                stack.addArrangedSubview(btn)
            }
        }
        stack.addArrangedSubview(newTabButton)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        stack.addArrangedSubview(spacer)
        needsDisplay = true
    }

    @objc private func tabClicked(_ sender: NSButton) {
        onSelectTab?(UInt32(sender.tag))
    }

    @objc private func newTabClicked() {
        onNewTab?()
    }
}
