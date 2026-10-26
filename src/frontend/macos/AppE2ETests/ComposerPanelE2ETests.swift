import AppKit
import MuxtermChrome
import XCTest
@testable import MuxtermAppLib

/// 独立输入框的装配冒烟：不需要 tmux，只验证徽章、纯文本与 Markdown 预览。
final class ComposerPanelE2ETests: XCTestCase {
    func testPNGOnlyPasteboardReachesTheImageHandlerViaAppKit() throws {
        AppE2E.ensureApp()
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let data = Data([137, 80, 78, 71, 13, 10, 26, 10])
        board.setData(data, forType: .png)
        let editor = ComposerTextView()
        editor.isRichText = true
        var received: Data?
        editor.onImage = { received = $0 }
        XCTAssertNotNil(board.availableType(from: editor.readablePasteboardTypes),
                        "PNG-only 剪贴板必须被 AppKit 判定为可粘贴，不能在调用 paste: 前被禁用")
        XCTAssertTrue(editor.readSelection(from: board))
        XCTAssertEqual(received, data)
        XCTAssertTrue(editor.string.isEmpty, "图片只能走上传通路，不能成为富文本附件")
    }

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
