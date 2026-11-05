import AppKit
import XCTest
@testable import MuxtermAppLib

/// 更新提醒条：Core 给什么状态，前端就渲染什么。
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
            XCTAssertFalse(presentation.visible, "\(phase) 不应显示提醒条")
            XCTAssertFalse(presentation.actionEnabled, "\(phase) 不应给出按钮")
        }
    }

    func testAvailableOffersOneClickInstall() {
        let presentation = UpdateBannerPresentation.make(
            status: status(phase: "available", version: "0.2.0")
        )
        XCTAssertTrue(presentation.visible)
        XCTAssertTrue(presentation.actionEnabled, "有更新时必须可以一键更新")
        let i18n = MuxtermI18n.shared
        XCTAssertEqual(
            presentation.message,
            i18n.tr(.updateAvailableMessage, arguments: ["version": "0.2.0"])
        )
        XCTAssertEqual(presentation.actionTitle, i18n.tr(.updateInstallNow))
    }

    func testBusyAndInstalledStatesHideTheAction() {
        for phase in ["checking", "installing", "installed"] {
            let presentation = UpdateBannerPresentation.make(status: status(phase: phase))
            XCTAssertTrue(presentation.visible, "\(phase) 应保持提醒条可见以便反馈进度")
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
                "\(phase) 应允许关闭提醒条"
            )
        }
        for phase in ["checking", "installing"] {
            XCTAssertFalse(
                UpdateBannerPresentation.make(
                    status: status(phase: phase, version: "0.2.0")
                ).showsDismiss,
                "\(phase) 进行中不应能关闭提醒条"
            )
        }
    }

    func testInstalledTellsUserToRestart() {
        let presentation = UpdateBannerPresentation.make(status: status(phase: "installed"))
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

/// 渲染层行为：按状态显隐、同版本关闭后不再打扰、按钮接通 Core 回调。
final class UpdateBannerViewTests: XCTestCase {
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
        let view = UpdateBannerView()
        XCTAssertTrue(view.isHidden, "启动时不应显示更新提醒")
        view.apply(status(phase: "idle", version: nil))
        XCTAssertTrue(view.isHidden)
        view.apply(status(phase: "up_to_date", version: "0.1.0"))
        XCTAssertTrue(view.isHidden)
    }

    func testBannerAppearsForAvailableUpdate() {
        let view = UpdateBannerView()
        view.apply(status(phase: "available", version: "0.2.0"))
        XCTAssertFalse(view.isHidden)
        XCTAssertEqual(view.testActionTitle(), MuxtermI18n.shared.tr(.updateInstallNow))
        XCTAssertFalse(view.testMessageText().isEmpty, "必须渲染出「有新版本」的文案")
    }

    func testDismissedVersionIsNotShownAgain() {
        let view = UpdateBannerView()
        view.apply(status(phase: "available", version: "0.2.0"))
        view.dismiss()
        XCTAssertTrue(view.isHidden)
        view.apply(status(phase: "available", version: "0.2.0"))
        XCTAssertTrue(view.isHidden, "同一版本被关闭后不得反复弹出")
    }

    func testNewVersionAfterDismissIsShownAgain() {
        let view = UpdateBannerView()
        view.apply(status(phase: "available", version: "0.2.0"))
        view.dismiss()
        view.apply(status(phase: "available", version: "0.3.0"))
        XCTAssertFalse(view.isHidden, "更高版本应重新提醒")
    }

    func testPrimaryButtonClickReachesCoreCallback() throws {
        let view = UpdateBannerView()
        var fired = 0
        view.onPrimaryAction = { fired += 1 }
        view.apply(status(phase: "available", version: "0.2.0"))
        XCTAssertEqual(fired, 0, "渲染状态本身不得触发更新")

        let button = try XCTUnwrap(
            find(view, "muxterm.updateAction") as? NSButton,
            "提醒条必须暴露一键更新按钮"
        )
        XCTAssertFalse(button.isHidden)
        XCTAssertTrue(button.isEnabled)
        button.performClick(nil)
        XCTAssertEqual(fired, 1, "点击一键更新必须回调 Core")
    }

    func testDismissButtonHidesBannerAndNotifies() throws {
        let view = UpdateBannerView()
        var dismissed = 0
        view.onDismiss = { dismissed += 1 }
        view.apply(status(phase: "available", version: "0.2.0"))

        let button = try XCTUnwrap(find(view, "muxterm.updateDismiss") as? NSButton)
        button.performClick(nil)
        XCTAssertEqual(dismissed, 1)
        XCTAssertTrue(view.isHidden)
    }

    func testInstallingDisablesTheActionButton() throws {
        let view = UpdateBannerView()
        view.apply(status(phase: "available", version: "0.2.0"))
        view.apply(status(phase: "installing", version: "0.2.0"))
        let button = try XCTUnwrap(find(view, "muxterm.updateAction") as? NSButton)
        XCTAssertFalse(button.isEnabled, "安装期间不得重复触发更新")
        XCTAssertFalse(view.isHidden, "安装进度必须仍然可见")
        XCTAssertFalse(view.testDismissVisible(), "安装进行中不应能关闭提醒条")
    }

    /// 安装完成后没有主按钮，但必须保留一个可见的关闭键。
    func testInstalledBannerOffersDismiss() throws {
        let view = UpdateBannerView()
        view.apply(status(phase: "installed", version: "0.2.0"))
        XCTAssertFalse(view.isHidden)
        XCTAssertTrue(view.testDismissVisible())
        let button = try XCTUnwrap(find(view, "muxterm.updateDismiss") as? NSButton)
        XCTAssertFalse(button.isHidden)
        XCTAssertEqual(button.toolTip, MuxtermI18n.shared.tr(.updateDismiss))
    }

    func testDismissInstalledBannerHidesItAndNotifies() throws {
        let view = UpdateBannerView()
        var dismissed = 0
        view.onDismiss = { dismissed += 1 }
        view.apply(status(phase: "installed", version: "0.2.0"))
        let button = try XCTUnwrap(find(view, "muxterm.updateDismiss") as? NSButton)
        button.performClick(nil)
        XCTAssertEqual(dismissed, 1)
        XCTAssertTrue(view.isHidden)
    }

    /// 真实 ContentView 里：HUD 贴着内容宽度，且不会压住顶部 tab/status bar。
    func testHUDStaysBelowTopChromeAndHugsContent() throws {
        AppE2E.ensureApp()
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
            XCTAssertGreaterThan(banner.width, 150)
            XCTAssertLessThanOrEqual(banner.width, 620)
            XCTAssertFalse(
                banner.intersects(content.statusBar.frame),
                "HUD 不能压住 \(position) 状态栏"
            )
        }
    }

    /// HUD 只占内容宽度，短文案不能变成一条横贯窗口的色带。
    func testBannerHugsContentInsteadOfFillingTheWindow() {
        let container = NSView(frame: NSRect(x: 0, y: 0, width: 1000, height: 200))
        let view = UpdateBannerView()
        view.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(view)
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 10),
            view.topAnchor.constraint(equalTo: container.topAnchor, constant: 6),
            view.trailingAnchor.constraint(
                lessThanOrEqualTo: container.trailingAnchor,
                constant: -10
            ),
            view.widthAnchor.constraint(lessThanOrEqualToConstant: 620),
        ])
        view.apply(status(phase: "installed", version: "0.2.0"))
        container.layoutSubtreeIfNeeded()
        XCTAssertGreaterThan(view.frame.width, 200, "提醒条必须有内容宽度")
        XCTAssertLessThan(view.frame.width, 620, "短文案应贴合内容，不能铺满窗口")
    }
}
