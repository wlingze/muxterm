import XCTest
@testable import MuxtermAppLib

/// 远端目录窗口曾经在按钮行进入视图树之前激活宽度约束，AppKit 直接 SIGABRT。
final class RemoteDirectoryPickerE2ETests: XCTestCase {
    func testPickerBuildsButtonRowInsideTheSameViewHierarchy() {
        AppE2E.ensureApp()
        let window = RemoteDirectoryPickerWindow(
            alias: "muxterm-test-host",
            owner: nil,
            startsListing: false
        )
        defer { window.close() }

        let confirm = findView(window.contentView, identifier: "muxterm.remoteDirectory.confirm")
        XCTAssertNotNil(confirm)
        XCTAssertTrue(confirm?.window === window)
        var ancestor: NSView? = confirm?.superview
        var reachedContent = false
        while let view = ancestor {
            if view === window.contentView {
                reachedContent = true
                break
            }
            ancestor = view.superview
        }
        XCTAssertTrue(reachedContent)
    }

    private func findView(_ root: NSView?, identifier: String) -> NSView? {
        guard let root else { return nil }
        if root.accessibilityIdentifier() == identifier { return root }
        for subview in root.subviews {
            if let found = findView(subview, identifier: identifier) { return found }
        }
        return nil
    }
}
