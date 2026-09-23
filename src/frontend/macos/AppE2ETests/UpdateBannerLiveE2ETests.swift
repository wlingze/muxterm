import AppKit
import Foundation
import XCTest
@testable import MuxtermAppLib

/// 更新提醒条的 AppKit GUI e2e：真实 Core 状态机 + 真实 HTTP + 真实更新包，
/// 走生产路径（MainWindowController → ContentView → UpdateBannerView）。
///
/// 需要一个只在 loopback 上服务的发布目录（`scripts/release-package.sh`
/// 的产物），其中 `latest.json` 的资产 URL 是相对路径：
///
/// ```bash
/// VERSION=v0.2.0-alpha.1 CHANNEL=alpha PRERELEASE=true \
///   bash scripts/release-package.sh /tmp/muxterm-feed
/// python3 -m http.server 8231 --bind 127.0.0.1 --directory /tmp/muxterm-feed
/// MUXTERM_UPDATE_TEST_FEED=http://127.0.0.1:8231 \
/// MUXTERM_UPDATE_VERSION=v0.1.0 \
/// MUXTERM_UI_SNAPSHOT_DIR=/tmp/muxterm-update-banner-snapshots \
///   swift test --disable-swift-testing --filter UpdateBannerLiveE2ETests
/// ```
///
/// 未设置 `MUXTERM_UPDATE_TEST_FEED` 时整组跳过：默认 CI 不依赖网络、
/// 本地端口与已发布的 release。
final class UpdateBannerLiveE2ETests: XCTestCase {
    /// feed 根地址（不含 `/latest.json`）。
    private static var feedBase: String? {
        guard let raw = ProcessInfo.processInfo.environment["MUXTERM_UPDATE_TEST_FEED"],
              !raw.trimmingCharacters(in: .whitespaces).isEmpty
        else { return nil }
        return raw.hasSuffix("/") ? String(raw.dropLast()) : raw
    }

    /// 当前平台是否有对应资产；没有（例如 Linux ARM）时，跳过该用例。
    private static var platformAssetKey: String? {
        #if arch(arm64)
        #if os(macOS)
        return "macos-arm64"
        #else
        return nil
        #endif
        #elseif arch(x86_64)
        #if os(macOS)
        return nil
        #else
        return "linux-gui-x86_64"
        #endif
        #else
        return nil
        #endif
    }

    private func requireFeed() throws -> String {
        guard let base = Self.feedBase else {
            throw XCTSkip("需要 MUXTERM_UPDATE_TEST_FEED 指向本地发布 feed")
        }
        guard Self.platformAssetKey != nil else {
            throw XCTSkip("当前平台没有发布产物，更新链路不适用")
        }
        guard let manifest = URL(string: "\(base)/latest.json"),
              let text = try? String(contentsOf: manifest, encoding: .utf8),
              text.contains("version")
        else {
            throw XCTSkip("feed 不可达或缺少 latest.json: \(base)")
        }
        return base
    }

    /// 设置测试专用环境变量，并在用例结束时恢复调用方原值。
    private func overrideEnvironment(_ values: [String: String]) -> () -> Void {
        let previous = values.map { key, value in
            (key, getenv(key).map { String(cString: $0) }, value)
        }
        for (key, _, value) in previous {
            setenv(key, value, 1)
        }
        return {
            for (key, oldValue, _) in previous {
                if let oldValue {
                    setenv(key, oldValue, 1)
                } else {
                    unsetenv(key)
                }
            }
        }
    }

