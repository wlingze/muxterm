import AppKit
import XCTest
@testable import MuxtermAppLib

/// Surface seed 的 AppKit 可见性契约：host 不能在 SwiftTerm 分块恢复期间
/// 暴露空白/半截帧，seed 与同期间 live catch-up 完成后才一次性显示。
final class SurfaceVisibilityE2ETests: XCTestCase {
    private func makeManager() throws -> (CoreBridge, TerminalManager) {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        return (bridge, TerminalManager(bridge: bridge))
    }

    func testHostStaysHiddenUntilSeedAndLiveCatchupFinish() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let view = MuxTerminalView(
            paneId: 1,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let host = PaneHostView(paneId: 1, terminal: view)
        manager.onSurfaceReadinessChanged = { _, ready in
            host.setSurfaceReady(ready)
        }
        host.setSurfaceReady(false)

        var seed = Data(repeating: 0x20, count: 96 * 1024)
        seed.append(contentsOf: Array("SEED_COMPLETE\r\n".utf8))
        manager.testQueueSurfaceSeed(paneId: 1, view: view, data: seed)
        manager.testQueueSurfaceLiveOutput(
            paneId: 1,
            data: Data("LIVE_CATCHUP\r\n".utf8)
        )

        XCTAssertTrue(host.isHidden, "seed 进行中不得显示 PaneHostView")
        XCTAssertFalse(manager.isSurfaceReady(for: 1))

        for _ in 0..<20 where !manager.isSurfaceReady(for: 1) {
            manager.testFlushSurfaceSeeds()
        }
        AppE2E.pump(20)

        XCTAssertTrue(manager.isSurfaceReady(for: 1))
        XCTAssertFalse(host.isHidden, "seed + live catch-up 完成后必须一次性显示")
        XCTAssertTrue(view.visibleScreenText().contains("LIVE_CATCHUP"))
    }

    func testEmptySurfaceSeedRevealsAsBlankFrame() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let view = MuxTerminalView(
            paneId: 2,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let host = PaneHostView(paneId: 2, terminal: view)
        manager.onSurfaceReadinessChanged = { _, ready in
            host.setSurfaceReady(ready)
        }
        host.setSurfaceReady(false)
        manager.testQueueSurfaceSeed(paneId: 2, view: view, data: Data())

        XCTAssertTrue(host.isHidden)
        manager.testFlushSurfaceSeeds()

        XCTAssertTrue(manager.isSurfaceReady(for: 2))
        XCTAssertFalse(host.isHidden, "空白 snapshot 也必须有可见的完整首帧")
    }

    func testMultipleSeedsRemainHiddenIndividuallyUntilComplete() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let first = MuxTerminalView(
            paneId: 3,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let second = MuxTerminalView(
            paneId: 4,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let firstHost = PaneHostView(paneId: 3, terminal: first)
        let secondHost = PaneHostView(paneId: 4, terminal: second)
        manager.onSurfaceReadinessChanged = { paneId, ready in
            switch paneId {
            case 3: firstHost.setSurfaceReady(ready)
            case 4: secondHost.setSurfaceReady(ready)
            default: break
            }
        }
        firstHost.setSurfaceReady(false)
        secondHost.setSurfaceReady(false)
        manager.testQueueSurfaceSeed(
            paneId: 3,
            view: first,
            data: Data(repeating: 0x41, count: 128 * 1024)
        )
        manager.testQueueSurfaceSeed(
            paneId: 4,
            view: second,
            data: Data("SECOND\r\n".utf8)
        )

        XCTAssertTrue(firstHost.isHidden)
        XCTAssertTrue(secondHost.isHidden)
        for _ in 0..<40 where !manager.isSurfaceReady(for: 3)
            || !manager.isSurfaceReady(for: 4)
        {
            manager.testFlushSurfaceSeeds()
        }

        XCTAssertTrue(manager.isSurfaceReady(for: 3))
        XCTAssertTrue(manager.isSurfaceReady(for: 4))
        XCTAssertFalse(firstHost.isHidden)
        XCTAssertFalse(secondHost.isHidden)
    }

    func testReadySurfaceReseedDoesNotHideHost() throws {
        let (bridge, manager) = try makeManager()
        defer { bridge.shutdown() }

        let view = MuxTerminalView(
            paneId: 1,
            frame: NSRect(x: 0, y: 0, width: 640, height: 360)
        )
        let host = PaneHostView(paneId: 1, terminal: view)
        manager.onSurfaceReadinessChanged = { _, ready in
            host.setSurfaceReady(ready)
        }
        host.setSurfaceReady(false)
        manager.testQueueSurfaceSeed(
            paneId: 1,
            view: view,
            data: Data("FIRST_FRAME\r\n".utf8)
        )
        for _ in 0..<10 where !manager.isSurfaceReady(for: 1) {
            manager.testFlushSurfaceSeeds()
        }
        AppE2E.pump(10)

        XCTAssertTrue(manager.isSurfaceReady(for: 1))
        XCTAssertFalse(host.isHidden)

        manager.handleSnapshot(paneId: 1, data: Data("SECOND_FRAME\r\n".utf8))
        XCTAssertTrue(
            manager.isSurfaceReady(for: 1),
            "已经在画的 Surface 再 seed 不得藏白"
        )
        XCTAssertFalse(host.isHidden, "切 tab / output-dropped 不得把已显示的 host 藏起来")

        for _ in 0..<10 {
            manager.testFlushSurfaceSeeds()
        }
        AppE2E.pump(10)

        XCTAssertTrue(manager.isSurfaceReady(for: 1))
        XCTAssertFalse(host.isHidden)
        XCTAssertTrue(view.visibleScreenText().contains("SECOND_FRAME"))
    }
}
