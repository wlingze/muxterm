//! 布局数学：把 `ClientLayout` 摊成每个 pane 的像素矩形。
//!
//! 纯函数。渲染与 resize 都读同一份结果，避免两个写入者给 pane 算出不同的列数
//! （`SURFACE.md` §3 定律 9b）。

use crate::frontend::utils::corebridge::ClientLayout;

/// 像素矩形，原点在窗口左上角。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn max_width(self) -> f32 {
        self.w.max(0.0)
    }

    pub fn max_height(self) -> f32 {
        self.h.max(0.0)
    }
}

/// 分屏之间留出的拖拽带宽度。
///
/// 对照 macOS `PaneTitleBarGeometry.dividerLength`：Swift 的相邻 pane 之间有一条
/// 6pt 的 `DividerHandleView`（底下是 `windowBackgroundColor`），所以两个终端
/// **不贴边**。gpui 之前让 pane 直接相邻，看起来比 Swift 挤。
pub const DIVIDER_LENGTH: f32 = 6.0;

/// `ratio` 是千分比：`ratio/1000` 分给 `first`，其余给 `second`。
pub fn pane_rects(layout: &ClientLayout, area: Rect) -> Vec<(u32, Rect)> {
    let mut out = Vec::new();
    split_layout(layout, area, &mut out);
    out
}

fn split_layout(layout: &ClientLayout, area: Rect, out: &mut Vec<(u32, Rect)>) {
    match layout {
        ClientLayout::Leaf { pane_id } => out.push((*pane_id, area)),
        ClientLayout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => {
            let ratio = (*ratio).min(1000) as f32 / 1000.0;
            if *horizontal {
                let usable = (area.w - DIVIDER_LENGTH).max(1.0);
                let first_width = (usable * ratio).round().clamp(1.0, usable);
                let second_width = (usable - first_width).max(0.0);
                split_layout(first, Rect::new(area.x, area.y, first_width, area.h), out);
                split_layout(
                    second,
                    Rect::new(
                        area.x + first_width + DIVIDER_LENGTH,
                        area.y,
                        second_width,
                        area.h,
                    ),
                    out,
                );
            } else {
                let usable = (area.h - DIVIDER_LENGTH).max(1.0);
                let first_height = (usable * ratio).round().clamp(1.0, usable);
                let second_height = (usable - first_height).max(0.0);
                split_layout(first, Rect::new(area.x, area.y, area.w, first_height), out);
                split_layout(
                    second,
                    Rect::new(
                        area.x,
                        area.y + first_height + DIVIDER_LENGTH,
                        area.w,
                        second_height,
                    ),
                    out,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(pane_id: u32) -> ClientLayout {
        ClientLayout::Leaf { pane_id }
    }

    fn split(
        horizontal: bool,
        ratio: u32,
        first: ClientLayout,
        second: ClientLayout,
    ) -> ClientLayout {
        ClientLayout::Split {
            horizontal,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[test]
    fn leaf_fills_area() {
        let rects = pane_rects(&leaf(1), Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(rects, vec![(1, Rect::new(0.0, 0.0, 800.0, 600.0))]);
    }

    /// 左右分屏：扣掉 6pt 拖拽带后按比例分，两段之间留 `DIVIDER_LENGTH`。
    #[test]
    fn horizontal_split_divides_width() {
        let layout = split(true, 500, leaf(1), leaf(2));
        let rects = pane_rects(&layout, Rect::new(10.0, 0.0, 800.0, 600.0));
        // 可用 794，各 397。
        assert_eq!(rects[0], (1, Rect::new(10.0, 0.0, 397.0, 600.0)));
        assert_eq!(
            rects[1],
            (
                2,
                Rect::new(10.0 + 397.0 + DIVIDER_LENGTH, 0.0, 397.0, 600.0)
            )
        );
    }

    /// 上下分屏同样留一条水平拖拽带。
    #[test]
    fn vertical_split_divides_height() {
        let layout = split(false, 250, leaf(1), leaf(2));
        let rects = pane_rects(&layout, Rect::new(0.0, 0.0, 800.0, 600.0));
        // 可用 594，first = 148.5 → round 149，second = 445。
        assert_eq!(rects[0], (1, Rect::new(0.0, 0.0, 800.0, 149.0)));
        assert_eq!(
            rects[1],
            (2, Rect::new(0.0, 149.0 + DIVIDER_LENGTH, 800.0, 445.0))
        );
    }

    #[test]
    fn nested_split_keeps_children_inside_parent() {
        let layout = split(true, 500, leaf(1), split(false, 500, leaf(2), leaf(3)));
        let rects = pane_rects(&layout, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert_eq!(rects.len(), 3);
        // 右半边：x = 397 + 6 = 403，宽 397；高度按 594 平分（297 / 297）。
        assert_eq!(rects[1].1, Rect::new(403.0, 0.0, 397.0, 297.0));
        assert_eq!(
            rects[2].1,
            Rect::new(403.0, 297.0 + DIVIDER_LENGTH, 397.0, 297.0)
        );
    }

    #[test]
    fn ratio_is_clamped_to_full_range() {
        let layout = split(true, 2000, leaf(1), leaf(2));
        let rects = pane_rects(&layout, Rect::new(0.0, 0.0, 100.0, 100.0));
        assert_eq!(rects[0].1.w, 94.0);
        assert_eq!(rects[1].1.w, 0.0);
    }
}
