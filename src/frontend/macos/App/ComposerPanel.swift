import AppKit
import MuxtermChrome

/// 输入框里的编辑视图：图片粘贴交给独立输入框，不变成文本附件。
final class ComposerTextView: NSTextView {
    /// 系统剪贴板或拖拽里带着图片。由面板负责编码和上传。
    var onImage: ((Data) -> Void)?
    /// Esc 收起输入框；组合键编辑动作仍交给 AppKit。
    var onEscape: (() -> Void)?

    override func paste(_ sender: Any?) {
        if let data = Self.imageData(from: NSPasteboard.general) {
            onImage?(data)
            return
        }
        super.paste(sender)
    }

    /// 拖拽图片同样只插入路径，不把图片塞进文本。
    override func readSelection(from pboard: NSPasteboard, type: NSPasteboard.PasteboardType) -> Bool {
        if let data = Self.imageData(from: pboard) {
            onImage?(data)
            return true
        }
        return super.readSelection(from: pboard, type: type)
    }

    override func keyDown(with event: NSEvent) {
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        if event.keyCode == 53, flags.isEmpty {
            onEscape?()
            return
        }
        super.keyDown(with: event)
    }

    private static func imageData(from pasteboard: NSPasteboard) -> Data? {
        guard let type = pasteboard.availableType(from: [.png, .tiff]) else { return nil }
        return pasteboard.data(forType: type)
    }
}

/// 面板根视图：Cmd-Enter 必须发送，而不是被按钮 key equivalent 抢成别的动作。
final class ComposerRootView: NSView {
    var onSend: (() -> Void)?

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        if flags == .command, ["\r", "\n"].contains(event.charactersIgnoringModifiers ?? "") {
            onSend?()
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
}

/// 独立输入框：写一段纯文本，随时复制，或 Cmd-Enter 写进当前 pane。
///
/// 面板不知道自己属于哪个 Workspace；目标由 `targetProvider` 在打开和
/// 发送前读取，图片上传也用同一份快照，避免切换工作区后写错机器。
final class ComposerPanelController: NSWindowController, NSTextViewDelegate {
    /// 当前要写入的终端；nil 表示还没有打开的 workspace（仍可当草稿用）。
    var targetProvider: (() -> ComposerTarget?)?
    /// 把文本写进目标 pane；不自动回车。
    var onSend: ((String, ComposerTarget?) -> Void)?
    /// 把 PNG 交给 Core 存到目标机器，完成后回调 `insertImagePath`。
    var onPastePNG: ((Data, ComposerTarget?) -> Void)?
    /// 图片编码阶段，让主窗口复用现有“正在处理图片”的提示。
    var onEncodingImage: (() -> Void)?

    private let root = ComposerRootView()
    private let badge = NSTextField(labelWithString: "")
    private let badgeContainer = NSView()
    private let detailLabel = NSTextField(labelWithString: "")
    private let editor = ComposerTextView()
    private let editorScroll = NSScrollView()
    private let preview = NSTextView()
    private let previewScroll = NSScrollView()
    private let previewCaption = NSTextField(labelWithString: "")
    private let previewEmpty = NSTextField(labelWithString: "")
    private let statusLabel = NSTextField(labelWithString: "")
    private let copyButton = NSButton()
    private let sendButton = NSButton()
    private var target: ComposerTarget?
    private var isEncodingImage = false

