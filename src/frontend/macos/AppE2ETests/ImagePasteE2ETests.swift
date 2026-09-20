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

private final class ImagePasteInputRecorder: TerminalInputHandler {
    var bytes: [UInt8] = []
    func terminal(_ view: MuxTerminalView, send data: ArraySlice<UInt8>) { bytes.append(contentsOf: data) }
    func terminal(_ view: MuxTerminalView, sizeChanged cols: Int, rows: Int) {}
}
