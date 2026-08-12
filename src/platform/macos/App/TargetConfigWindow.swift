import AppKit
import MuxtermChrome

/// 目标配置窗口：runtime / transport / path / name。
///
/// - runtime: shell / tmux（NSPopUpButton）
/// - transport: local / ssh(name)（NSPopUpButton，ssh 时显示 name 输入框）
/// - path: 起始目录（可 Browse）
/// - name: 默认取 path 最小目录，可修改
final class TargetConfigWindow: NSWindow {
    var onSave: ((TargetConfig) -> Void)?

    private let runtimePopup = NSPopUpButton()
    private let transportPopup = NSPopUpButton()
    private let sshNameField = NSTextField()
    private let pathField = NSTextField()
    private let nameField = NSTextField()
    private let nameHint = NSTextField(labelWithString: "")
    private var editing: TargetConfig?

    init(editing config: TargetConfig? = nil, owner: NSWindow?) {
        self.editing = config
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 460, height: 300),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        title = config == nil ? "New Project" : "Edit Project"
        isReleasedWhenClosed = false
        build()
        load(config)
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

    private func build() {
        guard let content = contentView else { return }
        let root = NSGridView(views: [])
        root.translatesAutoresizingMaskIntoConstraints = false

        let runtimeLabel = NSTextField(labelWithString: "Runtime")
        runtimePopup.addItems(withTitles: TargetRuntime.allCases.map { $0.rawValue })
        let runtimeRow = [runtimeLabel, runtimePopup]

        let transportLabel = NSTextField(labelWithString: "Transport")
        transportPopup.addItems(withTitles: ["local", "ssh"])
        transportPopup.target = self
        transportPopup.action = #selector(transportChanged)
        let transportRow = [transportLabel, transportPopup]

        let sshLabel = NSTextField(labelWithString: "SSH name")
        let sshRow = [sshLabel, sshNameField]

        let pathLabel = NSTextField(labelWithString: "Path")
        let browse = NSButton(title: "Browse…", target: self, action: #selector(browsePath))
        let pathRow = [pathLabel, pathField, browse]

        let nameLabel = NSTextField(labelWithString: "Name")
        nameHint.font = NSFont.systemFont(ofSize: 10)
        nameHint.textColor = .secondaryLabelColor
        let nameRow = [nameLabel, nameField]

        root.addRow(with: runtimeRow)
        root.addRow(with: transportRow)
        root.addRow(with: sshRow)
        root.addRow(with: pathRow)
        root.addRow(with: nameRow)
        root.rowSpacing = 12
        root.columnSpacing = 10

        content.addSubview(root)
        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 20),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            root.topAnchor.constraint(equalTo: content.topAnchor, constant: 20),
        ])

        // 底部按钮
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelTapped))
        let save = NSButton(title: "Save", target: self, action: #selector(saveTapped))
        save.keyEquivalent = "\r"
        let buttonRow = NSStackView(views: [cancel, save])
        buttonRow.translatesAutoresizingMaskIntoConstraints = false
        buttonRow.orientation = .horizontal
        buttonRow.spacing = 10
        content.addSubview(buttonRow)
        NSLayoutConstraint.activate([
            buttonRow.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            buttonRow.topAnchor.constraint(equalTo: root.bottomAnchor, constant: 24),
        ])
    }

    private func load(_ config: TargetConfig?) {
        guard let config else {
            // 默认 runtime=tmux, transport=local
            runtimePopup.selectItem(withTitle: TargetRuntime.tmux.rawValue)
            transportPopup.selectItem(withTitle: "local")
            sshNameField.isHidden = true
            updateNameHint()
            return
        }
        runtimePopup.selectItem(withTitle: config.runtime.rawValue)
        switch config.transport {
        case .local:
            transportPopup.selectItem(withTitle: "local")
            sshNameField.isHidden = true
        case .ssh(let name):
            transportPopup.selectItem(withTitle: "ssh")
            sshNameField.stringValue = name
            sshNameField.isHidden = false
        }
        pathField.stringValue = config.path
        nameField.stringValue = config.name
        updateNameHint()
    }

    @objc private func transportChanged() {
        let isSSH = transportPopup.titleOfSelectedItem == "ssh"
        sshNameField.isHidden = !isSSH
    }

    @objc private func browsePath() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.begin { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            self?.pathField.stringValue = url.path
            // 默认 name = path 最小目录（若用户还没手改过）
            if self?.nameField.stringValue.isEmpty == true
                || self?.nameField.stringValue == QuickConnect.defaultName(for: self?.pathField.stringValue ?? "") {
                self?.nameField.stringValue = QuickConnect.defaultName(for: url.path)
            }
            self?.updateNameHint()
        }
    }

    private func updateNameHint() {
        nameHint.stringValue = "默认: \(QuickConnect.defaultName(for: pathField.stringValue))"
    }

    @objc private func cancelTapped() {
        close()
    }

    @objc private func saveTapped() {
        let runtime = TargetRuntime(rawValue: runtimePopup.titleOfSelectedItem ?? "tmux") ?? .tmux
        let transport: TargetTransport
        if transportPopup.titleOfSelectedItem == "ssh" {
            transport = .ssh(name: sshNameField.stringValue.trimmingCharacters(in: .whitespaces))
        } else {
            transport = .local
        }
        let path = pathField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        var name = nameField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty {
            name = QuickConnect.defaultName(for: path)
        }
        let config = TargetConfig(name: name, runtime: runtime, transport: transport, path: path)
        onSave?(config)
        close()
    }
}
