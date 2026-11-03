import AppKit
import Foundation
import MuxtermChrome

/// 只读诊断面板：Esc 关闭。Info 面板不应该挡住终端输入。
private final class MuxtermInfoPanel: NSPanel {
    override func cancelOperation(_ sender: Any?) {
        performClose(nil)
    }
}

/// 命令面板里的 Muxterm Info：版本、commit、调试日志和 GitHub issue。
final class MuxtermInfoWindowController: NSWindowController {
    /// 固定内容尺寸：反馈文本不能改变窗口大小。
    static let panelWidth: CGFloat = 520
    static let panelHeight: CGFloat = 300
    private let versionValue = NSTextField(labelWithString: "")
    private let commitValue = NSTextField(labelWithString: "")
    private let logValue = NSTextField(labelWithString: "")
    private let copyFeedback = NSTextField(labelWithString: " ")
    private let pasteboard: NSPasteboard
    private var logPath = ""
    private var clearFeedbackWork: DispatchWorkItem?

    /// 由主窗口持有 controller。NSWindow 和按钮 target 都不会替调用方保活它。
    func show(relativeTo owner: NSWindow?) {
        window?.appearance = owner?.appearance
        if let owner, let window {
            window.setFrameOrigin(NSPoint(
                x: owner.frame.midX - window.frame.width / 2,
                y: owner.frame.midY - window.frame.height / 2
            ))
        }
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    init(info: CoreBridge.BuildInfo = CoreBridge.buildInfo(), pasteboard: NSPasteboard = .general) {
        self.pasteboard = pasteboard
        let panel = MuxtermInfoPanel(
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: Self.panelWidth,
                height: Self.panelHeight
            ),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        panel.title = "Muxterm Info"
        panel.identifier = NSUserInterfaceItemIdentifier("muxterm.info")
        panel.isReleasedWhenClosed = false
        // 内容高度固定：复制反馈/长路径出现时不能把窗口撑大。
        // 旧实现只有 top 约束，点击复制后窗口会自己变高，看起来像「点坏了」。
        panel.contentMinSize = NSSize(width: Self.panelWidth, height: Self.panelHeight)
        panel.contentMaxSize = NSSize(width: Self.panelWidth, height: Self.panelHeight)
        super.init(window: panel)
        logPath = info.logFile
        versionValue.stringValue = info.version.isEmpty ? "unknown" : info.version
        commitValue.stringValue = info.commit.isEmpty ? "unknown" : info.commit
        logValue.stringValue = info.logFile.isEmpty ? "Debug log is off" : info.logFile
        logValue.lineBreakMode = .byTruncatingMiddle
        logValue.toolTip = logPath
        logValue.setAccessibilityIdentifier("muxterm.info.logPath")
        logValue.addGestureRecognizer(NSClickGestureRecognizer(
            target: self, action: #selector(copyLogPath)
        ))
        build(in: panel)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    private func build(in panel: NSPanel) {
        let rows = NSStackView()
        rows.orientation = .vertical
        rows.alignment = .width
        rows.spacing = 10
        rows.translatesAutoresizingMaskIntoConstraints = false
        rows.addArrangedSubview(line("Version", versionValue))
        rows.addArrangedSubview(line("Commit", commitValue))
        rows.addArrangedSubview(line("Debug log", logValue))

        let copyLog = button("Copy debug log path", #selector(copyLogPath))
        let copyCommit = button("Copy commit", #selector(copyCommit))
        let issue = button("Submit GitHub issue", #selector(submitIssue))
        copyLog.setAccessibilityIdentifier("muxterm.info.copyLog")
        copyCommit.setAccessibilityIdentifier("muxterm.info.copyCommit")
        issue.setAccessibilityIdentifier("muxterm.info.submitIssue")
        copyLog.isEnabled = !logPath.isEmpty
        let actions = NSStackView(views: [copyLog, copyCommit, issue])
        actions.orientation = .horizontal
        actions.spacing = 8
        rows.addArrangedSubview(actions)
        copyFeedback.font = .systemFont(ofSize: 11)
        copyFeedback.maximumNumberOfLines = 1
        copyFeedback.lineBreakMode = .byTruncatingTail
        copyFeedback.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        copyFeedback.setAccessibilityIdentifier("muxterm.info.copyFeedback")
        rows.addArrangedSubview(copyFeedback)

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
            // 允许余量但不能反过来撑窗口；窗口尺寸由 contentMin/MaxSize 固定。
            rows.bottomAnchor.constraint(
                lessThanOrEqualTo: content.bottomAnchor, constant: -20
            ),
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
        pasteboard.clearContents()
        let copied = pasteboard.setString(text, forType: .string)
        showFeedback(
            MuxtermI18n.shared.trDynamic(
                copied ? "info.copied" : "info.copyFailed",
                fallback: copied ? "Copied to clipboard" : "Could not copy to clipboard"
            ),
            success: copied
        )
        CoreBridge.log("info clipboard copy \(copied ? "succeeded" : "failed")", level: copied ? "debug" : "warn")
    }

    /// 反馈只是一行状态：自动消失，不能变成常驻布局元素。
    private func showFeedback(_ text: String, success: Bool) {
        clearFeedbackWork?.cancel()
        copyFeedback.stringValue = text
        copyFeedback.textColor = success ? .systemGreen : .systemRed
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.copyFeedback.stringValue = " "
        }
        clearFeedbackWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5, execute: work)
    }

    @objc private func submitIssue() {
        let alert = NSAlert()
        alert.messageText = "Submit a GitHub issue?"
        alert.informativeText = "The report includes the version, commit, and the end of the debug log. Lines that look like passwords or tokens are removed."
        alert.addButton(withTitle: "Submit")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let info = CoreBridge.buildInfo()
        let title = "Muxterm debug report \(info.version) \(info.commit)"
        // 读日志 + `gh issue create` 走网络：必须在后台队列做。
        // 旧实现用 /usr/bin/env 同步 waitUntilExit()，点一下窗口就转圈数秒，
        // 而且把整份 debug 日志读进主线程内存（单日可以到几百 MB）。
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let logText = MuxtermIssueReport.readLogTail(
                path: info.logFile,
                maxBytes: 120_000
            )
            let tail = MuxtermIssueReport.tail(logText, maxBytes: 60_000)
            let body = """
            Muxterm \(info.version) (\(info.commit))
            Debug log: \(info.logFile.isEmpty ? "(not enabled)" : info.logFile)

            ```
            \(tail.isEmpty ? "(no debug log yet)" : tail)
            ```
            """
            let url = Self.submitWithGitHubCLI(title: title, body: body)
            DispatchQueue.main.async {
                guard let self else { return }
                if let url {
                    self.showFeedback("Opened \(url.absoluteString)", success: true)
                    NSWorkspace.shared.open(url)
                    return
                }
                // gh 不可用：把正文放进剪贴板，再打开浏览器 issue 表单。
                let short = """
                Muxterm \(info.version) (\(info.commit))
                Debug log: \(info.logFile.isEmpty ? "(not enabled)" : info.logFile)

                gh is not available, so the log tail was copied to the clipboard instead of the URL.
                """
                self.copy(body)
                if let url = MuxtermIssueReport.githubIssueURL(title: title, body: short) {
                    NSWorkspace.shared.open(url)
                    self.showFeedback("GitHub CLI unavailable; report copied", success: true)
                } else {
                    self.showFeedback("GitHub CLI unavailable", success: false)
                }
            }
        }
    }

    /// 后台队列：`gh` 的网络往返不能阻塞主线程。
    private static func submitWithGitHubCLI(title: String, body: String) -> URL? {
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
        // stderr 丢弃：只解析 stdout 上的 URL。两条管道各自读很容易在
        // 一个写满时把子进程卡死；这里只留一条需要读的管道。
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            return nil
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        let text = String(data: data, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if let url = URL(string: text), text.hasPrefix("https://") {
            return url
        }
        if !text.isEmpty, let url = URL(string: text) {
            return url
        }
        return URL(string: "https://github.com/\(MuxtermIssueReport.repository)/issues")
    }

}
