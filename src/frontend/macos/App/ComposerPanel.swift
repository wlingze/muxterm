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

/// 面板根视图：Cmd-Enter 清空当前草稿，Cmd-K 打开右下角页面菜单。
final class ComposerRootView: NSView {
    var onClear: (() -> Void)?
    var onActions: (() -> Void)?

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        guard flags == .command else { return super.performKeyEquivalent(with: event) }
        switch event.charactersIgnoringModifiers?.lowercased() {
        case "\r", "\n":
            onClear?()
            return true
        case "k":
            onActions?()
            return true
        default:
            return super.performKeyEquivalent(with: event)
        }
    }
}

/// 页面菜单里的一行：标题 + 快捷键提示，选中时整行高亮。
final class ComposerActionRow: NSView {
    var onClick: (() -> Void)?
    private let titleLabel = NSTextField(labelWithString: "")
    private let hintLabel = NSTextField(labelWithString: "")

    var isSelected = false {
        didSet { updateHighlight() }
    }

    init(title: String, hint: String) {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 5
        translatesAutoresizingMaskIntoConstraints = false

        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.stringValue = title
        titleLabel.font = .systemFont(ofSize: 12.5)
        titleLabel.lineBreakMode = .byTruncatingTail
        addSubview(titleLabel)

        hintLabel.translatesAutoresizingMaskIntoConstraints = false
        hintLabel.stringValue = hint
        hintLabel.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        hintLabel.textColor = .tertiaryLabelColor
        hintLabel.setContentHuggingPriority(.required, for: .horizontal)
        addSubview(hintLabel)

        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: 28),
            titleLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            titleLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
            hintLabel.leadingAnchor.constraint(
                greaterThanOrEqualTo: titleLabel.trailingAnchor,
                constant: 8
            ),
            hintLabel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            hintLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
        updateHighlight()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    override func mouseDown(with event: NSEvent) {
        onClick?()
    }

    private func updateHighlight() {
        layer?.backgroundColor = isSelected
            ? NSColor.controlAccentColor.withAlphaComponent(0.18).cgColor
            : NSColor.clear.cgColor
        titleLabel.textColor = isSelected ? .controlAccentColor : .labelColor
    }
}

/// Cmd-K 页面菜单：右下角三个动作，键盘上下选择、Return 执行、Esc 收起。
final class ComposerActionMenuView: NSVisualEffectView {
    var onSelect: ((Int) -> Void)?
    private let stack = NSStackView()
    private(set) var rows: [ComposerActionRow] = []

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        material = .popover
        blendingMode = .withinWindow
        state = .active
        wantsLayer = true
        layer?.cornerRadius = 10
        layer?.borderWidth = 1
        layer?.borderColor = NSColor.separatorColor.cgColor
        translatesAutoresizingMaskIntoConstraints = false
        isHidden = true
        setAccessibilityIdentifier("muxterm.composer.actions")

        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 1
        stack.distribution = .fill
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    func configure(title: String, items: [(title: String, hint: String)]) {
        for view in stack.arrangedSubviews {
            stack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        rows.removeAll()

        let header = NSTextField(labelWithString: title)
        header.font = .systemFont(ofSize: 11, weight: .semibold)
        header.textColor = .secondaryLabelColor
        header.lineBreakMode = .byTruncatingTail
        header.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(header)
        header.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        for (index, item) in items.enumerated() {
            let row = ComposerActionRow(title: item.title, hint: item.hint)
            row.onClick = { [weak self] in self?.onSelect?(index) }
            stack.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
            rows.append(row)
        }
        select(0)
    }

    func select(_ index: Int) {
        guard !rows.isEmpty else { return }
        let clamped = min(max(index, 0), rows.count - 1)
        for (offset, row) in rows.enumerated() {
            row.isSelected = offset == clamped
        }
    }

    var selectedIndex: Int {
        rows.firstIndex(where: \.isSelected) ?? 0
    }
}

/// 独立输入框：一个 pane 一份草稿，窗口里用 tab 切换。
///
/// 面板不知道自己属于哪个 Workspace；打开时由 `targetProvider` 绑定当前
/// pane，之后写入/图片上传都打到这一页自己的目标上，不跟前台焦点漂移。
final class ComposerPanelController: NSWindowController, NSTextViewDelegate, NSWindowDelegate {
    /// 当前要绑定/写入的终端；nil 表示还没有打开的 workspace（仍可当草稿用）。
    var targetProvider: (() -> ComposerTarget?)?
    /// 把文本写进目标 pane；不自动回车。
    var onSend: ((String, ComposerTarget) -> Void)?
    /// 把 PNG 交给 Core 存到目标机器；完成后按 `ComposerDraftKey` 插回原草稿。
    var onPastePNG: ((Data, ComposerTarget, ComposerDraftKey) -> Void)?

