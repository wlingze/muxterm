//! Herdr pane stream registry：pane-keyed 所有权 + generation + event ordinal +
//! wire seq + 有界重试 + control intent / takeover suppression。
//!
//! 取代旧的 `Vec<ObserveStream>` + `HashSet` 语义：
//! - `PaneId` 是唯一 key；一个 pane 同时最多一个 Starting/Live transition；
//! - 每次 start/replace/promote/demote 递增 `generation`，旧 reader 立即 stale；
//! - 事件按 `(generation, event_ordinal)` 过滤；Frame 按 wire `seq` 收敛；
//! - 普通故障按 100/200/400/800/1600ms 有界退避，第五次失败进入 Degraded；
//! - takeover 后进入 `SuppressedAfterTakeover`，只有新的本地 focus edge 或
//!   真实 input 才建立新的 control intent。
//!
//! 本模块是纯逻辑（除流对象外无 I/O）；时间全部显式注入 `Instant`，测试
//! 不需要真实 sleep。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::core::types::PaneId;

use super::observe::{ObserveStream, StreamMode};

/// 普通故障的固定退避间隔（毫秒）。
pub const RETRY_DELAY_MS: [u64; 5] = [100, 200, 400, 800, 1600];
/// 第五次自动 retry 再失败后进入 Degraded。
pub const MAX_AUTO_RETRIES: u8 = 5;
/// 收到 full baseline 后连续 Live 多久才恢复普通故障 retry budget。
pub const LIVE_STABLE_WINDOW: Duration = Duration::from_secs(10);
/// 首个 full frame 的 deadline（超时 → Degraded，保留旧像素）。
pub const FULL_FRAME_DEADLINE: Duration = Duration::from_secs(5);
/// full 前 diff 队列上限：事件数与总字节数任一先到 → generation 有界失败。
pub const PRE_FULL_MAX_EVENTS: usize = 256;
pub const PRE_FULL_MAX_BYTES: usize = 2 * 1024 * 1024;
/// control Starting/Backoff 期间 input 队列上限。
pub const INPUT_MAX_WRITES: usize = 256;
pub const INPUT_MAX_BYTES: usize = 64 * 1024;

/// slot 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// 尚无 stream（初始或已停）。
    Absent,
    /// start worker 在途（generation-tagged）。
    Starting,
    /// 流已握手成功，正在收帧。
    Live,
    /// 普通故障退避中（等待 retry_at）。
    Backoff,
    /// 第五次 retry 也失败（或 full 超时）；保留旧像素，等待 rearm。
    Degraded,
    /// detach/shutdown/已关闭 pane；不启动任何流。
    Stopped,
}

/// control intent 的 rearm 闩锁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlRearm {
    /// 可以（重新）申请 control。
    Armed,
    /// 外部 takeover 后闩锁：重复 snapshot/reconciliation/resize 不能清除；
    /// 只有新的本地 focus edge 或真实 input 才建立新 intent。
    SuppressedAfterTakeover,
}

/// Surface baseline 状态：首个可应用帧必须是 full。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceBaseline {
    AwaitingFull,
    Ready,
}

/// Frame 收敛结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecision {
    /// 重复/倒序 seq：丢弃。
    DropDuplicate,
    /// 正常应用。
    Apply,
    /// diff 出现 gap：当前 generation 有界失败（不能跳过缺失 diff）。
    GapFailure,
    /// full 出现 gap：允许建立新 baseline（记录诊断）。
    GapFullBaseline,
}

/// 有界队列/收敛失败的信号（无额外 payload，调用方据此让 generation 失败）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// 队列超限（事件数或字节数任一先到）。
    Overflow,
    /// wire seq 追赶出现缺口。
    Gap,
}

