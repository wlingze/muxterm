import AppKit
import XCTest
@testable import MuxtermAppLib
import MuxtermChrome

final class ImagePasteE2ETests: XCTestCase {
    private func image(_ type: NSBitmapImageRep.FileType = .png) throws -> Data {
        let bitmap = try XCTUnwrap(NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
            isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
        ))
        for x in 0..<2 {
            for y in 0..<2 { bitmap.setColor(.red, atX: x, y: y) }
        }
        return try XCTUnwrap(bitmap.representation(using: type, properties: [:]))
    }

    func testTIFFClipboardIsEncodedAsPNGWithoutSendingText() throws {
        AppE2E.ensureApp()
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setData(try image(.tiff), forType: .tiff)
        board.setString("do not paste this fallback", forType: .string)
        let view = MuxTerminalView(paneId: 1)
        let input = ImagePasteInputRecorder()
        view.inputHandler = input
        var received: Data?
        view.onImagePaste = { received = $0 }
        view.paste(from: board)
        XCTAssertTrue(AppE2E.wait(timeout: 3) { received != nil })
        let png = try XCTUnwrap(received)
        XCTAssertEqual(Array(png.prefix(8)), [137, 80, 78, 71, 13, 10, 26, 10])
        XCTAssertEqual(NSBitmapImageRep(data: png)?.pixelsWide, 2)
        XCTAssertTrue(input.bytes.isEmpty, "图片不能作为二进制或 fallback 文本写进 PTY")
    }

    func testInvalidImageReportsFailureAndTextPasteStillUsesBracketedPaste() {
        AppE2E.ensureApp()
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let view = MuxTerminalView(paneId: 1)
        let input = ImagePasteInputRecorder()
        view.inputHandler = input
        var error: String?
        view.onImagePaste = { _ in XCTFail("invalid image must not upload") }
        view.onImagePasteError = { error = $0 }
        board.setData(Data("broken".utf8), forType: .png)
        view.paste(from: board)
        XCTAssertTrue(AppE2E.wait(timeout: 3) { error != nil })
        XCTAssertTrue(input.bytes.isEmpty)
        board.clearContents()
        board.setString("hello", forType: .string)
        view.feedOutput(Data("\u{1b}[?2004h".utf8))
        view.paste(from: board)
        XCTAssertEqual(String(decoding: input.bytes, as: UTF8.self), "\u{1b}[200~hello\u{1b}[201~")
    }

    func testPasteUploadsPNGAndTargetsSourceWorkspaceAfterSwitch() throws {
        let first = OnePaneCat(label: "image-paste-source")
        let second = OnePaneCat(label: "image-paste-other")
        let app = try AppE2E.attachWindow(socket: first.socket, session: first.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())
        let sourceView = app.testActiveTerminalView()
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setData(try image(), forType: .png)
        var uploaded: Data?
        let handler = try XCTUnwrap(sourceView.onImagePaste)
        sourceView.onImagePaste = { png in uploaded = png; handler(png) }
        sourceView.paste(from: board)
        let other = try CoreBridge(backendType: "tmux", socket: second.socket, session: second.session)
        app.testActivateWorkspaceBridge(other, session: second.session)
        XCTAssertTrue(AppE2E.wait(timeout: 5) {
            app.testPollOnce()
            return uploaded != nil && sourceView.visibleScreenText().contains(".png")
        })
        let text = sourceView.visibleScreenText().components(separatedBy: .whitespacesAndNewlines).joined()
        let pattern = try NSRegularExpression(pattern: "/[^\\s]*muxterm-paste-[a-f0-9]+\\.png")
        let match = try XCTUnwrap(pattern.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)))
        let path = (text as NSString).substring(with: match.range)
        defer { try? FileManager.default.removeItem(atPath: path) }
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: path)), uploaded)
        XCTAssertEqual(app.testActiveWorkspaceSession(), second.session)
        XCTAssertFalse(app.testActivePaneTerminalText().contains("muxterm-paste-"))
    }
}

