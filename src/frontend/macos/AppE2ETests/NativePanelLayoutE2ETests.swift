import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

/// iTerm2 风格的 macOS 浮层应紧凑、原生，并始终收在 owner window 内。
final class NativePanelLayoutE2ETests: XCTestCase {
    func testCompactPanelGeometryClampsToOwnerContent() {
        XCTAssertEqual(
            CompactPanelLayout.contentSize(
                preferred: NSSize(width: 640, height: 420),
                available: nil
            ),
            NSSize(width: 640, height: 420)
        )
        XCTAssertEqual(
            CompactPanelLayout.contentSize(
                preferred: NSSize(width: 640, height: 420),
                available: NSSize(width: 480, height: 320)
            ),
            NSSize(width: 456, height: 296)
        )
    }

    func testCommandPaletteUsesCompactNativeMetricsAndEmptyState() {
        AppE2E.ensureApp()
        let panel = CommandPaletteController(ownerWindow: nil)
        panel.present(items: [
            PaletteItem(
                title: "New Tab",
                detail: "Create a local tab",
                keywords: "new tab",
                kind: .command(.newTab)
            ),
        ])
        defer { panel.dismiss() }
        AppE2E.pump(40)
        try? writeSnapshot(panel.window, name: "command-palette")

        let size = panel.testContentSize()
        XCTAssertLessThanOrEqual(size.width, 600)
        XCTAssertLessThanOrEqual(size.height, 360)
        XCTAssertLessThanOrEqual(panel.testSearchFontSize(), 15)
        XCTAssertLessThanOrEqual(panel.testRowHeight(), 38)

        panel.testSetQuery("no matching command")
        XCTAssertTrue(panel.testEmptyStateVisible())
        XCTAssertFalse(panel.testEmptyStateText().isEmpty)
    }

    func testUnifiedPanelUsesCompactNativeMetricsAndContextualEmptyState() {
        AppE2E.ensureApp()
        let store = QuickConnectStore()
        store.upsertProject(TargetConfig(
            name: "workspace",
            runtime: .tmux,
            transport: .local,
            path: "~/Developer"
        ))
        let panel = UnifiedPanelController(
            store: store,
            ownerWindow: nil,
            snapshot: { AttentionSnapshot(blockedCount: 0, workspaces: []) },
            paneOutput: { _ in Data() },
            sendInput: { _, _ in },
            search: { _, _ in [] }
        )
        panel.present(initial: .workspaces)
        defer { panel.dismiss() }
        AppE2E.pump(40)
        try? writeSnapshot(panel.window, name: "unified-panel")
        panel.present(initial: .search)
        AppE2E.pump(20)

        let size = panel.testContentSize()
        XCTAssertLessThanOrEqual(size.width, 640)
        XCTAssertLessThanOrEqual(size.height, 420)
        XCTAssertLessThanOrEqual(panel.testSearchFontSize(), 15)
        XCTAssertLessThanOrEqual(panel.testRowHeight(), 50)
        XCTAssertTrue(panel.testUsesSegmentedNavigation())

        panel.testSetQuery("missing token")
        XCTAssertTrue(panel.testEmptyStateVisible())
        XCTAssertFalse(panel.testEmptyStateText().isEmpty)
    }

    /// 设置 `MUXTERM_UI_SNAPSHOT_DIR` 时输出 AppKit 位图，供人工视觉 QA；
    /// 默认测试只做内存布局验证，不写文件。
    private func writeSnapshot(_ window: NSWindow?, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["MUXTERM_UI_SNAPSHOT_DIR"],
              let view = window?.contentView,
              !view.bounds.isEmpty,
              let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else {
            return
        }
        view.layoutSubtreeIfNeeded()
        view.displayIfNeeded()
        view.cacheDisplay(in: view.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else { return }
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try png.write(to: root.appendingPathComponent("\(name).png"), options: .atomic)
    }
}