    /// 有设置快照目录时输出窗口 PNG，供人工检查真实 AppKit 布局。
    private func writeSnapshot(_ view: NSView?, name: String) throws {
        let environment = ProcessInfo.processInfo.environment
        guard let directory = environment["MUXTERM_UI_SNAPSHOT_DIR"],
              let view,
              !view.bounds.isEmpty
        else { return }
        view.layoutSubtreeIfNeeded()
        guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else { return }
        view.displayIfNeeded()
        view.cacheDisplay(in: view.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else { return }
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try png.write(to: root.appendingPathComponent("\(name).png"), options: .atomic)
    }

    /// 隔离配置：把 Core 的 [update] 指到本地 feed，且不动用户真实配置。
    private func makeIsolatedConfig(feed: String) throws -> IsolatedMuxtermConfig {
        try IsolatedMuxtermConfig(
            label: "update-banner",
            toml: """
            config_version = 1

            [update]
            auto_check = true
            manifest_url = "\(feed)/latest.json"
            """
        )
    }

    /// 真实窗口流程：启动即自动检查 → 提醒条出现 → 文案/按钮来自 Core → 一键更新 → 待重启。
    func testBannerAppearsFromCoreThenOneClickInstalls() throws {
        let feed = try requireFeed()
        let config = try makeIsolatedConfig(feed: feed)
        defer { config.restore() }

        // 更新目标必须是沙箱，避免真的替换本机安装的 Muxterm.app。
        let sandbox = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-update-banner-\(ProcessInfo.processInfo.processIdentifier)")
        let appTarget = sandbox.appendingPathComponent("Muxterm.app")
        let installedBinary = appTarget.appendingPathComponent("Contents/MacOS/Muxterm")
        try FileManager.default.createDirectory(
            at: installedBinary.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try Data("old-version".utf8).write(to: installedBinary)
        defer { try? FileManager.default.removeItem(at: sandbox) }

        // 当前版本低于 feed 才会触发更新；更新包和缓存都放进沙箱。
        let restoreEnvironment = overrideEnvironment([
            "MUXTERM_INSTALL_TARGET": appTarget.path,
            "XDG_CACHE_HOME": sandbox.appendingPathComponent("cache").path,
            "MUXTERM_UPDATE_VERSION": "v0.0.1",
            "MUXTERM_UPDATE_MANIFEST_URL": "\(feed)/latest.json",
        ])
        defer { restoreEnvironment() }

        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge)
        defer { app.testShutdown() }
        app.showWindow(nil)
        app.window?.setFrame(
            AppE2E.fixedWindowFrame(width: 1000, height: 700),
            display: true
        )
        AppE2E.pump(120)

        // 1) 自动检查：Core 发现新版本后提醒条必须自己出现（不需要点菜单）。
        // 断言对象是渲染结果本身；Core 状态只在之后对账，避免用「测试自己
        // 多 poll 了一次」把 banner 还没渲染的中间态当成功。
        let appeared = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            AppE2E.pump(30)
            return app.testUpdateBannerVisible() && app.testUpdateBannerActionEnabled()
        }
        let status = try XCTUnwrap(
            app.testUpdateStatus(),
            "必须能从 Core 读到更新状态"
        )
        XCTAssertEqual(
            status.phase,
            "available",
            "Core 应发现比当前版本更新的发布"
        )
        XCTAssertTrue(
            appeared,
            "自动检查后提醒条应显示且按钮可点击；"
                + "文案=\(app.testUpdateBannerMessage())，"
                + "标题=\(app.testUpdateBannerActionTitle())"
        )
        let version = try XCTUnwrap(status.version, "available 状态必须带版本号")
        XCTAssertTrue(
            app.testUpdateBannerMessage().contains(version),
            "提醒条文案必须来自 Core 的清单版本，实际=\(app.testUpdateBannerMessage())"
        )
        XCTAssertEqual(
            app.testUpdateBannerActionTitle(),
            MuxtermI18n.shared.tr(.updateInstallNow),
            "提醒条必须给出「一键更新」入口"
        )
        XCTAssertTrue(
            app.testUpdateBannerActionEnabled(),
            "有更新时必须可以一键更新"
        )
        XCTAssertNotNil(status.downloadURL, "Core 必须给出解析后的下载地址")
        try writeSnapshot(app.window?.contentView, name: "update-banner-available")

        // 2) 一键更新：点真实按钮，Core 下载 → 校验 → 安装。
        XCTAssertTrue(app.testClickUpdateAction(), "必须能点到一键更新按钮")
        let restartText = MuxtermI18n.shared.tr(.updateInstalledRestart)
        let installed = AppE2E.wait(timeout: 120) {
            app.testPollOnce()
            AppE2E.pump(30)
            return app.testUpdateBannerMessage() == restartText
        }
        let final = try XCTUnwrap(app.testUpdateStatus())
        XCTAssertTrue(
            installed,
            "一键更新应当在超时内完成，实际 phase=\(final.phase) message=\(final.message ?? "-")"
        )
        XCTAssertTrue(final.restartRequired, "安装完成后必须提示重启")
        XCTAssertTrue(
            app.testUpdateBannerVisible(),
            "安装完成后提醒条要保留，告诉用户重启生效"
        )
        XCTAssertEqual(
            app.testUpdateBannerMessage(),
            MuxtermI18n.shared.tr(.updateInstalledRestart)
        )
        XCTAssertFalse(
            app.testUpdateBannerActionEnabled(),
            "已完成安装后不得再给可点的更新按钮"
        )

        // 3) 目标确实被真实 DMG 内容替换（不是只改了状态）。
        let replaced = try Data(contentsOf: installedBinary)
        XCTAssertNotEqual(
            replaced,
            Data("old-version".utf8),
            "安装目标必须被 DMG 内容替换；"
                + "实际字节数=\(replaced.count)"
        )
        let backup = sandbox.appendingPathComponent("Muxterm.app.backup")
        XCTAssertTrue(
            FileManager.default.fileExists(atPath: backup.path),
            "替换 .app 前必须保留旧 bundle 备份"
        )
        try writeSnapshot(app.window?.contentView, name: "update-banner-installed")
    }

