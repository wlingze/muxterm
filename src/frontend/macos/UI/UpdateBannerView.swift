import AppKit
import Foundation

/// 更新提醒卡片（AppKit）：右下角常驻 HUD，提供更新 / 关闭 / 不再提醒。
///
/// 纯渲染层：状态来自 Core（CoreBridge.updateStatus），按钮只调 Core 的
/// check/install。前端不做版本比较、不下载、不落盘；只有「不再提醒」这一
/// 用户偏好保存在 UserDefaults。
final class UpdateBannerView: NSView {
    static let suppressRemindersKey = "muxterm.update.suppressReminders"

    var onPrimaryAction: (() -> Void)?
    var onDismiss: (() -> Void)?
    var onNeverRemind: (() -> Void)?
    /// 卡片显示/隐藏时通知宿主；ContentView 只在可见时挂布局约束，
    /// 避免隐藏的 HUD 把紧凑窗口撑宽。
    var onVisibilityChanged: ((Bool) -> Void)?

    private let defaults: UserDefaults
    private var suppressReminders: Bool
    private let titleLabel = NSTextField(labelWithString: "")
    private let messageLabel = NSTextField(labelWithString: "")
    private let actionButton = NSButton()
    private let closeButton = NSButton()
    private let neverButton = NSButton()
    private let textStack = NSStackView()
    private let buttonStack = NSStackView()
    private let contentStack = NSStackView()
    /// 用户本次关闭的版本；同一版本不再重复提醒。
    private var dismissedVersion: String?
    /// 最近渲染的状态；未变化时不重排。
    private var lastRendered: CoreBridge.UpdateStatus?

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        self.suppressReminders = defaults.bool(forKey: Self.suppressRemindersKey)
        super.init(frame: .zero)
        configure()
    }

    override init(frame frameRect: NSRect) {
        self.defaults = .standard
        self.suppressReminders = UserDefaults.standard.bool(
            forKey: Self.suppressRemindersKey
        )
        super.init(frame: frameRect)
        configure()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func configure() {
        wantsLayer = true
        layer?.masksToBounds = false
        setAccessibilityIdentifier("muxterm.updateBanner")
        isHidden = true
        refreshAppearance()

        titleLabel.font = NSFont.systemFont(ofSize: 13, weight: .semibold)
        titleLabel.textColor = .labelColor
        titleLabel.lineBreakMode = .byTruncatingTail

        messageLabel.font = NSFont.systemFont(ofSize: 12)
        messageLabel.textColor = .secondaryLabelColor
        messageLabel.lineBreakMode = .byWordWrapping
        messageLabel.maximumNumberOfLines = 3
        messageLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        messageLabel.setContentCompressionResistancePriority(.defaultLow, for: .vertical)

        textStack.translatesAutoresizingMaskIntoConstraints = false
        textStack.orientation = .vertical
        textStack.alignment = .leading
        textStack.spacing = 3
        textStack.detachesHiddenViews = true
        textStack.addArrangedSubview(titleLabel)
        textStack.addArrangedSubview(messageLabel)

        configureButton(
            actionButton,
            title: MuxtermI18n.shared.tr(.updateInstallNow),
            identifier: "muxterm.updateAction",
            action: #selector(primaryClicked),
            emphasized: true
        )
        configureButton(
            closeButton,
            title: MuxtermI18n.shared.tr(.updateClose),
            identifier: "muxterm.updateDismiss",
            action: #selector(closeClicked),
            emphasized: false
        )
        configureButton(
            neverButton,
            title: MuxtermI18n.shared.tr(.updateNeverRemind),
            identifier: "muxterm.updateNeverRemind",
            action: #selector(neverClicked),
            emphasized: false
        )
        closeButton.toolTip = MuxtermI18n.shared.tr(.updateClose)
        neverButton.toolTip = MuxtermI18n.shared.tr(.updateNeverRemind)

        // 按钮靠右；leading spacer 吸收剩余宽度。
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        spacer.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        buttonStack.translatesAutoresizingMaskIntoConstraints = false
        buttonStack.orientation = .horizontal
        buttonStack.alignment = .centerY
        buttonStack.spacing = 8
        buttonStack.detachesHiddenViews = true
        buttonStack.addArrangedSubview(spacer)
        buttonStack.addArrangedSubview(neverButton)
        buttonStack.addArrangedSubview(closeButton)
        buttonStack.addArrangedSubview(actionButton)

        contentStack.translatesAutoresizingMaskIntoConstraints = false
        contentStack.orientation = .vertical
        contentStack.alignment = .leading
        contentStack.spacing = 10
        contentStack.detachesHiddenViews = true
        contentStack.addArrangedSubview(textStack)
        contentStack.addArrangedSubview(buttonStack)
        addSubview(contentStack)

        NSLayoutConstraint.activate([
            widthAnchor.constraint(lessThanOrEqualToConstant: 360),
            contentStack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            contentStack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            contentStack.topAnchor.constraint(equalTo: topAnchor, constant: 12),
            contentStack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12),
            textStack.widthAnchor.constraint(equalTo: contentStack.widthAnchor),
            buttonStack.widthAnchor.constraint(equalTo: contentStack.widthAnchor),
        ])
    }

    private func configureButton(
        _ button: NSButton,
        title: String,
        identifier: String,
        action: Selector,
        emphasized: Bool
    ) {
        button.translatesAutoresizingMaskIntoConstraints = false
        button.title = title
        button.bezelStyle = .rounded
        button.controlSize = .small
        button.font = NSFont.systemFont(
            ofSize: 12,
            weight: emphasized ? .semibold : .regular
        )
        button.target = self
        button.action = action
        button.setContentHuggingPriority(.required, for: .horizontal)
        button.setContentCompressionResistancePriority(.required, for: .horizontal)
        button.setAccessibilityIdentifier(identifier)
        if emphasized {
            button.bezelColor = .controlAccentColor
            button.contentTintColor = .white
        }
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        refreshAppearance()
    }

    private func refreshAppearance() {
        layer?.cornerRadius = 10
        layer?.backgroundColor = NSColor.controlBackgroundColor.withAlphaComponent(0.98).cgColor
        layer?.borderWidth = 1
        layer?.borderColor = NSColor.separatorColor.withAlphaComponent(0.8).cgColor
        layer?.shadowColor = NSColor.black.cgColor
        layer?.shadowOpacity = 0.18
        layer?.shadowRadius = 14
        layer?.shadowOffset = CGSize(width: 0, height: -4)
    }

    /// 按 Core 状态渲染；状态未变化时不动 widget。
    func apply(_ status: CoreBridge.UpdateStatus) {
        if let lastRendered, lastRendered == status { return }
        lastRendered = status

        let version = status.version ?? ""
        if suppressReminders {
            setBannerHidden(true)
            return
        }
        if !version.isEmpty, dismissedVersion == version {
            setBannerHidden(true)
            return
        }
        if version.isEmpty {
            dismissedVersion = nil
        }

        let presentation = UpdateBannerPresentation.make(status: status)
        setBannerHidden(!presentation.visible)
        guard presentation.visible else { return }
        titleLabel.stringValue = presentation.title
        messageLabel.stringValue = presentation.message
        messageLabel.isHidden = presentation.message.isEmpty
        actionButton.title = presentation.actionTitle
        actionButton.isEnabled = presentation.actionEnabled
        actionButton.isHidden = !presentation.actionEnabled
        closeButton.isHidden = !presentation.showsDismiss
        neverButton.isHidden = !presentation.showsNeverRemind
        needsLayout = true
    }

    /// 用户点「关闭」：只收起当前版本；下次启动仍会提醒。
    func dismiss() {
        dismissedVersion = lastRendered?.version
        setBannerHidden(true)
    }

    /// 用户点「不再提醒」：持久化偏好，后续版本也不再自动弹出。
    func neverRemind() {
        suppressReminders = true
        defaults.set(true, forKey: Self.suppressRemindersKey)
        dismissedVersion = lastRendered?.version
        setBannerHidden(true)
    }

    /// 用户从菜单显式点「检查更新」时重新允许提醒。
    func allowFutureReminders() {
        if suppressReminders {
            suppressReminders = false
            defaults.set(false, forKey: Self.suppressRemindersKey)
        }
        dismissedVersion = nil
        lastRendered = nil
        setBannerHidden(true)
    }

    func refreshLocalization() {
        guard let status = lastRendered else { return }
        let i18n = MuxtermI18n.shared
        let presentation = UpdateBannerPresentation.make(status: status)
        titleLabel.stringValue = presentation.title
        messageLabel.stringValue = presentation.message
        actionButton.title = presentation.actionTitle
        closeButton.title = i18n.tr(.updateClose)
        closeButton.toolTip = i18n.tr(.updateClose)
        neverButton.title = i18n.tr(.updateNeverRemind)
        neverButton.toolTip = i18n.tr(.updateNeverRemind)
    }

    private func setBannerHidden(_ hidden: Bool) {
        guard isHidden != hidden else { return }
        isHidden = hidden
        onVisibilityChanged?(!hidden)
    }

    // MARK: - 测试钩子

    func testTitleText() -> String { titleLabel.stringValue }
    func testMessageText() -> String { messageLabel.stringValue }
    func testActionTitle() -> String { actionButton.title }
    func testDismissVisible() -> Bool { !closeButton.isHidden }
    func testNeverRemindVisible() -> Bool { !neverButton.isHidden }

    @objc private func primaryClicked() {
        onPrimaryAction?()
    }

    @objc private func closeClicked() {
        dismiss()
        onDismiss?()
    }

    @objc private func neverClicked() {
        neverRemind()
        onNeverRemind?()
    }
}

