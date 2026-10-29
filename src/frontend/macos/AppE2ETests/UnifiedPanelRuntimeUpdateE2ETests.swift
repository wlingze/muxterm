import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

final class UnifiedPanelRuntimeUpdateE2ETests: XCTestCase {
    func testWorkspacePanelShowsUpdateOnlyForSupportedProviders() throws {
        AppE2E.ensureApp()
        let workspaces = [item("unsupported", supported: false), item("supported", supported: true)]
        let panel = makePanel { workspaces }
        defer { panel.dismiss() }
        panel.present()

        let supportedCell = try XCTUnwrap(panel.testWorkspaceCell(matching: "supported"))
        let button = try XCTUnwrap(findView(supportedCell, id: "muxterm.panel.workspace.update.supported") as? NSButton)
        XCTAssertFalse(button.isHidden)
        XCTAssertTrue(button.isEnabled)
        let unsupportedCell = try XCTUnwrap(panel.testWorkspaceCell(matching: "unsupported"))
        XCTAssertNil(findView(unsupportedCell, id: "muxterm.panel.workspace.update.unsupported"))
    }

    func testUpdateRoutesToWorkspaceWithoutActivatingOrClosingPanel() throws {
        AppE2E.ensureApp()
        let workspaces = [item("background", supported: true)]
        let panel = makePanel { workspaces }
        defer { panel.dismiss() }
        var requested: [String] = []
        var activated: [String] = []
        var closed: [String] = []
        panel.onWorkspaceUpdate = { requested.append($0) }
        panel.onWorkspaceActivate = { activated.append($0) }
        panel.onWorkspaceClose = { closed.append($0) }
        panel.present()

        let cell = try XCTUnwrap(panel.testWorkspaceCell(matching: "background"))
        let button = try XCTUnwrap(findView(cell, id: "muxterm.panel.workspace.update.background") as? NSButton)
        try click(button, in: panel)

        XCTAssertEqual(requested, ["background"])
        XCTAssertTrue(activated.isEmpty)
        XCTAssertTrue(closed.isEmpty)
        XCTAssertTrue(panel.window?.isVisible == true)
        XCTAssertTrue(panel.testWorkspaceCloseVisible(matching: "background"))
        let closeButton = try XCTUnwrap(findView(cell, id: "muxterm.panel.workspace.close.background") as? NSButton)
        cell.layoutSubtreeIfNeeded()
        let updateFrame = cell.convert(button.bounds, from: button)
        let closeFrame = cell.convert(closeButton.bounds, from: closeButton)
        XCTAssertEqual(updateFrame.width, 18, accuracy: 0.5)
        XCTAssertEqual(closeFrame.minX - updateFrame.maxX, 6, accuracy: 0.5)
        XCTAssertEqual(cell.bounds.width - closeFrame.maxX, 12, accuracy: 1)
        let title = try XCTUnwrap(cell.subviews.compactMap { $0 as? NSTextField }.first {
            $0.stringValue == "background"
        })
        let titleFrame = title.alignmentRect(forFrame: cell.convert(title.bounds, from: title))
        XCTAssertEqual(titleFrame.minX, 42, accuracy: 0.5)
        XCTAssertLessThanOrEqual(titleFrame.maxX, updateFrame.minX)
    }

    func testWorkspaceActionsStayOnTrailingEdgeWhenResized() throws {
        AppE2E.ensureApp()
        let workspaces = [item("supported", supported: true), item("unsupported", supported: false)]
        let panel = makePanel { workspaces }
        defer { panel.dismiss() }
        panel.present()
        let window = try XCTUnwrap(panel.window)

        for width: CGFloat in [480, 640, 920] {
            window.setContentSize(NSSize(width: width, height: 420))
            window.layoutIfNeeded()
            AppE2E.pump(40)
            let columnWidth = panel.testTableColumnWidth()
            for workspace in workspaces {
                let cell = try XCTUnwrap(panel.testWorkspaceCell(matching: workspace.name))
                cell.layoutSubtreeIfNeeded()
                XCTAssertEqual(cell.bounds.width, columnWidth, accuracy: 2)
                let close = try XCTUnwrap(findView(cell, id: "muxterm.panel.workspace.close.\(workspace.workspaceId)") as? NSButton)
                let closeFrame = cell.convert(close.bounds, from: close)
                XCTAssertEqual(cell.bounds.width - closeFrame.maxX, 12, accuracy: 1)
                XCTAssertEqual(closeFrame.width, 18, accuracy: 0.5)
                let title = try XCTUnwrap(cell.subviews.compactMap { $0 as? NSTextField }.first {
                    $0.stringValue == workspace.name
                })
                let titleFrame = title.alignmentRect(forFrame: cell.convert(title.bounds, from: title))
                XCTAssertEqual(titleFrame.minX, 42, accuracy: 0.5)
                XCTAssertLessThanOrEqual(titleFrame.maxX, closeFrame.minX)
                if workspace.canUpdateRuntime {
                    let update = try XCTUnwrap(findView(cell, id: "muxterm.panel.workspace.update.\(workspace.workspaceId)") as? NSButton)
                    let updateFrame = cell.convert(update.bounds, from: update)
                    XCTAssertEqual(closeFrame.minX - updateFrame.maxX, 6, accuracy: 0.5)
                    XCTAssertEqual(cell.bounds.width - updateFrame.maxX, 36, accuracy: 1)
                    XCTAssertLessThanOrEqual(titleFrame.maxX, updateFrame.minX)
                } else {
                    XCTAssertNil(findView(cell, id: "muxterm.panel.workspace.update.\(workspace.workspaceId)"))
                }
            }
        }
    }

