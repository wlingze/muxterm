import XCTest
@testable import MuxtermChrome

final class ImagePasteFeedbackTests: XCTestCase {
    private let t0 = Date(timeIntervalSinceReferenceDate: 1_000)

    private func at(_ seconds: TimeInterval) -> Date { t0.addingTimeInterval(seconds) }

    // MARK: 阶段 → 展示

    func testIdleHasNoBanner() {
        XCTAssertNil(ImagePasteFeedback.make(.idle))
    }

    func testEncodingAndUploadingSpinWithoutResultDetail() {
        for stage in [ImagePasteStage.encoding, .uploading] {
            let feedback = ImagePasteFeedback.make(stage)
            XCTAssertEqual(feedback?.tone, .progress)
            XCTAssertEqual(feedback?.showsSpinner, true)
            XCTAssertNil(feedback?.detail)
        }
    }

    func testDoneCarriesTheFullPathAndStopsSpinning() {
        let feedback = ImagePasteFeedback.make(.done(path: "/tmp/muxterm-paste-ab12.png"))
        XCTAssertEqual(feedback?.tone, .success)
        XCTAssertEqual(feedback?.showsSpinner, false)
        XCTAssertEqual(feedback?.detail, "/tmp/muxterm-paste-ab12.png")
    }

    func testFailureCarriesTheMessage() {
        let feedback = ImagePasteFeedback.make(.failed(message: "SSH image transfer timed out"))
        XCTAssertEqual(feedback?.tone, .failure)
        XCTAssertEqual(feedback?.showsSpinner, false)
        XCTAssertEqual(feedback?.detail, "SSH image transfer timed out")
    }

    func testFileNameKeepsOnlyTheLastPathComponent() {
        XCTAssertEqual(
            ImagePasteFeedback.fileName(of: "/tmp/muxterm-paste-ab12.png"),
            "muxterm-paste-ab12.png"
        )
        XCTAssertEqual(ImagePasteFeedback.fileName(of: "relative.png"), "relative.png")
        // 空路径和只有斜杠的路径不能显示成空白。
        XCTAssertEqual(ImagePasteFeedback.fileName(of: ""), "")
        XCTAssertEqual(ImagePasteFeedback.fileName(of: "/"), "/")
        XCTAssertEqual(ImagePasteFeedback.fileName(of: "/tmp/dir/"), "dir")
    }

    func testOnlyResultsAutoDismissAndFailuresStayLonger() {
        XCTAssertNil(ImagePasteFeedback.hold(for: .idle))
        XCTAssertNil(ImagePasteFeedback.hold(for: .encoding))
        XCTAssertNil(ImagePasteFeedback.hold(for: .uploading))
        XCTAssertEqual(ImagePasteFeedback.hold(for: .done(path: "/tmp/a.png")), ImagePasteFeedback.successHold)
        XCTAssertEqual(ImagePasteFeedback.hold(for: .failed(message: "x")), ImagePasteFeedback.failureHold)
        XCTAssertGreaterThan(ImagePasteFeedback.failureHold, ImagePasteFeedback.successHold)
    }

    // MARK: 时间线

    func testNewTimelineIsIdleAndNotBusy() {
        var timeline = ImagePasteTimeline()
        timeline.tick(now: at(0))
        XCTAssertEqual(timeline.visibleStage(now: at(0)), .idle)
        XCTAssertFalse(timeline.isBusy)
    }

