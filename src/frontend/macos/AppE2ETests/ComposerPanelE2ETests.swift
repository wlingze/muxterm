import AppKit
import MuxtermChrome
import XCTest
@testable import MuxtermAppLib

/// 独立输入框的装配冒烟：不需要 tmux，只验证徽章、纯文本与 Markdown 预览。
private final class ComposerMenuProbe: NSObject {
    var hits = 0
    @objc func fire(_ sender: Any?) { hits += 1 }
}

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
        panel.onSend = { text, _ in sent.append(text); return true }

        panel.setDraftForTesting("   ")
        panel.sendForTesting()
        XCTAssertTrue(sent.isEmpty, "空白草稿不能写进终端")
        XCTAssertEqual(panel.statusText, MuxtermI18n.shared.tr(.composerNothingToSend))

        panel.setDraftForTesting("git status")
        panel.sendForTesting()
        XCTAssertEqual(sent, ["git status"])
        XCTAssertEqual(panel.statusText, MuxtermI18n.shared.tr(.composerSent))
        XCTAssertTrue(panel.draftText.isEmpty, "Write 发完要清空这一页")
    }

    /// 目标已经关掉时不能假装成功：草稿必须留着。
    func testWriteKeepsTheDraftWhenDeliveryFails() {
        let panel = ComposerPanelController()
        panel.targetProvider = { self.target(pane: 1) }
        panel.onSend = { _, _ in false }
        panel.setDraftForTesting("keep me")
        panel.sendForTesting()
        XCTAssertEqual(panel.draftText, "keep me", "没送到就不能清空")
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

    private func target(pane: UInt32, workspace: String = "api") -> ComposerTarget {
        ComposerTarget(
            kind: .local,
            host: "",
            workspaceName: workspace,
            workspaceID: "local:\(workspace)",
            paneID: pane
        )
    }

    /// Cmd-Enter = 写入并清空：内容进 pane，草稿被消费（⌘Z 可撤销）。
    func testCmdEnterWritesTheDraftAndClearsIt() {
        let panel = ComposerPanelController()
        panel.targetProvider = { self.target(pane: 1) }
        var sent: [String] = []
        panel.onSend = { text, _ in sent.append(text); return true }
        panel.setDraftForTesting("git status")
        XCTAssertTrue(panel.performKeyEquivalentForTesting("\r"))
        XCTAssertEqual(sent, ["git status"])
        XCTAssertTrue(panel.draftText.isEmpty, "写入后必须清空这一页")
        XCTAssertEqual(panel.statusText, MuxtermI18n.shared.tr(.composerSent))
        panel.undoForTesting()
        XCTAssertEqual(panel.draftText, "git status", "⌘Z 能把写入后的清空恢复回来")
    }

    /// 主菜单里 `Cmd-Enter`（pane zoom）和 `Cmd-K`（Shells）是全局 key
    /// equivalent。输入框必须在菜单之前拿到事件，否则用户按 Cmd-Enter 只会
    /// 放大终端 pane，草稿一个字都没写进去。
    func testComposerOwnsCmdEnterAndCmdKBeforeTheAppMenu() throws {
        AppE2E.ensureApp()
        let probe = ComposerMenuProbe()
        let previousMenu = NSApp.mainMenu
        let menu = NSMenu()
        let root = NSMenuItem()
        let submenu = NSMenu(title: "View")
        root.submenu = submenu
        menu.addItem(root)
        for (title, key) in [("Toggle Pane Fullscreen", "\r"), ("Shells", "k")] {
            let item = NSMenuItem(title: title, action: #selector(ComposerMenuProbe.fire(_:)), keyEquivalent: key)
            item.keyEquivalentModifierMask = .command
            item.target = probe
            submenu.addItem(item)
        }
        NSApp.mainMenu = menu
        defer { NSApp.mainMenu = previousMenu }

        let panel = ComposerPanelController()
        panel.targetProvider = { self.target(pane: 1) }
        var sent: [String] = []
        panel.onSend = { text, _ in sent.append(text); return true }
        panel.present(from: nil)
        defer { panel.dismiss() }
        panel.setDraftForTesting("must reach the pane")

        panel.dispatchKeyEventForTesting("\r")
        XCTAssertEqual(sent, ["must reach the pane"], "⌘↩ 必须写进 pane，不能被菜单抢走")
        XCTAssertTrue(panel.draftText.isEmpty)
        XCTAssertEqual(probe.hits, 0, "主菜单的 Cmd-Enter 不能被触发")

        panel.setDraftForTesting("draft")
        panel.dispatchKeyEventForTesting("k", keyCode: 40)
        XCTAssertTrue(panel.actionMenuVisible, "⌘K 必须打开输入框页面菜单")
        XCTAssertEqual(probe.hits, 0, "主菜单的 Cmd-K 不能被触发")
        panel.dispatchKeyEventForTesting("k", keyCode: 40)
        XCTAssertFalse(panel.actionMenuVisible)
    }

    /// 每页一份草稿：切到新 pane 再切回来，原文和光标都还在；关窗不清空。
    func testDraftTabsFollowPanesAndSurviveReopen() {
        let panel = ComposerPanelController()
        var current = target(pane: 1)
        panel.targetProvider = { current }
        panel.present(from: nil)
        defer { panel.dismiss() }
        panel.setDraftForTesting("first pane")
        XCTAssertEqual(panel.pageTitles, ["api #1"])

        current = target(pane: 2)
        panel.updateTarget(current)
        XCTAssertEqual(panel.pageTitles, ["api #1", "api #2"], "每个 pane 一个 tab")
        XCTAssertEqual(panel.draftText, "", "新 pane 不继承上一页的草稿")
        panel.setDraftForTesting("second pane")

        panel.selectPageForTesting(0)
        XCTAssertEqual(panel.draftText, "first pane")
        panel.dismiss()
        panel.present(from: nil)
        XCTAssertEqual(panel.draftText, "second pane", "重新打开落在当前 pane 的草稿")
        panel.selectPageForTesting(0)
        XCTAssertEqual(panel.draftText, "first pane", "关窗后草稿必须保留")
    }

    func testClearingOnlyWipesTheActivePageAndUndoRestoresIt() {
        let panel = ComposerPanelController()
        var current = target(pane: 1)
        panel.targetProvider = { current }
        panel.present(from: nil)
        defer { panel.dismiss() }
        panel.setDraftForTesting("one")
        current = target(pane: 2)
        panel.updateTarget(current)
        panel.setDraftForTesting("two")

        panel.clearForTesting()
        XCTAssertEqual(panel.draftText, "")
        panel.undoForTesting()
        XCTAssertEqual(panel.draftText, "two", "⌘Z 必须能把清空恢复回来")
        panel.selectPageForTesting(0)
        XCTAssertEqual(panel.draftText, "one", "清空第 2 页不能动第 1 页")
    }

    /// 切页等于换文档：上一页的撤销记录不能把文本拼进这一页。
    func testSwitchingPagesDoesNotLeakUndoAcrossDrafts() {
        let panel = ComposerPanelController()
        var current = target(pane: 1)
        panel.targetProvider = { current }
        panel.present(from: nil)
        defer { panel.dismiss() }
        panel.setDraftForTesting("one")
        current = target(pane: 2)
        panel.updateTarget(current)
        panel.setDraftForTesting("two")

        panel.selectPageForTesting(0)
        panel.undoForTesting()
        XCTAssertEqual(panel.draftText, "one", "撤销不能把两页文本拼在一起")
    }

    func testWindowIsNormalAndPortraitInsteadOfAlwaysOnTop() {
        let panel = ComposerPanelController()
        panel.present(from: nil)
        defer { panel.dismiss() }
        let window = panel.window
        XCTAssertEqual(window?.level, .normal, "输入框不能总是浮在最前面")
        XCTAssertTrue(window?.styleMask.contains(.closable) == true)
        XCTAssertTrue(window?.styleMask.contains(.resizable) == true)
        XCTAssertGreaterThan(window?.frame.height ?? 0, window?.frame.width ?? 0)
        let screenWidth = window?.screen?.frame.width ?? NSScreen.main?.frame.width ?? 1440
        XCTAssertLessThanOrEqual(window?.frame.width ?? .infinity, screenWidth / 2)
    }

    /// Cmd-K 打开当前页菜单：复制 / 写入 / 清空，键盘选择也能执行。
    func testCmdKOpensPageMenuWithCopyWriteClear() {
        let panel = ComposerPanelController()
        panel.targetProvider = { self.target(pane: 1) }
        var sent: [String] = []
        panel.onSend = { text, _ in sent.append(text); return true }
        panel.setDraftForTesting("draft")
        XCTAssertTrue(panel.performKeyEquivalentForTesting("k"))
        XCTAssertTrue(panel.actionMenuVisible)
        XCTAssertEqual(panel.actionMenuTitles, [
            MuxtermI18n.shared.tr(.composerCopy),
            MuxtermI18n.shared.tr(.composerSend),
            MuxtermI18n.shared.tr(.composerClear),
        ])

        panel.moveActionSelectionForTesting(by: 2)
        XCTAssertEqual(panel.actionMenuSelection, 2)
        panel.activateActionForTesting()
        XCTAssertFalse(panel.actionMenuVisible)
        XCTAssertTrue(sent.isEmpty)
        XCTAssertTrue(panel.draftText.isEmpty)

        panel.setDraftForTesting("draft")
        panel.toggleActionsForTesting()
        XCTAssertTrue(panel.actionMenuVisible)
        panel.moveActionSelectionForTesting(by: -2)
        panel.activateActionForTesting()
        XCTAssertEqual(sent, ["draft"], "菜单里的写入仍走同一条发送路径")
        XCTAssertTrue(panel.draftText.isEmpty, "菜单写入也要清空")
    }

    /// 输入框图片粘贴复用终端横幅：编码 → 上传 → 已存路径，全部画在右下角。
    func testComposerShowsImageProgressInItsOwnBanner() throws {
        let panel = ComposerPanelController()
        panel.targetProvider = { self.target(pane: 1) }
        panel.present(from: nil)
        defer { panel.dismiss() }
        XCTAssertFalse(panel.imageBannerVisible)

        panel.advanceImage(.encoding)
        XCTAssertTrue(panel.imageBannerVisible)
        XCTAssertEqual(
            panel.imageBannerMessage,
            ImagePasteBannerView.message(for: try XCTUnwrap(ImagePasteFeedback.make(.encoding)))
        )
        panel.advanceImage(.uploading)
        XCTAssertTrue(panel.imageBannerVisible)
    }

    /// 路径插进草稿后不该再弹「已就绪」：路径本身已经在编辑器里，横幅只负责进度。
    func testSuccessfulImageInsertionHidesTheBannerWithoutASuccessToast() {
        let panel = ComposerPanelController()
        panel.targetProvider = { self.target(pane: 1) }
        panel.present(from: nil)
        defer { panel.dismiss() }
        panel.advanceImage(.uploading)
        XCTAssertTrue(panel.imageBannerVisible)

        XCTAssertTrue(panel.insertImagePath("/tmp/muxterm-paste-abc.png", for: panel.activeDraftKeyForTesting))
        panel.finishImageInsertion()
        panel.tickImageFeedbackForTesting()
        XCTAssertFalse(panel.imageBannerVisible, "路径可见后不能再留一条成功提示")

        panel.advanceImage(.failed(message: "boom"))
        XCTAssertTrue(panel.imageBannerVisible, "失败仍要画出来")
    }

    /// 空草稿不占 tab：只有当前页和有内容的页出现在 tab 栏。
    func testBlankDraftsDoNotTakeATab() {
        let panel = ComposerPanelController()
        var current = self.target(pane: 1)
        panel.targetProvider = { current }
        panel.present(from: nil)
        defer { panel.dismiss() }
        panel.setDraftForTesting("written here")
        current = self.target(pane: 2)
        panel.updateTarget(current)
        XCTAssertEqual(panel.pageTitles, ["api #1", "api #2"], "当前页即使空也要在")
        current = self.target(pane: 3)
        panel.updateTarget(current)
        XCTAssertEqual(panel.pageTitles, ["api #1", "api #3"], "空且非当前的 pane 不占 tab")
        panel.setDraftForTesting("third")
        XCTAssertEqual(panel.pageTitles, ["api #1", "api #3"])
        XCTAssertEqual(panel.allPageTitles, ["api #1", "api #2", "api #3"], "隐藏不代表删除")
    }
}
