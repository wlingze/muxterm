//! 图片粘贴的阶段与横幅时间线。
//!
//! 对照 macOS `Chrome/ImagePasteFeedback.swift` 与
//! `ChromeTests/ImagePasteFeedbackTests.swift`（18 个用例）。
//!
//! 粘贴一张图要走「读剪贴板并转 PNG → 上传到目标机器 → 把路径写进 pane」。
//! 上传阶段对 SSH 目标可能是几百毫秒到数十秒，这段时间终端里什么都不会出现，
//! 所以每个阶段都要有用户马上能看见的提示，结果也要在屏幕上留一小会儿。

use std::time::{Duration, Instant};

/// 图片粘贴的用户可见阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImagePasteStage {
    /// 没有进行中的粘贴，也没有需要展示的结果。
    Idle,
    /// 正在读剪贴板并转成 PNG。
    Encoding,
    /// 正在把 PNG 传到目标机器。
    Uploading,
    /// 已把路径写进原 pane。`path` 是目标机器上的绝对路径。
    Done(String),
    /// 失败；消息直接给用户看，不再只藏在状态栏红点的 tooltip 里。
    Failed(String),
}

impl ImagePasteStage {
    /// 进行中：不会自己消失，也不可关闭。
    pub fn is_progress(&self) -> bool {
        matches!(self, Self::Encoding | Self::Uploading)
    }

    /// 结果阶段：停留 `ImagePasteFeedback::hold` 后自动消失，可手动关闭。
    pub fn is_result(&self) -> bool {
        matches!(self, Self::Done(_) | Self::Failed(_))
    }
}

/// 横幅的语义色调（gpui 侧渲染用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Progress,
    Success,
    Failure,
}

/// 阶段到横幅内容的映射（纯逻辑，便于单测）。
#[derive(Debug, Clone, PartialEq)]
pub struct ImagePasteFeedback {
    pub stage: ImagePasteStage,
    pub tone: Tone,
    /// 进行中的阶段为 true：横幅显示转圈。
    pub shows_spinner: bool,
    /// 成功时是目标机器上的完整路径，失败时是错误消息；进行中为 None。
    pub detail: Option<String>,
}

impl ImagePasteFeedback {
    /// 成功提示停留时间：足够看清「已粘贴」又不挡终端。
    pub const SUCCESS_HOLD: Duration = Duration::from_millis(2500);
    /// 失败提示停留更久，用户也可以手动关掉。
    pub const FAILURE_HOLD: Duration = Duration::from_secs(10);
    /// 极短的进度也要让横幅画出来，避免一帧内闪过。
    pub const MINIMUM_VISIBLE: Duration = Duration::from_millis(400);

    /// 阶段对应的展示；`Idle` 返回 None，表示横幅应隐藏。
    pub fn make(stage: &ImagePasteStage) -> Option<Self> {
        match stage {
            ImagePasteStage::Idle => None,
            ImagePasteStage::Encoding | ImagePasteStage::Uploading => Some(Self {
                stage: stage.clone(),
                tone: Tone::Progress,
                shows_spinner: true,
                detail: None,
            }),
            ImagePasteStage::Done(path) => Some(Self {
                stage: stage.clone(),
                tone: Tone::Success,
                shows_spinner: false,
                detail: Some(path.clone()),
            }),
            ImagePasteStage::Failed(message) => Some(Self {
                stage: stage.clone(),
                tone: Tone::Failure,
                shows_spinner: false,
                detail: Some(message.clone()),
            }),
        }
    }

    /// 路径最后一段；空路径或只有斜杠时退回原字符串，避免横幅显示成空白。
    pub fn file_name(path: &str) -> String {
        path.rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or(path)
            .to_string()
    }

    /// 结果阶段之后多久自动消失；进行中的阶段不自动消失。
    pub fn hold(stage: &ImagePasteStage) -> Option<Duration> {
        match stage {
            ImagePasteStage::Done(_) => Some(Self::SUCCESS_HOLD),
            ImagePasteStage::Failed(_) => Some(Self::FAILURE_HOLD),
            ImagePasteStage::Idle | ImagePasteStage::Encoding | ImagePasteStage::Uploading => None,
        }
    }
}

/// 横幅时间线：进度至少停留 `MINIMUM_VISIBLE`，结果停留 `hold` 后回到 `Idle`。
///
/// 不依赖 UI，由事件泵每个 tick 驱动，所以可以直接单测。
#[derive(Debug, Clone)]
pub struct ImagePasteTimeline {
    stage: ImagePasteStage,
    shown_since: Option<Instant>,
    /// 进度还没到最短显示时间就到了的结果：先挂起，到点再落下来。
    pending: Option<ImagePasteStage>,
}