    private let root = ComposerRootView()
    private let badge = NSTextField(labelWithString: "")
    private let badgeContainer = NSView()
    private let detailLabel = NSTextField(labelWithString: "")
    private let tabs = NSSegmentedControl()
    private let editor = ComposerTextView()
    private let editorScroll = NSScrollView()
    private let statusLabel = NSTextField(labelWithString: "")
    private let copyButton = NSButton()
    private let sendButton = NSButton()
    private let imageBanner = ImagePasteBannerView()
    private let actionMenu = ComposerActionMenuView()
    private var actionMenuBottom: NSLayoutConstraint?
    private(set) var drafts = ComposerDraftStore()
    private var imageTimeline = ImagePasteTimeline()
    private var target: ComposerTarget?
    private var isEncodingImage = false
    /// 上色时改 textStorage 不能再进 textDidChange，也不能进 undo。
    private var isHighlighting = false
    /// 切页/写入草稿时程序性改 string，也不再回写。
    private var isSyncingDrafts = false
    private var statusShowsHint = true
    private var keyMonitor: Any?
    private var hasPlacedWindow = false

    init() {
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: CGSize(width: 560, height: 760)),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = MuxtermI18n.shared.tr(.composerTitle)
        window.isMovableByWindowBackground = false
        // 普通窗口：不置顶、不跟随 Space、不抢其它应用的层级。
        window.level = .normal
        window.collectionBehavior = [.managed, .fullScreenAuxiliary]
        window.hasShadow = true
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(
            width: ComposerWindowLayout.minimumWidth,
            height: ComposerWindowLayout.minimumHeight
        )
        super.init(window: window)
        window.delegate = self
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
        _ = drafts.activate(targetProvider?())
        renderTabs()
        loadActivePage()
        refreshStrings()
        guard let window else { return }
        placeWindowIfNeeded(owner: owner, force: false)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        window.makeFirstResponder(editor)
    }

    func dismiss() {
        commitEditor()
        hideActions()
        window?.orderOut(nil)
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        commitEditor()
        hideActions()
        sender.orderOut(nil)
        return false
    }

    /// 主窗口切换场景后，把当前焦点对应的 pane 带进 tab（已有草稿就切过去）。
    func updateTarget(_ target: ComposerTarget?) {
        _ = drafts.activate(target)
        renderTabs()
        loadActivePage()
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

    /// 图片上传完成后插入纯路径；迟到回调写回发起粘贴的那一页。
    @discardableResult
    func insertImagePath(_ path: String, for key: ComposerDraftKey) -> Bool {
        guard let page = drafts.page(for: key) else { return false }
        let result = ComposerDraft.insertingImagePath(
            path,
            into: page.text,
            selectedRange: page.selection
        )
        drafts.update(
            text: result.text,
            selection: NSRange(location: result.caret, length: 0),
            for: key
        )
        if drafts.activeKey == key {
            loadActivePage()
        }
        return true
    }

    /// 测试/单页调用：插到当前页。
    @discardableResult
    func insertImagePath(_ path: String) -> Bool {
        if drafts.activeKey == nil {
            _ = drafts.activate(nil)
        }
        guard let key = drafts.activeKey else { return false }
        return insertImagePath(path, for: key)
    }

    func setStatus(_ message: String) {
        statusShowsHint = false
        statusLabel.stringValue = message
    }

    // MARK: - 图片反馈（与终端同一条时间线）

    /// 进入某个阶段并立即渲染右下角横幅。
    func advanceImage(_ stage: ImagePasteStage) {
        imageTimeline.advance(to: stage, now: Date())
        renderImageFeedback()
    }

    /// 事件泵每个 tick 调用：挂起的结果到点落下，成功提示到点消失。
    func tickImageFeedback(now: Date = Date()) {
        imageTimeline.tick(now: now)
        renderImageFeedback()
    }

    /// 失败同时写横幅和状态行，输入框不弹窗。
    func failImage(_ message: String) {
        setStatus(message)
        advanceImage(.failed(message: message))
    }

    private func renderImageFeedback() {
        imageBanner.apply(imageTimeline.visibleStage(now: Date()))
        layoutActionMenu()
    }

    // MARK: - 页面菜单（Cmd-K）

    func toggleActions() {
        if actionMenu.isHidden {
            showActions()
        } else {
            hideActions()
        }
    }

    private func showActions() {
        actionMenu.configure(
            title: actionMenuTitle(),
            items: [
                (MuxtermI18n.shared.tr(.composerCopy), ""),
                (MuxtermI18n.shared.tr(.composerSend), ""),
                (MuxtermI18n.shared.tr(.composerClear), "⌘↩"),
            ]
        )
        actionMenu.isHidden = false
        layoutActionMenu()
        actionMenu.select(0)
        installActionKeyMonitor()
    }

    private func hideActions() {
        actionMenu.isHidden = true
        removeActionKeyMonitor()
    }

    private func actionMenuTitle() -> String {
        guard let page = drafts.activePage else {
            return MuxtermI18n.shared.tr(.composerTitle)
        }
        return page.title
    }

    private func installActionKeyMonitor() {
        guard keyMonitor == nil else { return }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.actionMenu.isHidden == false,
                  self.window?.isKeyWindow == true
            else { return event }
            if Self.isBareCommandK(event) {
                self.hideActions()
                return nil
            }
            switch event.keyCode {
            case 53: // Escape
                self.hideActions()
                return nil
            case 125: // ↓
                self.moveActionSelection(by: 1)
                return nil
            case 126: // ↑
                self.moveActionSelection(by: -1)
                return nil
            case 36, 76: // Return / keypad Enter
                self.activateActionSelection()
                return nil
            default:
                return event
            }
        }
    }