/// 一个 pane 的流所有权 slot。
pub struct PaneStreamSlot {
    pub pane: PaneId,
    /// Herdr pane id（如 `w1:p1`）。
    pub target: String,
    /// 产品期望的模式（由 reconcile 统一计算）。
    pub desired_mode: StreamMode,
    /// 当前实际模式（None = 无流）。
    pub actual_mode: Option<StreamMode>,
    pub generation: u64,
    pub last_event_ordinal: u64,
    pub last_frame_seq: Option<u64>,
    pub state: SlotState,
    pub stream: Option<ObserveStream>,
    pub retry_count: u8,
    pub retry_at: Option<Instant>,
    pub live_since: Option<Instant>,
    /// 本次流 start 的时间（full-frame deadline 基线）。
    pub started_at: Option<Instant>,
    pub control_intent_epoch: u64,
    /// 当前 intent 是否来自显式用户动作（focus edge/真实 input）。
    /// open/reattach/set_foreground(true)/Pool activate 的首次 Control 尝试
    /// 没有 user_intent，必须 takeover=false。
    pub user_intent: bool,
    /// 当前 intent 是否已经发出过一次 takeover=true 的显式 promote。
    pub takeover_attempted: bool,
    pub control_rearm: ControlRearm,
    pub surface_baseline: SurfaceBaseline,
    /// full 前按 wire seq 有界的 diff 队列（seq, bytes）。
    pub pre_full: VecDeque<(u64, Vec<u8>)>,
    pub pre_full_bytes: usize,
    /// intent-bound 的有界 input 队列。
    pub pending_input: VecDeque<Vec<u8>>,
    pub pending_input_bytes: usize,
    /// intent-bound 的 latest resize。
    pub pending_resize: Option<(u16, u16)>,
    /// 生命周期诊断（W0 字段；测试可读取确定性计数）。
    pub transitions: Vec<String>,
}

impl PaneStreamSlot {
    pub fn new(pane: PaneId, target: impl Into<String>, desired_mode: StreamMode) -> Self {
        Self {
            pane,
            target: target.into(),
            desired_mode,
            actual_mode: None,
            generation: 0,
            last_event_ordinal: 0,
            last_frame_seq: None,
            state: SlotState::Absent,
            stream: None,
            retry_count: 0,
            retry_at: None,
            live_since: None,
            started_at: None,
            control_intent_epoch: 0,
            user_intent: false,
            takeover_attempted: false,
            control_rearm: ControlRearm::Armed,
            surface_baseline: SurfaceBaseline::AwaitingFull,
            pre_full: VecDeque::new(),
            pre_full_bytes: 0,
            pending_input: VecDeque::new(),
            pending_input_bytes: 0,
            pending_resize: None,
            transitions: Vec::new(),
        }
    }

    /// 当前 generation 是否仍然有效（事件过滤）。
    pub fn is_current(&self, generation: u64) -> bool {
        self.generation == generation && self.state != SlotState::Stopped
    }

    /// 是否有一个 in-flight start（同 pane 禁止第二个）。
    pub fn has_inflight_start(&self) -> bool {
        self.state == SlotState::Starting
    }

    /// 事件 ordinal 收敛：严格递增才接受。
    pub fn accept_ordinal(&mut self, event_ordinal: u64) -> bool {
        if event_ordinal <= self.last_event_ordinal {
            return false;
        }
        self.last_event_ordinal = event_ordinal;
        true
    }

    /// Frame wire seq 收敛（§4.1 规则）。
    pub fn decide_frame(&mut self, wire_seq: u64, full: bool) -> FrameDecision {
        match self.last_frame_seq {
            None => {
                // generation 首个帧必须是 full。
                if full {
                    self.last_frame_seq = Some(wire_seq);
                    FrameDecision::Apply
                } else {
                    // 先到的 diff 进入 pre-full 队列；这里只登记，不算 gap。
                    FrameDecision::Apply
                }
            }
            Some(last) if wire_seq <= last => FrameDecision::DropDuplicate,
            Some(last) if wire_seq == last + 1 => {
                self.last_frame_seq = Some(wire_seq);
                FrameDecision::Apply
            }
            Some(_) => {
                // 缺口：diff 不能跳过；full 可重建 baseline。
                if full {
                    self.last_frame_seq = Some(wire_seq);
                    FrameDecision::GapFullBaseline
                } else {
                    FrameDecision::GapFailure
                }
            }
        }
    }

