import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// 把 pane 拖成新 tab = tmux `break-pane`（iTerm2 breakOutWindowPane / MoveSessionToNewTab）。
final class BreakPaneE2ETests: XCTestCase {
    func testTitleReservationIsFixedAndIgnoresLayoutTree() {
        XCTAssertEqual(PaneTitleBarGeometry.reservedHeight(showsTitles: false), 0)
        XCTAssertEqual(
            PaneTitleBarGeometry.reservedHeight(showsTitles: true),
            PaneTitleBarGeometry.height,
            "有 title 只扣一次固定高度"
        )
        let container = NSSize(width: 800, height: 600)
        let withTitle = PaneTitleBarGeometry.clientContentSize(
            container: container,
            showsTitles: true
        )
        let again = PaneTitleBarGeometry.clientContentSize(
            container: container,
            showsTitles: true
        )
        XCTAssertEqual(withTitle, again)
        XCTAssertEqual(withTitle.width, container.width)
        XCTAssertEqual(
            withTitle.height,
            container.height - PaneTitleBarGeometry.height,
            accuracy: 0.001
        )
        XCTAssertEqual(
            PaneTitleBarGeometry.clientContentSize(
                container: container,
                showsTitles: false
            ).height,
            container.height,
            accuracy: 0.001
        )
        XCTAssertFalse(
            PaneTitleBarGeometry.shouldSend(previous: (178, 49), next: (178, 49))
        )
        XCTAssertTrue(
            PaneTitleBarGeometry.shouldSend(previous: (178, 50), next: (178, 49))
        )
    }

    func testTitledHostTerminalFillsSpaceBelowTitle() {
        AppE2E.ensureApp()
        let terminal = MuxTerminalView(
            paneId: 11,
            frame: NSRect(x: 0, y: 0, width: 400, height: 200)
        )
        let host = PaneHostView(paneId: 11, title: "shell", terminal: terminal)
        host.setShowsTitleBar(true)
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

        XCTAssertEqual(
            host.titleBarFrameForTesting.height,
            PaneTitleBarGeometry.height,
            accuracy: 0.5
        )
        XCTAssertEqual(
            host.titleBarFrameForTesting.height + host.terminalHeightForTesting,
            host.bounds.height,
            accuracy: 1,
            "标题栏和终端必须一次铺满 host，底下不得再空一截"
        )
    }

    func testPaneTitleOffersSplitAndCloseActions() {
        AppE2E.ensureApp()
        let terminal = MuxTerminalView(
            paneId: 7,
            frame: NSRect(x: 0, y: 0, width: 400, height: 200)
        )
        let host = PaneHostView(paneId: 7, title: "shell", terminal: terminal)
        host.setShowsTitleBar(true)
        var actions: [(UInt32, PaneTitleAction)] = []
        host.onTitleAction = { actions.append(($0, $1)) }

        XCTAssertEqual(
            host.titleActionsForTesting,
            [.splitHorizontal, .splitVertical, .fullscreen, .cycleLayout, .close]
        )
        host.triggerTitleAction(.fullscreen)
        XCTAssertEqual(actions.count, 1)
        XCTAssertEqual(actions.first?.0, 7)
        XCTAssertEqual(actions.first?.1, .fullscreen)
        host.triggerTitleAction(.splitVertical)
        XCTAssertEqual(actions.last?.1, .splitVertical)
    }

