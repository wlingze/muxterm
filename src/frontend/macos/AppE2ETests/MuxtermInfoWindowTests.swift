import AppKit
import XCTest
@testable import MuxtermAppLib

final class MuxtermInfoWindowTests: XCTestCase {
    private func find(_ root: NSView?, _ id: String) -> NSView? {
        guard let root else { return nil }
        if root.accessibilityIdentifier() == id { return root }
        for child in root.subviews {
            if let match = find(child, id) { return match }
        }
        return nil
    }

    func testPaletteOwnsInfoControllerAfterActionReturns() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge)
        defer { app.testShutdown() }
        autoreleasepool {
            app.testOpenCommandPalette()
            app.testSelectPaletteTitle(MuxtermI18n.shared.trDynamic(
                "command.muxtermInfo", fallback: "Muxterm Info"
            ))
        }
        let infoWindow = try XCTUnwrap(NSApp.windows.first {
            $0.identifier?.rawValue == "muxterm.info"
        })
        defer { infoWindow.orderOut(nil) }
        let copy = try XCTUnwrap(find(infoWindow.contentView, "muxterm.info.copyCommit") as? NSButton)
        XCTAssertNotNil(copy.target, "显示函数返回后 controller 必须仍被主窗口持有")
        XCTAssertNotNil(infoWindow.windowController as? MuxtermInfoWindowController)
    }

    func testCopyButtonsDispatchAndShowFeedbackUsingPrivatePasteboard() throws {
        AppE2E.ensureApp()
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let path = "/tmp/" + String(repeating: "long-log-directory/", count: 8) + "muxterm.log"
        let controller = MuxtermInfoWindowController(
            info: .init(version: "test-version", commit: "abc123", logFile: path),
            pasteboard: board
        )
        let window = try XCTUnwrap(controller.window)
        defer { window.orderOut(nil) }
        controller.show(relativeTo: nil)
        window.contentView?.layoutSubtreeIfNeeded()
        let commit = try XCTUnwrap(find(window.contentView, "muxterm.info.copyCommit") as? NSButton)
        commit.performClick(nil)
        XCTAssertEqual(board.string(forType: .string), "abc123")
        let log = try XCTUnwrap(find(window.contentView, "muxterm.info.copyLog") as? NSButton)
        log.performClick(nil)
        XCTAssertEqual(board.string(forType: .string), path)
        let feedback = try XCTUnwrap(find(window.contentView, "muxterm.info.copyFeedback") as? NSTextField)
        XCTAssertFalse(feedback.stringValue.isEmpty)
        let logValue = try XCTUnwrap(find(window.contentView, "muxterm.info.logPath") as? NSTextField)
        let frame = logValue.convert(logValue.bounds, to: window.contentView)
        XCTAssertLessThanOrEqual(frame.maxX, window.contentView!.bounds.maxX - 15)
        XCTAssertEqual(logValue.toolTip, path)
        let gesture = try XCTUnwrap(logValue.gestureRecognizers.first as? NSClickGestureRecognizer)
        XCTAssertTrue(NSApp.sendAction(try XCTUnwrap(gesture.action), to: gesture.target, from: gesture))
        XCTAssertEqual(board.string(forType: .string), path)
    }

    /// 用户报告：点「Copy debug log path」之后窗口自己变高/变宽。
    /// Info 面板是固定尺寸的只读诊断窗口；复制反馈只能换一行文字。
    func testCopyDoesNotResizeTheInfoWindow() throws {
        AppE2E.ensureApp()
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let path = "/tmp/" + String(repeating: "very-long-log-directory/", count: 12) + "muxterm.log"
        let controller = MuxtermInfoWindowController(
            info: .init(version: "test-version", commit: "abc123", logFile: path),
            pasteboard: board
        )
        let window = try XCTUnwrap(controller.window)
        defer { window.orderOut(nil) }
        controller.show(relativeTo: nil)
        AppE2E.pump(60)
        window.contentView?.layoutSubtreeIfNeeded()
        let before = window.frame
        let contentBefore = window.contentView?.bounds.size

        let log = try XCTUnwrap(find(window.contentView, "muxterm.info.copyLog") as? NSButton)
        log.performClick(nil)
        AppE2E.pump(200)
        window.contentView?.layoutSubtreeIfNeeded()

        XCTAssertEqual(window.frame, before, "复制路径不得改变 Info 窗口位置/尺寸")
        XCTAssertEqual(window.contentView?.bounds.size, contentBefore)
        XCTAssertEqual(
            window.contentView?.bounds.size,
            NSSize(
                width: MuxtermInfoWindowController.panelWidth,
                height: MuxtermInfoWindowController.panelHeight
            ),
            "Info 面板必须保持固定的内容区尺寸"
        )
        XCTAssertEqual(
            window.contentMinSize,
            NSSize(
                width: MuxtermInfoWindowController.panelWidth,
                height: MuxtermInfoWindowController.panelHeight
            )
        )
    }
}
