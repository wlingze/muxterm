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
    /// Core Catalog 登记顺序；UI 不自行维护 runtime allowlist。
    private let availableRuntimes: [TargetRuntime]

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
    /// 目录输入 / 候选选择 / 异步请求 generation 的纯状态模型。
    private var pathController = DirectorySuggestionController(path: "~")
    /// 目录列表请求防抖（文本变化不立即发请求）。
    private var pathDebounce: DispatchWorkItem?

    init(
        editing config: TargetConfig? = nil,
        owner: NSWindow?,
        store: QuickConnectStore,
        sshHosts: [SSHHostInfo],
        availableRuntimes: [TargetRuntime] = TargetRuntime.allCases
    ) {
        self.editing = config
        self.store = store
        self.sshHosts = sshHosts
        var runtimes = availableRuntimes
        if let runtime = config?.runtime, !runtimes.contains(runtime) {
            runtimes.append(runtime)
        }
        self.availableRuntimes = runtimes.isEmpty ? TargetRuntime.allCases : runtimes
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
        for (index, runtime) in availableRuntimes.enumerated() {
            let card = optionCard(title: runtime.rawValue, subtitle: runtimeSubtitle(runtime))
            card.tag = index
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
        // 禁用 NSComboBox 默认文本补全：它会把完整路径当 basename 拼接。
        // 候选完全由 DirectorySuggestionController 管理，选择候选=进入目录。
        pathCombo.completes = false
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
            pathController = DirectorySuggestionController(
                path: config.path.isEmpty ? "~" : config.path
            )
            nameCombo.stringValue = config.name
            nameManuallyEdited = true
        } else {
            selection = TargetOptionSelection()
            pathController = DirectorySuggestionController(path: "~")
            nameCombo.stringValue = ""
            nameManuallyEdited = false
        }
        if case .ssh(let alias) = selection.transport {
            _ = pathController.setTransport(isSSH: true, alias: alias)
        }
        updateRuntimeCards()
        updateTransportCards()
        updateSSHVisibility()
        pathCombo.stringValue = pathController.text
        refreshPathSuggestions()
        updateNameHint()
    }

    private func updateRuntimeCards() {
        for view in runtimeStack.arrangedSubviews {
            guard let button = view as? NSButton,
                  availableRuntimes.indices.contains(button.tag)
            else { continue }
            let runtime = availableRuntimes[button.tag]
            let selected = selection.isSelected(runtime: runtime)
            button.state = selected ? .on : .off
            applyOptionCardStyle(
                button,
                selected: selected,
                kind: "runtime",
                option: runtime.rawValue,
                subtitle: runtimeSubtitle(runtime)
            )
        }
    }

    private func runtimeSubtitle(_ runtime: TargetRuntime) -> String {
        switch runtime {
        case .tmux:
            return "attach/create tmux"
        case .herdr:
            return "attach/create Herdr workspace"
        case .shell:
            return "plain shell"
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
            if pathController.text == "~" || pathController.text.isEmpty {
                _ = pathController.updateInput("~")
                pathCombo.stringValue = "~"
            }
            let alias = sshNameCombo.stringValue
                .trimmingCharacters(in: .whitespaces)
            _ = pathController.setTransport(isSSH: true, alias: alias.isEmpty ? nil : alias)
            refreshPathSuggestions()
        } else {
            _ = pathController.setTransport(isSSH: false)
            refreshPathSuggestions()
        }
    }

    private func updateNameHint() {
        nameHint.stringValue = "默认: \(QuickConnect.defaultName(for: pathController.text))"
    }

    private func autoUpdateNameIfNeeded() {
        guard !nameManuallyEdited else { return }
        nameCombo.stringValue = QuickConnect.defaultName(for: pathController.text)
        updateNameHint()
    }

    // MARK: - Directory browsing

    private func refreshPathSuggestions(debounce: Bool = false) {
        let request = pathController.request
        let isSSH = request.isSSH
        let alias = request.alias
        let listPath = request.path
        // SSH 已选中但 alias 为空：不能退回本地列表，也不发无效请求。
        if isSSH && alias == nil {
            pathCombo.removeAllItems()
            return
        }
        if debounce {
            pathDebounce?.cancel()
            let work = DispatchWorkItem { [weak self] in
                self?.startPathListing(request: request, isSSH: isSSH, alias: alias, path: listPath)
            }
            pathDebounce = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.12, execute: work)
            return
        }
        startPathListing(request: request, isSSH: isSSH, alias: alias, path: listPath)
    }

    private func startPathListing(
        request: DirectoryListingRequest,
        isSSH: Bool,
        alias: String?,
        path: String
    ) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            do {
                let entries: [CoreFsEntry]
                if isSSH, let alias {
                    entries = try CoreBridge.listDir(
                        backendType: "ssh",
                        target: alias,
                        path: path
                    )
                } else {
                    entries = try CoreBridge.listDir(
                        backendType: "local",
                        path: path
                    )
                }
                let dirs = entries.filter { $0.is_dir }.map { $0.name }.sorted()
                DispatchQueue.main.async {
                    let response = DirectoryListingResponse(
                        request: request,
                        directories: dirs
                    )
                    guard self.pathController.apply(response) else { return }
                    self.pathCombo.removeAllItems()
                    self.pathCombo.addItems(withObjectValues: self.pathController.candidates)
                }
            } catch {
                DispatchQueue.main.async {
                    // 只有当前请求仍是最新的才清空，旧响应不得覆盖新输入。
                    let response = DirectoryListingResponse(request: request, directories: [])
                    if self.pathController.apply(response) {
                        self.pathCombo.removeAllItems()
                    }
                }
            }
        }
    }

    @objc private func pathComboSelected() {
        let selected = pathCombo.stringValue.trimmingCharacters(in: .whitespaces)
        guard !selected.isEmpty else { return }
        applyPathSelection(candidate: selected)
    }

    /// 选择候选 = 进入该目录。action 与 selectionDidChange 可能都触发，
    /// 已进入同一目录时保持幂等，避免重复拼接。
    private func applyPathSelection(candidate: String) {
        // 用户输入完整路径（含 /）按回车：直接采用该路径并重新列表，
        // 绝不把它当 basename 拼到当前目录。
        guard !candidate.contains("/") else {
            _ = pathController.updateInput(candidate)
            pathCombo.stringValue = pathController.text
            autoUpdateNameIfNeeded()
            refreshPathSuggestions()
            return
        }
        // action 与 selectionDidChange 会成对触发：当前文本最后一段已等于
        // 候选（且已进入该目录）时保持幂等，不重复拼接。
        if lastComponent(of: pathController.text) == candidate {
            refreshPathSuggestions()
            return
        }
        _ = pathController.select(candidate: candidate)
        pathCombo.stringValue = pathController.text
        autoUpdateNameIfNeeded()
        refreshPathSuggestions()
    }

    private func lastComponent(of raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        if trimmed == "~" || trimmed == "/" || trimmed.isEmpty { return "" }
        var withoutTrailing = trimmed
        while withoutTrailing.hasSuffix("/") {
            withoutTrailing.removeLast()
        }
        if withoutTrailing == "~" { return "" }
        return withoutTrailing.split(separator: "/").last.map(String.init) ?? ""
    }

    @objc private func goUp() {
        let before = pathController.text
        _ = pathController.goUp()
        guard pathController.text != before else { return }
        pathCombo.stringValue = pathController.text
        autoUpdateNameIfNeeded()
        refreshPathSuggestions()
    }

    // MARK: - Actions

    @objc private func runtimeSelected(_ sender: NSButton) {
        guard availableRuntimes.indices.contains(sender.tag) else { return }
        selection.selectRuntime(availableRuntimes[sender.tag])
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
                _ = pathController.updateInput(combo.stringValue)
                autoUpdateNameIfNeeded()
                refreshPathSuggestions(debounce: true)
            } else if combo === nameCombo {
                nameManuallyEdited = true
            } else if combo === sshNameCombo {
                if case .ssh = selection.transport {
                    let alias = combo.stringValue.trimmingCharacters(in: .whitespaces)
                    selection.selectTransport(.ssh(name: alias))
                    _ = pathController.setTransport(isSSH: true, alias: alias.isEmpty ? nil : alias)
                    refreshPathSuggestions()
                }
            }
        }
    }

    func comboBoxSelectionDidChange(_ notification: Notification) {
        guard let combo = notification.object as? NSComboBox else { return }
        if combo === pathCombo {
            let index = combo.indexOfSelectedItem
            if index >= 0, index < combo.numberOfItems {
                let candidate = combo.itemObjectValue(at: index) as? String ?? ""
                applyPathSelection(candidate: candidate)
            }
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
        let path = DirectoryPathModel.resolvedPath(for: pathController.text)
        var name = nameCombo.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty {
            name = QuickConnect.defaultName(for: path)
        }
        let preservesIdentity = editing?.runtime == selection.runtime
            && editing?.transport == transportValue
        let config = TargetConfig(
            name: name,
            runtime: selection.runtime,
            transport: transportValue,
            path: path,
            session: preservesIdentity ? editing?.session : nil,
            socket: preservesIdentity ? editing?.socket : nil,
            workspaceID: preservesIdentity ? editing?.workspaceID : nil
        )
        isSaving = true
        onSave?(config)
        close()
    }

    /// AppKit 回归测试：Project runtime 卡必须精确跟随 Catalog 列表。
    func testAvailableRuntimes() -> [TargetRuntime] {
        availableRuntimes
    }

    func testSetName(_ name: String) {
        nameCombo.stringValue = name
        nameManuallyEdited = true
    }

    func testSetPath(_ path: String) {
        _ = pathController.updateInput(path)
        pathCombo.stringValue = pathController.text
    }

    func testSave() {
        saveTapped()
    }
}