    init() {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 480),
            styleMask: [.titled, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.title = MuxtermI18n.shared.tr(.composerTitle)
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = true
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.hidesOnDeactivate = false
        panel.hasShadow = true
        super.init(window: panel)
        buildView()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    var isVisible: Bool {
        window?.isVisible == true
    }

    func present(from owner: NSWindow?) {
        target = targetProvider?()
        renderTarget()
        refreshStrings()
        renderPreview()
        guard let window else { return }
        window.level = .floating
        if let owner {
            let ownerFrame = owner.frame
            window.setFrameOrigin(NSPoint(
                x: ownerFrame.midX - window.frame.width / 2,
                y: ownerFrame.midY - window.frame.height / 2
            ))
        } else {
            window.center()
        }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        window.makeFirstResponder(editor)
    }

    func dismiss() {
        window?.orderOut(nil)
    }

    /// 主窗口切换场景后，下一次发送/上传前刷新徽章。
    func updateTarget(_ target: ComposerTarget?) {
        self.target = target
        renderTarget()
    }

    /// 复制按钮写的是全文；编辑器里的 Cmd+C 仍只复制选区。
    @discardableResult
    func copyAll() -> Bool {
        let text = editor.string
        guard !text.isEmpty else {
            setStatus(MuxtermI18n.shared.tr(.composerNothingToCopy))
            return false
        }
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        guard pasteboard.setString(text, forType: .string) else {
            setStatus(MuxtermI18n.shared.tr(.composerCopyFailed))
            return false
        }
        setStatus(MuxtermI18n.shared.tr(.composerCopied))
        return true
    }

    /// 图片上传完成后插入纯路径。是否仍在显示由主窗口判断，避免路径静默丢失。
    func insertImagePath(_ path: String) {
        let result = ComposerDraft.insertingImagePath(
            path,
            into: editor.string,
            selectedRange: editor.selectedRange()
        )
        editor.string = result.text
        editor.setSelectedRange(NSRange(location: result.caret, length: 0))
        renderPreview()
    }

    func setStatus(_ message: String) {
        statusLabel.stringValue = message
    }

    // MARK: - 测试读取（AppLib 内部）

    var draftText: String { editor.string }
    var previewText: String { preview.string }
    var badgeText: String { badge.stringValue }
    var detailText: String { detailLabel.stringValue }
    var statusText: String { statusLabel.stringValue }
    var editorIsFirstResponder: Bool { window?.firstResponder === editor }

    func setDraftForTesting(_ text: String) {
        editor.string = text
        renderPreview()
    }

    func sendForTesting() {
        sendCurrentText()
    }

    @objc private func copyTapped() {
        copyAll()
    }

    @objc private func sendTapped() {
        sendCurrentText()
    }

    private func sendCurrentText() {
        // 发送前重新读一次目标：面板打开期间可能换了场景。
        if let provider = targetProvider {
            target = provider()
            renderTarget()
        }
        guard let text = ComposerDraft(text: editor.string).payload() else {
            setStatus(MuxtermI18n.shared.tr(.composerNothingToSend))
            return
        }
        guard let target else {
            setStatus(MuxtermI18n.shared.tr(.composerNoTarget))
            return
        }
        onSend?(text, target)
        setStatus(MuxtermI18n.shared.tr(.composerSent))
    }

    private func beginImage(_ data: Data) {
        guard !isEncodingImage else {
            setStatus(MuxtermI18n.shared.tr(.imagePasteBusy))
            return
        }
        if let provider = targetProvider {
            target = provider()
            renderTarget()
        }
        guard let target else {
            setStatus(MuxtermI18n.shared.tr(.composerNoTarget))
            return
        }
        isEncodingImage = true
        onEncodingImage?()
        setStatus(MuxtermI18n.shared.tr(.composerImageEncoding))
        // 与终端粘贴一致：先复制数据，编码期间切换目标不改变目的地。
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result = Result { try ClipboardImageCodec.png(from: data) }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.isEncodingImage = false
                switch result {
                case .success(let png):
                    self.setStatus(MuxtermI18n.shared.tr(.composerImageUploading))
                    // 回调可能同步失败并改状态（没有 workspace、Core 忙），
                    // 所以先写「上传中」，让失败信息成为最后一句。
                    self.onPastePNG?(png, target)
                case .failure:
                    self.setStatus(MuxtermI18n.shared.tr(.imagePasteInvalid))
                }
            }
        }
    }

    func textDidChange(_ notification: Notification) {
        renderPreview()
    }

    // MARK: - 视图

