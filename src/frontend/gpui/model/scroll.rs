//! 滚轮路由与本地历史滚动。
//!
//! 移植自 `chrome/FlatChrome.swift` 的 `WheelPassthroughPolicy` 与
//! `PaneHistoryScrollPolicy`。
//!
//! 核心约束：**不抢 live TUI 的触控板与 PageUp/PageDown**。htop、Cursor
//! 这类全屏程序自己要这些事件；前端只在应用没要鼠标、也不在 alternate
//! screen 时才把滚轮当成历史滚动。

/// 滚轮这一下应该交给谁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelRoute {
    /// 应用自己要求了鼠标报告。
    ApplicationMouse,
    /// 服务端（Herdr）自己处理滚动：必须让它看到子终端的模式。
    ServerScroll,
    /// alternate screen 且应用没要鼠标：发方向键。
    ApplicationArrows,
    /// 本机 scrollback。
    LocalHistory,
}

/// 服务端滚动优先级最高：Herdr 要在子终端模式基础上决定怎么滚。
pub fn route_wheel(
    mouse_reporting: bool,
    shift_bypasses_mouse: bool,
    alternate_screen: bool,
    has_server_scroll: bool,
) -> WheelRoute {
    if has_server_scroll {
        return WheelRoute::ServerScroll;
    }
    if mouse_reporting && !shift_bypasses_mouse {
        return WheelRoute::ApplicationMouse;
    }
    if alternate_screen {
        return WheelRoute::ApplicationArrows;
    }
    WheelRoute::LocalHistory
}

/// 滚轮路由的最终落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelOutcome {
    /// 交给服务端（Herdr）滚动，前端不转发、也不动本地历史。
    Server(i32),
    /// 以 SGR 鼠标报告发给应用。
    MouseReport(i32),
    /// 发 CSI 方向键（alternate screen 且应用没要鼠标）。
    Arrows(i32),
    /// 移动本地 scrollback 视口。
    History(i32),
}

/// 把一次滚轮事件收成一个动作。`lines == 0`（余数还没攒够一格）返回 `None`。
///
/// macOS 的两处细节照抄：应用鼠标路径**始终**用 SGR（不跟随 1006），方向键
/// 路径用 `CSI A` / `CSI B` 而不用 application-cursor 形式。
pub fn wheel_outcome(
    mouse_reporting: bool,
    shift_bypasses_mouse: bool,
    alternate_screen: bool,
    has_server_scroll: bool,
    lines: i32,
) -> Option<WheelOutcome> {
    if lines == 0 {
        return None;
    }
    let outcome = match route_wheel(
        mouse_reporting,
        shift_bypasses_mouse,
        alternate_screen,
        has_server_scroll,
    ) {
        // 服务端滚动不按每次事件截断：一次手势要能走完整段。
        WheelRoute::ServerScroll => WheelOutcome::Server(lines),
        WheelRoute::ApplicationMouse => WheelOutcome::MouseReport(capped_wheel_lines(lines)),
        WheelRoute::ApplicationArrows => WheelOutcome::Arrows(capped_wheel_lines(lines)),
        // 本地历史按行步进，因此也用截断后的格数。
        WheelRoute::LocalHistory => WheelOutcome::History(capped_wheel_lines(lines)),
    };
    match outcome {
        WheelOutcome::MouseReport(0) | WheelOutcome::Arrows(0) | WheelOutcome::History(0) => None,
        other => Some(other),
    }
}

/// 从 live TUI 手里抢触控板会把 Cursor/htop 弄死，所以恒为 false。
pub const STEALS_LIVE_TRACKPAD: bool = false;

/// PageUp/PageDown 必须留给 htop/Cursor，不能改 viewport。
pub const STEALS_LIVE_PAGE_KEYS: bool = false;

/// 搜索跳转也只移动 native scroll position；不能把 core 历史帧重播到正在
/// 工作的 VT，否则会清掉 live 光标 / alternate-screen 状态。
pub fn should_replace_live_screen(_is_search_jump: bool) -> bool {
    false
}

/// 单次事件最多转发的滚轮格数。触控板精确 delta 否则会把一次手势拆成
/// 几十条控制命令，把一核顶满。
pub const MAX_WHEEL_LINES_PER_EVENT: i32 = 4;

