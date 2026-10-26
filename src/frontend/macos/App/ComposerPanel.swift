import AppKit
import MuxtermChrome

/// 输入框里的编辑视图：图片粘贴交给独立输入框，不变成文本附件。
final class ComposerTextView: NSTextView {
    /// 系统剪贴板或拖拽里带着图片。由面板负责编码和上传。
    var onImage: ((Data) -> Void)?
    /// Esc 收起输入框；组合键编辑动作仍交给 AppKit。
    var onEscape: (() -> Void)?

    /// AppKit 先检查可读取类型，再分发菜单 Cmd-V；只重写 paste: 不够。
    /// 文本只接收原文，避免外来 RTF/HTML 附件绕过图片上传通路。
    override var readablePasteboardTypes: [NSPasteboard.PasteboardType] {
        [.png, .tiff, .string]
    }

    override func paste(_ sender: Any?) {
        _ = readSelection(from: .general)
    }

    override func readSelection(from pasteboard: NSPasteboard) -> Bool {
        if let data = Self.imageData(from: pasteboard) {
            guard let onImage else { return false }
            onImage(data)
            return true
        }
        guard let text = pasteboard.string(forType: .string) else { return false }
        insertText(text, replacementRange: selectedRange())
        return true
    }

    override func readSelection(from pboard: NSPasteboard, type: NSPasteboard.PasteboardType) -> Bool {
        readSelection(from: pboard)
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
    private let statusLabel = NSTextField(labelWithString: "")
    private let copyButton = NSButton()
    private let sendButton = NSButton()
    private var target: ComposerTarget?
    private var isEncodingImage = false
    /// 上色时改 textStorage 不能再进 textDidChange，也不能进 undo。
    private var isHighlighting = false

    init() {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 560, height: 360),
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
        applyHighlight()
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
        applyHighlight()
    }

    func setStatus(_ message: String) {
        statusLabel.stringValue = message
    }

    // MARK: - 测试读取（AppLib 内部）

    var draftText: String { editor.string }
    var badgeText: String { badge.stringValue }
    var detailText: String { detailLabel.stringValue }
    var statusText: String { statusLabel.stringValue }
    var editorIsFirstResponder: Bool { window?.firstResponder === editor }

    func setDraftForTesting(_ text: String) {
        editor.string = text
        applyHighlight()
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
        guard !isHighlighting, !editor.hasMarkedText() else { return }
        applyHighlight()
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
        // 富文本只为了显示语法色；字符串仍是原文，发送时读 editor.string。
        editor.isRichText = true
        editor.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        editor.textColor = .labelColor
        editor.typingAttributes = [
            .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .regular),
            .foregroundColor: NSColor.labelColor,
        ]
        editor.drawsBackground = false
        editor.isAutomaticQuoteSubstitutionEnabled = false
        editor.isAutomaticDashSubstitutionEnabled = false
        editor.isAutomaticDataDetectionEnabled = false
        editor.isAutomaticLinkDetectionEnabled = false
        editor.isAutomaticTextReplacementEnabled = false
        editor.isContinuousSpellCheckingEnabled = false
        editor.isGrammarCheckingEnabled = false
        editor.allowsUndo = true
        editor.usesFontPanel = false
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

        [badgeContainer, detailLabel, editorScroll, statusLabel, copyButton, sendButton]
            .forEach(root.addSubview)

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
            editorScroll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            editorScroll.topAnchor.constraint(equalTo: badgeContainer.bottomAnchor, constant: 12),
            editorScroll.bottomAnchor.constraint(equalTo: statusLabel.topAnchor, constant: -12),
            editorScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 180),

            statusLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            statusLabel.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -14),
            statusLabel.trailingAnchor.constraint(equalTo: copyButton.leadingAnchor, constant: -12),

            copyButton.trailingAnchor.constraint(equalTo: sendButton.leadingAnchor, constant: -8),
            copyButton.centerYAnchor.constraint(equalTo: statusLabel.centerYAnchor),
            sendButton.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            sendButton.centerYAnchor.constraint(equalTo: statusLabel.centerYAnchor),
        ])

        renderTarget()
        applyHighlight()
    }

    private func refreshStrings() {
        window?.title = MuxtermI18n.shared.tr(.composerTitle)
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

    /// 给原文上色。不改字符，所以选区和要发送的字符串都保持不动。
    private func applyHighlight() {
        guard let storage = editor.textStorage else { return }
        let source = editor.string as NSString
        let full = NSRange(location: 0, length: source.length)
        isHighlighting = true
        editor.undoManager?.disableUndoRegistration()
        defer {
            editor.undoManager?.enableUndoRegistration()
            isHighlighting = false
        }
        storage.beginEditing()
        if full.length > 0 {
            storage.setAttributes([
                .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .regular),
                .foregroundColor: NSColor.labelColor,
            ], range: full)
            for span in ComposerMarkdown.highlights(editor.string) {
                let range = NSIntersectionRange(span.range, full)
                guard range.length > 0 else { continue }
                storage.addAttributes(Self.attributes(for: span.style), range: range)
            }
        }
        storage.endEditing()
    }

    private static func attributes(for style: ComposerHighlightStyle) -> [NSAttributedString.Key: Any] {
        let mono = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
        switch style {
        case .marker:
            return [
                .font: mono,
                .foregroundColor: NSColor.tertiaryLabelColor,
            ]
        case .header:
            return [
                .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .semibold),
                .foregroundColor: NSColor.labelColor,
            ]
        case .bold:
            return [
                .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .bold),
                .foregroundColor: NSColor.labelColor,
            ]
        case .italic:
            return [
                .font: NSFontManager.shared.convert(mono, toHaveTrait: .italicFontMask),
                .foregroundColor: NSColor.labelColor,
            ]
        case .inlineCode, .codeBlock:
            return [
                .font: mono,
                .foregroundColor: NSColor.labelColor,
                .backgroundColor: NSColor.quaternaryLabelColor.withAlphaComponent(0.35),
            ]
        case .quote:
            return [
                .font: mono,
                .foregroundColor: NSColor.secondaryLabelColor,
            ]
        case .listMarker:
            return [
                .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .semibold),
                .foregroundColor: NSColor.controlAccentColor,
            ]
        case .path:
            return [
                .font: mono,
                .foregroundColor: NSColor.linkColor,
            ]
        }
    }
}
