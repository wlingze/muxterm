//! Pane 分隔条拖动的无 GUI 数学。
//!
//! 移植自 `chrome/PaneResizeMath.swift`。AppKit 与 gpui 共用同一套规则，
//! 所以这里只放纯数值函数，两边渲染层各自调用。

/// 比例下限：0.05，避免某个 pane 被压成零宽/零高。
pub const MINIMUM_RATIO: f64 = 0.05;
/// 比例上限：0.95。
pub const MAXIMUM_RATIO: f64 = 0.95;

/// 把分隔比例限制在可渲染范围内。
pub fn clamped_ratio(ratio: f64) -> f64 {
    ratio.clamp(MINIMUM_RATIO, MAXIMUM_RATIO)
}

/// 根据拖动位移计算新的 first/second 比例。
///
/// `usable` 是扣掉分隔条之后的可用长度，下限为 1，避免除以零。
pub fn ratio_after_drag(
    start_ratio: f64,
    delta: f64,
    total_length: f64,
    divider_length: f64,
) -> f64 {
    let usable = (total_length - divider_length).max(1.0);
    let start_length = clamped_ratio(start_ratio) * usable;
    clamped_ratio((start_length + delta) / usable)
}

/// 把 backing pixel 长度转换为字符格数量。
///
/// 只接受 2..9999 格：1 格无法显示内容，>=10000 会超出 `u16` 语义且多半
/// 是尺寸计算的 bug。
pub fn character_count(pixel_length: f64, cell_pixels: i32) -> Option<u16> {
    if cell_pixels <= 0 {
        return None;
    }
    let count = (pixel_length / f64::from(cell_pixels)).floor() as i64;
    if (2..10_000).contains(&count) {
        Some(count as u16)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCURACY: f64 = 0.0001;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < ACCURACY
    }

    /// 对照 `PaneResizeMathTests.testRatioIsBounded`。
    #[test]
    fn ratio_is_bounded() {
        assert!(close(clamped_ratio(-1.0), 0.05));
        assert!(close(clamped_ratio(2.0), 0.95));
        assert!(close(clamped_ratio(0.5), 0.5));
    }

    /// 对照 `testDragKeepsDividerLengthOutOfCollapsedRange`：极值拖动仍然
    /// 落在 [0.05, 0.95]。
    #[test]
    fn drag_stays_inside_collapsed_range() {
        assert!(close(ratio_after_drag(0.5, -10_000.0, 1_000.0, 6.0), 0.05));
        assert!(close(ratio_after_drag(0.5, 10_000.0, 1_000.0, 6.0), 0.95));
    }

    /// 分隔条长度参与分母：994 可用长度上拖 +497 应走到 0.95 上限前。
    #[test]
    fn divider_length_is_excluded_from_usable_length() {
        // usable = 1000 - 6 = 994；起始 0.5 → 497；+100 → 597/994 ≈ 0.6006
        let ratio = ratio_after_drag(0.5, 100.0, 1_000.0, 6.0);
        assert!((ratio - 597.0 / 994.0).abs() < 1e-9);
    }

    /// 总长小于分隔条时，可用长度退化为 1，仍然给出有限值。
    #[test]
    fn degenerate_total_length_does_not_divide_by_zero() {
        let ratio = ratio_after_drag(0.5, 0.0, 4.0, 6.0);
        assert!(ratio.is_finite());
        assert!((MINIMUM_RATIO..=MAXIMUM_RATIO).contains(&ratio));
    }

    /// 对照 `testPixelLengthMapsToCharacterCells`。
    #[test]
    fn pixel_length_maps_to_character_cells() {
        assert_eq!(character_count(480.0, 8), Some(60));
        assert_eq!(character_count(8.0, 8), None);
    }

    /// 对照 `testCharacterCountRejectsInvalidInput`。
    #[test]
    fn character_count_rejects_invalid_input() {
        assert_eq!(character_count(100.0, 0), None);
        assert_eq!(character_count(100_000_000.0, 8), None);
        assert_eq!(character_count(8.0, 8), None);
    }

    #[test]
    fn character_count_accepts_the_exact_bounds() {
        assert_eq!(character_count(16.0, 8), Some(2));
        assert_eq!(character_count(79_992.0, 8), Some(9_999));
        assert_eq!(character_count(80_000.0, 8), None);
    }
}
