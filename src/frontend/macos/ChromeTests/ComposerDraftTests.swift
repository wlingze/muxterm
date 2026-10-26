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

    /// 单栏语法高亮：原文不动，只标记区间。
    func testMarkdownHighlightsDoNotRewriteTheDraft() {
        let source = "# Hello\n\n/tmp/muxterm-paste-ab12cd.png"
        let spans = ComposerMarkdown.highlights(source)
        XCTAssertTrue(
            spans.contains { span in
                span.style == .path
                    && (source as NSString).substring(with: span.range)
                        == "/tmp/muxterm-paste-ab12cd.png"
            },
            "路径必须作为原文的一部分被标记出来"
        )
        XCTAssertTrue(
            spans.contains { span in
                span.style == .header(1)
                    && (source as NSString).substring(with: span.range) == "Hello"
            }
        )
        XCTAssertTrue(spans.contains { $0.style == .marker })
    }

    func testMarkdownHighlightsMarkListSymbolsInsteadOfRenamingThem() {
        let source = "- one\n1. two"
        let spans = ComposerMarkdown.highlights(source)
        let markers = spans.filter { $0.style == .listMarker }
            .map { (source as NSString).substring(with: $0.range) }
        XCTAssertEqual(markers, ["- ", "1. "])
    }

    func testMarkdownHighlightsCoverCodeFenceAndQuote() {
        let source = "```\nlet x = 1\n```\n\n> note"
        let spans = ComposerMarkdown.highlights(source)
        XCTAssertTrue(spans.contains { span in
            span.style == .codeBlock
                && (source as NSString).substring(with: span.range) == "let x = 1"
        })
        XCTAssertTrue(spans.contains { span in
            span.style == .quote
                && (source as NSString).substring(with: span.range) == "note"
        })
    }

    func testMarkdownHighlightsStayEmptyForAnEmptyDraft() {
        XCTAssertTrue(ComposerMarkdown.highlights("").isEmpty)
    }

    /// 高亮范围必须落在原文的真实位置上（UTF-16）。
    func testMarkdownHighlightsHitTheExpectedRuns() {
        let source = "# Title\n\n**bold** and `code`"
        func substring(_ style: ComposerHighlightStyle) -> String? {
            guard let span = ComposerMarkdown.highlights(source).first(where: { $0.style == style }) else {
                return nil
            }
            return (source as NSString).substring(with: span.range)
        }
        XCTAssertEqual(substring(.header(1)), "Title")
        XCTAssertEqual(substring(.bold), "bold")
        XCTAssertEqual(substring(.inlineCode), "code")
        XCTAssertNil(substring(.path), "纯文本里的 / 不是路径时不要乱标")
    }

    func testUnclosedMarkersStayPlainUntilTheyAreClosed() {
        let spans = ComposerMarkdown.highlights("**半截")
        XCTAssertFalse(spans.contains { $0.style == .bold })
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
}