impl Default for ImagePasteTimeline {
    fn default() -> Self {
        Self::new()
    }
}

impl ImagePasteTimeline {
    pub fn new() -> Self {
        Self {
            stage: ImagePasteStage::Idle,
            shown_since: None,
            pending: None,
        }
    }

    /// 当前阶段（可能还是挂起前的进度）。
    pub fn stage(&self) -> &ImagePasteStage {
        &self.stage
    }

    /// 进行中，或还有挂起的结果没有落下来。此时不应开始第二次粘贴。
    pub fn is_busy(&self) -> bool {
        self.stage.is_progress() || self.pending.is_some()
    }

    /// 当前应显示的阶段：结果超过停留时间后即使还没 tick 也算已消失。
    pub fn visible_stage(&self, now: Instant) -> ImagePasteStage {
        let Some(shown_since) = self.shown_since else {
            return self.stage.clone();
        };
        let Some(hold) = ImagePasteFeedback::hold(&self.stage) else {
            return self.stage.clone();
        };
        if now.duration_since(shown_since) >= hold {
            ImagePasteStage::Idle
        } else {
            self.stage.clone()
        }
    }

    /// 推进到新阶段。
    pub fn advance(&mut self, next: ImagePasteStage, now: Instant) {
        // 进度刚开始就收到结果：先让进度画出来，结果挂起。
        if self.stage.is_progress() && next.is_result() {
            if let Some(shown_since) = self.shown_since {
                if now.duration_since(shown_since) < ImagePasteFeedback::MINIMUM_VISIBLE {
                    self.pending = Some(next);
                    return;
                }
            }
        }
        self.pending = None;
        // 同一阶段重复报告不重置计时，否则最短显示时间会被一直往后推。
        if next == self.stage {
            return;
        }
        self.stage = next;
        self.shown_since = Some(now);
    }

    /// 每个 tick 调用：到点落下挂起的结果，并清掉过期的结果。
    pub fn tick(&mut self, now: Instant) {
        if let Some(pending) = self.pending.clone() {
            if let Some(shown_since) = self.shown_since {
                if now.duration_since(shown_since) >= ImagePasteFeedback::MINIMUM_VISIBLE {
                    self.pending = None;
                    self.stage = pending;
                    self.shown_since = Some(now);
                    return;
                }
            }
        }
        if self.pending.is_none()
            && self.visible_stage(now) == ImagePasteStage::Idle
            && self.stage != ImagePasteStage::Idle
        {
            self.stage = ImagePasteStage::Idle;
            self.shown_since = None;
        }
    }

    /// 直接回到 Idle，不展示结果。
    ///
    /// 输入框把路径插进草稿后不该再弹一条成功横幅（路径本身已经可见），
    /// 但之前的上传进度必须收掉，否则会留下永远转圈的横幅。
    pub fn clear(&mut self) {
        self.stage = ImagePasteStage::Idle;
        self.shown_since = None;
        self.pending = None;
    }

