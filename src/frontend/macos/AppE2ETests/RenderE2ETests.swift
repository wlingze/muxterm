import AppKit
import XCTest
@testable import MuxtermAppLib

/// 对标 `linux_render_e2e` CUP 末帧：SwiftTerm 停在 frame-19，不含 frame-0。
final class RenderE2ETests: XCTestCase {
    func testCupStormKeepsLastFrame() {
        AppE2E.ensureApp()
        let view = MuxTerminalView(paneId: 1, frame: NSRect(x: 0, y: 0, width: 800, height: 400))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 400),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = view
        window.orderFront(nil)
        AppE2E.pump(40)
        for i in 0..<20 {
            let frame = "\u{1b}[H\u{1b}[2Jframe-\(i)\n"
            view.feedOutput(Data(frame.utf8))
        }
        AppE2E.pump(40)
        let text = view.visibleScreenText()
        XCTAssertTrue(text.contains("frame-19"), "应停在末帧 frame-19。got=\(text)")
        XCTAssertFalse(text.contains("frame-0"), "不应残留 frame-0。got=\(text)")
        // 用 orderOut 而不是 close：XCTest memory checker 在 SwiftPM 测试进程
        // 里 dealloc 已 close 的 NSWindow 会过度 release 崩溃。
        window.orderOut(nil)
    }
}
