import AppKit
import XCTest
@testable import MuxtermAppLib

/// W13：attach 已有 2tab/3pane，SwiftTerm 非空、几何 ≥ 40px、切 tab 像素还在、CUP 洪水有上界。
final class AttachE2ETests: XCTestCase {
    func testSizedAttachPublishesSurfaceSnapshotWithoutFrontendResize() throws {
        try assertSizedAttachPublishesSnapshot(
            size: (125, 51),
            label: "attach-sized"
        )
    }

    func testSmallSizedAttachPublishesSurfaceSnapshotWithoutFrontendResize() throws {
        try assertSizedAttachPublishesSnapshot(
            size: (42, 12),
            label: "attach-small"
        )
    }

    func testAttachPreexist2Tab3PaneIsUsable() throws {
        let painted = PaintedWorkspace(label: "gtk-attach")
        let app = try AppE2E.attachWindow(socket: painted.socket, session: painted.session)
        defer { app.testShutdown() }

        XCTAssertTrue(
            app.waitReady(minTabs: 2, minLeaves: 3),
            "attach 后应有 2 tab / 3 pane 布局，实际 \(app.testTabAndPaneCounts()) leaves=\(app.testLayoutLeafIDs())"
        )

        app.testFlushFeeds()
        for id in app.testLayoutLeafIDs() {
            let size = app.testPaneAllocation(id)
            XCTAssertGreaterThanOrEqual(
                size.width,
                AppE2E.minPanePx,
                "pane \(id) 宽 \(size.width) < \(AppE2E.minPanePx)px（白屏/错布局）"
            )
            XCTAssertGreaterThanOrEqual(
                size.height,
                AppE2E.minPanePx,
                "pane \(id) 高 \(size.height) < \(AppE2E.minPanePx)px（白屏/错布局）"
            )
        }

        let text = app.testAllVisibleTerminalText()
        XCTAssertFalse(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, "attach 后 SwiftTerm 不能空（1820 白屏）")
        for token in painted.tab1Tokens {
            XCTAssertTrue(app.waitTerminalContains(token), "可见 pane 应含播种 token \(token)。vte=\(app.testAllVisibleTerminalText())")
        }

        let tabs = app.testTabIDs()
        let current = app.testActiveTabID()
        let other = try XCTUnwrap(tabs.first { $0 != current }, "应有第二个 tab")
        app.testClickStatusTab(other)
        XCTAssertTrue(
            app.waitTerminalContains(painted.tab2Token),
            "切到 tab 2 应看到 \(painted.tab2Token)"
        )

        app.testClickStatusTab(current)
        for token in painted.tab1Tokens {
            XCTAssertTrue(
                app.waitTerminalContains(token),
                "切回 tab 1 像素缓存应仍有 \(token)"
            )
        }

        Tmux.ok(socket: painted.socket, args: [
            "respawn-pane", "-k", "-t", painted.tab1Panes[0],
            "bash -c 'for i in $(seq 1 \(AppE2E.cupFloodFrames)); do printf \"\\033[H\\033[2Jframe-%s\\n\" \"$i\"; done; printf \"FLOOD_DONE\\n\"; exec /bin/cat'",
        ])

        let start = Date()
        var outputEvents = 0
        while Date().timeIntervalSince(start) < 1 {
            outputEvents += app.testPollOutputEventCount()
            AppE2E.pump(16)
        }
        XCTAssertLessThanOrEqual(
            outputEvents,
            AppE2E.maxOutputEventsPerSec,
            "AppKit 1s 内 PaneOutput=\(outputEvents) > \(AppE2E.maxOutputEventsPerSec)（1820 CPU）"
        )

        XCTAssertTrue(
            AppE2E.wait(timeout: AppE2E.attachTimeout) {
                app.testPollOnce()
                app.testFlushFeeds()
                let t = app.testAllVisibleTerminalText()
                return t.contains("FLOOD_DONE") || t.contains("frame-")
            },
            "CUP 洪水后 SwiftTerm 应留下末帧，不能白屏"
        )
        XCTAssertFalse(
            app.testAllVisibleTerminalText().trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            "洪水后 SwiftTerm 仍不能空"
        )
    }

    private func assertSizedAttachPublishesSnapshot(
        size: (UInt16, UInt16),
        label: String
    ) throws {
        let fixture = OnePaneCat(label: label)
        let bridge = try CoreBridge.connect(
            backendType: "tmux",
            socket: fixture.socket,
            session: fixture.session,
            initialClientSize: size
        )
        defer { bridge.shutdown() }

        var snapshots: [StateChange] = []
        let received = AppE2E.wait(timeout: AppE2E.attachTimeout) {
            snapshots.append(contentsOf: bridge.pollEvents().filter(\.isPaneSnapshot))
            return snapshots.contains { event in
                !event.data.isEmpty &&
                    String(decoding: event.data, as: UTF8.self).contains(fixture.token)
            }
        }

        XCTAssertTrue(
            received,
            "带初始尺寸 \(size.0)x\(size.1) attach 后，即使前端不 resize，也必须发布含可见内容的 PaneSnapshot；实际 snapshots=\(snapshots.map { ($0.paneId, $0.data.count) })"
        )
    }
}
