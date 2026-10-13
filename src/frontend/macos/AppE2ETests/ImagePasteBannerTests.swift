import AppKit
import MuxtermChrome
import XCTest
@testable import MuxtermAppLib

/// 图片粘贴反馈条：时间线给什么阶段，横幅就渲染什么。
/// 只校验「阶段 → 文案 / 转圈 / 关闭键」，不依赖 Core 和真实上传。
final class ImagePasteBannerTests: XCTestCase {
    private func banner() -> ImagePasteBannerView {
        AppE2E.ensureApp()
        return ImagePasteBannerView(frame: NSRect(x: 0, y: 0, width: 480, height: 28))
    }

    private static func find(_ root: NSView, identifier: String) -> NSView? {
        if root.accessibilityIdentifier() == identifier { return root }
        for child in root.subviews {
            if let match = find(child, identifier: identifier) { return match }
        }
        return nil
    }

    func testStartsHiddenAndIdle() {
        let view = banner()
        XCTAssertTrue(view.isHidden)
        XCTAssertEqual(view.renderedStage, .idle)
        XCTAssertFalse(view.testDismissVisible())
    }

    func testProgressStagesAreVisibleSpinAndCannotBeDismissed() {
        let i18n = MuxtermI18n.shared
        for (stage, key) in [
            (ImagePasteStage.encoding, MuxtermTextKey.imagePasteEncoding),
            (.uploading, .imagePasteProgress),
        ] {
            let view = banner()
            view.apply(stage)
            XCTAssertFalse(view.isHidden, "\(stage) 必须马上可见")
            XCTAssertEqual(view.testMessageText(), i18n.tr(key))
            XCTAssertTrue(view.testSpinnerAnimating(), "\(stage) 必须转圈")
            XCTAssertFalse(view.testDismissVisible(), "进行中的粘贴不能被关掉")
        }
    }

    func testDoneShowsTheFileNameWithTheFullPathOnHover() {
        let view = banner()
        view.apply(.done(path: "/tmp/muxterm-paste-ab12cd.png"))
        XCTAssertFalse(view.isHidden)
        XCTAssertEqual(
            view.testMessageText(),
            MuxtermI18n.shared.tr(.imagePasteDone, arguments: ["name": "muxterm-paste-ab12cd.png"])
        )
        XCTAssertFalse(view.testMessageText().contains("/tmp/"), "横幅只放文件名，避免长路径挤掉文案")
        XCTAssertEqual(view.toolTip, "/tmp/muxterm-paste-ab12cd.png")
        XCTAssertFalse(view.testSpinnerAnimating())
        XCTAssertTrue(view.testDismissVisible())
    }

    func testFailureShowsTheReasonAndCanBeDismissed() {
        let view = banner()
        view.apply(.failed(message: "SSH image transfer timed out"))
        XCTAssertFalse(view.isHidden)
        let text = view.testMessageText()
        XCTAssertTrue(text.contains(MuxtermI18n.shared.tr(.imagePasteFailed)), text)
        XCTAssertTrue(text.contains("SSH image transfer timed out"), text)
        XCTAssertEqual(view.toolTip, "SSH image transfer timed out")
        XCTAssertFalse(view.testSpinnerAnimating())
        XCTAssertTrue(view.testDismissVisible())
    }

    func testIdleAfterAResultHidesTheBannerAgain() {
        let view = banner()
        view.apply(.done(path: "/tmp/a.png"))
        XCTAssertFalse(view.isHidden)
        view.apply(.idle)
        XCTAssertTrue(view.isHidden)
        XCTAssertFalse(view.testSpinnerAnimating())
        XCTAssertFalse(view.testDismissVisible())
    }

    func testReapplyingTheSameStageDoesNotRestartTheSpinner() {
        let view = banner()
        view.apply(.uploading)
        let before = view.testMessageText()
        view.apply(.uploading)
        XCTAssertEqual(view.testMessageText(), before)
        XCTAssertTrue(view.testSpinnerAnimating())
    }

    func testDismissButtonCallsBack() throws {
        let view = banner()
        var dismissed = 0
        view.onDismiss = { dismissed += 1 }
        view.apply(.failed(message: "boom"))
        let button = try XCTUnwrap(
            Self.find(view, identifier: "muxterm.imagePasteBanner.dismiss") as? NSButton
        )
        button.performClick(nil)
        XCTAssertEqual(dismissed, 1)
    }

    func testLocalizationRefreshKeepsTheCurrentStage() {
        let view = banner()
        view.apply(.done(path: "/tmp/muxterm-paste-x.png"))
        view.refreshLocalization()
        XCTAssertEqual(view.renderedStage, .done(path: "/tmp/muxterm-paste-x.png"))
        XCTAssertFalse(view.isHidden)
        XCTAssertTrue(view.testMessageText().contains("muxterm-paste-x.png"))
    }

    func testAccessibilityValueNamesEveryStage() {
        XCTAssertEqual(ImagePasteBannerView.accessibilityValue(for: .idle), "idle")
        XCTAssertEqual(ImagePasteBannerView.accessibilityValue(for: .encoding), "encoding")
        XCTAssertEqual(ImagePasteBannerView.accessibilityValue(for: .uploading), "uploading")
        XCTAssertEqual(ImagePasteBannerView.accessibilityValue(for: .done(path: "/tmp/a")), "done")
        XCTAssertEqual(ImagePasteBannerView.accessibilityValue(for: .failed(message: "x")), "failed")
    }

    func testNewStringsExistInBothShippingCatalogs() {
        let roots = MuxtermI18nLocator.searchRoots()
        for language in [MuxtermLanguage.english, .simplifiedChinese] {
            let catalog = MuxtermI18nLocator.loadCatalog(tag: language.catalogTag, roots: roots)
            XCTAssertNotNil(catalog["image_paste_encoding"], language.catalogTag)
            XCTAssertTrue(
                catalog["image_paste_done"]?.contains("{{name}}") == true,
                "\(language.catalogTag) 的 image_paste_done 必须带 {{name}} 占位符"
            )
        }
    }
}
