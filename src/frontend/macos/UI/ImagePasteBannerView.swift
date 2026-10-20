import AppKit
import Foundation
import MuxtermChrome

/// 图片粘贴反馈条（AppKit）：粘贴一按下就出现，上传中转圈，完成或失败后留一小会儿。
///
/// 纯渲染层：阶段和停留时间都来自 `ImagePasteTimeline`（MuxtermChrome），这里只负责
/// 把 `ImagePasteFeedback` 画出来。上传可能要几百毫秒到数十秒，终端里这段时间什么都
/// 不会出现，所以不能只靠状态栏角落里的 14pt 转圈。
final class ImagePasteBannerView: NSView {
    /// 用户点了关闭：只对结果阶段有效，进行中的粘贴不可关。
    var onDismiss: (() -> Void)?
    /// 测试可注入；生产默认写入系统剪贴板。
    var onCopy: ((String) -> Void)?

    private let spinner = NSProgressIndicator()
    private let iconView = NSImageView()
    private let messageLabel = NSTextField(labelWithString: "")
    private let copyButton = NSButton()
    private let dismissButton = NSButton()
    private(set) var renderedStage: ImagePasteStage = .idle
    /// 转圈是否在动。NSProgressIndicator 没有公开的 isAnimating，自己记。
    private(set) var isSpinning = false

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.cornerRadius = 6
        setAccessibilityIdentifier("muxterm.imagePasteBanner")
        isHidden = true

        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        spinner.translatesAutoresizingMaskIntoConstraints = false
        spinner.setAccessibilityIdentifier("muxterm.imagePasteBanner.spinner")
        addSubview(spinner)

        iconView.translatesAutoresizingMaskIntoConstraints = false
        iconView.imageScaling = .scaleProportionallyDown
        addSubview(iconView)

        messageLabel.translatesAutoresizingMaskIntoConstraints = false
        messageLabel.font = NSFont.systemFont(ofSize: 12, weight: .medium)
        messageLabel.textColor = .labelColor
        // 路径很长时保留文件名，截断中间。
        messageLabel.lineBreakMode = .byTruncatingMiddle
        messageLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        messageLabel.setAccessibilityIdentifier("muxterm.imagePasteBanner.message")
        addSubview(messageLabel)

        copyButton.translatesAutoresizingMaskIntoConstraints = false
        copyButton.title = MuxtermI18n.shared.tr(.menuCopy)
        copyButton.bezelStyle = .inline
        copyButton.isBordered = false
        copyButton.controlSize = .small
        copyButton.target = self
        copyButton.action = #selector(copyClicked)
        copyButton.setAccessibilityIdentifier("muxterm.imagePasteBanner.copy")
        copyButton.setAccessibilityLabel(MuxtermI18n.shared.tr(.menuCopy))
        copyButton.isHidden = true
        addSubview(copyButton)

        dismissButton.translatesAutoresizingMaskIntoConstraints = false
        dismissButton.title = "✕"
        dismissButton.bezelStyle = .inline
        dismissButton.isBordered = false
        dismissButton.controlSize = .small
        dismissButton.target = self
        dismissButton.action = #selector(dismissClicked)
        dismissButton.setAccessibilityIdentifier("muxterm.imagePasteBanner.dismiss")
        addSubview(dismissButton)