extension ImagePasteE2ETests {
    /// 用户抱怨「粘贴后没反应，过一会才出现目录」：真实链路里，横幅必须在粘贴一按下就
    /// 出现进度，路径写进 pane 之后落成「已粘贴」，并最终自己消失。
    func testBannerShowsProgressThenResultThenGoesAway() throws {
        let fixture = OnePaneCat(label: "image-paste-banner")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())
        XCTAssertFalse(app.testImagePasteBannerVisible(), "没粘贴时不能有横幅")

        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setData(try image(), forType: .png)
        let view = app.testActiveTerminalView()
        view.paste(from: board)

        // 编码在后台线程，但「开始编码」是同步通知：返回时横幅必须已经画出来。
        XCTAssertTrue(app.testImagePasteBannerVisible(), "按下粘贴的当下就要有反馈")
        XCTAssertEqual(app.testImagePasteStage(), .encoding)
        XCTAssertTrue(app.testImagePasteBannerSpinning())
        XCTAssertFalse(app.testImagePasteBannerDismissVisible())

        var sawUploading = false
        XCTAssertTrue(AppE2E.wait(timeout: 8) {
            app.testPollOnce()
            if app.testImagePasteStage() == .uploading { sawUploading = true }
            if case .done = app.testImagePasteStage() { return true }
            return false
        }, "stage=\(app.testImagePasteStage())")
        _ = sawUploading // 本地写盘很快，uploading 可能一个 tick 都撑不住；done 才是必须的。

        guard case .done(let path) = app.testImagePasteStage() else {
            return XCTFail("expected done, got \(app.testImagePasteStage())")
        }
        defer { try? FileManager.default.removeItem(atPath: path) }
        XCTAssertTrue(path.hasPrefix("/tmp/muxterm-paste-"), path)
        XCTAssertTrue(app.testImagePasteBannerMessage().contains((path as NSString).lastPathComponent))
        XCTAssertFalse(app.testImagePasteBannerSpinning())
        XCTAssertTrue(app.testImagePasteBannerDismissVisible())

        // 结果停留一小会儿后自己消失，不需要用户去关。
        XCTAssertTrue(AppE2E.wait(timeout: ImagePasteFeedback.successHold + 3) {
            app.testPollOnce()
            return !app.testImagePasteBannerVisible()
        }, "成功提示必须自己消失")
    }

    /// 失败要看得见：图片无效时横幅给出原因，用户可以手动关掉。
    func testInvalidImageShowsAFailureBannerThatCanBeDismissed() throws {
        let fixture = OnePaneCat(label: "image-paste-fail")
        let app = try AppE2E.attachWindow(socket: fixture.socket, session: fixture.session)
        defer { app.testShutdown() }
        XCTAssertTrue(app.waitReady())

        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setData(Data("broken".utf8), forType: .png)
        app.testActiveTerminalView().paste(from: board)

        XCTAssertTrue(AppE2E.wait(timeout: 5) {
            app.testPollOnce()
            if case .failed = app.testImagePasteStage() { return true }
            return false
        }, "stage=\(app.testImagePasteStage())")
        XCTAssertTrue(app.testImagePasteBannerVisible())
        XCTAssertTrue(
            app.testImagePasteBannerMessage().contains(MuxtermI18n.shared.tr(.imagePasteInvalid)),
            app.testImagePasteBannerMessage()
        )
        XCTAssertTrue(app.testImagePasteBannerDismissVisible())
        XCTAssertTrue(app.testClickImagePasteDismiss())
        XCTAssertFalse(app.testImagePasteBannerVisible(), "手动关闭后横幅必须消失")
    }
}

private final class ImagePasteInputRecorder: TerminalInputHandler {
    var bytes: [UInt8] = []
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) { bytes.append(contentsOf: data) }
    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {}
}