    private func buildView() {
        guard let window else { return }
        root.onSend = { [weak self] in self?.sendCurrentText() }
        window.contentView = root

        badgeContainer.translatesAutoresizingMaskIntoConstraints = false
        badgeContainer.wantsLayer = true
        badgeContainer.layer?.cornerRadius = 5
        badgeContainer.layer?.masksToBounds = true
        badge.translatesAutoresizingMaskIntoConstraints = false
        badge.font = .systemFont(ofSize: 11, weight: .semibold)
        badge.alignment = .center
        badgeContainer.addSubview(badge)

        detailLabel.translatesAutoresizingMaskIntoConstraints = false
        detailLabel.font = .systemFont(ofSize: 11)
        detailLabel.textColor = .secondaryLabelColor
        detailLabel.lineBreakMode = .byTruncatingTail

        editor.string = ""
        editor.isRichText = false
        editor.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        editor.textColor = .labelColor
        editor.drawsBackground = false
        editor.isAutomaticQuoteSubstitutionEnabled = false
        editor.isAutomaticDashSubstitutionEnabled = false
        editor.isAutomaticDataDetectionEnabled = false
        editor.isAutomaticLinkDetectionEnabled = false
        editor.isAutomaticTextReplacementEnabled = false
        editor.isContinuousSpellCheckingEnabled = false
        editor.isGrammarCheckingEnabled = false
        editor.allowsUndo = true
        editor.delegate = self
        editor.textContainerInset = NSSize(width: 10, height: 10)
        editor.isVerticallyResizable = true
        editor.isHorizontallyResizable = false
        editor.autoresizingMask = [.width]
        editor.minSize = .zero
        editor.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        editor.textContainer?.widthTracksTextView = true
        editor.textContainer?.containerSize = NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude)
        editor.onImage = { [weak self] data in self?.beginImage(data) }
        editor.onEscape = { [weak self] in self?.dismiss() }

        editorScroll.translatesAutoresizingMaskIntoConstraints = false
        editorScroll.documentView = editor
        editorScroll.hasVerticalScroller = true
        editorScroll.drawsBackground = true
        editorScroll.backgroundColor = .textBackgroundColor
        editorScroll.borderType = .noBorder
        editorScroll.wantsLayer = true
        editorScroll.layer?.cornerRadius = 8
        editorScroll.layer?.borderWidth = 1
        editorScroll.layer?.borderColor = NSColor.separatorColor.cgColor

        previewCaption.translatesAutoresizingMaskIntoConstraints = false
        previewCaption.stringValue = MuxtermI18n.shared.tr(.composerPreview)
        previewCaption.font = .systemFont(ofSize: 11, weight: .medium)
        previewCaption.textColor = .secondaryLabelColor

