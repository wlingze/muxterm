import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 对齐 iTerm2「Dim inactive split panes」：分屏里未聚焦 pane 变暗。
final class InactivePaneDimmingE2ETests: XCTestCase {
    func testHostDimsOnlyInactiveSplitPanes() {
        AppE2E.ensureApp()
        let terminal = MuxTerminalView(
            paneId: 8,
            frame: NSRect(x: 0, y: 0, width: 400, height: 200)
        )
        let host = PaneHostView(paneId: 8, title: "shell", terminal: terminal)

        host.setActive(false)
        XCTAssertFalse(host.isContentDimmedForTesting, "单 pane 即使未标 active 也不蒙灰")

        host.setActive(true, visiblePaneCount: 2)
        XCTAssertFalse(host.isContentDimmedForTesting, "聚焦 pane 保持原亮度")

        host.setActive(false, visiblePaneCount: 2)
        XCTAssertTrue(host.isContentDimmedForTesting, "分屏未聚焦 pane 必须变暗")
        XCTAssertFalse(
            host.dimOverlayBlocksHitsForTesting(at: NSPoint(x: 10, y: 10)),
            "蒙层不得截获点击，否则无法点选未聚焦 pane"
        )

        host.setActive(false, visiblePaneCount: 1)
        XCTAssertFalse(host.isContentDimmedForTesting, "zoom / 单 pane 取消蒙层")
    }

    func testDimOverlayCoversTerminalNotTitleBar() {
        AppE2E.ensureApp()
        let terminal = MuxTerminalView(
            paneId: 9,
            frame: NSRect(x: 0, y: 0, width: 400, height: 200)
        )
        let host = PaneHostView(paneId: 9, title: "shell", terminal: terminal)
        host.setShowsTitleBar(true)
        host.setActive(false, visiblePaneCount: 2)

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 240),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 400, height: 240))
        window.contentView = root
        root.addSubview(host)
        NSLayoutConstraint.activate([
            host.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            host.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            host.topAnchor.constraint(equalTo: root.topAnchor),
            host.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
        window.layoutIfNeeded()
        defer { window.orderOut(nil) }

        XCTAssertGreaterThan(host.titleBarFrameForTesting.height, 0)
        XCTAssertGreaterThan(host.dimOverlayFrameForTesting.height, 0)
        XCTAssertEqual(
            host.dimOverlayFrameForTesting.maxY,
            host.titleBarFrameForTesting.minY,
            accuracy: 1,
            "蒙层只盖终端，不盖标题栏"
        )
        XCTAssertEqual(
            host.dimOverlayFrameForTesting.height,
            host.terminalHeightForTesting,
            accuracy: 1
        )
    }

    func testSplitLayoutDimsInactiveAndFullscreenClearsDim() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        defer { bridge.shutdown() }
        let layout = PaneLayoutView(terminalManager: TerminalManager(bridge: bridge))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 400),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = layout
        window.orderFront(nil)
        defer { window.orderOut(nil) }

        let panes = [
            Pane(id: 1, cols: 80, rows: 24, isActive: true),
            Pane(id: 2, cols: 80, rows: 24, isActive: false),
        ]
        let split = LayoutNode.split(
            horizontal: true,
            ratio: 500,
            first: .leaf(paneId: 1),
            second: .leaf(paneId: 2)
        )
        XCTAssertTrue(layout.apply(layout: split, panes: panes, tabId: 1))

        let active = try XCTUnwrap(layout.testHost(for: 1))
        let inactive = try XCTUnwrap(layout.testHost(for: 2))
        XCTAssertFalse(active.isContentDimmedForTesting)
        XCTAssertTrue(inactive.isContentDimmedForTesting)

        layout.markActivePane(2)
        XCTAssertTrue(active.isContentDimmedForTesting)
        XCTAssertFalse(inactive.isContentDimmedForTesting)

        layout.toggleFullscreen(paneId: 2)
        let zoomed = try XCTUnwrap(layout.testHost(for: 2))
        XCTAssertFalse(zoomed.isContentDimmedForTesting, "全屏后只有一个可见 pane，不得蒙灰")
        XCTAssertNil(layout.testHost(for: 1))
    }
}