    func testMouseClicksReachUpdateAndCloseWithoutActivatingWorkspace() throws {
        AppE2E.ensureApp()
        let workspaces = [item("supported", supported: true), item("unsupported", supported: false)]
        let panel = makePanel { workspaces }
        defer { panel.dismiss() }
        var requested: [String] = []
        var closed: [String] = []
        var activated: [String] = []
        panel.onWorkspaceUpdate = { requested.append($0) }
        panel.onWorkspaceClose = { closed.append($0) }
        panel.onWorkspaceActivate = { activated.append($0) }
        panel.present()

        let supportedCell = try XCTUnwrap(panel.testWorkspaceCell(matching: "supported"))
        let update = try XCTUnwrap(findView(supportedCell, id: "muxterm.panel.workspace.update.supported") as? NSButton)
        try click(update, in: panel)
        XCTAssertEqual(requested, ["supported"])
        XCTAssertTrue(closed.isEmpty)
        XCTAssertTrue(activated.isEmpty)
        XCTAssertTrue(panel.window?.isVisible == true)

        for workspace in workspaces {
            let cell = try XCTUnwrap(panel.testWorkspaceCell(matching: workspace.name))
            let close = try XCTUnwrap(findView(cell, id: "muxterm.panel.workspace.close.\(workspace.workspaceId)") as? NSButton)
            try click(close, in: panel)
        }
        XCTAssertEqual(closed, ["supported", "unsupported"])
        XCTAssertEqual(requested, ["supported"])
        XCTAssertTrue(activated.isEmpty)
        XCTAssertTrue(panel.window?.isVisible == true)
    }

    func testProgressDisablesUpdateAndFailureAllowsRetry() throws {
        AppE2E.ensureApp()
        var workspace = item("progress", supported: true)
        let panel = makePanel { [workspace] }
        defer { panel.dismiss() }
        var requested: [String] = []
        var activated: [String] = []
        panel.onWorkspaceUpdate = { requested.append($0) }
        panel.onWorkspaceActivate = { activated.append($0) }
        panel.present()

        for phase in ["checking", "installing", "handoff", "reconnecting"] {
            workspace = item("progress", supported: true, phase: phase)
            panel.refreshData()
            let cell = try XCTUnwrap(panel.testWorkspaceCell(matching: "progress"))
            let button = try XCTUnwrap(findView(cell, id: "muxterm.panel.workspace.update.progress") as? NSButton)
            XCTAssertFalse(button.isEnabled, phase)
            XCTAssertEqual(button.toolTip, phase)
            let spinner = try XCTUnwrap(button.superview?.subviews.compactMap { $0 as? NSProgressIndicator }.first)
            XCTAssertFalse(spinner.isHidden)
            XCTAssertTrue(cell.subviews.compactMap { $0 as? NSTextField }.contains { $0.stringValue.contains(phase) })
            try click(button, in: panel)
        }
        XCTAssertTrue(requested.isEmpty)
        XCTAssertTrue(activated.isEmpty)
        XCTAssertTrue(panel.window?.isVisible == true)

        workspace = item("progress", supported: true, phase: "failed")
        panel.refreshData()
        let cell = try XCTUnwrap(panel.testWorkspaceCell(matching: "progress"))
        let button = try XCTUnwrap(findView(cell, id: "muxterm.panel.workspace.update.progress") as? NSButton)
        XCTAssertTrue(button.isEnabled)
        XCTAssertEqual(button.contentTintColor, .systemOrange)
        try click(button, in: panel)
        XCTAssertEqual(requested, ["progress"])
    }

