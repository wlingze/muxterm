import AppKit
import XCTest
@testable import MuxtermAppLib

/// 更新提醒卡片：Core 给什么状态，前端就渲染什么。
/// 这些用例只校验「状态 → 文案/按钮」的映射，不触发下载与安装。
final class UpdateBannerPresentationTests: XCTestCase {
    private func status(
        phase: String,
        version: String? = nil,
        message: String? = nil
    ) -> CoreBridge.UpdateStatus {
        CoreBridge.UpdateStatus(
            phase: phase,
            currentVersion: "0.1.0",
            version: version,
            message: message,
            releaseURL: nil,
            downloadURL: nil,
            assetName: nil,
            restartRequired: false
        )
    }

    func testIdleAndUpToDateStayHidden() {
        for phase in ["idle", "up_to_date"] {
            let presentation = UpdateBannerPresentation.make(status: status(phase: phase))
            XCTAssertFalse(presentation.visible, "\(phase) 不应显示提醒卡")
            XCTAssertFalse(presentation.actionEnabled, "\(phase) 不应给出按钮")
        }
    }

    func testAvailableOffersOneClickInstallAndBothDismissals() {
        let presentation = UpdateBannerPresentation.make(
            status: status(phase: "available", version: "0.2.0")
        )
        XCTAssertTrue(presentation.visible)
        XCTAssertTrue(presentation.actionEnabled, "有更新时必须可以一键更新")
        let i18n = MuxtermI18n.shared
        XCTAssertEqual(presentation.title, i18n.tr(.updateAvailable))
        XCTAssertEqual(
            presentation.message,
            i18n.tr(.updateAvailableMessage, arguments: ["version": "0.2.0"])
        )
        XCTAssertEqual(presentation.actionTitle, i18n.tr(.updateInstallNow))
        XCTAssertTrue(presentation.showsDismiss)
        XCTAssertTrue(presentation.showsNeverRemind)
    }

    func testBusyAndInstalledStatesHideTheAction() {
        for phase in ["checking", "installing", "installed"] {
            let presentation = UpdateBannerPresentation.make(status: status(phase: phase))
            XCTAssertTrue(presentation.visible, "\(phase) 应保持提醒卡可见以便反馈进度")
            XCTAssertFalse(
                presentation.actionEnabled,
                "\(phase) 期间不得允许重复点击更新"
            )
        }
    }

    /// 下载/安装是瞬态，不能关；安装完成后的重启提示必须能手动收起。
    func testDismissIsOfferedOnlyForSettledStates() {
        for phase in ["available", "installed", "failed"] {
            XCTAssertTrue(
                UpdateBannerPresentation.make(
                    status: status(phase: phase, version: "0.2.0")
                ).showsDismiss,
                "\(phase) 应允许关闭提醒卡"
            )
        }
        for phase in ["checking", "installing"] {
            XCTAssertFalse(
                UpdateBannerPresentation.make(
                    status: status(phase: phase, version: "0.2.0")
                ).showsDismiss,
                "\(phase) 进行中不应能关闭提醒卡"
            )
        }
    }

    /// 「不再提醒」只对还没安装的更新有意义；安装完成后只剩重启提示。
    func testNeverRemindIsOfferedOnlyForActionableStates() {
        for phase in ["available", "failed"] {
            XCTAssertTrue(
                UpdateBannerPresentation.make(
                    status: status(phase: phase, version: "0.2.0")
                ).showsNeverRemind,
                "\(phase) 应允许不再提醒"
            )
        }
        for phase in ["checking", "installing", "installed"] {
            XCTAssertFalse(
                UpdateBannerPresentation.make(
                    status: status(phase: phase, version: "0.2.0")
                ).showsNeverRemind,
                "\(phase) 不应提供不再提醒"
            )
        }
    }

    func testInstalledTellsUserToRestart() {
        let presentation = UpdateBannerPresentation.make(status: status(phase: "installed"))
        XCTAssertEqual(presentation.title, MuxtermI18n.shared.tr(.updateInstalledTitle))
        XCTAssertEqual(
            presentation.message,
            MuxtermI18n.shared.tr(.updateInstalledRestart)
        )
    }

