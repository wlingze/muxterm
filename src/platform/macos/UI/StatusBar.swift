import AppKit

/// 底部状态栏：连接状态、pane 数、活跃 tab/pane。
final class StatusBarView: NSView {
    private let label = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor

        label.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        label.textColor = NSColor.secondaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)

        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            label.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -10),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: 24),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func update(snapshot: FrameSnapshot) {
        let tabName = snapshot.tabs.first(where: { $0.id == snapshot.activeTab })?.name ?? "-"
        label.stringValue =
            "\(snapshot.status)  |  tabs: \(snapshot.tabs.count)  panes: \(snapshot.panes.count)  |  tab: \(tabName)  pane: @\(snapshot.activePane)"
    }
}
