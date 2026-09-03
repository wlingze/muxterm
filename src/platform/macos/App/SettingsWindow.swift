import AppKit
import Foundation
import MuxtermChrome

/// AppKit settings renderer backed by Core's Schema/Manifest transaction API.
///
/// The window renders every field published by the Core manifest; a new Core
/// field only needs a manifest entry to appear here. Controls are keyed by JSON
/// Pointer and are always written back through `SettingsService` transactions,
/// never by parsing or rewriting `config.toml` in Swift.
func settingsCategoryTitle(id: String, titleKey: String) -> String {
    let keyTail = titleKey.split(separator: ".").last.map(String.init) ?? ""
    let source = keyTail.isEmpty ? id : keyTail
    return source
        .replacingOccurrences(of: "_", with: " ")
        .replacingOccurrences(of: "-", with: " ")
        .split(separator: " ")
        .map { word in
            guard let first = word.first else { return "" }
            return first.uppercased() + word.dropFirst()
        }
        .joined(separator: " ")
}

final class SettingsWindowController: NSWindowController, NSWindowDelegate,
    NSTextFieldDelegate, NSSearchFieldDelegate, NSTableViewDataSource, NSTableViewDelegate
{
    private struct Category {
        let id: String
        let title: String
        let searchText: String
        let fields: [[String: Any]]
    }

    private let bridge: CoreBridge
    private let quickConnectStore: QuickConnectStore
    private var controls: [String: NSView] = [:]
    private var baselines: [String: Any] = [:]
    private var pendingFontPath: String?
    private var dirty = false
    private var summaryLabel = NSTextField(labelWithString: "")
    private let searchField = NSSearchField()
    private let categoryTable = NSTableView()
    private let categoryScroll = NSScrollView()
    private let sidebarView = NSView()
    private let pagesContainer = NSView()
    private var categories: [Category] = []
    private var visibleCategoryIDs: [String] = []
    private var selectedCategoryID: String?
    private var pages: [String: NSScrollView] = [:]
    private var projectEditorView: SettingsProjectEditorView?
    private var activeProjectEditor: TargetConfigWindow?

    init(bridge: CoreBridge, quickConnectStore: QuickConnectStore? = nil) {
        self.bridge = bridge
        self.quickConnectStore = quickConnectStore ?? Self.makeCoreBackedStore(bridge: bridge)
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 640),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Settings"
        window.minSize = NSSize(width: 680, height: 420)
        super.init(window: window)
        window.delegate = self
        window.setAccessibilityIdentifier("muxterm.settingsWindow")
        loadSnapshotAndBuild()
    }

    private static func makeCoreBackedStore(bridge: CoreBridge) -> QuickConnectStore {
        let projects = projects(from: bridge)
        return QuickConnectStore(projects: projects) { [weak bridge] updated in
            guard let bridge else { return }
            do {
                let transaction = try bridge.configBegin()
                try bridge.configPatch(
                    transaction: transaction,
                    operations: [[
                        "op": "replace",
                        "path": "/projects",
                        "value": QuickConnectStore.projectJSON(from: updated),
                    ]]
                )
                try bridge.configCommit(transaction: transaction)
            } catch {
                NSLog(
                    "muxterm: failed to persist projects from settings: %@",
                    error.localizedDescription
                )
            }
        }
    }

    private static func projects(from bridge: CoreBridge) -> [TargetConfig] {
        guard let text = bridge.configDescribeJSON(),
              let data = text.data(using: .utf8),
              let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let payload = envelope["data"] as? [String: Any],
              let values = payload["values"] as? [String: Any],
              let projects = values["projects"] as? [[String: Any]]
        else { return [] }
        return QuickConnectStore.targetConfigs(from: projects)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func showWindow(_ sender: Any?) {
        loadSnapshotAndBuild()
        super.showWindow(sender)
        window?.makeKeyAndOrderFront(sender)
    }

    // MARK: - Manifest rendering

    private func loadSnapshotAndBuild() {
        guard let text = bridge.configDescribeJSON(),
              let data = text.data(using: .utf8),
              let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let payload = envelope["data"] as? [String: Any],
              let values = payload["values"] as? [String: Any],
              let manifest = payload["manifest"] as? [String: Any]
        else { return }
        buildView(in: window!, values: values, manifest: manifest)
        loadValues(values)
    }

    private func buildView(in window: NSWindow, values: [String: Any], manifest: [String: Any]) {
        NSLayoutConstraint.deactivate(sidebarView.constraints)
        NSLayoutConstraint.deactivate(pagesContainer.constraints)
        sidebarView.subviews.forEach { $0.removeFromSuperview() }
        pagesContainer.subviews.forEach { $0.removeFromSuperview() }
        categoryTable.tableColumns.forEach { categoryTable.removeTableColumn($0) }
        controls = [:]
        baselines = [:]
        dirty = false
        categories = []
        visibleCategoryIDs = []
        selectedCategoryID = nil
        pages = [:]
        projectEditorView = nil

        guard let groups = manifest["groups"] as? [[String: Any]] else {
            let label = NSTextField(labelWithString: "No settings manifest")
            label.alignment = .center
            window.contentView = label
            return
        }

        for group in groups {
            guard let id = group["id"] as? String,
                  !id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            else { continue }
            let titleKey = group["title_key"] as? String ?? ""
            let fields = group["fields"] as? [[String: Any]] ?? []
            let fieldSearch = fields.flatMap { field in
                [
                    field["path"] as? String,
                    field["title_key"] as? String,
                    field["description_key"] as? String,
                ].compactMap { $0 }
            }.joined(separator: " ")
            let title = settingsCategoryTitle(id: id, titleKey: titleKey)
            categories.append(Category(
                id: id,
                title: title,
                searchText: "\(id) \(titleKey) \(title) \(fieldSearch)".lowercased(),
                fields: fields
            ))
        }
        visibleCategoryIDs = categories.map(\.id)

        summaryLabel.textColor = .secondaryLabelColor
        summaryLabel.lineBreakMode = .byWordWrapping
        summaryLabel.maximumNumberOfLines = 2
        summaryLabel.translatesAutoresizingMaskIntoConstraints = false

        let root = NSView()
        root.setAccessibilityIdentifier("muxterm.settings.root")
        sidebarView.translatesAutoresizingMaskIntoConstraints = false
        sidebarView.setAccessibilityIdentifier("muxterm.settings.categories")
        pagesContainer.translatesAutoresizingMaskIntoConstraints = false
        pagesContainer.setAccessibilityIdentifier("muxterm.settings.pages")

        searchField.translatesAutoresizingMaskIntoConstraints = false
        searchField.placeholderString = "Search Settings"
        searchField.delegate = self
        searchField.stringValue = ""
        searchField.setAccessibilityIdentifier("muxterm.settings.search")

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("category"))
        categoryTable.addTableColumn(column)
        categoryTable.headerView = nil
        categoryTable.rowHeight = 30
        categoryTable.style = .sourceList
        categoryTable.dataSource = self
        categoryTable.delegate = self
        categoryTable.setAccessibilityIdentifier("muxterm.settings.categoryList")
        categoryScroll.translatesAutoresizingMaskIntoConstraints = false
        categoryScroll.drawsBackground = false
        categoryScroll.hasVerticalScroller = true
        categoryScroll.documentView = categoryTable

        sidebarView.addSubview(searchField)
        sidebarView.addSubview(categoryScroll)
        root.addSubview(sidebarView)

        let right = NSView()
        right.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(right)
        right.addSubview(pagesContainer)

        for category in categories {
            let page = makePage(for: category, values: values)
            page.translatesAutoresizingMaskIntoConstraints = false
            page.isHidden = true
            pages[category.id] = page
            pagesContainer.addSubview(page)
            NSLayoutConstraint.activate([
                page.leadingAnchor.constraint(equalTo: pagesContainer.leadingAnchor),
                page.trailingAnchor.constraint(equalTo: pagesContainer.trailingAnchor),
                page.topAnchor.constraint(equalTo: pagesContainer.topAnchor),
                page.bottomAnchor.constraint(equalTo: pagesContainer.bottomAnchor),
            ])
        }

        let separator = NSBox()
        separator.boxType = .separator
        separator.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(separator)

        let footer = NSView()
        footer.translatesAutoresizingMaskIntoConstraints = false
        right.addSubview(footer)
        footer.addSubview(summaryLabel)
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelSettings))
        cancel.translatesAutoresizingMaskIntoConstraints = false
        cancel.setAccessibilityIdentifier("muxterm.settings.cancel")
        let apply = NSButton(title: "Apply", target: self, action: #selector(applySettings))
        apply.translatesAutoresizingMaskIntoConstraints = false
        apply.keyEquivalent = "\r"
        apply.setAccessibilityIdentifier("muxterm.settings.apply")
        footer.addSubview(cancel)
        footer.addSubview(apply)

        NSLayoutConstraint.activate([
            sidebarView.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            sidebarView.topAnchor.constraint(equalTo: root.topAnchor),
            sidebarView.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            sidebarView.widthAnchor.constraint(equalToConstant: 180),
            searchField.leadingAnchor.constraint(equalTo: sidebarView.leadingAnchor, constant: 10),
            searchField.trailingAnchor.constraint(equalTo: sidebarView.trailingAnchor, constant: -10),
            searchField.topAnchor.constraint(equalTo: sidebarView.topAnchor, constant: 12),
            categoryScroll.leadingAnchor.constraint(equalTo: sidebarView.leadingAnchor),
            categoryScroll.trailingAnchor.constraint(equalTo: sidebarView.trailingAnchor),
            categoryScroll.topAnchor.constraint(equalTo: searchField.bottomAnchor, constant: 8),
            categoryScroll.bottomAnchor.constraint(equalTo: sidebarView.bottomAnchor),
            separator.leadingAnchor.constraint(equalTo: sidebarView.trailingAnchor),
            separator.topAnchor.constraint(equalTo: root.topAnchor),
            separator.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            separator.widthAnchor.constraint(equalToConstant: 1),
            right.leadingAnchor.constraint(equalTo: separator.trailingAnchor),
            right.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            right.topAnchor.constraint(equalTo: root.topAnchor),
            right.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            pagesContainer.leadingAnchor.constraint(equalTo: right.leadingAnchor),
            pagesContainer.trailingAnchor.constraint(equalTo: right.trailingAnchor),
            pagesContainer.topAnchor.constraint(equalTo: right.topAnchor),
            pagesContainer.bottomAnchor.constraint(equalTo: footer.topAnchor),
            footer.leadingAnchor.constraint(equalTo: right.leadingAnchor),
            footer.trailingAnchor.constraint(equalTo: right.trailingAnchor),
            footer.bottomAnchor.constraint(equalTo: right.bottomAnchor),
            footer.heightAnchor.constraint(equalToConstant: 56),
            summaryLabel.leadingAnchor.constraint(equalTo: footer.leadingAnchor, constant: 20),
            summaryLabel.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
            summaryLabel.trailingAnchor.constraint(lessThanOrEqualTo: cancel.leadingAnchor, constant: -12),
            apply.trailingAnchor.constraint(equalTo: footer.trailingAnchor, constant: -20),
            apply.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
            cancel.trailingAnchor.constraint(equalTo: apply.leadingAnchor, constant: -8),
            cancel.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
        ])

        window.contentView = root
        categoryTable.reloadData()
        selectCategory(visibleCategoryIDs.first)
    }

    private func makePage(for category: Category, values: [String: Any]) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.setAccessibilityIdentifier("muxterm.settings.page.\(category.id)")

        let stack = NSStackView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.orientation = .vertical
        stack.alignment = .width
        stack.spacing = 14
        stack.edgeInsets = NSEdgeInsets(top: 20, left: 24, bottom: 24, right: 24)

        let title = NSTextField(labelWithString: category.title)
        title.font = .boldSystemFont(ofSize: 18)
        stack.addArrangedSubview(title)
        for field in category.fields {
            guard let path = field["path"] as? String else { continue }
            let control = makeControl(field: field, values: values)
            controls[path] = control
            baselines[path] = value(at: path, in: values)
            let label = settingsCategoryTitle(
                id: path.split(separator: "/").last.map(String.init) ?? path,
                titleKey: field["title_key"] as? String ?? ""
            )
            stack.addArrangedSubview(row(label, control))
        }
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .vertical)
        stack.addArrangedSubview(spacer)

        scroll.documentView = stack
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: scroll.contentView.trailingAnchor),
            stack.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            stack.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])
        return scroll
    }

    private func selectCategory(_ id: String?) {
        guard let id, visibleCategoryIDs.contains(id), pages[id] != nil else {
            selectedCategoryID = nil
            pages.values.forEach { $0.isHidden = true }
            categoryTable.deselectAll(nil)
            return
        }
        selectedCategoryID = id
        for (pageID, page) in pages {
            page.isHidden = pageID != id
        }
        if let row = visibleCategoryIDs.firstIndex(of: id), categoryTable.selectedRow != row {
            categoryTable.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }
    }

    private func applySearch() {
        let query = searchField.stringValue
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        visibleCategoryIDs = categories
            .filter { query.isEmpty || $0.searchText.contains(query) }
            .map(\.id)
        categoryTable.reloadData()
        if let selectedCategoryID, visibleCategoryIDs.contains(selectedCategoryID) {
            selectCategory(selectedCategoryID)
        } else {
            selectCategory(visibleCategoryIDs.first)
        }
    }

    private func makeControl(field: [String: Any], values: [String: Any]) -> NSView {
        let path = field["path"] as? String ?? ""
        let kind = field["control"] as? String ?? "text"
        let baseline = value(at: path, in: values)

        switch kind {
        case "switch":
            let checkbox = NSButton(checkboxWithTitle: "", target: self, action: #selector(controlChanged(_:)))
            checkbox.identifier = NSUserInterfaceItemIdentifier(path)
            checkbox.state = (baseline as? Bool == true) ? .on : .off
            return checkbox
        case "number":
            let field = NSTextField()
            field.identifier = NSUserInterfaceItemIdentifier(path)
            field.alignment = .right
            field.target = self
            field.action = #selector(controlChanged(_:))
            field.delegate = self
            if let number = baseline as? NSNumber {
                field.stringValue = number.stringValue
            }
            field.controlSize = .regular
            field.setContentHuggingPriority(.defaultLow, for: .horizontal)
            return field
        case "multiline":
            let textView = NSTextView()
            textView.isVerticallyResizable = true
            textView.isHorizontallyResizable = false
            textView.autoresizingMask = [.width]
            textView.textContainer?.widthTracksTextView = true
            textView.identifier = NSUserInterfaceItemIdentifier(path)
            let scroll = NSScrollView()
            scroll.hasVerticalScroller = true
            scroll.borderType = .bezelBorder
            scroll.documentView = textView
            scroll.heightAnchor.constraint(equalToConstant: 72).isActive = true
            scroll.widthAnchor.constraint(greaterThanOrEqualToConstant: 260).isActive = true
            if let items = baseline as? [String] {
                textView.string = items.joined(separator: "\n")
            }
            return scroll
        case "select", "theme_picker":
            let popup = NSPopUpButton(frame: .zero, pullsDown: false)
            popup.identifier = NSUserInterfaceItemIdentifier(path)
            popup.target = self
            popup.action = #selector(controlChanged(_:))
            if let options = field["options"] as? [String] {
                popup.addItems(withTitles: options)
            }
            if let current = baseline as? String {
                popup.selectItem(withTitle: current)
            }
            return popup
        case "font_fallback", "string_list":
            let entry = NSTextField()
            entry.identifier = NSUserInterfaceItemIdentifier(path)
            entry.target = self
            entry.action = #selector(controlChanged(_:))
            entry.delegate = self
            if let items = baseline as? [String] {
                entry.stringValue = items.joined(separator: ", ")
            }
            entry.controlSize = .regular
            entry.setContentHuggingPriority(.defaultLow, for: .horizontal)
            return entry
        case "font_picker":
            let row = NSStackView()
            row.orientation = .horizontal
            row.spacing = 6
            let entry = NSTextField()
            entry.identifier = NSUserInterfaceItemIdentifier(path)
            entry.target = self
            entry.action = #selector(controlChanged(_:))
            entry.delegate = self
            if let family = baseline as? String {
                entry.stringValue = family
            }
            entry.controlSize = .regular
            entry.setContentHuggingPriority(.defaultLow, for: .horizontal)
            let choose = NSButton(title: "Choose…", target: self, action: #selector(chooseFont(_:)))
            choose.identifier = NSUserInterfaceItemIdentifier(path)
            row.addArrangedSubview(entry)
            row.addArrangedSubview(choose)
            return row
        case "project_editor":
            let editor = SettingsProjectEditorView(projects: quickConnectStore.projects)
            editor.onNew = { [weak self] in
                self?.openProjectEditor(nil)
            }
            editor.onEdit = { [weak self] index in
                self?.openProjectEditor(at: index)
            }
            editor.onDelete = { [weak self] index in
                self?.deleteProject(at: index)
            }
            projectEditorView = editor
            return editor
        case "shortcut_editor":
            let label = NSTextField(labelWithString: "Managed by the dedicated editor")
            label.identifier = NSUserInterfaceItemIdentifier(path)
            label.textColor = .secondaryLabelColor
            return label
        default:
            let entry = NSTextField()
            entry.identifier = NSUserInterfaceItemIdentifier(path)
            entry.target = self
            entry.action = #selector(controlChanged(_:))
            entry.delegate = self
            if let text = baseline as? String {
                entry.stringValue = text
            }
            entry.controlSize = .regular
            entry.setContentHuggingPriority(.defaultLow, for: .horizontal)
            return entry
        }
    }

    private func row(_ title: String, _ control: NSView) -> NSStackView {
        let label = NSTextField(labelWithString: title)
        label.setContentHuggingPriority(.required, for: .horizontal)
        label.widthAnchor.constraint(equalToConstant: 180).isActive = true
        control.setContentHuggingPriority(.defaultLow, for: .horizontal)
        control.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        control.widthAnchor.constraint(greaterThanOrEqualToConstant: 240).isActive = true
        let row = NSStackView(views: [label, control])
        row.orientation = .horizontal
        row.distribution = .fill
        row.spacing = 12
        row.alignment = .centerY
        row.setContentHuggingPriority(.defaultLow, for: .horizontal)
        return row
    }

    private func loadValues(_ values: [String: Any]) {
        summaryLabel.stringValue = "Core manifest loaded · \(quickConnectStore.projects.count) project(s) · Apply is transactional"
    }

    private func refreshProjectEditor() {
        projectEditorView?.setProjects(quickConnectStore.projects)
        loadValues([:])
    }

    private func openProjectEditor(at index: Int) {
        guard quickConnectStore.projects.indices.contains(index) else { return }
        openProjectEditor(quickConnectStore.projects[index])
    }

    private func openProjectEditor(_ config: TargetConfig?) {
        guard let owner = window else { return }
        let hosts: [SSHHostInfo]
        switch ConnectionDiscovery().sshHosts() {
        case .success(let value):
            hosts = value
        case .failure:
            hosts = []
        }
        let availableRuntimes = (try? CoreBridge.runtimeCatalog())?
            .compactMap { TargetRuntime(rawValue: $0.id) }
            ?? TargetRuntime.allCases
        let editor = TargetConfigWindow(
            editing: config,
            owner: owner,
            store: quickConnectStore,
            sshHosts: hosts,
            availableRuntimes: availableRuntimes
        )
        activeProjectEditor = editor
        editor.onSave = { [weak self] saved in
            guard let self else { return }
            if let config {
                self.quickConnectStore.updateProject(saved, replacing: config)
            } else {
                self.quickConnectStore.upsertProject(saved)
            }
            self.refreshProjectEditor()
            self.activeProjectEditor = nil
        }
        editor.onCancel = { [weak self] in
            self?.activeProjectEditor = nil
        }
    }

    private func deleteProject(at index: Int) {
        guard quickConnectStore.projects.indices.contains(index), let owner = window else { return }
        let project = quickConnectStore.projects[index]
        let alert = NSAlert()
        alert.messageText = "Delete Project?"
        alert.informativeText = "Remove \"\(project.name)\" from the saved projects?"
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Delete")
        alert.beginSheetModal(for: owner) { [weak self] response in
            guard response == .alertSecondButtonReturn, let self else { return }
            self.quickConnectStore.removeProject(config: project)
            self.refreshProjectEditor()
        }
    }

    // MARK: - Value collection and Apply/Cancel

    private func collectOperations() -> [[String: Any]] {
        var operations: [[String: Any]] = []
        for (path, view) in controls {
            guard let value = controlValue(view, baseline: baselines[path]) else { continue }
            operations.append(["op": "replace", "path": path, "value": value])
        }
        return operations
    }

    private func controlValue(_ view: NSView, baseline: Any?) -> Any? {
        if let editor = view as? SettingsProjectEditorView {
            return QuickConnectStore.projectJSON(from: editor.projects)
        }
        if let checkbox = view as? NSButton {
            return checkbox.state == .on
        }
        if let field = view as? NSTextField {
            if let number = baseline as? NSNumber, let parsed = Double(field.stringValue) {
                let type = number.objCType.pointee
                if type == Int8(Character("q").asciiValue!)
                    || type == Int8(Character("Q").asciiValue!)
                    || type == Int8(Character("i").asciiValue!)
                    || type == Int8(Character("I").asciiValue!) {
                    return Int(parsed.rounded())
                }
                return parsed
            }
            return field.stringValue
        }
        if let scroll = view as? NSScrollView, let textView = scroll.documentView as? NSTextView {
            return textView.string.split(separator: "\n").map(String.init).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        }
        if let popup = view as? NSPopUpButton {
            return popup.titleOfSelectedItem
        }
        if let row = view as? NSStackView {
            // font_picker row: 第一个 NSTextField 就是 family。
            for subview in row.arrangedSubviews {
                if let field = subview as? NSTextField {
                    return field.stringValue
                }
            }
        }
        return nil
    }

    @objc private func applySettings() {
        var transaction: String?
        do {
            transaction = try bridge.configBegin()
            try bridge.configPatch(transaction: transaction!, operations: collectOperations())
            try bridge.configCommit(transaction: transaction!)
            dirty = false
            window?.close()
        } catch {
            if let transaction {
                bridge.configCancel(transaction: transaction)
            }
            let alert = NSAlert()
            alert.messageText = "Unable to save settings"
            alert.informativeText = error.localizedDescription
            alert.alertStyle = .warning
            alert.beginSheetModal(for: window!, completionHandler: nil)
        }
    }

    @objc private func cancelSettings() {
        if dirty {
            confirmDiscard { [weak self] in
                self?.dirty = false
                self?.window?.close()
            }
        } else {
            window?.close()
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if dirty {
            confirmDiscard { [weak self] in
                self?.dirty = false
                self?.window?.close()
            }
            return false
        }
        return true
    }

    private func confirmDiscard(onDiscard: @escaping () -> Void) {
        guard let window else { return }
        let alert = NSAlert()
        alert.messageText = "Discard unsaved changes?"
        alert.informativeText = "Your edits have not been applied to config.toml."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Discard")
        alert.beginSheetModal(for: window) { response in
            if response == .alertSecondButtonReturn {
                onDiscard()
            }
        }
    }

    @objc private func controlChanged(_ sender: Any?) {
        dirty = true
    }

    func controlTextDidChange(_ obj: Notification) {
        if let field = obj.object as? NSSearchField, field === searchField {
            applySearch()
            return
        }
        dirty = true
    }

    // MARK: - Category table

    func numberOfRows(in tableView: NSTableView) -> Int {
        visibleCategoryIDs.count
    }

    func tableView(
        _ tableView: NSTableView,
        viewFor tableColumn: NSTableColumn?,
        row: Int
    ) -> NSView? {
        guard visibleCategoryIDs.indices.contains(row),
              let category = categories.first(where: { $0.id == visibleCategoryIDs[row] })
        else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("muxterm.settings.categoryCell")
        let cell = (tableView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView)
            ?? NSTableCellView()
        cell.identifier = identifier
        let label: NSTextField
        if let existing = cell.textField {
            label = existing
        } else {
            label = NSTextField(labelWithString: "")
            label.translatesAutoresizingMaskIntoConstraints = false
            label.lineBreakMode = .byTruncatingTail
            cell.textField = label
            cell.addSubview(label)
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 10),
                label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -8),
                label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        label.stringValue = category.title
        cell.setAccessibilityIdentifier("muxterm.settings.category.\(category.id)")
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        let row = categoryTable.selectedRow
        guard visibleCategoryIDs.indices.contains(row) else { return }
        selectCategory(visibleCategoryIDs[row])
    }

    @objc private func chooseFont(_ sender: NSButton) {
        pendingFontPath = sender.identifier?.rawValue
        let fontManager = NSFontManager.shared
        fontManager.target = self
        fontManager.action = #selector(changeFont(_:))
        fontManager.orderFrontFontPanel(sender)
    }

    @objc private func changeFont(_ sender: NSFontManager) {
        guard let path = pendingFontPath, let field = textField(at: path) else { return }
        let current = NSFont(name: field.stringValue, size: 13) ?? NSFont.systemFont(ofSize: 13)
        field.stringValue = sender.convert(current).fontName
        dirty = true
    }

    private func textField(at path: String) -> NSTextField? {
        guard let view = controls[path] else { return nil }
        if let field = view as? NSTextField {
            return field
        }
        if let row = view as? NSStackView {
            for subview in row.arrangedSubviews {
                if let field = subview as? NSTextField {
                    return field
                }
            }
        }
        return nil
    }

    // MARK: - JSON Pointer lookup

    private func value(at path: String, in values: [String: Any]) -> Any? {
        let segments = path.split(separator: "/").map(String.init)
        var current: Any = values
        for segment in segments {
            guard let dict = current as? [String: Any], let next = dict[segment] else {
                return nil
            }
            current = next
        }
        return current
    }

    // MARK: - In-process E2E hooks

    func testCategoryIDs() -> [String] {
        categories.map(\.id)
    }

    func testVisibleCategoryIDs() -> [String] {
        visibleCategoryIDs
    }

    func testSelectedCategoryID() -> String? {
        selectedCategoryID
    }

    func testVisiblePageID() -> String? {
        pages.first(where: { !$0.value.isHidden })?.key
    }

    func testSelectCategory(_ id: String) {
        selectCategory(id)
    }

    func testTextField(path: String) -> NSTextField? {
        textField(at: path)
    }

    func testSetSearchQuery(_ query: String) {
        searchField.stringValue = query
        applySearch()
    }

    func testSidebarWidth() -> CGFloat {
        window?.contentView?.layoutSubtreeIfNeeded()
        return sidebarView.frame.width
    }

    func testVisiblePageIsScrollable() -> Bool {
        guard let selectedCategoryID, let page = pages[selectedCategoryID] else { return false }
        return page.hasVerticalScroller
    }

    func testControl(path: String) -> NSView? {
        controls[path]
    }

    func testProjectEditorVisible() -> Bool {
        projectEditorView != nil
    }

    func testProjectNames() -> [String] {
        quickConnectStore.projects.map(\.name)
    }

    func testHasNewProjectButton() -> Bool {
        projectEditorView?.hasNewProjectButton == true
    }

    func testProjectEditorContainsPlaceholder() -> Bool {
        projectEditorView?.containsPlaceholder == true
    }

    func testOpenProjectEditor(at index: Int) {
        openProjectEditor(at: index)
    }

    func testOpenNewProjectEditor() {
        openProjectEditor(nil)
    }

    func testActiveTargetConfigWindow() -> TargetConfigWindow? {
        activeProjectEditor
    }
}

/// Settings 中的 Project 列表；编辑细节复用 TargetConfigWindow。
private final class SettingsProjectEditorView: NSView {
    var onNew: (() -> Void)?
    var onEdit: ((Int) -> Void)?
    var onDelete: ((Int) -> Void)?

    private(set) var projects: [TargetConfig]
    private let rows = NSStackView()
    private let newButton = NSButton(title: "New Project", target: nil, action: nil)

    init(projects: [TargetConfig]) {
        self.projects = projects
        super.init(frame: .zero)
        build()
        setProjects(projects)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        return nil
    }

    var hasNewProjectButton: Bool {
        newButton.accessibilityIdentifier() == "muxterm.settings.projects.new"
    }

    var containsPlaceholder: Bool {
        rows.arrangedSubviews.contains { view in
            (view as? NSTextField)?.stringValue == "No projects yet"
        }
    }

    func setProjects(_ projects: [TargetConfig]) {
        self.projects = projects
        rows.arrangedSubviews.forEach {
            rows.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        if projects.isEmpty {
            let empty = NSTextField(labelWithString: "No projects yet")
            empty.textColor = .secondaryLabelColor
            rows.addArrangedSubview(empty)
            return
        }
        for (index, project) in projects.enumerated() {
            rows.addArrangedSubview(makeRow(project: project, index: index))
        }
    }

    private func build() {
        translatesAutoresizingMaskIntoConstraints = false

        let stack = NSStackView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.orientation = .vertical
        stack.alignment = .width
        stack.spacing = 10

        newButton.target = self
        newButton.action = #selector(newProject)
        newButton.bezelStyle = .rounded
        newButton.setAccessibilityIdentifier("muxterm.settings.projects.new")
        stack.addArrangedSubview(newButton)

        rows.orientation = .vertical
        rows.alignment = .width
        rows.spacing = 8
        stack.addArrangedSubview(rows)

        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    private func makeRow(project: TargetConfig, index: Int) -> NSView {
        let name = NSTextField(labelWithString: project.name)
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        name.lineBreakMode = .byTruncatingTail

        let details = NSTextField(labelWithString: projectDetail(project))
        details.textColor = .secondaryLabelColor
        details.font = .systemFont(ofSize: 11)
        details.lineBreakMode = .byTruncatingMiddle

        let info = NSStackView(views: [name, details])
        info.orientation = .vertical
        info.alignment = .leading
        info.spacing = 2
        info.setContentHuggingPriority(.defaultLow, for: .horizontal)
        info.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let edit = NSButton(title: "Edit", target: self, action: #selector(editProject(_:)))
        edit.tag = index
        edit.bezelStyle = .rounded
        edit.setAccessibilityIdentifier("muxterm.settings.projects.\(index).edit")
        let delete = NSButton(title: "Delete", target: self, action: #selector(deleteProject(_:)))
        delete.tag = index
        delete.bezelStyle = .rounded
        delete.setAccessibilityIdentifier("muxterm.settings.projects.\(index).delete")

        let row = NSStackView(views: [info, edit, delete])
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 8
        row.setAccessibilityIdentifier("muxterm.settings.projects.\(index)")
        return row
    }

    private func projectDetail(_ project: TargetConfig) -> String {
        let transport: String
        switch project.transport {
        case .local:
            transport = "local"
        case .ssh(let name):
            transport = "ssh:\(name)"
        }
        let path = project.path.isEmpty ? "(no path)" : project.path
        return "\(project.runtime.rawValue) · \(transport) · \(path)"
    }

    @objc private func newProject() {
        onNew?()
    }

    @objc private func editProject(_ sender: NSButton) {
        onEdit?(sender.tag)
    }

    @objc private func deleteProject(_ sender: NSButton) {
        onDelete?(sender.tag)
    }
}