        preview.isEditable = false
        preview.isSelectable = true
        preview.isRichText = true
        preview.drawsBackground = false
        preview.textColor = .labelColor
        preview.textContainerInset = NSSize(width: 10, height: 10)
        preview.isVerticallyResizable = true
        preview.isHorizontallyResizable = false
        preview.autoresizingMask = [.width]
        preview.minSize = .zero
        preview.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        preview.textContainer?.widthTracksTextView = true
        preview.textContainer?.containerSize = NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude)
        preview.textContainer?.lineFragmentPadding = 4
        preview.linkTextAttributes = [
            .foregroundColor: NSColor.linkColor,
            .underlineStyle: NSUnderlineStyle.single.rawValue,
        ]

        previewScroll.translatesAutoresizingMaskIntoConstraints = false
        previewScroll.documentView = preview
        previewScroll.hasVerticalScroller = true
        previewScroll.drawsBackground = true
        previewScroll.backgroundColor = .controlBackgroundColor
        previewScroll.borderType = .noBorder
        previewScroll.wantsLayer = true
        previewScroll.layer?.cornerRadius = 8
        previewScroll.layer?.borderWidth = 1
        previewScroll.layer?.borderColor = NSColor.separatorColor.cgColor

        previewEmpty.translatesAutoresizingMaskIntoConstraints = false
        previewEmpty.stringValue = MuxtermI18n.shared.tr(.composerPreviewEmpty)
        previewEmpty.font = .systemFont(ofSize: 12)
        previewEmpty.textColor = .tertiaryLabelColor
        previewEmpty.alignment = .center

        statusLabel.translatesAutoresizingMaskIntoConstraints = false
        statusLabel.setAccessibilityIdentifier("muxterm.composer.status")
        statusLabel.stringValue = MuxtermI18n.shared.tr(.composerPlaceholder)
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.font = .systemFont(ofSize: 11)
        statusLabel.textColor = .secondaryLabelColor

        copyButton.translatesAutoresizingMaskIntoConstraints = false
        copyButton.setAccessibilityIdentifier("muxterm.composer.copy")
        copyButton.target = self
        copyButton.action = #selector(copyTapped)
        copyButton.bezelStyle = .rounded
        copyButton.controlSize = .small
        copyButton.font = .systemFont(ofSize: 11)

        sendButton.translatesAutoresizingMaskIntoConstraints = false
        sendButton.setAccessibilityIdentifier("muxterm.composer.send")
        sendButton.target = self
        sendButton.action = #selector(sendTapped)
        sendButton.bezelStyle = .rounded
        sendButton.controlSize = .small
        sendButton.font = .systemFont(ofSize: 11)
        sendButton.keyEquivalent = "\r"
        sendButton.keyEquivalentModifierMask = .command

        [badgeContainer, detailLabel, editorScroll, previewCaption, previewScroll,
         previewEmpty, statusLabel, copyButton, sendButton].forEach(root.addSubview)

        NSLayoutConstraint.activate([
            badgeContainer.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            badgeContainer.topAnchor.constraint(equalTo: root.topAnchor, constant: 14),
            badgeContainer.heightAnchor.constraint(equalToConstant: 18),
            badge.leadingAnchor.constraint(equalTo: badgeContainer.leadingAnchor, constant: 7),
            badge.trailingAnchor.constraint(equalTo: badgeContainer.trailingAnchor, constant: -7),
            badge.centerYAnchor.constraint(equalTo: badgeContainer.centerYAnchor),

            detailLabel.leadingAnchor.constraint(equalTo: badgeContainer.trailingAnchor, constant: 8),
            detailLabel.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            detailLabel.centerYAnchor.constraint(equalTo: badgeContainer.centerYAnchor),

            editorScroll.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            editorScroll.topAnchor.constraint(equalTo: badgeContainer.bottomAnchor, constant: 12),
            editorScroll.trailingAnchor.constraint(equalTo: previewScroll.leadingAnchor, constant: -12),

            previewCaption.leadingAnchor.constraint(equalTo: editorScroll.trailingAnchor, constant: 12),
            previewCaption.topAnchor.constraint(equalTo: editorScroll.topAnchor, constant: 2),

            previewScroll.topAnchor.constraint(equalTo: previewCaption.bottomAnchor, constant: 6),
            previewScroll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            previewScroll.bottomAnchor.constraint(equalTo: editorScroll.bottomAnchor),

            previewEmpty.centerXAnchor.constraint(equalTo: previewScroll.centerXAnchor),
            previewEmpty.centerYAnchor.constraint(equalTo: previewScroll.centerYAnchor),

            statusLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            statusLabel.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -14),
            statusLabel.trailingAnchor.constraint(equalTo: copyButton.leadingAnchor, constant: -12),

            copyButton.trailingAnchor.constraint(equalTo: sendButton.leadingAnchor, constant: -8),
            copyButton.centerYAnchor.constraint(equalTo: statusLabel.centerYAnchor),
            sendButton.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            sendButton.centerYAnchor.constraint(equalTo: statusLabel.centerYAnchor),

            editorScroll.bottomAnchor.constraint(equalTo: statusLabel.topAnchor, constant: -12),
            editorScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 160),
        ])

        // 输入框与预览等宽，各占剩余宽度的一半。
        editorScroll.widthAnchor.constraint(
            equalTo: previewScroll.widthAnchor,
            multiplier: 1.1
        ).isActive = true

        renderTarget()
        renderPreview()
    }

    private func refreshStrings() {
        window?.title = MuxtermI18n.shared.tr(.composerTitle)
        previewCaption.stringValue = MuxtermI18n.shared.tr(.composerPreview)
        previewEmpty.stringValue = MuxtermI18n.shared.tr(.composerPreviewEmpty)
        copyButton.title = MuxtermI18n.shared.tr(.composerCopy)
        copyButton.toolTip = MuxtermI18n.shared.tr(.composerCopy)
        sendButton.title = MuxtermI18n.shared.tr(.composerSend)
        sendButton.toolTip = MuxtermI18n.shared.tr(.composerSend)
        if statusLabel.stringValue.isEmpty {
            statusLabel.stringValue = MuxtermI18n.shared.tr(.composerPlaceholder)
        }
    }

    private func renderTarget() {
        guard let target else {
            badge.stringValue = "—"
            badgeContainer.layer?.backgroundColor = NSColor.secondaryLabelColor
                .withAlphaComponent(0.14).cgColor
            badge.textColor = .secondaryLabelColor
            detailLabel.stringValue = MuxtermI18n.shared.tr(.composerNoTarget)
            return
        }
        badge.stringValue = target.badge
        switch target.kind {
        case .local:
            badgeContainer.layer?.backgroundColor = NSColor.controlAccentColor
                .withAlphaComponent(0.16).cgColor
            badge.textColor = .controlAccentColor
        case .ssh:
            badgeContainer.layer?.backgroundColor = NSColor.systemOrange
                .withAlphaComponent(0.18).cgColor
            badge.textColor = .systemOrange
        }
        detailLabel.stringValue = target.detail
        detailLabel.toolTip = target.detail
    }

    private func renderPreview() {
        let rendered = Self.styledPreview(ComposerMarkdown.preview(editor.string))
        preview.textStorage?.setAttributedString(rendered)
        previewEmpty.isHidden = !rendered.string.isEmpty
    }

    /// 把 Chrome 给出的 Markdown 样式段映射成 AppKit 字体/颜色。
    /// 标题、行内粗斜体/代码、代码块与引用块在预览里都要能区分出来。
    private static func styledPreview(_ preview: ComposerPreview) -> NSAttributedString {
        let rendered = NSMutableAttributedString(string: preview.text)
        let full = NSRange(location: 0, length: rendered.length)
        guard full.length > 0 else { return rendered }
        rendered.addAttribute(.foregroundColor, value: NSColor.labelColor, range: full)
        for span in preview.spans {
            let range = NSIntersectionRange(span.range, full)
            guard range.length > 0 else { continue }
            let font: NSFont
            switch span.style {
            case .body:
                font = .systemFont(ofSize: 13)
            case .bold:
                font = .boldSystemFont(ofSize: 13)
            case .italic:
                font = NSFontManager.shared.convert(
                    .systemFont(ofSize: 13),
                    toHaveTrait: .italicFontMask
                )
            case .boldItalic:
                let bold = NSFontManager.shared.convert(
                    .systemFont(ofSize: 13),
                    toHaveTrait: .boldFontMask
                )
                font = NSFontManager.shared.convert(bold, toHaveTrait: .italicFontMask)
            case .inlineCode, .codeBlock:
                font = .monospacedSystemFont(ofSize: 12.5, weight: .regular)
                rendered.addAttribute(
                    .backgroundColor,
                    value: NSColor.quaternaryLabelColor.withAlphaComponent(0.35),
                    range: range
                )
            case .header(let level):
                font = .systemFont(ofSize: headerPointSize(level), weight: .semibold)
            case .quote:
                font = .systemFont(ofSize: 13)
                rendered.addAttribute(
                    .foregroundColor,
                    value: NSColor.secondaryLabelColor,
                    range: range
                )
            }
            rendered.addAttribute(.font, value: font, range: range)
        }
        return rendered
    }

    private static func headerPointSize(_ level: Int) -> CGFloat {
        switch level {
        case 1: return 20
        case 2: return 17
        case 3: return 15
        default: return 13.5
        }
    }
}
