import AppKit
import Foundation

/// AppKit settings renderer backed by Core's Schema/Manifest transaction API.
/// The controls intentionally stay small and keyboard reachable; adding a new
/// Core field does not require another TOML parser or UserDefaults key.
final class SettingsWindowController: NSWindowController, NSWindowDelegate {
    private let bridge: CoreBridge
    private let theme = NSPopUpButton()
    private let fontFamily = NSTextField()
    private let fontFallback = NSTextField()
    private let fontSize = NSTextField()
    private let statusMode = NSPopUpButton()
    private var summaryLabel = NSTextField(labelWithString: "")

    init(bridge: CoreBridge) {
        self.bridge = bridge
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 560, height: 520),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Settings"
        window.minSize = NSSize(width: 480, height: 420)
        super.init(window: window)
        window.delegate = self
        window.setAccessibilityIdentifier("muxterm.settingsWindow")
        buildView(in: window)
        loadSnapshot()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func showWindow(_ sender: Any?) {
        loadSnapshot()
        super.showWindow(sender)
        window?.makeKeyAndOrderFront(sender)
    }

    private func buildView(in window: NSWindow) {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 14
        stack.edgeInsets = NSEdgeInsets(top: 18, left: 20, bottom: 18, right: 20)

        let title = NSTextField(labelWithString: "Appearance and behavior")
        title.font = .boldSystemFont(ofSize: 18)
        stack.addArrangedSubview(title)
        stack.addArrangedSubview(row("Theme", theme))
        stack.addArrangedSubview(row("Font family", fontFamily))
        stack.addArrangedSubview(row("Fallback families", fontFallback))
        stack.addArrangedSubview(row("Font size", fontSize))
        stack.addArrangedSubview(row("Status bar", statusMode))
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
        theme.addItems(withTitles: ["System", "Black", "White"])
        theme.target = self
        statusMode.addItems(withTitles: ["tmux", "theme"])
        fontFamily.placeholderString = "JetBrains Mono"
        fontFallback.placeholderString = "Noto Sans Mono, monospace"
        fontSize.alignment = .right
        for control in [fontFamily, fontFallback, fontSize] {
            control.controlSize = .regular
            control.setContentHuggingPriority(.defaultLow, for: .horizontal)
        }
    }

    private func row(_ title: String, _ control: NSView) -> NSStackView {
        let label = NSTextField(labelWithString: title)
        label.setContentHuggingPriority(.required, for: .horizontal)
        label.widthAnchor.constraint(equalToConstant: 150).isActive = true
        control.widthAnchor.constraint(greaterThanOrEqualToConstant: 260).isActive = true
        let row = NSStackView(views: [label, control])
        row.orientation = .horizontal
        row.spacing = 12
        row.alignment = .centerY
        return row
    }

    private func loadSnapshot() {
        guard let text = bridge.configDescribeJSON(),
              let data = text.data(using: .utf8),
              let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let payload = envelope["data"] as? [String: Any],
              let values = payload["values"] as? [String: Any]
        else { return }
        let font = values["font"] as? [String: Any] ?? [:]
        fontFamily.stringValue = font["family"] as? String ?? "JetBrains Mono"
        fontFallback.stringValue = (font["fallback"] as? [String] ?? []).joined(separator: ", ")
        fontSize.stringValue = String(font["size"] as? Double ?? 13.0)
        let themeValues = values["theme"] as? [String: Any] ?? [:]
        let themeName = themeValues["name"] as? String ?? "system"
        theme.selectItem(withTitle: themeName == "black" ? "Black" : themeName == "white" ? "White" : "System")
        let status = values["statusbar"] as? [String: Any] ?? [:]
        statusMode.selectItem(withTitle: status["mode"] as? String ?? "tmux")
        let projects = (values["projects"] as? [[String: Any]])?.count ?? 0
        summaryLabel.stringValue = "Core Schema loaded · (projects) project(s) · Apply is transactional"
    }

    @objc private func applySettings() {
        var transaction: String?
        do {
            transaction = try bridge.configBegin()
            let themeName: String = switch theme.titleOfSelectedItem {
            case "Black": "black"
            case "White": "white"
            default: "system"
            }
            let fallback = fontFallback.stringValue.split(separator: ",").map { String($0).trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            let size = Double(fontSize.stringValue) ?? 13.0
            try bridge.configPatch(transaction: transaction!, operations: [
                ["op": "replace", "path": "/theme/name", "value": themeName],
                ["op": "replace", "path": "/font/family", "value": fontFamily.stringValue],
                ["op": "replace", "path": "/font/fallback", "value": fallback],
                ["op": "replace", "path": "/font/size", "value": size],
                ["op": "replace", "path": "/statusbar/mode", "value": statusMode.titleOfSelectedItem ?? "tmux"],
            ])
            try bridge.configCommit(transaction: transaction!)
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
        window?.close()
    }
}
