import Foundation
import XCTest
@testable import MuxtermChrome

final class ComposerDraftTests: XCTestCase {
    func testSSHBadgeNamesTheMachineAndStaysShort() {
        let target = ComposerTarget(
            kind: .ssh,
            host: "ryzen",
            workspaceName: "api",
            workspaceID: "ssh:ryzen:api",
            paneID: 3
        )
        XCTAssertEqual(target.badge, "SSH")
        XCTAssertEqual(target.detail, "ryzen · api")
    }

    func testLocalBadgeOnlyShowsTheWorkspace() {
        let target = ComposerTarget(
            kind: .local,
            host: "",
            workspaceName: "~/Developer/self/muxterm",
            workspaceID: "local:shell",
            paneID: 1
        )
        XCTAssertEqual(target.badge, "Local")
        XCTAssertEqual(target.detail, "~/Developer/self/muxterm")
    }

    func testBlankDraftNeverSendsWhitespace() {
        XCTAssertNil(ComposerDraft(text: "  \n\t").payload())
        XCTAssertEqual(ComposerDraft(text: "git status").payload(), "git status")
    }

    func testImagePathIsInsertedRawWithoutEscaping() {
        let result = ComposerDraft.insertingImagePath(
            "/tmp/muxterm-paste-ab12cd.png",
            into: "",
            selectedRange: NSRange(location: 0, length: 0)
        )
        XCTAssertEqual(result.text, "/tmp/muxterm-paste-ab12cd.png")
        XCTAssertEqual(result.caret, 29)
    }

    func testImagePathAfterTextAddsExactlyOneSeparator() {
        let result = ComposerDraft.insertingImagePath(
            "/tmp/a.png",
            into: "see",
            selectedRange: NSRange(location: 3, length: 0)
        )
        XCTAssertEqual(result.text, "see /tmp/a.png")
        XCTAssertEqual(result.caret, "see /tmp/a.png".count)
    }

    func testImagePathReplacesTheSelectionWithoutDoubleSpaces() {
        let source = "before OLD after"
        let result = ComposerDraft.insertingImagePath(
            "/tmp/new.png",
            into: source,
            selectedRange: (source as NSString).range(of: "OLD")
        )
        XCTAssertEqual(result.text, "before /tmp/new.png after")
        XCTAssertFalse(result.text.contains("  "), "选区间已经自带空格，不能再补")
    }

    func testBlankImagePathLeavesTextUntouched() {
        let result = ComposerDraft.insertingImagePath(
            "   ",
            into: "keep",
            selectedRange: NSRange(location: 4, length: 0)
        )
        XCTAssertEqual(result.text, "keep")
        XCTAssertEqual(result.caret, 4)
    }

    /// 纯路径必须原样留在预览里：不变成图片语法、不被吞掉。
    func testMarkdownPreviewKeepsTheInsertedPathVisible() {
        let source = "# Hello\n\n/tmp/muxterm-paste-ab12cd.png"
        let rendered = ComposerMarkdown.preview(source).text
        XCTAssertTrue(rendered.contains("/tmp/muxterm-paste-ab12cd.png"), "\(rendered)")
        XCTAssertFalse(rendered.contains("# Hello"), "预览应已解析标题：\(rendered)")
        XCTAssertTrue(rendered.contains("Hello"))
    }

    func testCmdIOpensTheComposerAndShiftIsNotStolen() {
        XCTAssertEqual(
            KeyBindings.action(for: KeyChord(command: true, key: "i")),
            .openComposer
        )
        XCTAssertEqual(KeyBindingsConfig.action(from: "open_composer"), .openComposer)
        XCTAssertEqual(KeyBindingsConfig.name(for: .openComposer), "open_composer")
        XCTAssertNil(KeyBindings.action(for: KeyChord(command: true, shift: true, key: "i")))
    }

    func testMarkdownPreviewDrawsListMarkersThatAppKitWouldOtherwiseOmit() {
        let unordered = ComposerMarkdown.preview("- one\n- two").text
        XCTAssertEqual(unordered, "•  one\n•  two")
        let ordered = ComposerMarkdown.preview("1. one\n2. two").text
        XCTAssertEqual(ordered, "1. one\n2. two")
    }

    /// 预览不能把块合并成一行；代码围栏与引用块要能被 UI 分别上样式。
    func testMarkdownPreviewKeepsBlocksOnTheirOwnLines() {
        let preview = ComposerMarkdown.preview("# T\n\n```\nlet x = 1\n```\n\n> note")
        XCTAssertEqual(preview.text, "T\n\nlet x = 1\n\nnote")
        XCTAssertTrue(preview.spans.contains { $0.style == .codeBlock })
        XCTAssertTrue(preview.spans.contains { $0.style == .quote })
        XCTAssertTrue(preview.spans.contains { $0.style == .header(1) })
    }

    func testMarkdownPreviewOfEmptyDraftStaysEmpty() {
        let preview = ComposerMarkdown.preview("")
        XCTAssertTrue(preview.text.isEmpty)
        XCTAssertTrue(preview.spans.isEmpty)
    }

    /// 预览样式段必须落在最终文本的真实位置上（UTF-16），UI 才能按 range 上色。
    func testMarkdownPreviewSpansHitTheExpectedRuns() {
        let preview = ComposerMarkdown.preview("# Title\n\n**bold** and `code`")
        func substring(_ style: ComposerPreviewStyle) -> String? {
            guard let span = preview.spans.first(where: { $0.style == style }) else { return nil }
            return (preview.text as NSString).substring(with: span.range)
        }
        XCTAssertEqual(substring(.header(1)), "Title")
        XCTAssertEqual(substring(.bold), "bold")
        XCTAssertEqual(substring(.inlineCode), "code")
    }
}