    /// 用户手动关掉当前结果。进行中的阶段不可关闭，否则用户会重复粘贴。
    pub fn dismiss(&mut self) {
        if !self.stage.is_result() {
            return;
        }
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 所有用例共用同一个时间原点，对应 Swift 的 `t0`。
    fn t0() -> Instant {
        Instant::now()
    }

    fn at(base: Instant, seconds: f32) -> Instant {
        base + Duration::from_secs_f32(seconds)
    }

    fn done(path: &str) -> ImagePasteStage {
        ImagePasteStage::Done(path.to_string())
    }

    fn failed(message: &str) -> ImagePasteStage {
        ImagePasteStage::Failed(message.to_string())
    }

    // MARK: 阶段 → 展示

    /// 对照 `testIdleHasNoBanner`。
    #[test]
    fn idle_has_no_banner() {
        assert!(ImagePasteFeedback::make(&ImagePasteStage::Idle).is_none());
    }

    /// 对照 `testEncodingAndUploadingSpinWithoutResultDetail`。
    #[test]
    fn encoding_and_uploading_spin_without_detail() {
        for stage in [ImagePasteStage::Encoding, ImagePasteStage::Uploading] {
            let feedback = ImagePasteFeedback::make(&stage).unwrap();
            assert_eq!(feedback.tone, Tone::Progress);
            assert!(feedback.shows_spinner);
            assert_eq!(feedback.detail, None);
        }
    }

    /// 对照 `testDoneCarriesTheFullPathAndStopsSpinning`。
    #[test]
    fn done_carries_the_full_path_and_stops_spinning() {
        let feedback = ImagePasteFeedback::make(&done("/tmp/muxterm-paste-ab12.png")).unwrap();
        assert_eq!(feedback.tone, Tone::Success);
        assert!(!feedback.shows_spinner);
        assert_eq!(
            feedback.detail.as_deref(),
            Some("/tmp/muxterm-paste-ab12.png")
        );
    }

    /// 对照 `testFailureCarriesTheMessage`。
    #[test]
    fn failure_carries_the_message() {
        let feedback = ImagePasteFeedback::make(&failed("SSH image transfer timed out")).unwrap();
        assert_eq!(feedback.tone, Tone::Failure);
        assert!(!feedback.shows_spinner);
        assert_eq!(
            feedback.detail.as_deref(),
            Some("SSH image transfer timed out")
        );
    }

    /// 对照 `testFileNameKeepsOnlyTheLastPathComponent`。
    #[test]
    fn file_name_keeps_only_the_last_path_component() {
        assert_eq!(
            ImagePasteFeedback::file_name("/tmp/muxterm-paste-ab12.png"),
            "muxterm-paste-ab12.png"
        );
        assert_eq!(
            ImagePasteFeedback::file_name("relative.png"),
            "relative.png"
        );
        // 空路径和只有斜杠的路径不能显示成空白。
        assert_eq!(ImagePasteFeedback::file_name(""), "");
        assert_eq!(ImagePasteFeedback::file_name("/"), "/");
        assert_eq!(ImagePasteFeedback::file_name("/tmp/dir/"), "dir");
    }

    /// 对照 `testOnlyResultsAutoDismissAndFailuresStayLonger`。
    #[test]
    fn only_results_auto_dismiss_and_failures_stay_longer() {
        assert_eq!(ImagePasteFeedback::hold(&ImagePasteStage::Idle), None);
        assert_eq!(ImagePasteFeedback::hold(&ImagePasteStage::Encoding), None);
        assert_eq!(ImagePasteFeedback::hold(&ImagePasteStage::Uploading), None);
        assert_eq!(
            ImagePasteFeedback::hold(&done("/tmp/a.png")),
            Some(ImagePasteFeedback::SUCCESS_HOLD)
        );
        assert_eq!(
            ImagePasteFeedback::hold(&failed("x")),
            Some(ImagePasteFeedback::FAILURE_HOLD)
        );
        assert!(ImagePasteFeedback::FAILURE_HOLD > ImagePasteFeedback::SUCCESS_HOLD);
    }

    // MARK: 时间线

    /// 对照 `testNewTimelineIsIdleAndNotBusy`。
    #[test]
    fn new_timeline_is_idle_and_not_busy() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.tick(at(base, 0.0));
        assert_eq!(timeline.visible_stage(at(base, 0.0)), ImagePasteStage::Idle);
        assert!(!timeline.is_busy());
    }

