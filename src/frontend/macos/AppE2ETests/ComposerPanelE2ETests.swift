import AppKit
import MuxtermChrome
import XCTest
@testable import MuxtermAppLib

/// 独立输入框的装配冒烟：不需要 tmux，只验证徽章、纯文本与 Markdown 预览。
final class ComposerPanelE2ETests: XCTestCase {
    func testPanelStartsAsAnEmptyDraftWithAHint() {
        let panel = ComposerPanelController()
        XCTAssertTrue(panel.draftText.isEmpty)
        XCTAssertEqual(panel.statusText, MuxtermI18n.shared.tr(.composerPlaceholder))
    }

    func testTargetBadgeDistinguishesLocalAndSSH() {
        let panel = ComposerPanelController()
        panel.updateTarget(ComposerTarget(
            kind: .ssh,
            host: "ryzen",
            workspaceName: "api",
            workspaceID: "ssh:ryzen:api",
            paneID: 2
        ))
        XCTAssertEqual(panel.badgeText, "SSH")
        XCTAssertEqual(panel.detailText, "ryzen · api")

        panel.updateTarget(ComposerTarget(
            kind: .local,
            host: "",
            workspaceName: "muxterm",
            workspaceID: "local:muxterm",
            paneID: 1
        ))
        XCTAssertEqual(panel.badgeText, "Local")
        XCTAssertEqual(panel.detailText, "muxterm")
    }

    /// 图片只插路径：单栏编辑器里就是原文，不变成图片语法。
    func testInsertedImagePathStaysPlainText() {
        let panel = ComposerPanelController()
        panel.insertImagePath("/tmp/muxterm-paste-smoke.png")
        XCTAssertEqual(panel.draftText, "/tmp/muxterm-paste-smoke.png")
        XCTAssertFalse(panel.draftText.contains("!["))
    }

    /// Cmd-Enter / 写入按钮走同一条路径：空白不发，有目标时把原文交出去。
    func testWriteSendsTheExactDraftAndRefusesWhitespace() {
        let panel = ComposerPanelController()
        let target = ComposerTarget(
            kind: .local,
            host: "",
            workspaceName: "muxterm",
            workspaceID: "local:muxterm",
            paneID: 1
        )
        panel.targetProvider = { target }
        var sent: [String] = []
        panel.onSend = { text, _ in sent.append(text) }

        panel.setDraftForTesting("   ")
        panel.sendForTesting()
        XCTAssertTrue(sent.isEmpty, "空白草稿不能写进终端")
        XCTAssertEqual(panel.statusText, MuxtermI18n.shared.tr(.composerNothingToSend))

        panel.setDraftForTesting("git status")
        panel.sendForTesting()
        XCTAssertEqual(sent, ["git status"])
        XCTAssertEqual(panel.statusText, MuxtermI18n.shared.tr(.composerSent))
        XCTAssertEqual(panel.draftText, "git status", "写入后保留原文，不能清空")
    }

    func testPresentingFocusesTheEditor() {
        let panel = ComposerPanelController()
        panel.present(from: nil)
        defer { panel.dismiss() }
        XCTAssertTrue(panel.isVisible)
        XCTAssertTrue(panel.editorIsFirstResponder, "打开后键盘必须直接进输入框")
    }

    /// 单栏高亮只改属性，不删 Markdown 符号；发送的仍是这一串原文。
    func testHighlightingKeepsTheMarkdownSourceInTheDraft() {
        let panel = ComposerPanelController()
        panel.setDraftForTesting("# Title\n\n**bold** and `code`")
        XCTAssertEqual(panel.draftText, "# Title\n\n**bold** and `code`")
    }

    func testMissingTargetShowsTheUnboundBadge() {
        let panel = ComposerPanelController()
        panel.updateTarget(nil)
        XCTAssertEqual(panel.badgeText, "—")
        XCTAssertEqual(panel.detailText, MuxtermI18n.shared.tr(.composerNoTarget))
    }
}
