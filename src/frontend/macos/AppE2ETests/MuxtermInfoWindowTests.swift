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
}