    private func removeActionKeyMonitor() {
        if let keyMonitor {
            NSEvent.removeMonitor(keyMonitor)
        }
        keyMonitor = nil
    }

    private func moveActionSelection(by offset: Int) {
        let count = actionMenu.rows.count
        guard count > 0 else { return }
        actionMenu.select((actionMenu.selectedIndex + offset + count) % count)
    }

    private func activateActionSelection() {
        performAction(at: actionMenu.selectedIndex)
    }

    private func performAction(at index: Int) {
        hideActions()
        switch index {
        case 0: copyAll()
        case 1: sendCurrentText()
        case 2: clearCurrentPage()
        default: break
        }
    }

    private static func isBareCommandK(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags
            .intersection(.deviceIndependentFlagsMask)
            .intersection([.command, .option, .control, .shift])
        guard flags == [.command] else { return false }
        if let characters = event.charactersIgnoringModifiers?.lowercased(), !characters.isEmpty {
            return characters == "k"
        }
        return event.keyCode == 40
    }

    // MARK: - 草稿页

    private func commitEditor() {
        guard let key = drafts.activeKey, !isSyncingDrafts else { return }
        drafts.update(text: editor.string, selection: editor.selectedRange(), for: key)
    }

    private func loadActivePage() {
        isSyncingDrafts = true
        defer { isSyncingDrafts = false }
        guard let page = drafts.activePage else {
            editor.string = ""
            editor.setSelectedRange(NSRange(location: 0, length: 0))
            target = nil
            renderTarget()
            applyHighlight()
            return
        }
        editor.string = page.text
        let length = (page.text as NSString).length
        let location = min(max(page.selection.location, 0), length)
        editor.setSelectedRange(NSRange(location: location, length: 0))
        target = page.target
        renderTarget()
        applyHighlight()
    }

    @objc private func tabChanged() {
        let index = tabs.selectedSegment
        guard drafts.pages.indices.contains(index) else { return }
        commitEditor()
        _ = drafts.select(index: index)
        loadActivePage()
    }

    private func renderTabs() {
        let pages = drafts.pages
        tabs.isHidden = pages.count <= 1
        tabs.segmentCount = max(pages.count, 1)
        for (index, page) in pages.enumerated() {
            tabs.setLabel(page.title, forSegment: index)
            tabs.setToolTip(page.target?.detail ?? MuxtermI18n.shared.tr(.composerNoTarget), forSegment: index)
        }
        if let index = drafts.activeIndex {
            tabs.selectedSegment = index
        } else {
            tabs.selectedSegment = -1
        }
    }