pub fn capped_wheel_lines(lines: i32) -> i32 {
    lines.clamp(-MAX_WHEEL_LINES_PER_EVENT, MAX_WHEEL_LINES_PER_EVENT)
}

/// `delta_lines > 0` 往历史上滚（macOS `scrollingDeltaY > 0`）。
pub fn next_offset(current: u32, delta_lines: i32, max_offset: u32) -> u32 {
    let next = i64::from(current) + i64::from(delta_lines);
    if next <= 0 {
        return 0;
    }
    if next >= i64::from(max_offset) {
        return max_offset;
    }
    next as u32
}

/// 把滚轮 / 触控板 delta 收成整数行，余数留在 accumulator。
///
/// 返回值**没有**按 `MAX_WHEEL_LINES_PER_EVENT` 截断：调用方需要先看到原始
/// 行数（日志、诊断），再决定是否转发。Swift 侧同样是两步。
pub fn wheel_lines(delta_y: f32, precise: bool, cell_height: f32, accumulator: &mut f32) -> i32 {
    if delta_y == 0.0 {
        return 0;
    }
    let cell = cell_height.max(1.0);
    if precise {
        *accumulator += delta_y;
        // Swift 的 `Int(CGFloat)` 向零截断，Rust 的 `as i32` 语义相同。
        let lines = (*accumulator / cell) as i32;
        *accumulator -= lines as f32 * cell;
        return lines;
    }
    *accumulator = 0.0;
    let rounded = delta_y.round() as i32;
    if rounded != 0 {
        return rounded;
    }
    if delta_y > 0.0 {
        1
    } else {
        -1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `testNativeScrollbackOwnsTrackpadAndPageKeys`。
    #[test]
    fn native_scrollback_owns_trackpad_and_page_keys() {
        assert!(
            !STEALS_LIVE_TRACKPAD,
            "拦触控板再 dump 历史会把 live TUI 弄死"
        );
        assert!(!STEALS_LIVE_PAGE_KEYS, "PageUp 必须留给 htop/Cursor");
        assert!(!should_replace_live_screen(false));
        assert!(!should_replace_live_screen(true), "搜索跳转也只移 viewport");
    }

    /// 对照 `testWheelUpIncreasesOffsetUntilMax`。
    #[test]
    fn wheel_up_increases_offset_until_max() {
        assert_eq!(next_offset(0, 3, 40), 3);
        assert_eq!(next_offset(38, 10, 40), 40);
        assert_eq!(next_offset(5, -8, 40), 0);
    }

    /// 对照 `testWheelLinesAreCappedPerEvent`。
    #[test]
    fn wheel_lines_are_capped_per_event() {
        assert_eq!(capped_wheel_lines(40), 4);
        assert_eq!(capped_wheel_lines(-40), -4);
        assert_eq!(capped_wheel_lines(2), 2);
        assert_eq!(capped_wheel_lines(0), 0);

        let mut acc = 0.0;
        let raw = wheel_lines(16.0 * 20.0, true, 16.0, &mut acc);
        assert_eq!(raw, 20, "原始行数不截断");
        assert_eq!(capped_wheel_lines(raw), 4);
    }

    /// 对照 `testPreciseTrackpadAccumulatesPartialCells`。
    #[test]
    fn precise_trackpad_accumulates_partial_cells() {
        let mut acc = 0.0;
        assert_eq!(wheel_lines(10.0, true, 16.0, &mut acc), 0);
        assert_eq!(acc, 10.0);
        assert_eq!(wheel_lines(10.0, true, 16.0, &mut acc), 1);
        assert_eq!(acc, 4.0);
    }

    /// 对照 `testPreciseTrackpadHandlesNegativePixelsAndClampsToWholeCells`。
    #[test]
    fn precise_trackpad_handles_negative_pixels_and_clamps_to_whole_cells() {
        let mut acc = 0.0;
        assert_eq!(wheel_lines(-8.0, true, 16.0, &mut acc), 0);
        assert_eq!(acc, -8.0);
        assert_eq!(wheel_lines(-8.0, true, 16.0, &mut acc), -1);
        assert_eq!(acc, 0.0);
        assert_eq!(next_offset(0, -1, 40), 0);
        assert_eq!(next_offset(40, 1, 40), 40);
    }

    /// 对照 `testServerScrollKeepsHerdrInChargeOfApplicationWheelRouting`。
    #[test]
    fn server_scroll_keeps_herdr_in_charge_of_application_wheel_routing() {
        assert_eq!(
            route_wheel(true, false, false, true),
            WheelRoute::ServerScroll,
            "Herdr 必须看到子终端模式才能决定怎么滚"
        );
    }

    /// 对照 `testShiftBypassesMouseReportingForLocalSelect`。
    #[test]
    fn shift_bypasses_mouse_reporting_for_local_select() {
        assert_eq!(
            route_wheel(true, true, false, true),
            WheelRoute::ServerScroll
        );
        assert_eq!(
            route_wheel(true, true, false, false),
            WheelRoute::LocalHistory,
            "Shift 绕过应用鼠标时落回本地历史"
        );
    }

    /// 对照 `testServerScrollUsedWhenAppDidNotRequestMouse`。
    #[test]
    fn server_scroll_used_when_app_did_not_request_mouse() {
        assert_eq!(
            route_wheel(false, false, true, true),
            WheelRoute::ServerScroll
        );
        assert_eq!(
            route_wheel(false, false, false, false),
            WheelRoute::LocalHistory
        );
        assert_eq!(
            route_wheel(false, false, true, false),
            WheelRoute::ApplicationArrows
        );
    }

    /// 滚轮落点：alternate screen 发方向键，向上是 `CSI A`。
    #[test]
    fn wheel_outcome_sends_arrows_in_the_alternate_screen() {
        assert_eq!(
            wheel_outcome(false, false, true, false, 2),
            Some(WheelOutcome::Arrows(2))
        );
        assert_eq!(
            wheel_outcome(false, false, true, false, -5),
            Some(WheelOutcome::Arrows(-4)),
            "单次事件最多四行"
        );
        assert_eq!(wheel_outcome(false, false, true, false, 0), None);
    }

    /// 应用要了鼠标：走 SGR 报告，且同样按四行截断。
    #[test]
    fn wheel_outcome_reports_mouse_when_the_app_asked_for_it() {
        assert_eq!(
            wheel_outcome(true, false, true, false, 1),
            Some(WheelOutcome::MouseReport(1))
        );
        assert_eq!(
            wheel_outcome(true, false, false, false, -40),
            Some(WheelOutcome::MouseReport(-4))
        );
    }

    /// 服务端滚动优先，且不截断（一次手势要走完整段）。
    #[test]
    fn wheel_outcome_prefers_the_server_without_capping() {
        assert_eq!(
            wheel_outcome(true, false, true, true, 40),
            Some(WheelOutcome::Server(40))
        );
    }

    /// Shift 绕过应用鼠标 → 落回本地历史。
    #[test]
    fn wheel_outcome_shift_falls_back_to_local_history() {
        assert_eq!(
            wheel_outcome(true, true, false, false, 3),
            Some(WheelOutcome::History(3))
        );
        assert_eq!(
            wheel_outcome(false, false, false, false, -3),
            Some(WheelOutcome::History(-3))
        );
    }

    /// 应用要了鼠标且没有服务端滚动时，滚轮回给应用。
    #[test]
    fn application_mouse_wins_when_reporting_and_no_server_scroll() {
        assert_eq!(
            route_wheel(true, false, true, false),
            WheelRoute::ApplicationMouse
        );
    }

    /// 非精确滚轮不需要累积，整数 delta 直接当行数。
    #[test]
    fn discrete_wheel_uses_rounded_delta() {
        let mut acc = 7.0;
        assert_eq!(wheel_lines(3.0, false, 16.0, &mut acc), 3);
        assert_eq!(acc, 0.0, "非精确路径清空 accumulator");
        // 小于半格的抖动按方向记一格，避免完全不动。
        assert_eq!(wheel_lines(0.2, false, 16.0, &mut acc), 1);
        assert_eq!(wheel_lines(-0.2, false, 16.0, &mut acc), -1);
        assert_eq!(wheel_lines(0.0, false, 16.0, &mut acc), 0);
    }
}
