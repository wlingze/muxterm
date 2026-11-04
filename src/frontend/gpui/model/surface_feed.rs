//! Surface 喂字节的纯策略：镜像应答、鼠标报告合并、事件批处理、输出合并、
//! 全屏重绘边界、大段粘贴分批。
//!
//! 移植自 `chrome/FlatChrome.swift` 的对应 policy 枚举。这些规则决定了
//! 「哪些字节能进 VT、一次进多少、超限怎么办」，是 gpui Surface 与 macOS
//! SwiftTerm 行为对齐的关键。

/// tmux 镜像必须丢弃解析 pane 输出产生的 OSC/CSI 应答；本地 PTY 则要转发。
///
/// `during_remote_output_feed` 曾经用来判断是否在远端 feed 期间，但那样会
/// 在 feed 结束后漏掉迟到的应答；最终只按「是不是 tmux 镜像」区分。
pub fn should_forward_parser_response(is_tmux_mirror: bool) -> bool {
    !is_tmux_mirror
}

/// 用户主动触发的滚轮 mouse report 不属于终端解析器应答。
///
/// tmux 镜像必须丢弃 VT 解析 pane 输出产生的应答，但 alternate-screen 的
/// TUI（opencode / Cursor / htop）依赖用户滚轮报告翻页。两者走同一条写出
/// 通道，因此由调用方在滚轮事件的短生命周期内显式标记放行。
pub fn should_forward_user_initiated_mouse_report() -> bool {
    true
}

/// 鼠标 hover 是位置更新，队列拥塞时只需保留同一 pane 的最新位置。
/// 按下、释放、拖动和滚轮序列都必须原样按序送达。
///
/// SGR 形式 `ESC [ < b ; x ; y M`，其中未按键的移动用释放码 3 加移动位 32。
pub fn is_coalescible_motion(data: &[u8]) -> bool {
    if data.len() < 9 || !data.starts_with(b"\x1b[<") || data.last() != Some(&b'M') {
        return false;
    }
    let body = &data[3..data.len() - 1];
    let fields = body.split(|b| *b == b';').collect::<Vec<_>>();
    if fields.len() != 3 {
        return false;
    }
    fn number(field: &[u8]) -> Option<i64> {
        std::str::from_utf8(field).ok()?.parse::<i64>().ok()
    }
    let Some(button) = number(fields[0]) else {
        return false;
    };
    if number(fields[1]).is_none() || number(fields[2]).is_none() {
        return false;
    }
    button >= 32 && button & 0b11 == 0b11
}

/// 前台 Workspace：PTY 事件一律交给 Surface（没有就建）。
/// 后台 Workspace 槽：只喂已经存在的 Surface，不新建 widget。
pub fn should_deliver(view_creation_enabled: bool, has_view: bool) -> bool {
    view_creation_enabled || has_view
}

/// 事件泵每拍的预算。
pub mod batch {
    use std::time::Duration;

    /// 隐藏 scene 每拍最多处理的事件数。
    pub const MAX_EVENTS_PER_PASS: usize = 48;
    /// 隐藏 scene 每拍预算：给绘制和输入留出时间。
    pub const TIME_BUDGET: Duration = Duration::from_millis(4);
    /// 前台 scene 积压时加大预算，尽快排空以免切到 Cursor 仍转圈。
    pub const ACTIVE_TIME_BUDGET: Duration = Duration::from_millis(24);
    /// 前台每拍最多处理的事件数（仍受 `ACTIVE_TIME_BUDGET` 约束）。
    pub const ACTIVE_MAX_EVENTS_PER_PASS: usize = 96;
    /// Cursor 一类主屏 inplace redraw 单帧常超过 512KiB；过小会误触发
    /// overflow → 暂停 / 重新 capture，既撕帧又把控制通道占满。
    pub const MAX_COALESCED_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
    /// 事件泵轮询间隔（与 macOS `FlatChrome.eventPollInterval` 同值）。
    pub const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(8);
    /// Surface 积压时降低 attention / sidebar 刷新频率。
    pub const ATTENTION_REFRESH_MIN_INTERVAL: Duration = Duration::from_millis(250);

    /// 中文注释：预算用 `Duration` 传，测试可以给 `Duration::MAX`。
    pub fn should_yield(
        processed_events: usize,
        elapsed: Duration,
        max_events: usize,
        time_budget: Duration,
    ) -> bool {
        processed_events >= max_events.max(1) || elapsed >= time_budget
    }
}