    func testDraggingTitleOntoAnotherPaneRequestsSwap() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        defer { bridge.shutdown() }
        let layout = PaneLayoutView(terminalManager: TerminalManager(bridge: bridge))
        var swapped: (UInt32, UInt32)?
        var moved: UInt32?
        layout.onSwapPanes = { swapped = ($0, $1) }
        layout.onMovePaneToNewTab = { moved = $0 }
        layout.testHandlePaneDrag(1, to: 2)
        XCTAssertEqual(swapped?.0, 1)
        XCTAssertEqual(swapped?.1, 2)
        XCTAssertNil(moved)
        layout.testHandlePaneDrag(1, to: 1)
        XCTAssertNil(moved)
        layout.testHandlePaneDrag(1, to: nil)
        XCTAssertEqual(moved, 1)
    }

    func testPaneTitleMoveMenuPinsNewTabThenOtherTabs() {
        AppE2E.ensureApp()
        let terminal = MuxTerminalView(
            paneId: 4,
            frame: NSRect(x: 0, y: 0, width: 400, height: 200)
        )
        let host = PaneHostView(paneId: 4, title: "shell", terminal: terminal)
        host.setShowsTitleBar(true)
        host.moveDestinationsProvider = {
            [(nil, "New Tab"), (9, "logs"), (12, "main")]
        }
        XCTAssertEqual(
            host.titleActionsForTesting,
            [
                .splitHorizontal,
                .splitVertical,
                .fullscreen,
                .cycleLayout,
                .moveToNewTab,
                .moveToTab(9),
                .moveToTab(12),
                .close,
            ]
        )
    }

    func testPaneTitlesAppearOnlyForSplitTabs() throws {
        let single = OnePaneCat(label: "pane-title-single")
        let singleApp = try AppE2E.attachWindow(socket: single.socket, session: single.session)
        XCTAssertTrue(singleApp.waitReady(minLeaves: 1))
        let onlyPane = try XCTUnwrap(singleApp.testLayoutLeafIDs().first)
        XCTAssertFalse(singleApp.testPaneTitleVisible(onlyPane))
        singleApp.testShutdown()

        let split = TwoPaneCat(label: "pane-title-split")
        let splitApp = try AppE2E.attachWindow(socket: split.socket, session: split.session)
        defer { splitApp.testShutdown() }
        XCTAssertTrue(splitApp.waitReady(minLeaves: 2))
        XCTAssertTrue(
            splitApp.testLayoutLeafIDs().allSatisfy { splitApp.testPaneTitleVisible($0) },
            "多 pane tab 的每个 Surface 都应显示布局内标题条"
        )
        for pane in splitApp.testLayoutLeafIDs() {
            let title = try XCTUnwrap(splitApp.testPaneTitle(pane))
            XCTAssertFalse(title.isEmpty)
            XCTAssertFalse(title.hasPrefix("Pane @"), "标题应来自 Core PaneInfo，而不是 pane id")
            XCTAssertEqual(
                splitApp.testPaneAllocation(pane).height - splitApp.testPaneTerminalHeight(pane),
                22,
                accuracy: 1,
                "标题条必须占据独立布局行，不能悬浮覆盖 terminal"
            )
        }
        XCTAssertEqual(
            splitApp.testPaneLayoutFrame().height
                - splitApp.testTerminalClientContentSize().height,
            PaneTitleBarGeometry.height,
            accuracy: 1,
            "左右分屏必须用扣除标题栏后的高度计算 tmux client 字符格"
        )
    }

    func testBreakPaneCreatesNewTabWithoutExtraHierarchy() throws {
        let fx = TwoPaneCat(label: "break-p")
        let app = try AppE2E.attachWindow(socket: fx.socket, session: fx.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady(minLeaves: 2))

        let tabsBefore = app.testTabAndPaneCounts().tabs
        XCTAssertEqual(tabsBefore, 1, "夹具开始应是单 tab 两 pane")
        app.testBreakActivePaneToNewTab()
        XCTAssertTrue(
            AppE2E.wait(timeout: 5) {
                app.testPollOnce()
                AppE2E.pump(40)
                return app.testTabAndPaneCounts().tabs == 2
                    && app.testLayoutLeafIDs().count == 1
            },
            "break-pane 后必须变成 2 个 tab，且新 tab 布局完成。got=\(app.testTabAndPaneCounts()) leaves=\(app.testLayoutLeafIDs())"
        )
        XCTAssertEqual(
            app.testLayoutLeafIDs().count,
            1,
            "新 tab 应只有被拆出的那一个 pane。leaves=\(app.testLayoutLeafIDs())"
        )

        let windows = Tmux.out(
            socket: fx.socket,
            args: ["list-windows", "-t", fx.session, "-F", "#{window_id}"]
        )
        .split(whereSeparator: \.isNewline)
        .filter { !$0.isEmpty }
        XCTAssertEqual(windows.count, 2, "tmux 必须真的执行 break-pane，got=\(windows)")
    }
}
