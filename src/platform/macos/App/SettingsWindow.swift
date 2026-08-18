import AppKit
import Foundation

/// AppKit settings renderer backed by Core's Schema/Manifest transaction API.
///
/// The window renders every field published by the Core manifest; a new Core
/// field only needs a manifest entry to appear here. Controls are keyed by JSON
/// Pointer and are always written back through `SettingsService` transactions,
/// never by parsing or rewriting `config.toml` in Swift.
final class SettingsWindowController: NSWindowController, NSWindowDelegate, NSControlTextEditingDelegate {
    private let bridge: CoreBridge
    private var controls: [String: NSView] = [:]
    private var baselines: [String: Any] = [:]
    private var pendingFontPath: String?
    private var dirty = false
    private var summaryLabel = NSTextField(labelWithString: "")

    init(bridge: CoreBridge) {
        self.bridge = bridge
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 640),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Settings"
        window.minSize = NSSize(width: 520, height: 420)
        super.init(window: window)
        window.delegate = self
        window.setAccessibilityIdentifier("muxterm.settingsWindow")
        loadSnapshotAndBuild()
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
        controls = [:]
        baselines = [:]
        dirty = false

        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 14
        stack.edgeInsets = NSEdgeInsets(top: 18, left: 20, bottom: 18, right: 20)

        let title = NSTextField(labelWithString: "Settings")
        title.font = .boldSystemFont(ofSize: 18)
        stack.addArrangedSubview(title)

        guard let groups = manifest["groups"] as? [[String: Any]] else {
            summaryLabel.stringValue = "No settings manifest"
            stack.addArrangedSubview(summaryLabel)
            scroll.documentView = stack
            window.contentView = scroll
            return
        }

        for group in groups {
            let groupTitle = NSTextField(labelWithString: group["title_key"] as? String ?? "Settings")
            groupTitle.font = .boldSystemFont(ofSize: 14)
            stack.addArrangedSubview(groupTitle)
            guard let fields = group["fields"] as? [[String: Any]] else { continue }
            for field in fields {
                guard let path = field["path"] as? String else { continue }
                let control = makeControl(field: field, values: values)
                controls[path] = control
                baselines[path] = value(at: path, in: values)
                stack.addArrangedSubview(row(field["title_key"] as? String ?? path, control))
            }
        }

        summaryLabel.textColor = .secondaryLabelColor
        summaryLabel.lineBreakMode = .byWordWrapping
        summaryLabel.maximumNumberOfLines = 2
        stack.addArrangedSubview(summaryLabel)

        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .vertical)
        stack.addArrangedSubview(spacer)

        let buttons = NSStackView()
        buttons.orientation = .horizontal
        buttons.spacing = 8
        buttons.addArrangedSubview(NSView())
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelSettings))
        cancel.setAccessibilityIdentifier("muxterm.settings.cancel")
        let apply = NSButton(title: "Apply", target: self, action: #selector(applySettings))
        apply.keyEquivalent = "\r"
        apply.setAccessibilityIdentifier("muxterm.settings.apply")
        buttons.addArrangedSubview(cancel)
        buttons.addArrangedSubview(apply)
        stack.addArrangedSubview(buttons)

        scroll.documentView = stack
        window.contentView = scroll
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
        case "project_editor", "shortcut_editor":
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
        control.widthAnchor.constraint(greaterThanOrEqualToConstant: 240).isActive = true
        let row = NSStackView(views: [label, control])
        row.orientation = .horizontal
        row.spacing = 12
        row.alignment = .centerY
        return row
    }

    private func loadValues(_ values: [String: Any]) {
        var projectCount = 0
        if let projects = values["projects"] as? [[String: Any]] {
            projectCount = projects.count
        }
        summaryLabel.stringValue = "Core manifest loaded · \(projectCount) project(s) · Apply is transactional"
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
        dirty = true
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
}