    private func sendCurrentText() {
        commitEditor()
        guard let page = drafts.activePage,
              let text = ComposerDraft(text: page.text).payload()
        else {
            setStatus(MuxtermI18n.shared.tr(.composerNothingToSend))
            return
        }
        guard let target = page.target else {
            setStatus(MuxtermI18n.shared.tr(.composerNoTarget))
            return
        }
        onSend?(text, target)
        setStatus(MuxtermI18n.shared.tr(.composerSent))
    }

    /// 清空只针对当前页；用 AppKit 删除，Cmd-Z 可以撤销，避免误清。
    private func clearCurrentPage() {
        commitEditor()
        guard let page = drafts.activePage else { return }
        guard !page.text.isEmpty else {
            setStatus(MuxtermI18n.shared.tr(.composerNothingToClear))
            return
        }
        editor.selectAll(nil)
        editor.deleteBackward(nil)
        editor.setSelectedRange(NSRange(location: 0, length: 0))
        commitEditor()
        applyHighlight()
        setStatus(MuxtermI18n.shared.tr(.composerCleared))
    }

    private func beginImage(_ data: Data) {
        guard !isEncodingImage, !imageTimeline.isBusy else {
            setStatus(MuxtermI18n.shared.tr(.imagePasteBusy))
            return
        }
        commitEditor()
        guard let page = drafts.activePage, let target = page.target else {
            setStatus(MuxtermI18n.shared.tr(.composerNoTarget))
            return
        }
        isEncodingImage = true
        setStatus(MuxtermI18n.shared.tr(.composerImageEncoding))
        advanceImage(.encoding)
        let key = page.key
        // 与终端粘贴一致：先复制数据，编码期间切换目标/切页不改变目的地。
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
                    self.onPastePNG?(png, target, key)
                case .failure:
                    self.failImage(MuxtermI18n.shared.tr(.imagePasteInvalid))
                }
            }
        }
    }

    func textDidChange(_ notification: Notification) {
        guard !isHighlighting, !isSyncingDrafts, !editor.hasMarkedText() else { return }
        commitEditor()
        applyHighlight()
    }

    // MARK: - 视图

    private func buildView() {
        guard let window else { return }
        root.onClear = { [weak self] in self?.clearCurrentPage() }
        root.onActions = { [weak self] in self?.toggleActions() }
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

        tabs.translatesAutoresizingMaskIntoConstraints = false
        tabs.setAccessibilityIdentifier("muxterm.composer.tabs")
        tabs.segmentStyle = .separated
        tabs.trackingMode = .selectOne
        tabs.segmentDistribution = .fillEqually
        tabs.controlSize = .small
        tabs.font = .systemFont(ofSize: 11, weight: .medium)
        tabs.target = self
        tabs.action = #selector(tabChanged)
        tabs.isHidden = true

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
        // Cmd-Enter 让给「清空」；写入走按钮或 Cmd-K 页面菜单。
        sendButton.keyEquivalent = ""

        imageBanner.translatesAutoresizingMaskIntoConstraints = false
        imageBanner.onDismiss = { [weak self] in
            guard let self else { return }
            self.imageTimeline.dismiss()
            self.renderImageFeedback()
        }

        [badgeContainer, detailLabel, tabs, editorScroll, statusLabel, copyButton, sendButton, imageBanner, actionMenu]
            .forEach(root.addSubview)

        actionMenuBottom = actionMenu.bottomAnchor.constraint(
            equalTo: statusLabel.topAnchor,
            constant: -8
        )
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

            tabs.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            tabs.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            tabs.topAnchor.constraint(equalTo: badgeContainer.bottomAnchor, constant: 10),
            tabs.heightAnchor.constraint(equalToConstant: 22),

            editorScroll.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            editorScroll.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            editorScroll.topAnchor.constraint(equalTo: tabs.bottomAnchor, constant: 10),
            editorScroll.bottomAnchor.constraint(equalTo: statusLabel.topAnchor, constant: -12),
            editorScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 180),

            statusLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            statusLabel.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -14),
            statusLabel.trailingAnchor.constraint(equalTo: copyButton.leadingAnchor, constant: -12),

            copyButton.trailingAnchor.constraint(equalTo: sendButton.leadingAnchor, constant: -8),
            copyButton.centerYAnchor.constraint(equalTo: statusLabel.centerYAnchor),
            sendButton.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            sendButton.centerYAnchor.constraint(equalTo: statusLabel.centerYAnchor),

            imageBanner.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            imageBanner.bottomAnchor.constraint(equalTo: statusLabel.topAnchor, constant: -8),
            imageBanner.widthAnchor.constraint(equalToConstant: 300),

            actionMenu.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            actionMenu.widthAnchor.constraint(equalToConstant: 240),
            actionMenuBottom!,
        ])

        renderTabs()
        renderTarget()
        applyHighlight()
    }

    private func placeWindowIfNeeded(owner: NSWindow?, force: Bool) {
        guard let window, force || !hasPlacedWindow else { return }
        let screen = owner?.screen ?? NSScreen.main
        let visible = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let size = ComposerWindowLayout.size(
            screenWidth: visible.width,
            screenHeight: visible.height
        )
        let origin: NSPoint
        if let owner {
            origin = NSPoint(
                x: owner.frame.midX - size.width / 2,
                y: owner.frame.midY - size.height / 2
            )
        } else {
            origin = NSPoint(
                x: visible.midX - size.width / 2,
                y: visible.midY - size.height / 2
            )
        }
        window.setFrame(
            NSRect(origin: origin, size: size),
            display: false
        )
        hasPlacedWindow = true
    }

    private func layoutActionMenu() {
        guard let actionMenuBottom else { return }
        actionMenuBottom.constant = imageBanner.isHidden ? -8 : -(imageBanner.frame.height + 8)
    }

    private func refreshStrings() {
        window?.title = MuxtermI18n.shared.tr(.composerTitle)
        copyButton.title = MuxtermI18n.shared.tr(.composerCopy)
        copyButton.toolTip = MuxtermI18n.shared.tr(.composerCopy)
        sendButton.title = MuxtermI18n.shared.tr(.composerSend)
        sendButton.toolTip = MuxtermI18n.shared.tr(.composerSend)
        if statusShowsHint {
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

    // MARK: - 测试读取（AppLib 内部）

    var draftText: String { editor.string }
    var badgeText: String { badge.stringValue }
    var detailText: String { detailLabel.stringValue }
    var statusText: String { statusLabel.stringValue }
    var editorIsFirstResponder: Bool { window?.firstResponder === editor }
    var pageTitles: [String] { drafts.pages.map(\.title) }
    var activePageIndex: Int? { drafts.activeIndex }
    var actionMenuVisible: Bool { !actionMenu.isHidden }
    var actionMenuTitles: [String] { actionMenu.rows.map(\.testTitle) }
    var actionMenuSelection: Int { actionMenu.selectedIndex }
    var imageBannerVisible: Bool { !imageBanner.isHidden }
    var imageBannerMessage: String { imageBanner.testMessageText() }

    func setDraftForTesting(_ text: String) {
        if drafts.activeKey == nil {
            _ = drafts.activate(targetProvider?())
            renderTabs()
        }
        editor.string = text
        commitEditor()
        applyHighlight()
    }

    func sendForTesting() {
        sendCurrentText()
    }

    func clearForTesting() {
        clearCurrentPage()
    }

    func selectPageForTesting(_ index: Int) {
        guard drafts.select(index: index) else { return }
        renderTabs()
        loadActivePage()
    }

    func toggleActionsForTesting() {
        toggleActions()
    }

    func moveActionSelectionForTesting(by offset: Int) {
        moveActionSelection(by: offset)
    }

    func activateActionForTesting() {
        activateActionSelection()
    }

    func performActionForTesting(_ index: Int) {
        performAction(at: index)
    }

    func tickImageFeedbackForTesting() {
        tickImageFeedback()
    }

    func undoForTesting() {
        editor.undoManager?.undo()
    }

    /// 走真实的 performKeyEquivalent 通路，验证 ⌘↩ / ⌘K 的绑定而不是直接调动作。
    func performKeyEquivalentForTesting(
        _ characters: String,
        modifiers: NSEvent.ModifierFlags = .command
    ) -> Bool {
        guard let event = NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: modifiers,
            timestamp: 0,
            windowNumber: window?.windowNumber ?? 0,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: characters,
            isARepeat: false,
            keyCode: 0
        ) else {
            return false
        }
        return root.performKeyEquivalent(with: event)
    }

    @objc private func copyTapped() {
        copyAll()
    }

    @objc private func sendTapped() {
        sendCurrentText()
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

extension ComposerActionRow {
    var testTitle: String {
        subviews.compactMap { $0 as? NSTextField }.first?.stringValue ?? ""
    }
}