    func testFailedShowsMessageAndOffersRetry() {
        let presentation = UpdateBannerPresentation.make(
            status: status(phase: "failed", message: "checksum mismatch")
        )
        XCTAssertTrue(presentation.visible)
        XCTAssertTrue(presentation.actionEnabled, "失败后必须能重试")
        XCTAssertTrue(
            presentation.message.contains("checksum mismatch"),
            "失败原因必须透出给用户，实际=\(presentation.message)"
        )
        XCTAssertEqual(presentation.actionTitle, MuxtermI18n.shared.tr(.updateRetry))
    }
}

/// 渲染层行为：按状态显隐、本次关闭不重复、永久不再提醒会跨视图保存。
final class UpdateBannerViewTests: XCTestCase {
    private var suiteName = ""
    private var defaults = UserDefaults.standard

    override func setUp() {
        super.setUp()
        suiteName = "muxterm.update-banner.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName) ?? .standard
        defaults.removePersistentDomain(forName: suiteName)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suiteName)
        super.tearDown()
    }

    private func makeView() -> UpdateBannerView {
        UpdateBannerView(defaults: defaults)
    }

    private func status(phase: String, version: String?) -> CoreBridge.UpdateStatus {
        CoreBridge.UpdateStatus(
            phase: phase,
            currentVersion: "0.1.0",
            version: version,
            message: nil,
            releaseURL: nil,
            downloadURL: nil,
            assetName: nil,
            restartRequired: false
        )
    }

    private func find(_ root: NSView, _ id: String) -> NSView? {
        if root.accessibilityIdentifier() == id { return root }
        return root.subviews.lazy.compactMap { self.find($0, id) }.first
    }

    func testBannerStartsHiddenUntilAnUpdateExists() {
        let view = makeView()
        XCTAssertTrue(view.isHidden, "启动时不应显示更新提醒")
        view.apply(status(phase: "idle", version: nil))
        XCTAssertTrue(view.isHidden)
        view.apply(status(phase: "up_to_date", version: "0.1.0"))
        XCTAssertTrue(view.isHidden)
    }

    func testBannerAppearsForAvailableUpdate() {
        let view = makeView()
        view.apply(status(phase: "available", version: "0.2.0"))
        XCTAssertFalse(view.isHidden)
        XCTAssertEqual(view.testTitleText(), MuxtermI18n.shared.tr(.updateAvailable))
        XCTAssertEqual(view.testActionTitle(), MuxtermI18n.shared.tr(.updateInstallNow))
        XCTAssertFalse(view.testMessageText().isEmpty, "必须渲染出「有新版本」的文案")
        XCTAssertTrue(view.testDismissVisible())
        XCTAssertTrue(view.testNeverRemindVisible())
    }

    func testDismissedVersionIsNotShownAgain() {
        let view = makeView()
        view.apply(status(phase: "available", version: "0.2.0"))
        view.dismiss()
        XCTAssertTrue(view.isHidden)
        view.apply(status(phase: "available", version: "0.2.0"))
        XCTAssertTrue(view.isHidden, "同一版本被关闭后不得反复弹出")
    }

    func testNewVersionAfterDismissIsShownAgain() {
        let view = makeView()
        view.apply(status(phase: "available", version: "0.2.0"))
        view.dismiss()
        view.apply(status(phase: "available", version: "0.3.0"))
        XCTAssertFalse(view.isHidden, "更高版本应重新提醒")
    }

    func testPrimaryButtonClickReachesCoreCallback() throws {
        let view = makeView()
        var fired = 0
        view.onPrimaryAction = { fired += 1 }
        view.apply(status(phase: "available", version: "0.2.0"))
        XCTAssertEqual(fired, 0, "渲染状态本身不得触发更新")

        let button = try XCTUnwrap(
            find(view, "muxterm.updateAction") as? NSButton,
            "提醒卡必须暴露一键更新按钮"
        )
        XCTAssertFalse(button.isHidden)
        XCTAssertTrue(button.isEnabled)
        button.performClick(nil)
        XCTAssertEqual(fired, 1, "点击一键更新必须回调 Core")
    }

    func testCloseButtonHidesBannerAndNotifies() throws {
        let view = makeView()
        var dismissed = 0
        view.onDismiss = { dismissed += 1 }
        view.apply(status(phase: "available", version: "0.2.0"))

        let button = try XCTUnwrap(find(view, "muxterm.updateDismiss") as? NSButton)
        XCTAssertEqual(button.title, MuxtermI18n.shared.tr(.updateClose))
        button.performClick(nil)
        XCTAssertEqual(dismissed, 1)
        XCTAssertTrue(view.isHidden)
    }

    /// 「关闭」只影响当前视图/本次运行；重新创建视图后同一版本仍会出现。
    func testCloseDoesNotPersistAcrossViews() {
        let first = makeView()
        first.apply(status(phase: "available", version: "0.2.0"))
        first.dismiss()

        let second = makeView()
        second.apply(status(phase: "available", version: "0.2.0"))
        XCTAssertFalse(second.isHidden, "关闭不是永久偏好，新窗口仍应提醒")
    }

    func testNeverRemindPersistsAcrossViewsAndCanBeReenabled() throws {
        let first = makeView()
        var fired = 0
        first.onNeverRemind = { fired += 1 }
        first.apply(status(phase: "available", version: "0.2.0"))
        let button = try XCTUnwrap(
            find(first, "muxterm.updateNeverRemind") as? NSButton
        )
        XCTAssertEqual(button.title, MuxtermI18n.shared.tr(.updateNeverRemind))
        button.performClick(nil)
        XCTAssertEqual(fired, 1)
        XCTAssertTrue(first.isHidden)
        XCTAssertTrue(defaults.bool(forKey: UpdateBannerView.suppressRemindersKey))

        let second = makeView()
        second.apply(status(phase: "available", version: "0.3.0"))
        XCTAssertTrue(second.isHidden, "不再提醒必须跨视图生效")

        second.allowFutureReminders()
        XCTAssertFalse(defaults.bool(forKey: UpdateBannerView.suppressRemindersKey))
        second.apply(status(phase: "available", version: "0.3.0"))
        XCTAssertFalse(second.isHidden, "显式检查更新后应恢复提醒")
    }

    func testInstallingDisablesTheActionButton() throws {
        let view = makeView()
        view.apply(status(phase: "available", version: "0.2.0"))
        view.apply(status(phase: "installing", version: "0.2.0"))
        let button = try XCTUnwrap(find(view, "muxterm.updateAction") as? NSButton)
        XCTAssertFalse(button.isEnabled, "安装期间不得重复触发更新")
        XCTAssertFalse(view.isHidden, "安装进度必须仍然可见")
        XCTAssertFalse(view.testDismissVisible(), "安装进行中不应能关闭提醒卡")
        XCTAssertFalse(view.testNeverRemindVisible())
    }

    /// 安装完成后没有主按钮，但必须保留一个可见的关闭键。
    func testInstalledBannerOffersCloseButNotNeverRemind() throws {
        let view = makeView()
        view.apply(status(phase: "installed", version: "0.2.0"))
        XCTAssertFalse(view.isHidden)
        XCTAssertTrue(view.testDismissVisible())
        XCTAssertFalse(view.testNeverRemindVisible())
        let button = try XCTUnwrap(find(view, "muxterm.updateDismiss") as? NSButton)
        XCTAssertFalse(button.isHidden)
        XCTAssertEqual(button.title, MuxtermI18n.shared.tr(.updateClose))
    }

    func testCloseInstalledBannerHidesItAndNotifies() throws {
        let view = makeView()
        var dismissed = 0
        view.onDismiss = { dismissed += 1 }
        view.apply(status(phase: "installed", version: "0.2.0"))
        let button = try XCTUnwrap(find(view, "muxterm.updateDismiss") as? NSButton)
        button.performClick(nil)
        XCTAssertEqual(dismissed, 1)
        XCTAssertTrue(view.isHidden)
    }

    /// 真实 ContentView 里：HUD 在右下角，有可读宽度，且不会压住状态栏。
    func testHUDStaysInBottomRightAndClearsChrome() throws {
        AppE2E.ensureApp()
        let suppressionKey = UpdateBannerView.suppressRemindersKey
        let previous = UserDefaults.standard.object(forKey: suppressionKey)
        UserDefaults.standard.removeObject(forKey: suppressionKey)
        defer {
            if let previous {
                UserDefaults.standard.set(previous, forKey: suppressionKey)
            } else {
                UserDefaults.standard.removeObject(forKey: suppressionKey)
            }
        }

        let bridge = try CoreBridge(backendType: "local")
        defer { bridge.shutdown() }
        let content = ContentView(terminalManager: TerminalManager(bridge: bridge))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 900, height: 600),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = content
        window.orderFront(nil)
        defer { window.orderOut(nil) }

        let installed = status(phase: "installed", version: "0.2.0")
        for position in [TabBarPosition.top, .bottom] {
            content.applyTabBarPosition(position)
            content.updateBanner.apply(installed)
            content.layoutSubtreeIfNeeded()
            let banner = content.updateBanner.frame
            XCTAssertGreaterThanOrEqual(banner.width, 300)
            XCTAssertLessThanOrEqual(banner.width, 360)
            XCTAssertGreaterThan(
                banner.minX,
                content.bounds.midX,
                "HUD 必须靠右，实际 frame=\(banner)"
            )
            XCTAssertLessThan(
                banner.maxY,
                content.bounds.midY,
                "HUD 必须靠下，实际 frame=\(banner)"
            )
            XCTAssertFalse(
                banner.intersects(content.statusBar.frame),
                "HUD 不能压住 \(position) 状态栏"
            )
        }
    }

    /// 显示 HUD 也不能把用户收窄的窗口重新撑宽。
    func testVisibleHUDDoesNotWidenCompactWindow() throws {
        AppE2E.ensureApp()
        let suppressionKey = UpdateBannerView.suppressRemindersKey
        let previous = UserDefaults.standard.object(forKey: suppressionKey)
        UserDefaults.standard.removeObject(forKey: suppressionKey)
        defer {
            if let previous {
                UserDefaults.standard.set(previous, forKey: suppressionKey)
            } else {
                UserDefaults.standard.removeObject(forKey: suppressionKey)
            }
        }

        let bridge = try CoreBridge(backendType: "local")
        defer { bridge.shutdown() }
        let content = ContentView(terminalManager: TerminalManager(bridge: bridge))
        let window = NSWindow(
            contentRect: NSRect(x: 40, y: 40, width: 500, height: 320),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.contentView = content
        window.orderFront(nil)
        defer { window.orderOut(nil) }
        content.layoutSubtreeIfNeeded()

        content.updateBanner.apply(status(phase: "available", version: "0.2.0"))
        content.layoutSubtreeIfNeeded()
        XCTAssertEqual(window.frame.width, 500, accuracy: 1, "HUD 不能撑宽紧凑窗口")
        if content.updateBanner.isHidden {
            XCTFail("有更新时 HUD 必须可见")
        }
        XCTAssertGreaterThan(
            content.updateBanner.frame.width,
            200,
            "紧凑窗口里 HUD 仍需保留可读宽度"
        )
    }

    /// 卡片固定右下角，不再是一条横贯窗口的色带。
    func testBannerUsesBottomRightCardWidth() {
        let container = NSView(frame: NSRect(x: 0, y: 0, width: 1000, height: 400))
        let view = makeView()
        view.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(view)
        NSLayoutConstraint.activate([
            view.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -16),
            view.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -16),
        ])
        view.apply(status(phase: "available", version: "0.2.0"))
        container.layoutSubtreeIfNeeded()
        XCTAssertEqual(view.frame.maxX, container.frame.maxX - 16, accuracy: 1)
        XCTAssertEqual(view.frame.minY, container.frame.minY + 16, accuracy: 1)
        XCTAssertGreaterThan(view.frame.width, 300)
        XCTAssertLessThanOrEqual(view.frame.width, 360)
        XCTAssertLessThan(view.frame.height, 180, "卡片不应铺满窗口")
    }
}
