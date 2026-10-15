import AppKit
import Foundation
import MuxtermChrome

/// 命令面板里的 Muxterm Info：版本、commit、调试日志和 GitHub issue。
final class MuxtermInfoWindowController: NSWindowController {
    private let versionValue = NSTextField(labelWithString: "")
    private let commitValue = NSTextField(labelWithString: "")
    private let logValue = NSTextField(labelWithString: "")
    private var logPath = ""

    static func show(relativeTo owner: NSWindow?) {
        let controller = MuxtermInfoWindowController()
        controller.window?.appearance = owner?.appearance
        if let owner, let window = controller.window {
            window.setFrameOrigin(NSPoint(
                x: owner.frame.midX - window.frame.width / 2,
                y: owner.frame.midY - window.frame.height / 2
            ))
        }
        controller.showWindow(nil)
        controller.window?.makeKeyAndOrderFront(nil)
    }

    init() {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 280),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        panel.title = "Muxterm Info"
        super.init(window: panel)
        let info = CoreBridge.buildInfo()
        logPath = info.logFile
        versionValue.stringValue = info.version.isEmpty ? "unknown" : info.version
        commitValue.stringValue = info.commit.isEmpty ? "unknown" : info.commit
        logValue.stringValue = info.logFile.isEmpty ? "Debug log is off" : info.logFile
        logValue.lineBreakMode = .byTruncatingMiddle
        build(in: panel)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    private func build(in panel: NSPanel) {
        let rows = NSStackView()
        rows.orientation = .vertical
        rows.alignment = .leading
        rows.spacing = 10
        rows.translatesAutoresizingMaskIntoConstraints = false
        rows.addArrangedSubview(line("Version", versionValue))
        rows.addArrangedSubview(line("Commit", commitValue))
        rows.addArrangedSubview(line("Debug log", logValue))

        let copyLog = button("Copy debug log path", #selector(copyLogPath))
        let copyCommit = button("Copy commit", #selector(copyCommit))
        let issue = button("Submit GitHub issue", #selector(submitIssue))
        copyLog.isEnabled = !logPath.isEmpty
        let actions = NSStackView(views: [copyLog, copyCommit, issue])
        actions.orientation = .horizontal
        actions.spacing = 8
        rows.addArrangedSubview(actions)

        let note = NSTextField(wrappingLabelWithString: "Debug logging starts from Settings → Debug, or with --debug. The issue includes the version, commit, and the tail of the debug log.")
        note.font = .systemFont(ofSize: 11)
        note.textColor = .secondaryLabelColor
        note.preferredMaxLayoutWidth = 480
        rows.addArrangedSubview(note)

        panel.contentView?.addSubview(rows)
        guard let content = panel.contentView else { return }
        NSLayoutConstraint.activate([
            rows.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 20),
            rows.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            rows.topAnchor.constraint(equalTo: content.topAnchor, constant: 20),
        ])
    }

    private func line(_ title: String, _ value: NSTextField) -> NSView {
        let label = NSTextField(labelWithString: title)
        label.font = .systemFont(ofSize: 12, weight: .semibold)
        label.widthAnchor.constraint(equalToConstant: 84).isActive = true
        value.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        value.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let row = NSStackView(views: [label, value])
        row.orientation = .horizontal
        row.alignment = .firstBaseline
        row.spacing = 8
        return row
    }

    private func button(_ title: String, _ action: Selector) -> NSButton {
        let button = NSButton(title: title, target: self, action: action)
        button.bezelStyle = .rounded
        return button
    }

    @objc private func copyLogPath() {
        copy(logPath)
    }

    @objc private func copyCommit() {
        copy(commitValue.stringValue)
    }

    private func copy(_ text: String) {
        guard !text.isEmpty else { return }
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }

    @objc private func submitIssue() {
        let alert = NSAlert()
        alert.messageText = "Submit a GitHub issue?"
        alert.informativeText = "The report includes the version, commit, and the end of the debug log. Lines that look like passwords or tokens are removed."
        alert.addButton(withTitle: "Submit")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let info = CoreBridge.buildInfo()
        let logText = Self.readLog(info.logFile)
        let tail = MuxtermIssueReport.tail(logText, maxBytes: 60_000)
        let title = "Muxterm debug report \(info.version) \(info.commit)"
        let body = """
        Muxterm \(info.version) (\(info.commit))
        Debug log: \(info.logFile.isEmpty ? "(not enabled)" : info.logFile)

        ```
        \(tail.isEmpty ? "(no debug log yet)" : tail)
        ```
        """
        if let url = Self.submitWithGitHubCLI(title: title, body: body) {
            Self.note("Opened \(url)")
            return
        }
        let short = """
        Muxterm \(info.version) (\(info.commit))
        Debug log: \(info.logFile.isEmpty ? "(not enabled)" : info.logFile)

        gh is not available, so the log tail was copied to the clipboard instead of the URL.
        """
        copy(body)
        if let url = MuxtermIssueReport.githubIssueURL(title: title, body: short) {
            NSWorkspace.shared.open(url)
        }
        Self.note("GitHub CLI was not available. The report was copied, and a browser issue form was opened.")
    }

    private static func submitWithGitHubCLI(title: String, body: String) -> String? {
        let file = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-issue-\(UUID().uuidString).md")
        do {
            try body.write(to: file, atomically: true, encoding: .utf8)
        } catch {
            return nil
        }
        defer { try? FileManager.default.removeItem(at: file) }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = [
            "gh", "issue", "create",
            "--repo", MuxtermIssueReport.repository,
            "--title", title,
            "--body-file", file.path,
        ]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = Pipe()
        do {
            try process.run()
        } catch {
            return nil
        }
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        let text = String(data: data, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if let url = URL(string: text), text.hasPrefix("https://") {
            NSWorkspace.shared.open(url)
            return text
        }
        return text.isEmpty ? "https://github.com/\(MuxtermIssueReport.repository)/issues" : text
    }

    private static func readLog(_ path: String) -> String {
        guard !path.isEmpty, let data = FileManager.default.contents(atPath: path) else {
            return ""
        }
        return String(data: data, encoding: .utf8) ?? ""
    }

    private static func note(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "Muxterm Info"
        alert.informativeText = message
        alert.runModal()
    }
}