        NSLayoutConstraint.activate([
            spinner.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            spinner.centerYAnchor.constraint(equalTo: centerYAnchor),
            spinner.widthAnchor.constraint(equalToConstant: 14),
            spinner.heightAnchor.constraint(equalToConstant: 14),
            iconView.centerXAnchor.constraint(equalTo: spinner.centerXAnchor),
            iconView.centerYAnchor.constraint(equalTo: centerYAnchor),
            iconView.widthAnchor.constraint(equalToConstant: 14),
            iconView.heightAnchor.constraint(equalToConstant: 14),
            messageLabel.leadingAnchor.constraint(equalTo: spinner.trailingAnchor, constant: 8),
            messageLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            copyButton.leadingAnchor.constraint(
                greaterThanOrEqualTo: messageLabel.trailingAnchor,
                constant: 8
            ),
            copyButton.trailingAnchor.constraint(equalTo: dismissButton.leadingAnchor, constant: -4),
            copyButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            copyButton.widthAnchor.constraint(greaterThanOrEqualToConstant: 38),
            dismissButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            dismissButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: 28),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    /// 按当前阶段渲染；阶段未变化时不动 widget。
    func apply(_ stage: ImagePasteStage) {
        guard stage != renderedStage else { return }
        renderedStage = stage
        guard let feedback = ImagePasteFeedback.make(stage) else {
            setSpinning(false)
            isHidden = true
            return
        }
        isHidden = false
        messageLabel.stringValue = Self.message(for: feedback)
        toolTip = Self.toolTip(for: feedback)
        copyButton.title = MuxtermI18n.shared.tr(.menuCopy)
        copyButton.toolTip = MuxtermI18n.shared.tr(.menuCopy)
        copyButton.isHidden = !Self.copyButtonVisible(for: feedback.stage)
        layer?.backgroundColor = Self.background(for: feedback.tone).cgColor
        if feedback.showsSpinner {
            iconView.isHidden = true
            setSpinning(true)
        } else {
            setSpinning(false)
            iconView.isHidden = false
            iconView.image = Self.icon(for: feedback.tone)
            iconView.contentTintColor = Self.tint(for: feedback.tone)
        }
        // 只有结果阶段可以手动关掉。
        dismissButton.isHidden = feedback.showsSpinner
        setAccessibilityLabel(messageLabel.stringValue)
        setAccessibilityValue(Self.accessibilityValue(for: stage))
    }

    func refreshLocalization() {
        let stage = renderedStage
        renderedStage = .idle
        apply(stage)
    }

    // MARK: 文案（纯函数，便于测试）

    static func message(for feedback: ImagePasteFeedback) -> String {
        let i18n = MuxtermI18n.shared
        switch feedback.stage {
        case .idle:
            return ""
        case .encoding:
            return i18n.tr(.imagePasteEncoding)
        case .uploading:
            return i18n.tr(.imagePasteProgress)
        case .done(let path):
            return i18n.tr(
                .imagePasteDone,
                arguments: ["name": ImagePasteFeedback.fileName(of: path)]
            )
        case .failed(let message):
            return i18n.tr(.imagePasteFailed) + ": " + message
        }
    }

    static func copyText(for stage: ImagePasteStage) -> String? {
        return switch stage {
        case .done(let path): path
        case .failed(let message): MuxtermI18n.shared.tr(.imagePasteFailed) + ": " + message
        case .idle, .encoding, .uploading: nil
        }
    }

    static func copyButtonVisible(for stage: ImagePasteStage) -> Bool {
        return switch stage {
        case .done, .failed: true
        case .idle, .encoding, .uploading: false
        }
    }

    static func toolTip(for feedback: ImagePasteFeedback) -> String? {
        switch feedback.stage {
        case .done(let path): return path
        case .failed(let message): return message
        case .idle, .encoding, .uploading: return nil
        }
    }

    static func accessibilityValue(for stage: ImagePasteStage) -> String {
        switch stage {
        case .idle: return "idle"
        case .encoding: return "encoding"
        case .uploading: return "uploading"
        case .done: return "done"
        case .failed: return "failed"
        }
    }

    private static func background(for tone: ImagePasteFeedback.Tone) -> NSColor {
        switch tone {
        case .progress: return NSColor.controlAccentColor.withAlphaComponent(0.14)
        case .success: return NSColor.systemGreen.withAlphaComponent(0.16)
        case .failure: return NSColor.systemRed.withAlphaComponent(0.16)
        }
    }

    private static func tint(for tone: ImagePasteFeedback.Tone) -> NSColor {
        switch tone {
        case .progress: return .controlAccentColor
        case .success: return .systemGreen
        case .failure: return .systemRed
        }
    }

    private static func icon(for tone: ImagePasteFeedback.Tone) -> NSImage? {
        let name: String
        switch tone {
        case .progress: name = "arrow.up.circle.fill"
        case .success: name = "checkmark.circle.fill"
        case .failure: name = "exclamationmark.triangle.fill"
        }
        return NSImage(systemSymbolName: name, accessibilityDescription: nil)
    }

    private func setSpinning(_ spinning: Bool) {
        isSpinning = spinning
        // 不在转就整个藏起来：isDisplayedWhenStopped=false 只影响绘制，不改 isHidden，
        // 而图标要占同一个位置。
        spinner.isHidden = !spinning
        if spinning {
            spinner.startAnimation(nil)
        } else {
            spinner.stopAnimation(nil)
        }
    }

    @objc private func copyClicked() {
        guard let text = Self.copyText(for: renderedStage) else { return }
        if let onCopy {
            onCopy(text)
        } else {
            let pasteboard = NSPasteboard.general
            pasteboard.clearContents()
            pasteboard.setString(text, forType: .string)
        }
    }

    @objc private func dismissClicked() {
        onDismiss?()
    }

    // MARK: 测试钩子

    func testMessageText() -> String { messageLabel.stringValue }
    func testSpinnerAnimating() -> Bool { isSpinning && !spinner.isHidden && !isHidden }
    func testDismissVisible() -> Bool { !dismissButton.isHidden && !isHidden }
    func testCopyVisible() -> Bool { !copyButton.isHidden && !isHidden }
    func testClickCopy() { copyButton.performClick(nil) }
}