    func testCapabilityChangeHidesReusedUpdateButton() throws {
        AppE2E.ensureApp()
        var workspace = item("reused", supported: true)
        let panel = makePanel { [workspace] }
        defer { panel.dismiss() }
        panel.present()
        let cell = try XCTUnwrap(panel.testWorkspaceCell(matching: "reused"))
        let button = try XCTUnwrap(findView(cell, id: "muxterm.panel.workspace.update.reused") as? NSButton)
        workspace = item("reused", supported: false)
        panel.refreshData()
        let reusedCell = try XCTUnwrap(panel.testWorkspaceCell(matching: "reused"))
        XCTAssertNil(findView(reusedCell, id: "muxterm.panel.workspace.update.reused"))
        if cell === reusedCell {
            XCTAssertTrue(button.isHidden)
        }
    }

    func testMouseUpdateUsesCoreWorkspaceIdentityAndShowsDispatchErrors() throws {
        AppE2E.ensureApp()
        let config = try IsolatedMuxtermConfig(label: "runtime-update-dispatch", toml: "")
        defer { config.restore() }
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge, quickConnectStore: QuickConnectStore())
        defer { app.testShutdown() }
        app.testPollOnce()
        let coreWorkspace = try XCTUnwrap(bridge.workspaceList().first)
        let sidebarWorkspace = try XCTUnwrap(app.runtimeSidebarItems().first)
        XCTAssertNotEqual(sidebarWorkspace.workspaceId, coreWorkspace.id)

        let panel = makePanel {
            app.runtimeSidebarItems().map { workspace in
                WorkspaceSidebarItem(
                    workspaceId: workspace.workspaceId, name: workspace.name, runtime: workspace.runtime,
                    transport: workspace.transport, isActive: workspace.isActive, canUpdateRuntime: true,
                    runtimeUpdatePhase: workspace.runtimeUpdatePhase,
                    runtimeUpdateMessage: workspace.runtimeUpdateMessage
                )
            }
        }
        panel.onWorkspaceUpdate = app.unifiedPanel.onWorkspaceUpdate
        app.unifiedPanel.dismiss()
        app.unifiedPanel = panel
        panel.present()

        let cell = try XCTUnwrap(panel.testWorkspaceCell(matching: sidebarWorkspace.name))
        let buttonID = "muxterm.panel.workspace.update.\(sidebarWorkspace.workspaceId)"
        let button = try XCTUnwrap(findView(cell, id: buttonID) as? NSButton)
        try click(button, in: panel, pumpAfter: false)
        XCTAssertEqual(app.runtimeSidebarItems().first?.runtimeUpdatePhase, "checking")
        let queuedCount = app.testQueuedCommandCount()
        app.updateWorkspaceRuntime(sidebarWorkspace.workspaceId)
        XCTAssertEqual(app.testQueuedCommandCount(), queuedCount)
        let pendingCell = try XCTUnwrap(panel.testWorkspaceCell(matching: sidebarWorkspace.name))
        XCTAssertFalse(try XCTUnwrap(findView(pendingCell, id: buttonID) as? NSButton).isEnabled)

        app.testPollOnce()
        let failed = try XCTUnwrap(app.runtimeSidebarItems().first)
        XCTAssertEqual(failed.runtimeUpdatePhase, "failed")
        XCTAssertTrue(failed.runtimeUpdateMessage?.contains("not supported") == true,
                      "Core must reject the existing shell, not a misrouted/missing workspace: \(failed.runtimeUpdateMessage ?? "nil")")
        let failedCell = try XCTUnwrap(panel.testWorkspaceCell(matching: sidebarWorkspace.name))
        let retry = try XCTUnwrap(findView(failedCell, id: buttonID) as? NSButton)
        XCTAssertTrue(retry.isEnabled)
        XCTAssertEqual(retry.contentTintColor, .systemOrange)
        XCTAssertTrue(failedCell.subviews.compactMap { $0 as? NSTextField }.contains {
            $0.stringValue.contains("not supported")
        })
        XCTAssertTrue(panel.window?.isVisible == true)
        XCTAssertEqual(bridge.workspaceList().first?.id, coreWorkspace.id)