/// 同一 pane 连续 `PaneOutput` 的合并决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoalesceDecision {
    /// 合并进已有 `PaneOutput`。
    Combine,
    /// 丢掉该 pane 尚未交付的旧输出，只保留本段新字节。
    KeepNewest,
    /// 单段已超过上限：必须 fence，向 Runtime 要权威 baseline。
    MarkOverflow,
}

/// 超限时保留最新一段，而不是整段丢弃再要 snapshot：Cursor 主屏 redraw
/// 中途 capture 会花屏。
pub fn decide_coalesce(previous_bytes: usize, incoming_bytes: usize) -> CoalesceDecision {
    decide_coalesce_with_limit(
        previous_bytes,
        incoming_bytes,
        batch::MAX_COALESCED_OUTPUT_BYTES,
    )
}

pub fn decide_coalesce_with_limit(
    previous_bytes: usize,
    incoming_bytes: usize,
    max_bytes: usize,
) -> CoalesceDecision {
    if incoming_bytes == 0 {
        return CoalesceDecision::Combine;
    }
    if incoming_bytes > max_bytes {
        return CoalesceDecision::MarkOverflow;
    }
    match previous_bytes.checked_add(incoming_bytes) {
        Some(sum) if sum <= max_bytes => CoalesceDecision::Combine,
        _ => CoalesceDecision::KeepNewest,
    }
}

const CLEAR: &[u8] = b"\x1b[2J";
const SHORT_HOME: &[u8] = b"\x1b[H";
const EXPLICIT_HOME: &[u8] = b"\x1b[1;1H";

/// 在原始 VT 字节里定位可独立解析的完整全屏重绘。
///
/// Cursor / Pi 一类程序通常以 `CSI H` + `CSI 2J`（顺序也可能相反）开始新帧。
/// 只有这种明确边界允许高水位队列淘汰旧帧；帧尾即使停在半截 CSI，也会和
/// 后续 `PaneOutput` 继续拼接，不能再从尾部任意裁剪。
pub struct TerminalRedrawBoundary;

impl TerminalRedrawBoundary {
    /// 返回从最后一帧起点到末尾的切片；没有边界或超过上限时返回 `None`。
    pub fn latest_full_redraw_suffix(data: &[u8], max_bytes: usize) -> Option<&[u8]> {
        let start = Self::last_full_redraw_start(data)?;
        let suffix = &data[start..];
        if suffix.len() <= max_bytes.max(1) {
            Some(suffix)
        } else {
            None
        }
    }

    fn last_full_redraw_start(bytes: &[u8]) -> Option<usize> {
        if bytes.len() < CLEAR.len() {
            return None;
        }
        let mut last = None;
        for clear_at in 0..=(bytes.len() - CLEAR.len()) {
            if !matches_at(CLEAR, bytes, clear_at) {
                continue;
            }
            if clear_at >= SHORT_HOME.len()
                && matches_at(SHORT_HOME, bytes, clear_at - SHORT_HOME.len())
            {
                last = Some(clear_at - SHORT_HOME.len());
            } else if clear_at >= EXPLICIT_HOME.len()
                && matches_at(EXPLICIT_HOME, bytes, clear_at - EXPLICIT_HOME.len())
            {
                last = Some(clear_at - EXPLICIT_HOME.len());
            } else if matches_at(SHORT_HOME, bytes, clear_at + CLEAR.len())
                || matches_at(EXPLICIT_HOME, bytes, clear_at + CLEAR.len())
            {
                last = Some(clear_at);
            }
        }
        last
    }
}

fn matches_at(needle: &[u8], bytes: &[u8], index: usize) -> bool {
    match bytes.get(index..index + needle.len()) {
        Some(slice) => slice == needle,
        None => false,
    }
}

/// 大段粘贴仍分批写进控制通道，但逻辑上是同一次粘贴。
///
/// 一整条 `send-keys -H` / `pane.send_text` 会按字节膨胀并占住这条通道，
/// 切 tab、切 pane 都要排在它后面。每批只是传输切片，括号粘贴包住整段，
/// 目标 pane 在开始时钉死，不跟随后面的焦点。
pub mod large_paste {
    use std::time::{Duration, Instant};

