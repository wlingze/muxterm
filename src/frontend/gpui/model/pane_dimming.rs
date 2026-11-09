//! 未聚焦 split pane 的明暗。
//!
//! 对照 macOS `InactivePaneDimmingPolicy`（对齐 iTerm2 的
//! `SplitPaneDimmingAmount`）：多 pane 时把非活动 pane 的内容整体压暗，
//! 让活动 pane 一眼可辨。单 pane / zoom 全屏不压，否则整窗发灰。

/// 0 = 不压暗，1 = 全黑。默认对齐 iTerm2 的 `SplitPaneDimmingAmount`。
pub const INACTIVE_PANE_DIM: f32 = 0.15;

/// 这个 pane 要不要压暗。
///
/// 对照 `InactivePaneDimmingPolicy.shouldDim(isActive:visiblePaneCount:)`。
pub fn should_dim(is_active: bool, visible_pane_count: usize) -> bool {
    visible_pane_count > 1 && !is_active
}

/// 把颜色往黑色方向插值 `amount`（0..1）。alpha 不变。
pub fn dim(color: gpui::Rgba, amount: f32) -> gpui::Rgba {
    let factor = 1.0 - amount.clamp(0.0, 1.0);
    gpui::Rgba {
        r: color.r * factor,
        g: color.g * factor,
        b: color.b * factor,
        a: color.a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::rgb;

    /// 对照 `InactivePaneDimmingPolicyTests`：单 pane 不压，多 pane 只压非活动。
    #[test]
    fn only_inactive_panes_are_dimmed_when_split() {
        assert!(!should_dim(true, 1), "单个 pane 不压暗");
        assert!(!should_dim(false, 1), "zoom 全屏不压暗");
        assert!(should_dim(false, 2), "分屏后非活动 pane 要压暗");
        assert!(!should_dim(true, 2), "活动 pane 不压暗");
        assert!(should_dim(false, 4));
    }

    /// 默认强度 0.15：往黑色插值，alpha 保持。
    #[test]
    fn dimming_pulls_towards_black_by_the_configured_amount() {
        let white = rgb(0xffffff);
        let dimmed = dim(white, INACTIVE_PANE_DIM);
        let expected = 1.0 - INACTIVE_PANE_DIM;
        assert!((dimmed.r - expected).abs() < 1e-6, "{dimmed:?}");
        assert!((dimmed.g - expected).abs() < 1e-6, "{dimmed:?}");
        assert!((dimmed.b - expected).abs() < 1e-6, "{dimmed:?}");
        assert!((dimmed.a - white.a).abs() < 1e-6);
        // 0 = 原样，1 = 全黑。
        assert!((dim(white, 0.0).r - 1.0).abs() < 1e-6);
        assert!(dim(white, 1.0).r < 1e-6);
    }
}
