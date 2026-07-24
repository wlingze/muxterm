import AppKit

/// 自绘 Tab 栏：从 `getTabs()` 渲染按钮，点击执行 SwitchTab。
final class TabBarView: NSView {
    var onSelectTab: ((UInt32) -> Void)?
    var onNewTab: (() -> Void)?

    private var tabs: [Tab] = []
    private var buttons: [NSButton] = []
    private let stack = NSStackView()
    private let newTabButton = NSButton()

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

        stack.orientation = .horizontal
        stack.spacing = 4
        stack.edgeInsets = NSEdgeInsets(top: 4, left: 8, bottom: 4, right: 8)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)

        newTabButton.title = "+"
        newTabButton.bezelStyle = .flexiblePush
        newTabButton.target = self
        newTabButton.action = #selector(newTabClicked)
        newTabButton.toolTip = "新建 Tab（Alt+T）"

        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            heightAnchor.constraint(equalToConstant: 36),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func update(tabs: [Tab]) {
        self.tabs = tabs
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        buttons.removeAll()

        for tab in tabs {
            let btn = NSButton(title: tab.name.isEmpty ? "Tab \(tab.id)" : tab.name, target: self, action: #selector(tabClicked(_:)))
            btn.bezelStyle = .flexiblePush
            btn.tag = Int(tab.id)
            btn.contentTintColor = tab.isActive ? NSColor.controlAccentColor : NSColor.labelColor
            if tab.isActive {
                btn.state = .on
            }
            buttons.append(btn)
            stack.addArrangedSubview(btn)
        }
        stack.addArrangedSubview(newTabButton)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        stack.addArrangedSubview(spacer)
    }

    @objc private func tabClicked(_ sender: NSButton) {
        onSelectTab?(UInt32(sender.tag))
    }

    @objc private func newTabClicked() {
        onNewTab?()
    }
}