        try click(retry, in: panel, pumpAfter: false)
        XCTAssertEqual(app.runtimeSidebarItems().first?.runtimeUpdatePhase, "checking")
        app.testPollOnce()
        XCTAssertEqual(app.runtimeSidebarItems().first?.runtimeUpdatePhase, "failed")
    }

    func testWorkspacePanelSnapshot() throws {
        AppE2E.ensureApp()
        var workspaces = [
            WorkspaceSidebarItem(
                workspaceId: AggregateWorkspaceIdentity.agents, name: "Agents", runtime: "aggregate",
                transport: "all workspaces", isActive: false, isClosable: false
            ),
            WorkspaceSidebarItem(
                workspaceId: "yaklang-workspace:w4@ryzen", name: "yaklang-workspace", runtime: "herdr",
                transport: "ryzen", isActive: false, shortcut: 1, canUpdateRuntime: true
            ),
            WorkspaceSidebarItem(
                workspaceId: "muxterm@local", name: "muxterm", runtime: "tmux", transport: "local",
                isActive: true, shortcut: 2
            ),
        ]
        let panel = makePanel { workspaces }
        defer { panel.dismiss() }
        panel.present()
        AppE2E.pump(40)
        try writeSnapshot(panel, name: "workspace-actions")
        for phase in ["installing", "failed"] {
            workspaces[1] = WorkspaceSidebarItem(
                workspaceId: "yaklang-workspace:w4@ryzen", name: "yaklang-workspace", runtime: "herdr",
                transport: "ryzen", isActive: false, shortcut: 1, canUpdateRuntime: true,
                runtimeUpdatePhase: phase, runtimeUpdateMessage: "\(phase) (test fixture)"
            )
            panel.refreshData()
            AppE2E.pump(40)
            try writeSnapshot(panel, name: "workspace-actions-\(phase)")
        }
        panel.window?.setContentSize(NSSize(width: 480, height: 420))
        AppE2E.pump(40)
        try writeSnapshot(panel, name: "workspace-actions-narrow")
    }

    private func writeSnapshot(_ panel: UnifiedPanelController, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["MUXTERM_UI_SNAPSHOT_DIR"] else { return }
        let view = try XCTUnwrap(panel.window?.contentView)
        view.layoutSubtreeIfNeeded()
        view.displayIfNeeded()
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try png.write(to: root.appendingPathComponent("\(name).png"), options: .atomic)
    }

    private func item(_ id: String, supported: Bool, phase: String? = nil) -> WorkspaceSidebarItem {
        WorkspaceSidebarItem(
            workspaceId: id, name: id, runtime: "test-provider", transport: "ssh", isActive: false,
            canUpdateRuntime: supported, runtimeUpdatePhase: phase, runtimeUpdateMessage: phase
        )
    }

    private func makePanel(workspaces: @escaping () -> [WorkspaceSidebarItem]) -> UnifiedPanelController {
        UnifiedPanelController(
            store: QuickConnectStore(), ownerWindow: nil, snapshot: { nil }, sendInput: { _, _ in },
            search: { _ in }, sidebarWorkspaces: workspaces
        )
    }

    private func findView(_ root: NSView, id: String) -> NSView? {
        if root.accessibilityIdentifier() == id { return root }
        return root.subviews.lazy.compactMap { self.findView($0, id: id) }.first
    }

    private func click(_ button: NSButton, in panel: UnifiedPanelController, pumpAfter: Bool = true) throws {
        let window = try XCTUnwrap(panel.window)
        let content = try XCTUnwrap(window.contentView)
        window.layoutIfNeeded()
        content.layoutSubtreeIfNeeded()
        let location = button.convert(NSPoint(x: button.bounds.midX, y: button.bounds.midY), to: nil)
        let hitPoint = content.superview?.convert(location, from: nil) ?? location
        let hit = content.hitTest(hitPoint)
        XCTAssertTrue(hit === button, "Mouse must hit \(button.accessibilityIdentifier()), not \(String(describing: hit))")
        guard hit === button else { return }
        let timestamp = ProcessInfo.processInfo.systemUptime
        let down = try XCTUnwrap(NSEvent.mouseEvent(
            with: .leftMouseDown, location: location, modifierFlags: [], timestamp: timestamp,
            windowNumber: window.windowNumber, context: nil, eventNumber: 1, clickCount: 1, pressure: 1
        ))
        let up = try XCTUnwrap(NSEvent.mouseEvent(
            with: .leftMouseUp, location: location, modifierFlags: [], timestamp: timestamp + 0.01,
            windowNumber: window.windowNumber, context: nil, eventNumber: 2, clickCount: 1, pressure: 0
        ))
        NSApp.postEvent(up, atStart: true)
        window.sendEvent(down)
        if pumpAfter {
            AppE2E.pump(20)
        }
    }
}
