import AppKit
import MuxtermChrome

/// SSH 新 session 的远端目录选择器。输入会异步列出当前目录的子目录；
/// 首次回车进入候选目录，第二次回车才创建 session。
final class RemoteDirectoryPickerWindow: NSWindow, NSWindowDelegate,
    NSTextFieldDelegate, NSTableViewDataSource, NSTableViewDelegate
{
    var onChoose: ((String) -> Void)?

    private let alias: String
    private let pathField = NSTextField(string: "~")
    private let upButton = NSButton(title: "↑", target: nil, action: nil)
    private let directoriesTable = NSTableView()
    private let emptyLabel = NSTextField(labelWithString: "")
    private var pathController = DirectorySuggestionController(path: "~")
    private var debounceWork: DispatchWorkItem?
    private var keyMonitor: Any?
    private var isChoosing = false

    init(alias: String, owner: NSWindow?) {
        self.alias = alias
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 500, height: 360),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        title = MuxtermI18n.shared.tr(.chooseRemoteDirectory)
        isReleasedWhenClosed = false
        delegate = self
        _ = pathController.setTransport(isSSH: true, alias: alias)
        build()
        installKeyMonitor()
        refreshDirectories()
        if let owner {
            owner.beginSheet(self)
        } else {
            center()
            makeKeyAndOrderFront(nil)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    deinit {
        debounceWork?.cancel()
        if let keyMonitor {
            NSEvent.removeMonitor(keyMonitor)
        }
    }

    private func build() {
        guard let contentView else { return }
        let root = NSStackView()
        root.translatesAutoresizingMaskIntoConstraints = false
        root.orientation = .vertical
        root.alignment = .leading
        root.spacing = 10
        root.edgeInsets = NSEdgeInsets(top: 18, left: 18, bottom: 14, right: 18)
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

        let message = NSTextField(wrappingLabelWithString: MuxtermI18n.shared.tr(
            .remoteDirectoryMessage,
            arguments: ["host": alias]
        ))
        message.font = NSFont.systemFont(ofSize: 12)
        message.textColor = .secondaryLabelColor
        root.addArrangedSubview(message)

        pathField.placeholderString = "~/..."
        pathField.delegate = self
        pathField.setAccessibilityIdentifier("muxterm.remoteDirectory.path")
        pathField.toolTip = MuxtermI18n.shared.tr(.directorySuggestionHint)
        pathField.translatesAutoresizingMaskIntoConstraints = false

        upButton.bezelStyle = .rounded
        upButton.target = self
        upButton.action = #selector(goUp)
        upButton.setAccessibilityIdentifier("muxterm.remoteDirectory.up")
        let pathRow = NSStackView(views: [pathField, upButton])
        pathRow.orientation = .horizontal
        pathRow.alignment = .centerY
        pathRow.spacing = 8
        root.addArrangedSubview(pathRow)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("directory"))
        column.resizingMask = .autoresizingMask
        directoriesTable.addTableColumn(column)
        directoriesTable.headerView = nil
        directoriesTable.rowHeight = 24
        directoriesTable.intercellSpacing = NSSize(width: 0, height: 1)
        directoriesTable.usesAlternatingRowBackgroundColors = false
        directoriesTable.backgroundColor = .clear
        directoriesTable.delegate = self
        directoriesTable.dataSource = self
        directoriesTable.doubleAction = #selector(openSelectedDirectory)
        directoriesTable.setAccessibilityIdentifier("muxterm.remoteDirectory.suggestions")

        let emptyContainer = NSView()
        emptyLabel.font = NSFont.systemFont(ofSize: 11)
        emptyLabel.textColor = .secondaryLabelColor
        emptyLabel.translatesAutoresizingMaskIntoConstraints = false
        emptyContainer.addSubview(emptyLabel)
        NSLayoutConstraint.activate([
            emptyLabel.leadingAnchor.constraint(equalTo: emptyContainer.leadingAnchor, constant: 8),
            emptyLabel.centerYAnchor.constraint(equalTo: emptyContainer.centerYAnchor),
        ])

        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.documentView = directoriesTable
        scroll.translatesAutoresizingMaskIntoConstraints = false
        root.addArrangedSubview(scroll)
        scroll.heightAnchor.constraint(equalToConstant: 205).isActive = true
        scroll.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        root.addArrangedSubview(emptyContainer)
        emptyContainer.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        emptyContainer.heightAnchor.constraint(equalToConstant: 18).isActive = true

        let cancel = NSButton(title: MuxtermI18n.shared.tr(.cancel), target: self, action: #selector(cancelTapped))
        let choose = NSButton(
            title: MuxtermI18n.shared.tr(.createAndAttach),
            target: self,
            action: #selector(chooseTapped)
        )
        choose.keyEquivalent = "\r"
        choose.setAccessibilityIdentifier("muxterm.remoteDirectory.confirm")
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let buttons = NSStackView(views: [spacer, cancel, choose])
        buttons.orientation = .horizontal
        buttons.alignment = .centerY
        buttons.spacing = 9
        buttons.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        root.addArrangedSubview(buttons)

        contentView.addSubview(root)
        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
            root.topAnchor.constraint(equalTo: contentView.topAnchor),
            root.bottomAnchor.constraint(equalTo: contentView.bottomAnchor),
            pathRow.widthAnchor.constraint(equalTo: root.widthAnchor),
            pathField.widthAnchor.constraint(greaterThanOrEqualToConstant: 350),
        ])
    }

    private func installKeyMonitor() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.isKeyWindow, event.window === self else { return event }
            if event.keyCode == 53 {
                self.close()
                return nil
            }
            guard event.keyCode == 36 || event.keyCode == 76 else { return event }
            let responder = self.firstResponder
            if responder === self.pathField || responder === self.pathField.currentEditor() {
                return self.completePathFromReturn() ? nil : event
            }
            if responder === self.directoriesTable,
               self.directoriesTable.selectedRow >= 0,
               self.pathController.candidates.indices.contains(self.directoriesTable.selectedRow)
            {
                self.applyDirectory(self.pathController.candidates[self.directoriesTable.selectedRow])
                return nil
            }
            return event
        }
    }

    /// Enter once to accept a matching folder. The confirmation button handles
    /// the next Enter after the selected path has a trailing slash.
    private func completePathFromReturn() -> Bool {
        let prefix = DirectoryPathModel.inputPrefix(for: pathController.text)
        guard !prefix.isEmpty else { return false }
        if let candidate = pathController.candidates.first(where: {
            $0.localizedCaseInsensitiveCompare(prefix) == .orderedSame
        }) ?? pathController.candidates.first {
            applyDirectory(candidate)
        } else {
            _ = pathController.updateInput(pathController.text + "/")
            pathField.stringValue = pathController.text
            refreshDirectories()
        }
        return true
    }

    private func refreshDirectories(debounce: Bool = false) {
        let request = pathController.request
        debounceWork?.cancel()
        if debounce {
            let work = DispatchWorkItem { [weak self] in
                self?.startListing(request)
            }
            debounceWork = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.12, execute: work)
        } else {
            startListing(request)
        }
    }

    private func startListing(_ request: DirectoryListingRequest) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard self != nil else { return }
            let result = Result {
                try CoreBridge.listDir(
                    backendType: "ssh",
                    target: request.alias,
                    path: request.path
                ).filter(\.is_dir).map(\.name)
            }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                let directories = (try? result.get()) ?? []
                guard self.pathController.apply(DirectoryListingResponse(
                    request: request,
                    directories: directories
                )) else { return }
                self.directoriesTable.reloadData()
                self.emptyLabel.stringValue = self.pathController.candidates.isEmpty
                    ? MuxtermI18n.shared.tr(.directoryNoSuggestions)
                    : ""
            }
        }
    }

    private func applyDirectory(_ directory: String) {
        _ = pathController.select(candidate: directory)
        pathField.stringValue = pathController.text
        directoriesTable.deselectAll(nil)
        makeFirstResponder(pathField)
        refreshDirectories()
    }

    @objc private func goUp() {
        _ = pathController.goUp()
        pathField.stringValue = pathController.text
        refreshDirectories()
    }

    @objc private func chooseTapped() {
        guard !isChoosing else { return }
        let path = DirectoryPathModel.resolvedPath(for: pathField.stringValue)
        guard !path.isEmpty else { return }
        isChoosing = true
        onChoose?(path)
        close()
    }

    @objc private func cancelTapped() {
        close()
    }

    @objc private func openSelectedDirectory() {
        let row = directoriesTable.clickedRow
        guard pathController.candidates.indices.contains(row) else { return }
        applyDirectory(pathController.candidates[row])
    }

    func controlTextDidChange(_ notification: Notification) {
        guard notification.object as? NSTextField === pathField else { return }
        _ = pathController.updateInput(pathField.stringValue)
        directoriesTable.reloadData()
        refreshDirectories(debounce: true)
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        pathController.candidates.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard pathController.candidates.indices.contains(row) else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("RemoteDirectorySuggestion")
        let cell = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView
            ?? NSTableCellView()
        cell.identifier = identifier
        let label: NSTextField
        if let existing = cell.textField {
            label = existing
        } else {
            label = NSTextField(labelWithString: "")
            label.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(label)
            cell.textField = label
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 8),
                label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -8),
                label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        label.stringValue = pathController.candidates[row] + "/"
        label.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        return cell
    }

    func windowWillClose(_ notification: Notification) {
        debounceWork?.cancel()
        debounceWork = nil
        if let keyMonitor {
            NSEvent.removeMonitor(keyMonitor)
            self.keyMonitor = nil
        }
    }
}
