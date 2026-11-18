//! 侧栏几何：可拖动宽度。
//!
//! 分区本身不再由这里计算：对齐 VS Code 的侧边栏后，四段按内容自然高度
//! 排在同一列里、整栏一个滚动区（见 `chrome/sidebar.rs`）。这里只保留
//! 宽度夹取这个唯一口径，正文 origin 和实际侧栏宽度都用它。
//!
//! 分段比例（Workspaces : Agents : Commands = 3:2:1）与贴底的
//! Hidden Commands 已删除：折叠的段会留下固定高度的空白，贴底那段还会被
//! 窗口下沿裁掉 —— 用户报障截图里的「HIDDEN COMMANDS 被遮盖」。

/// 侧栏默认宽度：与 macOS `mainSplitController.setPosition(240)` 一致。
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// 拖动下限：Swift `sidebarItem.minimumThickness = 200`。
pub const SIDEBAR_MIN_WIDTH: f32 = 200.0;
/// 拖动上限：Swift `sidebarItem.maximumThickness = 480`。
pub const SIDEBAR_MAX_WIDTH: f32 = 480.0;
/// 分隔条命中宽度（拖动热区）。视觉上只有 1pt 线，热区宽一点才好抓。
pub const SIDEBAR_DIVIDER_HIT_WIDTH: f32 = 5.0;

/// 把一次拖动后的宽度夹进 `[min, max]`，并保证正文至少还有 `min_main` 宽。
///
/// 只在拖动时调用：窗口被拖窄时不主动改用户选的宽度（Swift 的
/// `NSSplitViewController` 也不会把 autosave 的值改小）。
pub fn clamp_sidebar_width(width: f32, window_width: f32, min_main: f32) -> f32 {
    let ceiling = (window_width - min_main).max(SIDEBAR_MIN_WIDTH);
    width.clamp(SIDEBAR_MIN_WIDTH, ceiling.min(SIDEBAR_MAX_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户报障：侧栏宽度必须能拖，且不能把正文挤没。
    #[test]
    fn dragging_the_divider_stays_within_the_swift_limits() {
        // 正常拖动。
        assert_eq!(clamp_sidebar_width(300.0, 1_200.0, 320.0), 300.0);
        // 拖太左：夹到 200。
        assert_eq!(clamp_sidebar_width(40.0, 1_200.0, 320.0), SIDEBAR_MIN_WIDTH);
        // 拖太右：夹到 480（Swift maximumThickness）。
        assert_eq!(
            clamp_sidebar_width(900.0, 1_200.0, 320.0),
            SIDEBAR_MAX_WIDTH
        );
        // 窗口很窄：正文优先，右界跟着缩小。
        assert_eq!(clamp_sidebar_width(900.0, 520.0, 320.0), SIDEBAR_MIN_WIDTH);
    }
}
