import AppKit
import Foundation
import XCTest
@testable import MuxtermAppLib

/// 对标 Linux 分类 Preferences：分类由 Core Manifest 驱动，右侧只显示当前页。
final class SettingsWindowE2ETests: XCTestCase {
    func testManifestCategoriesDriveSidebarAndSelection() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let settings = SettingsWindowController(bridge: bridge)
        defer {
            settings.window?.orderOut(nil)
            bridge.shutdown()
        }

        settings.showWindow(nil)
        AppE2E.pump(50)

        let expected = try manifestGroupIDs(bridge)
        XCTAssertFalse(expected.isEmpty, "Core Manifest 必须提供设置分类")
        XCTAssertEqual(settings.testCategoryIDs(), expected)
        XCTAssertEqual(settings.testVisibleCategoryIDs(), expected)
        XCTAssertEqual(settings.testSelectedCategoryID(), expected.first)
        XCTAssertNotNil(findView(settings.window?.contentView, id: "muxterm.settings.categories"))
        XCTAssertNotNil(findView(settings.window?.contentView, id: "muxterm.settings.pages"))

        let runtime = try XCTUnwrap(expected.first(where: { $0 == "runtime" }))
        settings.testSelectCategory(runtime)
        XCTAssertEqual(settings.testSelectedCategoryID(), runtime)
        XCTAssertEqual(settings.testVisiblePageID(), runtime)
        XCTAssertNotNil(
            findView(settings.window?.contentView, id: "muxterm.settings.page.\(runtime)"),
            "每个 Manifest 分类页必须有稳定 accessibility identifier"
        )
    }

    func testSearchSwitchesCategoryWithoutDiscardingDraft() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let settings = SettingsWindowController(bridge: bridge)
        defer {
            settings.window?.orderOut(nil)
            bridge.shutdown()
        }

        settings.showWindow(nil)
        AppE2E.pump(50)

        let sessionField = try XCTUnwrap(settings.testTextField(path: "/tmux/default_session"))
        let draft = "sidebar-draft-\(UUID().uuidString)"
        sessionField.stringValue = draft

        settings.testSelectCategory("appearance")
        settings.testSelectCategory("runtime")
        XCTAssertEqual(settings.testTextField(path: "/tmux/default_session")?.stringValue, draft)

        settings.testSetSearchQuery("default_session")
        XCTAssertEqual(settings.testVisibleCategoryIDs(), ["runtime"])
        XCTAssertEqual(settings.testSelectedCategoryID(), "runtime")
        XCTAssertEqual(settings.testVisiblePageID(), "runtime")
        XCTAssertEqual(settings.testTextField(path: "/tmux/default_session")?.stringValue, draft)

        settings.testSetSearchQuery("")
        XCTAssertTrue(settings.testVisibleCategoryIDs().contains("appearance"))
        XCTAssertEqual(settings.testTextField(path: "/tmux/default_session")?.stringValue, draft)
    }

    func testSidebarKeepsFixedWidthWhenWindowResizes() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let settings = SettingsWindowController(bridge: bridge)
        defer {
            settings.window?.orderOut(nil)
            bridge.shutdown()
        }

        settings.showWindow(nil)
        settings.window?.setContentSize(NSSize(width: 760, height: 560))
        settings.window?.layoutIfNeeded()
        let compactWidth = settings.testSidebarWidth()

        settings.window?.setContentSize(NSSize(width: 980, height: 720))
        settings.window?.layoutIfNeeded()
        let expandedWidth = settings.testSidebarWidth()

        XCTAssertEqual(compactWidth, 180, accuracy: 1)
        XCTAssertEqual(expandedWidth, compactWidth, accuracy: 1)
        XCTAssertTrue(settings.testVisiblePageIsScrollable())
    }

    func testSettingsControlsFillAvailablePageWidth() throws {
        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let settings = SettingsWindowController(bridge: bridge)
        defer {
            settings.window?.orderOut(nil)
            bridge.shutdown()
        }

        settings.showWindow(nil)
        settings.window?.setContentSize(NSSize(width: 980, height: 720))
        settings.window?.layoutIfNeeded()

        for (category, path) in [
            ("runtime", "/tmux/default_session"),
            ("appearance", "/font/family"),
            ("appearance", "/theme/name"),
            ("attention", "/attention/blocked_regex"),
            ("projects", "/projects"),
        ] {
            settings.testSelectCategory(category)
            settings.window?.contentView?.layoutSubtreeIfNeeded()
            let page = try XCTUnwrap(
                findView(settings.window?.contentView, id: "muxterm.settings.page.\(category)")
                    as? NSScrollView
            )
            let control = try XCTUnwrap(settings.testControl(path: path))
            page.contentView.layoutSubtreeIfNeeded()
            let controlFrame = control.convert(control.bounds, to: page.contentView)
            let rightGap = page.contentView.bounds.maxX - controlFrame.maxX
            XCTAssertLessThan(
                rightGap,
                80,
                "\(category) 页的 \(path) 控件右侧不应保留大块空白（gap=\(rightGap)）"
            )
        }
    }

    func testCategoryTitleHumanizesManifestKey() {
        XCTAssertEqual(
            settingsCategoryTitle(id: "appearance", titleKey: "settings.appearance"),
            "Appearance"
        )
        XCTAssertEqual(settingsCategoryTitle(id: "tab_bar", titleKey: ""), "Tab Bar")
    }

    func testProjectsPageEditsAndPersistsProjectFromGUI() throws {
        AppE2E.ensureApp()
        let config = try IsolatedMuxtermConfig(
            label: "settings-project-edit",
            toml: Self.projectConfig(name: "before", path: "/tmp/before")
        )
        let bridge = try CoreBridge(backendType: "local")
        let settings = SettingsWindowController(bridge: bridge)
        defer {
            settings.window?.orderOut(nil)
            bridge.shutdown()
            config.restore()
        }

        settings.showWindow(nil)
        AppE2E.pump(50)
        settings.testSelectCategory("projects")

        XCTAssertTrue(settings.testProjectEditorVisible())
        XCTAssertEqual(settings.testProjectNames(), ["before"])
        XCTAssertTrue(settings.testHasNewProjectButton())
        XCTAssertFalse(settings.testProjectEditorContainsPlaceholder())

        settings.testOpenProjectEditor(at: 0)
        let editor = try XCTUnwrap(settings.testActiveTargetConfigWindow())
        editor.testSetName("after")
        editor.testSetPath("/tmp/after")
        editor.testSave()
        AppE2E.pump(50)

        XCTAssertEqual(settings.testProjectNames(), ["after"])
        let saved = try String(contentsOf: config.configURL, encoding: .utf8)
        XCTAssertTrue(saved.contains("name = \"after\""))
        XCTAssertTrue(saved.contains("path = \"/tmp/after\""))
        let projectNames = try Self.projectNames(in: try XCTUnwrap(bridge.configDescribeJSON()))
        XCTAssertTrue(
            projectNames.contains("after"),
            "GUI 保存必须更新同一个 Core Catalog 的配置快照"
        )
    }

    func testProjectsPageCreatesProjectAndRefreshesList() throws {
        AppE2E.ensureApp()
        let config = try IsolatedMuxtermConfig(
            label: "settings-project-new",
            toml: Self.projectConfig(name: "existing", path: "/tmp/existing")
        )
        let bridge = try CoreBridge(backendType: "local")
        let settings = SettingsWindowController(bridge: bridge)
        defer {
            settings.window?.orderOut(nil)
            bridge.shutdown()
            config.restore()
        }

        settings.showWindow(nil)
        AppE2E.pump(50)
        settings.testSelectCategory("projects")
        settings.testOpenNewProjectEditor()

        let editor = try XCTUnwrap(settings.testActiveTargetConfigWindow())
        editor.testSetName("created")
        editor.testSetPath("/tmp/created")
        editor.testSave()
        AppE2E.pump(50)

        XCTAssertEqual(settings.testProjectNames(), ["existing", "created"])
        let projectNames = try Self.projectNames(in: try XCTUnwrap(bridge.configDescribeJSON()))
        XCTAssertTrue(projectNames.contains("created"))
    }

    private static func projectConfig(name: String, path: String) -> String {
        """
        [[projects]]
        id = "\(name)@local"
        name = "\(name)"
        path = "\(path)"

        [projects.runtime]
        id = "shell"

        [projects.transport]
        id = "local"
        target = ""
        """
    }

    private static func projectNames(in json: String) throws -> [String] {
        let envelope = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
        )
        let payload = try XCTUnwrap(envelope["data"] as? [String: Any])
        let values = try XCTUnwrap(payload["values"] as? [String: Any])
        let projects = try XCTUnwrap(values["projects"] as? [[String: Any]])
        return projects.compactMap { $0["name"] as? String }
    }

    private func manifestGroupIDs(_ bridge: CoreBridge) throws -> [String] {
        let text = try XCTUnwrap(bridge.configDescribeJSON())
        let envelope = try XCTUnwrap(
            try JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any]
        )
        let payload = try XCTUnwrap(envelope["data"] as? [String: Any])
        let manifest = try XCTUnwrap(payload["manifest"] as? [String: Any])
        let groups = try XCTUnwrap(manifest["groups"] as? [[String: Any]])
        return groups.compactMap { group in
            guard let id = group["id"] as? String, !id.trimmingCharacters(in: .whitespaces).isEmpty else {
                return nil
            }
            return id
        }
    }

    private func findView(_ root: NSView?, id: String) -> NSView? {
        guard let root else { return nil }
        if root.accessibilityIdentifier() == id { return root }
        for child in root.subviews {
            if let match = findView(child, id: id) {
                return match
            }
        }
        return nil
    }
}
