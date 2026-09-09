import AppKit
import MuxtermAppLib

/// LaunchServices can briefly retain an older copy of the bundle after a
/// developer copies Muxterm.app into /Applications. Enforce the same
/// single-instance rule for direct executable launches so two control clients
/// can never both forward one key event to the same tmux pane.
private func activateExistingInstanceIfNeeded() -> Bool {
    let pid = ProcessInfo.processInfo.processIdentifier
    let bundleID = Bundle.main.bundleIdentifier
    let existing = NSWorkspace.shared.runningApplications.first(where: { application in
        guard application.processIdentifier != pid else { return false }
        if let bundleID, !bundleID.isEmpty,
           application.bundleIdentifier == bundleID
        {
            return true
        }
        // Debug/release bundles intentionally use different identifiers so
        // LaunchServices cannot select an old copy, but they must still share
        // one terminal client when both copies are open.
        return application.bundleURL?.lastPathComponent == "Muxterm.app"
            || application.executableURL?.lastPathComponent == "Muxterm"
    })
    guard let existing else {
        return false
    }
    existing.activate(options: [.activateAllWindows, .activateIgnoringOtherApps])
    return true
}

if activateExistingInstanceIfNeeded() {
    exit(EXIT_SUCCESS)
}

// Muxterm macOS 原生客户端入口。
let app = NSApplication.shared
// 尽早声明为常规 GUI，避免启动瞬间被当成 background-only
app.setActivationPolicy(.regular)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