    /// 对照 `testProgressStaysVisibleUntilAResultArrives`。
    #[test]
    fn progress_stays_visible_until_a_result_arrives() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Encoding, at(base, 0.0));
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.1));
        // 上传可能很久：进度不会因为超时而自己消失。
        timeline.tick(at(base, 120.0));
        assert_eq!(
            timeline.visible_stage(at(base, 120.0)),
            ImagePasteStage::Uploading
        );
        assert!(timeline.is_busy());
    }

    /// 对照 `testResultThatArrivesInstantlyWaitsForTheMinimumVisibleTime`。
    #[test]
    fn result_that_arrives_instantly_waits_for_the_minimum_visible_time() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        // 本地写盘几毫秒就完成：进度必须至少画出来 MINIMUM_VISIBLE，用户才看得见。
        timeline.advance(done("/tmp/a.png"), at(base, 0.02));
        assert_eq!(
            timeline.visible_stage(at(base, 0.02)),
            ImagePasteStage::Uploading
        );
        assert!(
            timeline.is_busy(),
            "结果还没落下来时仍算忙，避免此时再开始第二次粘贴"
        );

        let landed = ImagePasteFeedback::MINIMUM_VISIBLE.as_secs_f32() + 0.01;
        timeline.tick(at(base, landed));
        assert_eq!(timeline.visible_stage(at(base, landed)), done("/tmp/a.png"));
        assert!(!timeline.is_busy());
    }

    /// 对照 `testResultAfterTheMinimumVisibleTimeShowsImmediately`。
    #[test]
    fn result_after_the_minimum_visible_time_shows_immediately() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        timeline.advance(done("/tmp/a.png"), at(base, 3.0));
        assert_eq!(timeline.visible_stage(at(base, 3.0)), done("/tmp/a.png"));
    }

    /// 对照 `testSuccessDisappearsAfterItsHoldTime`。
    #[test]
    fn success_disappears_after_its_hold_time() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        timeline.advance(done("/tmp/a.png"), at(base, 2.0));

        let almost = 2.0 + ImagePasteFeedback::SUCCESS_HOLD.as_secs_f32() - 0.1;
        timeline.tick(at(base, almost));
        assert_eq!(timeline.visible_stage(at(base, almost)), done("/tmp/a.png"));

        let after = 2.0 + ImagePasteFeedback::SUCCESS_HOLD.as_secs_f32() + 0.1;
        timeline.tick(at(base, after));
        assert_eq!(timeline.stage(), &ImagePasteStage::Idle);
        assert_eq!(
            timeline.visible_stage(at(base, after)),
            ImagePasteStage::Idle
        );
    }

    /// 对照 `testFailureStaysLongerThanSuccess`。
    #[test]
    fn failure_stays_longer_than_success() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        timeline.advance(failed("boom"), at(base, 2.0));

        let after_success = 2.0 + ImagePasteFeedback::SUCCESS_HOLD.as_secs_f32() + 0.1;
        timeline.tick(at(base, after_success));
        assert_eq!(
            timeline.visible_stage(at(base, after_success)),
            failed("boom")
        );

        let after_failure = 2.0 + ImagePasteFeedback::FAILURE_HOLD.as_secs_f32() + 0.1;
        timeline.tick(at(base, after_failure));
        assert_eq!(timeline.stage(), &ImagePasteStage::Idle);
    }

    /// 对照 `testUserCanDismissAResultButNotAProgressStage`。
    #[test]
    fn user_can_dismiss_a_result_but_not_a_progress_stage() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        timeline.dismiss();
        assert_eq!(
            timeline.stage(),
            &ImagePasteStage::Uploading,
            "进行中的粘贴不能被关掉，否则用户会重复粘贴"
        );

        timeline.advance(failed("boom"), at(base, 2.0));
        timeline.dismiss();
        assert_eq!(timeline.stage(), &ImagePasteStage::Idle);
        assert!(!timeline.is_busy());
    }

    /// 对照 `testANewPasteReplacesAnOldResultImmediately`。
    #[test]
    fn a_new_paste_replaces_an_old_result_immediately() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        timeline.advance(done("/tmp/a.png"), at(base, 2.0));
        // 结果还在显示时又粘贴了一张：马上回到进度，不能等旧结果过期。
        timeline.advance(ImagePasteStage::Encoding, at(base, 3.0));
        assert_eq!(
            timeline.visible_stage(at(base, 3.0)),
            ImagePasteStage::Encoding
        );
        assert!(timeline.is_busy());
    }

    /// 对照 `testRepeatingTheSameStageDoesNotRestartItsClock`。
    #[test]
    fn repeating_the_same_stage_does_not_restart_its_clock() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        // 事件泵每个 tick 都可能重复报告「仍在上传」，不能因此把最短显示时间往后推。
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.3));
        timeline.advance(done("/tmp/a.png"), at(base, 0.45));
        let landed = ImagePasteFeedback::MINIMUM_VISIBLE.as_secs_f32() + 0.01;
        timeline.tick(at(base, landed));
        assert_eq!(timeline.visible_stage(at(base, landed)), done("/tmp/a.png"));
    }

    /// 对照 `testClearHidesProgressWithoutShowingAResult`。
    #[test]
    fn clear_hides_progress_without_showing_a_result() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        assert!(timeline.is_busy());
        timeline.clear();
        assert_eq!(timeline.stage(), &ImagePasteStage::Idle);
        assert!(!timeline.is_busy());
    }

    /// 对照 `testClearDropsAPendingResultToo`。
    #[test]
    fn clear_drops_a_pending_result_too() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Encoding, at(base, 0.0));
        timeline.advance(done("/tmp/a.png"), at(base, 0.05));
        timeline.clear();
        timeline.tick(at(base, 5.0));
        assert_eq!(timeline.stage(), &ImagePasteStage::Idle);
    }

    /// 对照 `testDismissClearsAPendingResultToo`。
    #[test]
    fn dismiss_clears_a_pending_result_too() {
        let base = t0();
        let mut timeline = ImagePasteTimeline::new();
        timeline.advance(ImagePasteStage::Uploading, at(base, 0.0));
        timeline.advance(failed("boom"), at(base, 0.01));
        assert!(timeline.is_busy());
        timeline.tick(at(base, 1.0));
        timeline.dismiss();
        timeline.tick(at(base, 2.0));
        assert_eq!(timeline.visible_stage(at(base, 2.0)), ImagePasteStage::Idle);
        assert!(!timeline.is_busy());
    }
}
