import AppKit
import Foundation

/// 更新提醒条（AppKit）：有新版本时显示「一键更新」按钮。
///
/// 纯渲染层：状态来自 Core（CoreBridge.updateStatus），按钮只调 Core 的
/// check/install。前端不做版本比较、不下载、不落盘。
final class UpdateBannerView: NSView {
    var onPrimaryAction: (() -> Void)?
    var onDismiss: (() -> Void)?

    private let messageLabel = NSTextField(labelWithString: "")
    private let actionButton = NSButton()
    private let dismissButton = NSButton()
    /// 用户关掉的版本；同一版本不再重复提醒。
    private var dismissedVersion: String?
    /// 最近渲染的状态；未变化时不重排。
    private var lastRendered: CoreBridge.UpdateStatus?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.12).cgColor
        setAccessibilityIdentifier("muxterm.updateBanner")
        isHidden = true

        messageLabel.translatesAutoresizingMaskIntoConstraints = false
        messageLabel.font = NSFont.systemFont(ofSize: 12, weight: .medium)
        messageLabel.textColor = .labelColor
        messageLabel.lineBreakMode = .byTruncatingTail
        messageLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        addSubview(messageLabel)

        actionButton.translatesAutoresizingMaskIntoConstraints = false
        actionButton.title = MuxtermI18n.shared.tr(.updateInstallNow)
        actionButton.bezelStyle = .rounded
        actionButton.controlSize = .small
        actionButton.font = NSFont.systemFont(ofSize: 12, weight: .semibold)
        actionButton.target = self
        actionButton.action = #selector(primaryClicked)
        actionButton.setAccessibilityIdentifier("muxterm.updateAction")
        addSubview(actionButton)

        dismissButton.translatesAutoresizingMaskIntoConstraints = false
        dismissButton.title = "✕"
        dismissButton.bezelStyle = .inline
        dismissButton.isBordered = false
        dismissButton.controlSize = .small
        dismissButton.target = self
        dismissButton.action = #selector(dismissClicked)
        dismissButton.setAccessibilityIdentifier("muxterm.updateDismiss")
        addSubview(dismissButton)

        NSLayoutConstraint.activate([
            messageLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            messageLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            actionButton.leadingAnchor.constraint(
                greaterThanOrEqualTo: messageLabel.trailingAnchor,
                constant: 8
            ),
            actionButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            dismissButton.leadingAnchor.constraint(
                equalTo: actionButton.trailingAnchor,
                constant: 6
            ),
            dismissButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            dismissButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: 28),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// 按 Core 状态渲染；状态未变化时不动 widget。
    func apply(_ status: CoreBridge.UpdateStatus) {
        if let lastRendered, lastRendered == status { return }
        lastRendered = status

        let version = status.version ?? ""
        if !version.isEmpty, dismissedVersion == version {
            isHidden = true
            return
        }
        if version.isEmpty {
            dismissedVersion = nil
        }

        let presentation = UpdateBannerPresentation.make(status: status)
        isHidden = !presentation.visible
        guard presentation.visible else { return }
        messageLabel.stringValue = presentation.message
        actionButton.title = presentation.actionTitle
        actionButton.isEnabled = presentation.actionEnabled
        actionButton.isHidden = !presentation.actionEnabled
        dismissButton.isHidden = !presentation.actionEnabled
        needsLayout = true
    }

    /// 用户点关闭：记住这个版本，不再提醒。
    func dismiss() {
        dismissedVersion = lastRendered?.version
        isHidden = true
    }

    func refreshLocalization() {
        guard let status = lastRendered else { return }
        let presentation = UpdateBannerPresentation.make(status: status)
        actionButton.title = presentation.actionTitle
        messageLabel.stringValue = presentation.message
    }

    /// 测试钩子：当前渲染的文案。
    func testMessageText() -> String { messageLabel.stringValue }

    /// 测试钩子：主按钮标题。
    func testActionTitle() -> String { actionButton.title }

    @objc private func primaryClicked() {
        onPrimaryAction?()
    }

    @objc private func dismissClicked() {
        dismiss()
        onDismiss?()
    }
}

/// 纯逻辑：把 Core 状态映射为 banner 文案与按钮可用性（便于单测）。
struct UpdateBannerPresentation: Equatable {
    let visible: Bool
    let message: String
    let actionTitle: String
    let actionEnabled: Bool

    static func make(status: CoreBridge.UpdateStatus) -> UpdateBannerPresentation {
        let i18n = MuxtermI18n.shared
        switch status.phase {
        case "available":
            return UpdateBannerPresentation(
                visible: true,
                message: i18n.tr(
                    .updateAvailableMessage,
                    arguments: ["version": status.version ?? ""]
                ),
                actionTitle: i18n.tr(.updateInstallNow),
                actionEnabled: true
            )
        case "installing":
            return UpdateBannerPresentation(
                visible: true,
                message: i18n.tr(.updateInstalling),
                actionTitle: i18n.tr(.updateInstalling),
                actionEnabled: false
            )
        case "installed":
            return UpdateBannerPresentation(
                visible: true,
                message: i18n.tr(.updateInstalledRestart),
                actionTitle: i18n.tr(.updateInstalledRestart),
                actionEnabled: false
            )
        case "checking":
            return UpdateBannerPresentation(
                visible: true,
                message: i18n.tr(.updateChecking),
                actionTitle: i18n.tr(.updateChecking),
                actionEnabled: false
            )
        case "failed":
            return UpdateBannerPresentation(
                visible: true,
                message: i18n.tr(
                    .updateFailed,
                    arguments: ["message": status.message ?? ""]
                ),
                actionTitle: i18n.tr(.updateRetry),
                actionEnabled: true
            )
        default:
            return UpdateBannerPresentation(
                visible: false,
                message: "",
                actionTitle: "",
                actionEnabled: false
            )
        }
    }
}