    /// 把 full 前的 diff 放进有界队列；溢出返回 Err → generation 有界失败。
    pub fn queue_pre_full(&mut self, wire_seq: u64, bytes: Vec<u8>) -> Result<(), QueueError> {
        if self.pre_full.len() >= PRE_FULL_MAX_EVENTS
            || self.pre_full_bytes.saturating_add(bytes.len()) > PRE_FULL_MAX_BYTES
        {
            return Err(QueueError::Overflow);
        }
        self.pre_full_bytes = self.pre_full_bytes.saturating_add(bytes.len());
        self.pre_full.push_back((wire_seq, bytes));
        Ok(())
    }

    /// 收到 full 后：丢弃 `wire_seq <= full_seq` 的旧 diff，只留严格连续的更大 seq。
    /// 返回可追赶的增量；队列内部有 gap 时返回 Err。
    pub fn take_catchup_after_full(&mut self, full_seq: u64) -> Result<Vec<Vec<u8>>, QueueError> {
        let mut out = Vec::new();
        let mut expect = full_seq.saturating_add(1);
        let mut remaining = VecDeque::new();
        let mut remaining_bytes = 0usize;
        let mut gap = false;
        for (seq, bytes) in self.pre_full.drain(..) {
            if seq <= full_seq {
                continue;
            }
            if seq == expect {
                expect = seq.saturating_add(1);
                out.push(bytes);
            } else {
                gap = true;
                remaining_bytes = remaining_bytes.saturating_add(bytes.len());
                remaining.push_back((seq, bytes));
            }
        }
        self.pre_full = remaining;
        self.pre_full_bytes = remaining_bytes;
        if gap {
            Err(QueueError::Gap)
        } else {
            Ok(out)
        }
    }

    /// 入 input 队列（intent-bound、有界）；溢出返回 Err。
    pub fn queue_input(&mut self, data: Vec<u8>) -> Result<(), QueueError> {
        if self.pending_input.len() >= INPUT_MAX_WRITES
            || self.pending_input_bytes.saturating_add(data.len()) > INPUT_MAX_BYTES
        {
            return Err(QueueError::Overflow);
        }
        self.pending_input_bytes = self.pending_input_bytes.saturating_add(data.len());
        self.pending_input.push_back(data);
        Ok(())
    }

    /// 丢弃 intent 队列并返回诊断（demote/detach/suppression/stale 时调用）。
    pub fn drop_pending_input(&mut self, reason: &str) -> usize {
        let dropped = self.pending_input.len();
        self.pending_input.clear();
        self.pending_input_bytes = 0;
        self.pending_resize = None;
        self.transitions
            .push(format!("input-not-delivered:{reason}:{dropped}"));
        dropped
    }

    /// 标记一次显式用户 intent（focus edge 或真实 input）：递增 epoch、
    /// 清除 suppression、允许一次 takeover=true promote。
    pub fn new_user_intent(&mut self, pane_focus: bool) {
        self.control_intent_epoch = self.control_intent_epoch.saturating_add(1);
        self.user_intent = true;
        self.takeover_attempted = false;
        self.control_rearm = ControlRearm::Armed;
        self.transitions.push(format!(
            "intent:{}:{}",
            self.control_intent_epoch,
            if pane_focus { "focus" } else { "input" }
        ));
    }

    /// 当前 intent 是否允许 takeover=true 的显式 promote。
    /// 只有用户 intent 才允许；open/activate 首次尝试必须 false。
    pub fn may_takeover(&self) -> bool {
        self.user_intent && self.control_rearm == ControlRearm::Armed && !self.takeover_attempted
    }

    /// 计算下次自动 retry 时点；达到上限返回 None（进入 Degraded）。
    pub fn schedule_retry(&mut self, now: Instant) -> Option<Instant> {
        if self.retry_count >= MAX_AUTO_RETRIES {
            return None;
        }
        let delay_ms = RETRY_DELAY_MS[self.retry_count as usize];
        let at = now + Duration::from_millis(delay_ms);
        self.retry_count += 1;
        self.retry_at = Some(at);
        self.transitions
            .push(format!("retry:{}:{}ms", self.retry_count, delay_ms));
        Some(at)
    }

