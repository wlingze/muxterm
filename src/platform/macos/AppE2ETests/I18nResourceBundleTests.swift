import Foundation
import XCTest
@testable import MuxtermAppLib

/// 打包后的 Muxterm.app 没有 SPM 生成的 `Bundle.module` 回退路径时，
/// i18n 不得 fatalError（macOS 崩溃报告：`NSBundle.module` → `StatusBarView.init`）。
final class I18nResourceBundleTests: XCTestCase {
    private var scratch: URL!

    override func setUpWithError() throws {
        scratch = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-i18n-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        if let scratch {
            try? FileManager.default.removeItem(at: scratch)
        }
        scratch = nil
    }

    func testMissingResourceBundleFallsBackToKeyId() {
        let value = MuxtermI18nLocator.localizedString(
            key: .tabs,
            language: .english,
            roots: []
        )
        XCTAssertEqual(value, MuxtermTextKey.tabs.id)
    }

    func testLoadsFlatJsonFromSpmBundleBesideApp() throws {
        // SPM 生成的 accessor 查 Bundle.main.bundleURL + "MuxtermApp_MuxtermAppLib.bundle"
        // 即 Muxterm.app/MuxtermApp_MuxtermAppLib.bundle/en.json（扁平，无 i18n/ 子目录）。
        let app = scratch.appendingPathComponent("Muxterm.app", isDirectory: true)
        try FileManager.default.createDirectory(at: app, withIntermediateDirectories: true)
        let bundle = app.appendingPathComponent("MuxtermApp_MuxtermAppLib.bundle", isDirectory: true)
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        try #"{"tabs":"Tabs From SPM"}"#
            .write(to: bundle.appendingPathComponent("en.json"), atomically: true, encoding: .utf8)

        let value = MuxtermI18nLocator.localizedString(
            key: .tabs,
            language: .english,
            roots: [app]
        )
        XCTAssertEqual(value, "Tabs From SPM")
    }

    func testLoadsFlatJsonFromSpmBundleInContentsResources() throws {
        // 可签名布局：资源只能在 Contents/Resources，不能放在 .app 根目录。
        let app = scratch.appendingPathComponent("Muxterm.app", isDirectory: true)
        let bundle = app.appendingPathComponent(
            "Contents/Resources/MuxtermApp_MuxtermAppLib.bundle",
            isDirectory: true
        )
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        try #"{"tabs":"Tabs From Resources Bundle"}"#
            .write(to: bundle.appendingPathComponent("en.json"), atomically: true, encoding: .utf8)

        let value = MuxtermI18nLocator.localizedString(
            key: .tabs,
            language: .english,
            roots: [app, app.appendingPathComponent("Contents/Resources")]
        )
        XCTAssertEqual(value, "Tabs From Resources Bundle")
    }

    func testLoadsJsonFromContentsResourcesI18n() throws {
        let app = scratch.appendingPathComponent("Muxterm.app", isDirectory: true)
        let i18n = app.appendingPathComponent("Contents/Resources/i18n", isDirectory: true)
        try FileManager.default.createDirectory(at: i18n, withIntermediateDirectories: true)
        try #"{"tabs":"Tabs From Resources"}"#
            .write(to: i18n.appendingPathComponent("en.json"), atomically: true, encoding: .utf8)

        let value = MuxtermI18nLocator.localizedString(
            key: .tabs,
            language: .english,
            roots: [app, app.appendingPathComponent("Contents/Resources")]
        )
        XCTAssertEqual(value, "Tabs From Resources")
    }

    func testSearchRootsNeverRequireBundleModule() {
        // 只要调用不 trap 即通过：打包 app / swift test 都必须能拿到若干候选根目录。
        let roots = MuxtermI18nLocator.searchRoots()
        XCTAssertFalse(roots.isEmpty)
    }

    func testSharedTrFindsDevCatalogWithoutBundleModule() {
        let value = MuxtermI18n.shared.tr(.commandPalette)
        XCTAssertNotEqual(
            value,
            MuxtermTextKey.commandPalette.id,
            "swift test / 开发布局应能找到 catalog，不能静默退化成 key id"
        )
        XCTAssertFalse(value.isEmpty)
    }
}
