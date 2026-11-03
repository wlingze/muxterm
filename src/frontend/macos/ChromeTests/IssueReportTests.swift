import Foundation
import XCTest
@testable import MuxtermChrome

final class MuxtermIssueReportTests: XCTestCase {
    func testTailKeepsTheEndAndRedactsSecrets() {
        let text = "hello\npassword = hunter2\nline-end"
        let tail = MuxtermIssueReport.tail(text, maxBytes: 64)
        XCTAssertTrue(tail.contains("[redacted]"))
        XCTAssertFalse(tail.contains("hunter2"))
        XCTAssertTrue(tail.contains("line-end"))
        XCTAssertEqual(MuxtermIssueReport.tail("abcdef", maxBytes: 3), "def")
    }

    /// 尾部切片不能从多字节字符中间开始（否则会输出替换字符）。
    func testTailDoesNotSplitMultibyteCharacters() {
        let text = "中文日志行\n"
        let bytes = text.utf8.count
        let tail = MuxtermIssueReport.tail(text, maxBytes: bytes - 1)
        XCTAssertFalse(tail.contains("\u{fffd}"), "tail=\(tail)")
        XCTAssertTrue(tail.contains("日志行"))
    }

    /// 只读日志末尾：大文件不能被整份读进内存。
    func testReadLogTailOnlyReadsTheEndOfALargeFile() throws {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("muxterm-log-tail-\(UUID().uuidString).log")
        defer { try? FileManager.default.removeItem(at: url) }
        var content = "head-marker\n"
        content += String(repeating: "early line\n", count: 200_000)
        content += "tail-marker\n"
        try content.write(to: url, atomically: true, encoding: .utf8)
        XCTAssertGreaterThan(content.utf8.count, 2_000_000)

        let tail = MuxtermIssueReport.readLogTail(path: url.path, maxBytes: 4096)
        XCTAssertLessThanOrEqual(tail.utf8.count, 4096)
        XCTAssertTrue(tail.contains("tail-marker"), "必须包含日志末尾")
        XCTAssertFalse(tail.contains("head-marker"), "不应读整份日志")
    }

    func testReadLogTailMissingFileIsEmpty() {
        XCTAssertEqual(
            MuxtermIssueReport.readLogTail(
                path: "/tmp/muxterm-does-not-exist-\(UUID().uuidString).log",
                maxBytes: 1024
            ),
            ""
        )
        XCTAssertEqual(MuxtermIssueReport.readLogTail(path: "", maxBytes: 1024), "")
    }
}