    /// 手动检查失败时必须把原因透出给用户；自动检查失败保持静默。
    func testManualCheckSurfacesFailureInBanner() throws {
        guard Self.platformAssetKey != nil else {
            throw XCTSkip("当前平台没有发布产物，更新链路不适用")
        }
        // 指向一个必然不可达的端口：手动检查必须失败可见。
        let config = try IsolatedMuxtermConfig(
            label: "update-banner-failure",
            toml: """
            config_version = 1

            [update]
            auto_check = false
            manifest_url = "http://127.0.0.1:9/latest.json"
            """
        )
        defer { config.restore() }
        let restoreEnvironment = overrideEnvironment([
            "MUXTERM_UPDATE_VERSION": "v0.0.1",
            "MUXTERM_UPDATE_MANIFEST_URL": "http://127.0.0.1:9/latest.json",
        ])
        defer { restoreEnvironment() }

        AppE2E.ensureApp()
        let bridge = try CoreBridge(backendType: "local")
        let app = MainWindowController(bridge: bridge)
        defer { app.testShutdown() }
        app.showWindow(nil)
        AppE2E.pump(100)

        XCTAssertFalse(
            app.testUpdateBannerVisible(),
            "还没检查时不应出现提醒条"
        )
        _ = app.testCheckForUpdates()

        var latest: CoreBridge.UpdateStatus?
        let failed = AppE2E.wait(timeout: AppE2E.featureTimeout) {
            app.testPollOnce()
            AppE2E.pump(30)
            guard let status = app.testUpdateStatus() else { return false }
            latest = status
            return status.phase == "failed"
        }
        let status = try XCTUnwrap(latest)
        XCTAssertTrue(
            failed,
            "手动检查失败必须进入 failed，实际 \(status.phase)"
        )
        XCTAssertTrue(app.testUpdateBannerVisible(), "失败必须让用户看见")
        XCTAssertFalse(
            app.testUpdateBannerMessage().isEmpty,
            "失败原因必须透出到提醒条"
        )
        XCTAssertTrue(app.testUpdateBannerActionEnabled(), "失败后必须能重试")
        XCTAssertEqual(
            app.testUpdateBannerActionTitle(),
            MuxtermI18n.shared.tr(.updateRetry)
        )
    }
}