    func testProgressStaysVisibleUntilAResultArrives() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .encoding, now: at(0))
        timeline.advance(to: .uploading, now: at(0.1))
        // 上传可能很久：进度不会因为超时而自己消失。
        timeline.tick(now: at(120))
        XCTAssertEqual(timeline.visibleStage(now: at(120)), .uploading)
        XCTAssertTrue(timeline.isBusy)
    }

    func testResultThatArrivesInstantlyWaitsForTheMinimumVisibleTime() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .uploading, now: at(0))
        // 本地写盘几毫秒就完成：进度必须至少画出来 minimumVisible，用户才看得见。
        timeline.advance(to: .done(path: "/tmp/a.png"), now: at(0.02))
        XCTAssertEqual(timeline.visibleStage(now: at(0.02)), .uploading)
        XCTAssertTrue(timeline.isBusy, "结果还没落下来时仍算忙，避免此时再开始第二次粘贴")

        timeline.tick(now: at(ImagePasteFeedback.minimumVisible + 0.01))
        XCTAssertEqual(timeline.visibleStage(now: at(ImagePasteFeedback.minimumVisible + 0.01)),
                       .done(path: "/tmp/a.png"))
        XCTAssertFalse(timeline.isBusy)
    }

    func testResultAfterTheMinimumVisibleTimeShowsImmediately() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .uploading, now: at(0))
        timeline.advance(to: .done(path: "/tmp/a.png"), now: at(3))
        XCTAssertEqual(timeline.visibleStage(now: at(3)), .done(path: "/tmp/a.png"))
    }

    func testSuccessDisappearsAfterItsHoldTime() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .uploading, now: at(0))
        timeline.advance(to: .done(path: "/tmp/a.png"), now: at(2))
        timeline.tick(now: at(2 + ImagePasteFeedback.successHold - 0.1))
        XCTAssertEqual(timeline.visibleStage(now: at(2 + ImagePasteFeedback.successHold - 0.1)),
                       .done(path: "/tmp/a.png"))
        timeline.tick(now: at(2 + ImagePasteFeedback.successHold + 0.1))
        XCTAssertEqual(timeline.stage, .idle)
        XCTAssertEqual(timeline.visibleStage(now: at(2 + ImagePasteFeedback.successHold + 0.1)), .idle)
    }

    func testFailureStaysLongerThanSuccess() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .uploading, now: at(0))
        timeline.advance(to: .failed(message: "boom"), now: at(2))
        let afterSuccessHold = at(2 + ImagePasteFeedback.successHold + 0.1)
        timeline.tick(now: afterSuccessHold)
        XCTAssertEqual(timeline.visibleStage(now: afterSuccessHold), .failed(message: "boom"))
        let afterFailureHold = at(2 + ImagePasteFeedback.failureHold + 0.1)
        timeline.tick(now: afterFailureHold)
        XCTAssertEqual(timeline.stage, .idle)
    }

    func testUserCanDismissAResultButNotAProgressStage() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .uploading, now: at(0))
        timeline.dismiss()
        XCTAssertEqual(timeline.stage, .uploading, "进行中的粘贴不能被关掉，否则用户会重复粘贴")

        timeline.advance(to: .failed(message: "boom"), now: at(2))
        timeline.dismiss()
        XCTAssertEqual(timeline.stage, .idle)
        XCTAssertFalse(timeline.isBusy)
    }

    func testANewPasteReplacesAnOldResultImmediately() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .uploading, now: at(0))
        timeline.advance(to: .done(path: "/tmp/a.png"), now: at(2))
        // 结果还在显示时又粘贴了一张：马上回到进度，不能等旧结果过期。
        timeline.advance(to: .encoding, now: at(3))
        XCTAssertEqual(timeline.visibleStage(now: at(3)), .encoding)
        XCTAssertTrue(timeline.isBusy)
    }

    func testRepeatingTheSameStageDoesNotRestartItsClock() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .uploading, now: at(0))
        // 事件泵每个 tick 都可能重复报告「仍在上传」，不能因此把最短显示时间一直往后推。
        timeline.advance(to: .uploading, now: at(0.3))
        timeline.advance(to: .done(path: "/tmp/a.png"), now: at(0.45))
        timeline.tick(now: at(ImagePasteFeedback.minimumVisible + 0.01))
        XCTAssertEqual(timeline.visibleStage(now: at(ImagePasteFeedback.minimumVisible + 0.01)),
                       .done(path: "/tmp/a.png"))
    }

    func testDismissClearsAPendingResultToo() {
        var timeline = ImagePasteTimeline()
        timeline.advance(to: .uploading, now: at(0))
        timeline.advance(to: .failed(message: "boom"), now: at(0.01))
        XCTAssertTrue(timeline.isBusy)
        timeline.tick(now: at(1))
        timeline.dismiss()
        timeline.tick(now: at(2))
        XCTAssertEqual(timeline.visibleStage(now: at(2)), .idle)
        XCTAssertFalse(timeline.isBusy)
    }
}