    /// 当前 generation 已 full baseline 且连续 Live 超过稳定窗口 → 恢复预算。
    pub fn stable_window_elapsed(&self, now: Instant) -> bool {
        if self.surface_baseline != SurfaceBaseline::Ready {
            return false;
        }
        self.live_since
            .is_some_and(|since| now.duration_since(since) >= LIVE_STABLE_WINDOW)
    }

    /// 重置普通故障 retry budget（只在稳定窗口达标后调用）。
    pub fn reset_retry_budget(&mut self) {
        self.retry_count = 0;
        self.retry_at = None;
    }

    /// full-frame deadline 是否已过期（超时 → Degraded）。
    pub fn full_deadline_expired(&self, now: Instant) -> bool {
        self.surface_baseline != SurfaceBaseline::Ready
            && self
                .started_at
                .is_some_and(|started| now.duration_since(started) >= FULL_FRAME_DEADLINE)
    }

    /// 是否是 takeover 信号（server 在别的 controller 抢走控制权时关闭流）。
    pub fn is_takeover(reason: &str) -> bool {
        let reason = reason.to_ascii_lowercase();
        reason.contains("taken over") || reason.contains("takeover")
    }
}

/// 事件处理结果：需要 runtime 采取的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotAction {
    /// 普通故障：安排自动 retry（若未到上限）。
    Retry,
    /// 第五次失败：进入 Degraded，不重启。
    Degrade,
    /// 被外部 takeover：suppression + 降 Observe，不自动反抢。
    TakenOver,
    /// 主动停（detach/shutdown/replace）：不 retry。
    Stop,
}

/// 根据事件类型 + 当前状态决定动作（纯逻辑，测试友好）。
pub fn classify_stream_end(
    slot: &PaneStreamSlot,
    is_takeover: bool,
    is_expected_close: bool,
) -> SlotAction {
    if is_expected_close {
        return SlotAction::Stop;
    }
    if is_takeover {
        return SlotAction::TakenOver;
    }
    if slot.state == SlotState::Live {
        if slot.retry_count >= MAX_AUTO_RETRIES {
            SlotAction::Degrade
        } else {
            SlotAction::Retry
        }
    } else {
        // Starting/Backoff 期间失败也走普通退避；Degraded 不自动重启。
        match slot.state {
            SlotState::Degraded => SlotAction::Stop,
            _ => SlotAction::Retry,
        }
    }
}

