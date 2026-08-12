import AppKit
import MuxtermChrome

/// 目标配置窗口：runtime / transport / path / name。
///
/// - runtime / transport：两个并列「卡片」单选，选中项高亮。
/// - SSH name：可编辑下拉，选项来自 ~/.ssh/config Host alias。
/// - path：可编辑下拉 + 逐步浏览；选择子目录后自动更新 name。
/// - name：可编辑下拉（已有 project 名），path 变化时自动填充。
final class TargetConfigWindow: NSWindow, NSWindowDelegate, NSComboBoxDelegate {
    var onSave: ((TargetConfig) -> Void)?
    /// 关闭时回调（用于重新显示 QuickConnect 面板）。
    var onCancel: (() -> Void)?

    private let store: QuickConnectStore
    private let sshHosts: [SSHHostInfo]

    private let runtimeStack = NSStackView()
    private let transportStack = NSStackView()
    private let sshNameCombo = NSComboBox()
    private let pathCombo = NSComboBox()
    private let upButton = NSButton(title: "↑ 上级", target: nil, action: nil)
    private let nameCombo = NSComboBox()
    private let nameHint = NSTextField(labelWithString: "")
    private var editing: TargetConfig?
    private var keyMonitor: Any?
    private var isSaving = false
    /// 用户是否手动改过 name；手动改过后 path 变化不再覆盖。
    private var nameManuallyEdited = false
    /// runtime / transport 单选卡的纯状态模型。
    private var selection = TargetOptionSelection()
    /// 当前浏览路径（SSH 用远端路径，local 用本机路径）。
    private var currentPath: String = "~"

