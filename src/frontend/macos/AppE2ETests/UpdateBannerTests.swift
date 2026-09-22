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
    }
}