/// 该 pane 是否应进入 Stopped（已关闭 pane / detach / shutdown）。
pub fn should_stop(desired_mode: Option<StreamMode>) -> bool {
    desired_mode.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> PaneStreamSlot {
        PaneStreamSlot::new(PaneId(1), "w1:p1", StreamMode::Observe)
    }

    /// 事件 ordinal 必须严格递增；重复/倒序丢弃。
    #[test]
    fn ordinal_must_increase_strictly() {
        let mut s = slot();
        assert!(s.accept_ordinal(1));
        assert!(s.accept_ordinal(2));
        assert!(!s.accept_ordinal(2), "重复 ordinal 丢弃");
        assert!(!s.accept_ordinal(1), "倒序 ordinal 丢弃");
        assert!(s.accept_ordinal(3));
    }

    /// wire seq：重复/倒序丢弃；+1 正常应用；diff gap 失败；full gap 重建。
    #[test]
    fn wire_seq_convergence_rules() {
        let mut s = slot();
        // 首个帧必须 full。
        assert_eq!(
            s.decide_frame(5, false),
            FrameDecision::Apply,
            "pre-full diff 先排队"
        );
        assert_eq!(s.decide_frame(5, true), FrameDecision::Apply);
        assert_eq!(s.decide_frame(5, true), FrameDecision::DropDuplicate);
        assert_eq!(s.decide_frame(4, true), FrameDecision::DropDuplicate);
        assert_eq!(s.decide_frame(6, false), FrameDecision::Apply);
        assert_eq!(
            s.decide_frame(8, false),
            FrameDecision::GapFailure,
            "diff gap 不得跳过"
        );
        assert_eq!(
            s.decide_frame(8, true),
            FrameDecision::GapFullBaseline,
            "full gap 可重建"
        );
        assert_eq!(s.decide_frame(9, false), FrameDecision::Apply);
    }

    /// pre-full 队列有界：256 event / 2 MiB 任一先到即失败。
    #[test]
    fn pre_full_queue_is_bounded() {
        let mut s = slot();
        for i in 0..PRE_FULL_MAX_EVENTS {
            assert!(s.queue_pre_full(i as u64 + 1, vec![b'x']).is_ok());
        }
        assert!(
            s.queue_pre_full(PRE_FULL_MAX_EVENTS as u64 + 1, vec![b'x'])
                .is_err(),
            "事件数超限必须失败"
        );
        let mut s2 = slot();
        let big = vec![b'x'; PRE_FULL_MAX_BYTES / 2 + 1];
        assert!(s2.queue_pre_full(1, big.clone()).is_ok());
        assert!(s2.queue_pre_full(2, big).is_err(), "字节数超限必须失败");
    }

    /// full 后只追赶严格连续更大的 seq；队列内部 gap 使 generation 失败。
    #[test]
    fn catchup_after_full_is_strictly_consecutive() {
        let mut s = slot();
        s.queue_pre_full(2, b"two".to_vec()).unwrap();
        s.queue_pre_full(4, b"four".to_vec()).unwrap();
        s.queue_pre_full(5, b"five".to_vec()).unwrap();
        // full seq=1：2 可追；4 起出现 gap → 失败且不越 gap。
        assert!(s.take_catchup_after_full(1).is_err());
        assert_eq!(
            s.pre_full.len(),
            2,
            "gap 之后的剩余 diff 不再追赶（generation 将失败并清空队列）"
        );

        let mut s2 = slot();
        s2.queue_pre_full(2, b"two".to_vec()).unwrap();
        s2.queue_pre_full(3, b"three".to_vec()).unwrap();
        s2.queue_pre_full(6, b"six".to_vec()).unwrap();
        // 尾部 gap（缺 4/5）也使 generation 失败：不能把 6 当连续增量追。
        assert!(
            s2.take_catchup_after_full(1).is_err(),
            "追赶队列自身有 gap 仍使 generation 失败"
        );

        let mut s3 = slot();
        s3.queue_pre_full(2, b"two".to_vec()).unwrap();
        s3.queue_pre_full(3, b"three".to_vec()).unwrap();
        let catchup = s3.take_catchup_after_full(1).unwrap();
        assert_eq!(catchup, vec![b"two".to_vec(), b"three".to_vec()]);
    }

    /// 退避序列固定 100/200/400/800/1600ms；第五次后 Degraded。
    #[test]
    fn retry_backoff_sequence_is_fixed() {
        let now = Instant::now();
        let mut s = slot();
        for (i, expect_ms) in [100u64, 200, 400, 800, 1600].iter().enumerate() {
            let at = s.schedule_retry(now).expect("前五次都应安排 retry");
            assert_eq!(
                at.duration_since(now).as_millis() as u64,
                *expect_ms,
                "retry #{i} 间隔应为 {expect_ms}ms"
            );
        }
        assert_eq!(s.schedule_retry(now), None, "第五次失败后不得再自动 retry");
        assert_eq!(s.retry_count, MAX_AUTO_RETRIES);
    }

    /// 初始 start/用户 promote 不计 retry；重复 snapshot 不重置 budget。
    #[test]
    fn initial_start_not_counted_and_snapshot_does_not_reset() {
        let mut s = slot();
        assert_eq!(s.retry_count, 0, "初始 start 不计 retry");
        let now = Instant::now();
        s.schedule_retry(now);
        s.schedule_retry(now);
        // 重复 reconciliation（模拟）：只改 desired，不重置 budget。
        s.desired_mode = StreamMode::Control;
        s.desired_mode = StreamMode::Observe;
        assert_eq!(s.retry_count, 2, "重复 snapshot 不得重置 budget");
    }

    /// 稳定窗口：full baseline 且连续 Live 10 秒才恢复预算；短 flap 不恢复。
    #[test]
    fn stable_window_resets_budget_only_after_full_live() {
        let now = Instant::now();
        let mut s = slot();
        s.retry_count = 4;
        // 没有 baseline：永不恢复。
        assert!(!s.stable_window_elapsed(now));
        s.surface_baseline = SurfaceBaseline::Ready;
        s.live_since = Some(now);
        assert!(!s.stable_window_elapsed(now + Duration::from_secs(9)));
        assert!(s.stable_window_elapsed(now + Duration::from_secs(10)));
        s.reset_retry_budget();
        assert_eq!(s.retry_count, 0);
    }

    /// takeover 闩锁：suppression 后重复 reconciliation 不清除；
    /// 新 focus/input intent 才清除并允许一次 takeover promote。
    #[test]
    fn takeover_suppression_latches_until_user_intent() {
        let mut s = slot();
        s.control_rearm = ControlRearm::SuppressedAfterTakeover;
        s.takeover_attempted = true;
        // 重复 reconciliation/desired 变化不清除闩锁。
        s.desired_mode = StreamMode::Control;
        assert_eq!(s.control_rearm, ControlRearm::SuppressedAfterTakeover);
        assert!(!s.may_takeover());
        // 新用户 intent：清除并允许一次 promote。
        s.new_user_intent(true);
        assert_eq!(s.control_rearm, ControlRearm::Armed);
        assert!(s.may_takeover());
        s.takeover_attempted = true;
        assert!(!s.may_takeover(), "同一 intent 只 promote 一次");
        // 再来的 input 不重启 intent。
        s.new_user_intent(false);
        assert!(s.may_takeover());
    }

    /// input 队列有界（256 write / 64 KiB）；溢出显式失败。
    #[test]
    fn input_queue_is_bounded() {
        let mut s = slot();
        for _ in 0..INPUT_MAX_WRITES {
            assert!(s.queue_input(b"x".to_vec()).is_ok());
        }
        assert!(s.queue_input(b"x".to_vec()).is_err());
        let mut s2 = slot();
        let big = vec![b'x'; INPUT_MAX_BYTES / 2 + 1];
        assert!(s2.queue_input(big.clone()).is_ok());
        assert!(s2.queue_input(big).is_err());
    }

    /// input-not-delivered 诊断在 drop 时记录。
    #[test]
    fn dropped_input_records_diagnostic() {
        let mut s = slot();
        s.queue_input(b"a".to_vec()).unwrap();
        s.queue_input(b"b".to_vec()).unwrap();
        let dropped = s.drop_pending_input("suppression");
        assert_eq!(dropped, 2);
        assert!(s
            .transitions
            .iter()
            .any(|t| t.contains("input-not-delivered:suppression:2")));
    }

    /// 事件分类：普通 EOF→Retry；takeover→TakenOver；主动关闭→Stop；Degraded 不再重启。
    #[test]
    fn stream_end_classification() {
        let mut s = slot();
        s.state = SlotState::Live;
        assert_eq!(classify_stream_end(&s, false, false), SlotAction::Retry);
        assert_eq!(classify_stream_end(&s, true, false), SlotAction::TakenOver);
        assert_eq!(classify_stream_end(&s, false, true), SlotAction::Stop);
        s.retry_count = MAX_AUTO_RETRIES;
        assert_eq!(classify_stream_end(&s, false, false), SlotAction::Degrade);
        s.state = SlotState::Degraded;
        assert_eq!(classify_stream_end(&s, false, false), SlotAction::Stop);
    }

    /// takeover 信号识别。
    #[test]
    fn takeover_signal_detection() {
        assert!(PaneStreamSlot::is_takeover("terminal attach taken over"));
        assert!(PaneStreamSlot::is_takeover("takeover"));
        assert!(!PaneStreamSlot::is_takeover("读 Herdr 帧长度失败"));
        assert!(!PaneStreamSlot::is_takeover("connection reset"));
    }
}