    init(
        editing config: TargetConfig? = nil,
        owner: NSWindow?,
        store: QuickConnectStore,
        sshHosts: [SSHHostInfo]
    ) {
        self.editing = config
        self.store = store
        self.sshHosts = sshHosts
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 380),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        title = config == nil ? "New Project" : "Edit Project"
        isReleasedWhenClosed = false
        delegate = self
        build()
        load(config)
        installKeyMonitor()
        if let owner {
            owner.beginSheet(self)
        } else {
            center()
            makeKeyAndOrderFront(nil)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    deinit {
        if let keyMonitor {
            NSEvent.removeMonitor(keyMonitor)
        }
    }

    // MARK: - Build

    private func build() {
        guard let content = contentView else { return }

        let root = NSStackView()
        root.translatesAutoresizingMaskIntoConstraints = false
        root.orientation = .vertical
        root.alignment = .leading
        root.spacing = 14
        root.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 16, right: 20)

        // Runtime 并列选项
        let runtimeLabel = sectionLabel("Runtime")
        runtimeStack.orientation = .horizontal
        runtimeStack.spacing = 10
        for r in TargetRuntime.allCases {
            let card = optionCard(title: r.rawValue, subtitle: r == .tmux ? "attach/create tmux" : "plain shell")
            card.tag = r == .tmux ? 0 : 1
            card.target = self
            card.action = #selector(runtimeSelected(_:))
            runtimeStack.addArrangedSubview(card)
        }

        // Transport 并列选项
        let transportLabel = sectionLabel("Transport")
        transportStack.orientation = .horizontal
        transportStack.spacing = 10
        let localCard = optionCard(title: "local", subtitle: "本机")
        localCard.tag = 0
        localCard.target = self
        localCard.action = #selector(transportSelected(_:))
        transportStack.addArrangedSubview(localCard)
        let sshCard = optionCard(title: "ssh", subtitle: "SSH 远程")
        sshCard.tag = 1
        sshCard.target = self
        sshCard.action = #selector(transportSelected(_:))
        transportStack.addArrangedSubview(sshCard)

        // SSH name（可编辑下拉）
        let sshLabel = sectionLabel("SSH name")
        sshNameCombo.placeholderString = "选择或输入 SSH Host"
        sshNameCombo.completes = true
        sshNameCombo.usesDataSource = false
        sshNameCombo.addItems(withObjectValues: sshHosts.map { $0.alias })
        sshNameCombo.delegate = self
        sshNameCombo.isHidden = true
        sshNameCombo.translatesAutoresizingMaskIntoConstraints = false

        // Path（可编辑下拉 + 逐步浏览）
        let pathLabel = sectionLabel("Path")
        pathCombo.placeholderString = "~/... 或输入完整路径"
        pathCombo.completes = true
        pathCombo.usesDataSource = false
        pathCombo.delegate = self
        pathCombo.target = self
        pathCombo.action = #selector(pathComboSelected)
        pathCombo.translatesAutoresizingMaskIntoConstraints = false

        upButton.target = self
        upButton.action = #selector(goUp)
        upButton.bezelStyle = .rounded

        let pathRow = NSStackView(views: [pathCombo, upButton])
        pathRow.orientation = .horizontal
        pathRow.spacing = 8
        pathRow.translatesAutoresizingMaskIntoConstraints = false

        // Name（可编辑下拉）
        let nameLabel = sectionLabel("Name")
        nameCombo.placeholderString = "目标名"
        nameCombo.completes = true
        nameCombo.usesDataSource = false
        nameCombo.addItems(withObjectValues: store.projects.map { $0.name })
        nameCombo.delegate = self
        nameCombo.translatesAutoresizingMaskIntoConstraints = false
        nameHint.font = NSFont.systemFont(ofSize: 10)
        nameHint.textColor = .secondaryLabelColor
        nameHint.translatesAutoresizingMaskIntoConstraints = false

        root.addArrangedSubview(runtimeLabel)
        root.addArrangedSubview(runtimeStack)
        root.addArrangedSubview(transportLabel)
        root.addArrangedSubview(transportStack)
        root.addArrangedSubview(sshLabel)
        root.addArrangedSubview(sshNameCombo)
        root.addArrangedSubview(pathLabel)
        root.addArrangedSubview(pathRow)
        root.addArrangedSubview(nameLabel)
        root.addArrangedSubview(nameCombo)
        root.addArrangedSubview(nameHint)

        // 底部按钮
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelTapped))
        let save = NSButton(title: "Save", target: self, action: #selector(saveTapped))
        save.keyEquivalent = "\r"
        let buttonRow = NSStackView(views: [cancel, save])
        buttonRow.orientation = .horizontal
        buttonRow.spacing = 10

        root.addArrangedSubview(buttonRow)
        content.addSubview(root)
        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),

            sshNameCombo.widthAnchor.constraint(equalToConstant: 300),
            pathCombo.widthAnchor.constraint(equalToConstant: 300),
            nameCombo.widthAnchor.constraint(equalToConstant: 300),
        ])
    }

    private func sectionLabel(_ text: String) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = NSFont.systemFont(ofSize: 12, weight: .semibold)
        label.textColor = .secondaryLabelColor
        return label
    }

    private func optionCard(title: String, subtitle: String) -> NSButton {
        let button = NSButton(title: title, target: nil, action: nil)
        button.setButtonType(.toggle)
        button.isBordered = false
        button.controlSize = .large
        button.wantsLayer = true
        button.layer?.cornerRadius = 6
        button.layer?.borderWidth = 1
        button.layer?.masksToBounds = true
        button.toolTip = subtitle
        button.translatesAutoresizingMaskIntoConstraints = false
        button.widthAnchor.constraint(equalToConstant: 130).isActive = true
        button.heightAnchor.constraint(equalToConstant: 44).isActive = true
        return button
    }

    // MARK: - Load / state

    private func load(_ config: TargetConfig?) {
        if let config {
            selection = TargetOptionSelection(
                runtime: config.runtime,
                transport: config.transport
            )
            currentPath = config.path.isEmpty ? "~" : config.path
            nameCombo.stringValue = config.name
            nameManuallyEdited = true
        } else {
            selection = TargetOptionSelection()
            currentPath = "~"
            nameCombo.stringValue = ""
            nameManuallyEdited = false
        }
        updateRuntimeCards()
        updateTransportCards()
        updateSSHVisibility()
        pathCombo.stringValue = currentPath
        refreshPathSuggestions()
        updateNameHint()
    }

    private func updateRuntimeCards() {
        for view in runtimeStack.arrangedSubviews {
            guard let button = view as? NSButton else { continue }
            let isTMUX = button.tag == 0
            let selected = selection.isSelected(runtime: isTMUX ? .tmux : .shell)
            button.state = selected ? .on : .off
            applyOptionCardStyle(
                button,
                selected: selected,
                kind: "runtime",
                option: isTMUX ? "tmux" : "shell",
                subtitle: isTMUX ? "attach/create tmux" : "plain shell"
            )
        }
    }

    private func updateTransportCards() {
        for view in transportStack.arrangedSubviews {
            guard let button = view as? NSButton else { continue }
            let isSSH = button.tag == 1
            let candidate: TargetTransport = isSSH ? .ssh(name: "") : .local
            let selected = selection.isSelected(transport: candidate)
            button.state = selected ? .on : .off
            applyOptionCardStyle(
                button,
                selected: selected,
                kind: "transport",
                option: isSSH ? "ssh" : "local",
                subtitle: isSSH ? "SSH 远程" : "本机"
            )
        }
    }

    /// 卡片显式高亮：背景 / 边框 / 文字 / 勾选，深浅色均可读。
    private func applyOptionCardStyle(
        _ button: NSButton,
        selected: Bool,
        kind: String,
        option: String,
        subtitle: String
    ) {
        let accent = NSColor.controlAccentColor
        let background = selected
            ? accent.withAlphaComponent(0.18)
            : NSColor.controlBackgroundColor
        button.layer?.backgroundColor = background.cgColor
        button.layer?.borderColor = (selected ? accent : NSColor.separatorColor).cgColor
        button.layer?.borderWidth = selected ? 2 : 1

        let title = (selected ? "✓ " : "") + option
        let titleColor = selected ? NSColor.labelColor : NSColor.secondaryLabelColor
        let attributed = NSMutableAttributedString()
        attributed.append(NSAttributedString(
            string: title,
            attributes: [
                .font: NSFont.systemFont(ofSize: 13, weight: .semibold),
                .foregroundColor: titleColor,
            ]
        ))
        attributed.append(NSAttributedString(
            string: "\n" + subtitle,
            attributes: [
                .font: NSFont.systemFont(ofSize: 10),
                .foregroundColor: NSColor.secondaryLabelColor,
            ]
        ))
        button.attributedTitle = attributed
        button.alignment = .center
        button.setAccessibilityRole(.radioButton)
        button.setAccessibilityIdentifier(
            TargetOptionAccessibility.identifier(
                kind: kind,
                option: option,
                selected: selected
            )
        )
        button.setAccessibilityValue(selected ? "selected" : "unselected")
    }

    private func updateSSHVisibility() {
        let isSSH: Bool
        if case .ssh = selection.transport { isSSH = true } else { isSSH = false }
        sshNameCombo.isHidden = !isSSH
        if isSSH {
            // SSH 浏览从远端 home 开始
            if currentPath == "~" || currentPath.isEmpty {
                pathCombo.stringValue = "~"
                currentPath = "~"
            }
            refreshPathSuggestions()
        }
    }

    private func updateNameHint() {
        nameHint.stringValue = "默认: \(QuickConnect.defaultName(for: currentPath))"
    }

    private func autoUpdateNameIfNeeded() {
        guard !nameManuallyEdited else { return }
        nameCombo.stringValue = QuickConnect.defaultName(for: currentPath)
        updateNameHint()
    }

    private var currentSSHAlias: String? {
        if case .ssh(let name) = selection.transport {
            let trimmed = name.trimmingCharacters(in: .whitespaces)
            return trimmed.isEmpty ? nil : trimmed
        }
        return nil
    }

    // MARK: - Directory browsing

    private func refreshPathSuggestions() {
        guard !currentPath.isEmpty else { return }
        let isSSH = currentSSHAlias != nil
        let alias = currentSSHAlias
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            do {
                let entries: [CoreFsEntry]
                if isSSH, let alias {
                    entries = try CoreBridge.listDir(
                        backendType: "ssh",
                        target: alias,
                        path: self.currentPath
                    )
                } else {
                    entries = try CoreBridge.listDir(
                        backendType: "local",
                        path: self.currentPath
                    )
                }
                let dirs = entries.filter { $0.is_dir }.map { $0.name }.sorted()
                DispatchQueue.main.async {
                    guard self.currentSSHAlias == (isSSH ? alias : nil) else { return }
                    self.pathCombo.removeAllItems()
                    self.pathCombo.addItems(withObjectValues: dirs)
                }
            } catch {
                DispatchQueue.main.async {
                    self.pathCombo.removeAllItems()
                }
            }
        }
    }

    @objc private func pathComboSelected() {
        let selected = pathCombo.stringValue.trimmingCharacters(in: .whitespaces)
        guard !selected.isEmpty else { return }
        let joined: String
        if currentPath == "~" || currentPath == "/" {
            joined = currentPath == "/" ? "/" + selected : "~/\(selected)"
        } else {
            joined = "\(currentPath)/\(selected)"
        }
        currentPath = joined
        pathCombo.stringValue = joined
        autoUpdateNameIfNeeded()
        refreshPathSuggestions()
    }

    @objc private func goUp() {
        let trimmed = currentPath.trimmingCharacters(in: .whitespaces)
        if trimmed == "~" || trimmed == "/" || trimmed.isEmpty {
            return
        }
        let parent: String
        if trimmed == "~" {
            parent = "~"
        } else if trimmed.hasPrefix("~/") {
            let rest = String(trimmed.dropFirst(2))
            let parts = rest.split(separator: "/", omittingEmptySubsequences: true)
            parent = parts.count <= 1 ? "~" : "~/" + parts.dropLast().joined(separator: "/")
        } else {
            let url = URL(fileURLWithPath: trimmed).deletingLastPathComponent()
            parent = url.path
        }
        currentPath = parent
        pathCombo.stringValue = parent
        autoUpdateNameIfNeeded()
        refreshPathSuggestions()
    }

    // MARK: - Actions

    @objc private func runtimeSelected(_ sender: NSButton) {
        selection.selectRuntime(sender.tag == 0 ? .tmux : .shell)
        updateRuntimeCards()
    }

    @objc private func transportSelected(_ sender: NSButton) {
        if sender.tag == 0 {
            selection.selectTransport(.local)
        } else {
            selection.selectTransport(.ssh(name: sshNameCombo.stringValue))
        }
        updateTransportCards()
        updateSSHVisibility()
        // transport 变化后默认 name 保持 path 派生
        autoUpdateNameIfNeeded()
    }

    @objc private func cancelTapped() {
        close()
    }

    func windowWillClose(_ notification: Notification) {
        if !isSaving {
            onCancel?()
        }
    }

    private func installKeyMonitor() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.isKeyWindow else { return event }
            if event.keyCode == 53 { // Escape
                self.close()
                return nil
            }
            return event
        }
    }

    // MARK: - NSComboBoxDelegate

    func controlTextDidChange(_ obj: Notification) {
        if let combo = obj.object as? NSComboBox {
            if combo === pathCombo {
                currentPath = combo.stringValue.trimmingCharacters(in: .whitespaces)
                autoUpdateNameIfNeeded()
            } else if combo === nameCombo {
                nameManuallyEdited = true
            } else if combo === sshNameCombo {
                selection.selectTransport(.ssh(name: combo.stringValue))
            }
        }
    }

    func comboBoxSelectionDidChange(_ notification: Notification) {
        guard let combo = notification.object as? NSComboBox else { return }
        if combo === pathCombo {
            pathComboSelected()
        } else if combo === sshNameCombo {
            selection.selectTransport(.ssh(name: combo.stringValue))
            updateSSHVisibility()
        }
    }

    // MARK: - Save

    @objc private func saveTapped() {
        let transportValue: TargetTransport
        if case .ssh = selection.transport {
            let name = sshNameCombo.stringValue.trimmingCharacters(in: .whitespaces)
            transportValue = .ssh(name: name)
        } else {
            transportValue = .local
        }
        var path = currentPath.trimmingCharacters(in: .whitespacesAndNewlines)
        if path.isEmpty {
            path = "~"
        }
        var name = nameCombo.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty {
            name = QuickConnect.defaultName(for: path)
        }
        let config = TargetConfig(
            name: name,
            runtime: selection.runtime,
            transport: transportValue,
            path: path
        )
        isSaving = true
        onSave?(config)
        close()
    }
}