/// 纯逻辑：把 Core 状态映射为卡片文案与按钮可用性（便于单测）。
struct UpdateBannerPresentation: Equatable {
    let visible: Bool
    let title: String
    let message: String
    let actionTitle: String
    let actionEnabled: Bool
    /// 是否显示「关闭」。瞬态（检查/安装中）不可关，避免把重启提示一起藏掉。
    let showsDismiss: Bool
    /// 是否显示「不再提醒」。安装完成后的重启提示不需要再关闭自动提醒。
    let showsNeverRemind: Bool

    static func make(status: CoreBridge.UpdateStatus) -> UpdateBannerPresentation {
        let i18n = MuxtermI18n.shared
        switch status.phase {
        case "available":
            return UpdateBannerPresentation(
                visible: true,
                title: i18n.tr(.updateAvailable),
                message: i18n.tr(
                    .updateAvailableMessage,
                    arguments: ["version": status.version ?? ""]
                ),
                actionTitle: i18n.tr(.updateInstallNow),
                actionEnabled: true,
                showsDismiss: true,
                showsNeverRemind: true
            )
        case "installing":
            return UpdateBannerPresentation(
                visible: true,
                title: i18n.tr(.updateAvailable),
                message: i18n.tr(.updateInstalling),
                actionTitle: i18n.tr(.updateInstalling),
                actionEnabled: false,
                showsDismiss: false,
                showsNeverRemind: false
            )
        case "installed":
            return UpdateBannerPresentation(
                visible: true,
                title: i18n.tr(.updateInstalledTitle),
                message: i18n.tr(.updateInstalledRestart),
                actionTitle: i18n.tr(.updateInstalledRestart),
                actionEnabled: false,
                showsDismiss: true,
                showsNeverRemind: false
            )
        case "checking":
            return UpdateBannerPresentation(
                visible: true,
                title: i18n.tr(.updateAvailable),
                message: i18n.tr(.updateChecking),
                actionTitle: i18n.tr(.updateChecking),
                actionEnabled: false,
                showsDismiss: false,
                showsNeverRemind: false
            )
        case "failed":
            let detail = status.message?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return UpdateBannerPresentation(
                visible: true,
                title: i18n.tr(.updateFailedTitle),
                message: detail.isEmpty ? i18n.tr(.updateFailedTitle) : detail,
                actionTitle: i18n.tr(.updateRetry),
                actionEnabled: true,
                showsDismiss: true,
                showsNeverRemind: true
            )
        default:
            return UpdateBannerPresentation(
                visible: false,
                title: "",
                message: "",
                actionTitle: "",
                actionEnabled: false,
                showsDismiss: false,
                showsNeverRemind: false
            )
        }
    }
}