    /// 单条控制命令携带的原始字节。tmux `-H` 会再膨胀成约三倍的十六进制。
    pub const CHUNK_BYTES: usize = 16 * 1024;
    /// 不超过这一档就一次写完，不转圈。
    pub const IMMEDIATE_LIMIT: usize = CHUNK_BYTES;
    /// 每一轮事件泵最多写出的字节，给切 tab 之类的命令留出空隙。
    pub const BYTES_PER_TURN: usize = 64 * 1024;
    /// 很短的粘贴也要让转圈画出来，避免一帧内结束时用户什么都看不见。
    pub const MINIMUM_VISIBLE: Duration = Duration::from_millis(400);

    pub fn needs_progress(byte_count: usize) -> bool {
        byte_count > IMMEDIATE_LIMIT
    }

    /// 正在写，或者刚写完但还没到最短显示时间。
    pub fn shows_spinner(busy: bool, started_at: Option<Instant>, now: Instant) -> bool {
        if busy {
            return true;
        }
        match started_at {
            None => false,
            Some(start) => now.saturating_duration_since(start) < MINIMUM_VISIBLE,
        }
    }

    /// 按顺序切块。拼回去必须等于原来的字节。
    pub fn chunks(payload: &[u8]) -> Vec<&[u8]> {
        if payload.is_empty() {
            return Vec::new();
        }
        if payload.len() <= CHUNK_BYTES {
            return vec![payload];
        }
        payload.chunks(CHUNK_BYTES).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// 对照 `testTmuxMirrorDropsParserResponseDuringFeed`。
    #[test]
    fn tmux_mirror_drops_parser_response() {
        assert!(!should_forward_parser_response(true));
    }

    /// 对照 `testTmuxMirrorDropsParserResponseOutsideFeed`。
    #[test]
    fn tmux_mirror_drops_parser_response_regardless_of_feed_state() {
        assert!(!should_forward_parser_response(true));
    }

    /// 对照 `testLocalTerminalAlwaysForwardsResponses`。
    #[test]
    fn local_terminal_always_forwards_responses() {
        assert!(should_forward_parser_response(false));
    }

    /// 对照 `testUserMouseReportsRemainForwardedForTmuxMirror`。
    #[test]
    fn user_mouse_reports_remain_forwarded_for_tmux_mirror() {
        assert!(should_forward_user_initiated_mouse_report());
    }

    /// 对照 `testOnlyUnpressedSgrMotionCanBeCoalesced`。
    #[test]
    fn only_unpressed_sgr_motion_can_be_coalesced() {
        // 未按键移动：button 35 = 释放码 3 + 移动位 32。
        assert!(is_coalescible_motion(b"\x1b[<35;10;5M"));
        // 按住左键拖动：button 32 的低两位不是 3。
        assert!(!is_coalescible_motion(b"\x1b[<32;10;5M"));
        // 按下 / 释放 / 滚轮都不是移动。
        assert!(!is_coalescible_motion(b"\x1b[<0;10;5M"));
        assert!(!is_coalescible_motion(b"\x1b[<0;10;5m"));
        assert!(!is_coalescible_motion(b"\x1b[<64;10;5M"));
        // 非 SGR 编码（X10）不参与合并。
        assert!(!is_coalescible_motion(b"\x1b[M   "));
        // 参数不全。
        assert!(!is_coalescible_motion(b"\x1b[<35;10M"));
    }

    /// 对照 `testForegroundWorkspaceAlwaysDelivers`。
    #[test]
    fn foreground_workspace_always_delivers() {
        assert!(should_deliver(true, false));
        assert!(should_deliver(true, true));
    }

    /// 对照 `testBackgroundWorkspaceOnlyExistingViews`。
    #[test]
    fn background_workspace_only_existing_views() {
        assert!(!should_deliver(false, false));
        assert!(should_deliver(false, true));
    }

    /// 对照 `testSurfaceEventBatchPolicyHonoursCallerBudget`。
    #[test]
    fn surface_event_batch_policy_honours_caller_budget() {
        assert!(!batch::should_yield(
            1,
            Duration::from_millis(1),
            32,
            Duration::from_millis(4)
        ));
        assert!(batch::should_yield(
            1,
            Duration::from_millis(2),
            32,
            Duration::from_millis(1)
        ));
        assert!(
            batch::should_yield(32, Duration::ZERO, 32, Duration::MAX),
            "事件数到上限必须让出"
        );
        assert!(batch::ACTIVE_MAX_EVENTS_PER_PASS > batch::MAX_EVENTS_PER_PASS);
        assert!(batch::ACTIVE_TIME_BUDGET > batch::TIME_BUDGET);
        assert!(batch::ATTENTION_REFRESH_MIN_INTERVAL >= batch::EVENT_POLL_INTERVAL);
    }

    /// 对照 `testSurfaceOutputCoalesceKeepsNewestInsteadOfOverflow`。
    #[test]
    fn surface_output_coalesce_keeps_newest_instead_of_overflow() {
        assert_eq!(decide_coalesce(100, 50), CoalesceDecision::Combine);
        assert_eq!(
            decide_coalesce(batch::MAX_COALESCED_OUTPUT_BYTES - 10, 64),
            CoalesceDecision::KeepNewest,
            "超限应丢掉旧合并缓冲保留最新一段"
        );
        assert_eq!(
            decide_coalesce(0, batch::MAX_COALESCED_OUTPUT_BYTES + 1),
            CoalesceDecision::MarkOverflow
        );
        assert!(
            batch::MAX_COALESCED_OUTPUT_BYTES > 512 * 1024,
            "Cursor 主屏 redraw 常超过 512KiB"
        );
    }

    /// 对照 `testFullRedrawBoundaryKeepsLatestCompleteFrame`。
    #[test]
    fn full_redraw_boundary_keeps_latest_complete_frame() {
        let stream = b"prefix\x1b[2J\x1b[HOLD\x1b[1;1H\x1b[2JLATEST\x1b[31";
        assert_eq!(
            TerminalRedrawBoundary::latest_full_redraw_suffix(stream, 64),
            Some(&b"\x1b[1;1H\x1b[2JLATEST\x1b[31"[..])
        );
        assert_eq!(
            TerminalRedrawBoundary::latest_full_redraw_suffix(
                b"prefix\x1b[2Jcontent-without-home",
                64
            ),
            None
        );
        assert_eq!(
            TerminalRedrawBoundary::latest_full_redraw_suffix(stream, 8),
            None,
            "单个完整新帧仍超过队列上限时必须走权威 snapshot"
        );
        // 顺序相反（先 clear 再 home）同样算边界。
        let reversed = b"old\x1b[2J\x1b[Hnew";
        assert_eq!(
            TerminalRedrawBoundary::latest_full_redraw_suffix(reversed, 64),
            Some(&b"\x1b[2J\x1b[Hnew"[..])
        );
    }

    /// 对照 `testSmallPasteStaysOneWrite`。
    #[test]
    fn small_paste_stays_one_write() {
        let payload = vec![b'a'; large_paste::IMMEDIATE_LIMIT];
        assert!(!large_paste::needs_progress(payload.len()));
        assert_eq!(large_paste::chunks(&payload), vec![&payload[..]]);
    }

    /// 对照 `testLargePasteChunksReassembleWithoutDroppingBytes`。
    #[test]
    fn large_paste_chunks_reassemble_without_dropping_bytes() {
        let count = large_paste::CHUNK_BYTES * 2 + 7;
        let payload = vec![b'b'; count];
        assert!(large_paste::needs_progress(count));
        let chunks = large_paste::chunks(&payload);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), large_paste::CHUNK_BYTES);
        assert_eq!(chunks[1].len(), large_paste::CHUNK_BYTES);
        assert_eq!(chunks[2].len(), 7);
        assert_eq!(chunks.concat(), payload);
        assert!(
            large_paste::CHUNK_BYTES * 4 <= large_paste::BYTES_PER_TURN,
            "一轮事件泵至少要能写完四块"
        );
    }

    /// 对照 `testSpinnerStaysUpBrieflyAfterThePasteFinishes`。
    #[test]
    fn spinner_stays_up_briefly_after_the_paste_finishes() {
        let start = Instant::now();
        assert!(!large_paste::shows_spinner(false, None, start));
        assert!(large_paste::shows_spinner(true, Some(start), start));
        assert!(large_paste::shows_spinner(
            false,
            Some(start),
            start + large_paste::MINIMUM_VISIBLE - Duration::from_millis(10)
        ));
        assert!(!large_paste::shows_spinner(
            false,
            Some(start),
            start + large_paste::MINIMUM_VISIBLE + Duration::from_millis(10)
        ));
    }

    /// 空粘贴没有块，避免发出一条空的 `send-keys`。
    #[test]
    fn empty_paste_has_no_chunks() {
        assert!(large_paste::chunks(&[]).is_empty());
        assert!(!large_paste::needs_progress(0));
    }
}
