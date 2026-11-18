//! 侧栏几何：宽度夹取、展开段分配与分隔条拖动。

/// 侧栏默认宽度：与 macOS `mainSplitController.setPosition(240)` 一致。
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// 拖动下限：Swift `sidebarItem.minimumThickness = 200`。
pub const SIDEBAR_MIN_WIDTH: f32 = 200.0;
/// 拖动上限：Swift `sidebarItem.maximumThickness = 480`。
pub const SIDEBAR_MAX_WIDTH: f32 = 480.0;
/// 分隔条命中宽度（拖动热区）。视觉上只有 1pt 线，热区宽一点才好抓。
pub const SIDEBAR_DIVIDER_HIT_WIDTH: f32 = 5.0;
/// 段头高度：折叠段在底部只保留这个高度。
pub const SIDEBAR_SECTION_HEADER_HEIGHT: f32 = 26.0;
/// 展开段的最小高度；空间不足时按可用高度均分放宽。
pub const SIDEBAR_SECTION_MIN_HEIGHT: f32 = 80.0;
/// 内层垂直留白（上、下各 4pt）。
pub const SIDEBAR_VERTICAL_INSET: f32 = 4.0;
/// 默认 Workspaces:Agents:Commands = 2:1:1；Hidden Commands 保留独立权重。
pub const DEFAULT_SECTION_WEIGHTS: [f32; 4] = [2.0, 1.0, 1.0, 1.0];

/// 正在拖动相邻展开段之间的横向分隔条。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarSplitDrag {
    pub upper: usize,
    pub lower: usize,
    pub start_y: f32,
    pub start_upper_height: f32,
    pub pair_height: f32,
    pub pair_weight: f32,
}

/// 将一次拖动后的宽度夹进 `[min, max]`，并保证正文至少还有 `min_main` 宽。
///
/// 只在拖动时调用：窗口被拖窄时不主动改用户选的宽度。
pub fn clamp_sidebar_width(width: f32, window_width: f32, min_main: f32) -> f32 {
    let ceiling = (window_width - min_main).max(SIDEBAR_MIN_WIDTH);
    width.clamp(SIDEBAR_MIN_WIDTH, ceiling.min(SIDEBAR_MAX_WIDTH))
}

/// 给四段计算高度。展开段按权重填充折叠段头之后的空间；折叠段始终为 26pt。
///
/// `expanded` 和 `weights` 的顺序固定为 Workspaces / Agents / Commands / Hidden Commands。
/// 最小高度用带下限的比例分配实现，窗口过矮时下限自动降到可用均值。
pub fn section_heights(content_height: f32, expanded: [bool; 4], weights: [f32; 4]) -> [f32; 4] {
    let mut heights = [0.0; 4];
    let expanded_count = expanded.iter().filter(|&&value| value).count();
    let collapsed_count = 4 - expanded_count;
    for index in 0..4 {
        if !expanded[index] {
            heights[index] = SIDEBAR_SECTION_HEADER_HEIGHT;
        }
    }
    if expanded_count == 0 {
        return heights;
    }

    let available =
        (content_height - collapsed_count as f32 * SIDEBAR_SECTION_HEADER_HEIGHT).max(0.0);
    let minimum = SIDEBAR_SECTION_MIN_HEIGHT.min(available / expanded_count as f32);
    let mut active = expanded;
    let mut remaining = available;

    loop {
        let active_weight: f32 = (0..4)
            .filter(|&index| active[index])
            .map(|index| weights[index].max(0.0))
            .sum();
        if active_weight <= f32::EPSILON {
            let count = active.iter().filter(|&&value| value).count();
            if count == 0 {
                break;
            }
            for index in 0..4 {
                if active[index] {
                    heights[index] = remaining / count as f32;
                }
            }
            break;
        }

        let below_minimum: Vec<usize> = (0..4)
            .filter(|&index| {
                active[index] && remaining * weights[index].max(0.0) / active_weight < minimum
            })
            .collect();
        if below_minimum.is_empty() {
            for index in 0..4 {
                if active[index] {
                    heights[index] = remaining * weights[index].max(0.0) / active_weight;
                }
            }
            break;
        }

        for index in below_minimum {
            active[index] = false;
            heights[index] = minimum;
            remaining = (remaining - minimum).max(0.0);
        }
        if !active.iter().any(|&value| value) {
            break;
        }
    }
    heights
}

/// 改变相邻两段的权重，保持两段合计权重不变，其他段不受影响。
pub fn resize_adjacent_weights(
    weights: &mut [f32; 4],
    upper: usize,
    lower: usize,
    upper_height: f32,
    pair_height: f32,
    pair_weight: f32,
) {
    if upper >= weights.len() || lower >= weights.len() || pair_height <= f32::EPSILON {
        return;
    }
    let upper_weight = (upper_height / pair_height).clamp(0.0, 1.0) * pair_weight;
    weights[upper] = upper_weight;
    weights[lower] = (pair_weight - upper_weight).max(0.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_the_divider_stays_within_the_swift_limits() {
        assert_eq!(clamp_sidebar_width(300.0, 1_200.0, 320.0), 300.0);
        assert_eq!(clamp_sidebar_width(40.0, 1_200.0, 320.0), SIDEBAR_MIN_WIDTH);
        assert_eq!(
            clamp_sidebar_width(900.0, 1_200.0, 320.0),
            SIDEBAR_MAX_WIDTH
        );
        assert_eq!(clamp_sidebar_width(900.0, 520.0, 320.0), SIDEBAR_MIN_WIDTH);
    }

    #[test]
    fn default_sections_split_remaining_height_in_half_quarters() {
        let heights = section_heights(792.0, [true, true, true, false], DEFAULT_SECTION_WEIGHTS);
        assert_eq!(heights, [383.0, 191.5, 191.5, 26.0]);
    }

    #[test]
    fn collapsed_sections_keep_headers_and_relocate_to_the_bottom_group() {
        let heights = section_heights(792.0, [true, false, true, false], DEFAULT_SECTION_WEIGHTS);
        assert_eq!(heights, [493.33334, 26.0, 246.66667, 26.0]);
        let all_collapsed = section_heights(792.0, [false; 4], DEFAULT_SECTION_WEIGHTS);
        assert_eq!(all_collapsed, [26.0; 4]);
    }

    #[test]
    fn a_short_window_relaxes_the_expanded_minimum_without_negative_space() {
        let heights = section_heights(120.0, [true, true, true, false], DEFAULT_SECTION_WEIGHTS);
        assert_eq!(heights, [31.333334, 31.333334, 31.333334, 26.0]);
        assert!(heights.iter().all(|height| *height >= 0.0));
    }

    #[test]
    fn dragging_a_split_preserves_the_adjacent_pair_weight() {
        let mut weights = DEFAULT_SECTION_WEIGHTS;
        resize_adjacent_weights(&mut weights, 1, 2, 237.5, 383.0, 2.0);
        assert_eq!(weights[0], 2.0);
        assert!((weights[1] - 1.2402089).abs() < 1e-6);
        assert!((weights[2] - 0.7597911).abs() < 1e-6);
        assert_eq!(weights[3], 1.0);
        assert!((weights[1] + weights[2] - 2.0).abs() < f32::EPSILON);
    }
}
