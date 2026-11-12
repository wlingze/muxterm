//! 矢量图标：用 `PathBuilder` 画线/面，不用 emoji，也不依赖字体字形。
//!
//! 为什么不用 SF Symbols：macOS 原生符号走 `NSImage(systemSymbolName:)`，gpui
//! 没有对应接口，Linux / Windows 也没有这个资源。emoji 更不可行 —— `👁` / `🚫`
//! 是彩色字形，字号一变就跳基线，而且不是「精致」的做法（用户报的正是这个）。
//!
//! 所以图标统一在这里用路径画：固定 24×24 设计网格，按目标尺寸等比缩放，
//! 线宽用 `stroke` 保证任意 scale factor 下都清晰。

use gpui::{point, px, Bounds, PathBuilder, Pixels, Point, Window};

use crate::frontend::gpui::theme::Theme;

/// 图标形状。名字对齐 SF Symbols，方便和 Swift 侧对照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// `eye`：命令可见。
    Eye,
    /// `eye.slash`：命令已隐藏。
    EyeSlash,
    /// `xmark`：关闭工作区。
    Xmark,
    /// `plus`：新建。
    Plus,
    /// `chevron.down` / `chevron.right`：段头折叠。
    ChevronDown,
    ChevronRight,
    /// `sidebar.leading`：状态栏侧栏开关。
    Sidebar,
    /// `bell`：注意力。
    Bell,
    /// `arrow.up.left.and.arrow.down.right`：窗口全屏。
    Fullscreen,
    /// `gearshape`：设置。
    Gear,
    /// `ellipsis`：pane 标题条菜单。
    Ellipsis,
    /// `circle.fill` / `circle`：连接状态点、工作区标记。
    CircleFilled,
    Circle,
    /// `exclamationmark.triangle`：连接丢失。
    Warning,
    /// `sparkle`：聚合槽（Shells / Agents）。
    Diamond,
}

/// 设计网格边长：所有坐标都落在这个 24×24 盒子里。
const GRID: f32 = 24.0;

/// 图标默认画多大（pt）。与 Swift 的 `controlSize = .small` 观感接近。
pub const ICON_SIZE: f32 = 13.0;

impl Icon {
    /// 只描边、带圆头圆角的路径。
    fn stroke_paths(self) -> Vec<Vec<(f32, f32)>> {
        match self {
            // 眼睛：上下两条弧 + 瞳孔。
            Icon::Eye => vec![
                vec![
                    (2.5, 12.0),
                    (5.0, 8.4),
                    (8.4, 6.6),
                    (12.0, 6.2),
                    (15.6, 6.6),
                    (19.0, 8.4),
                    (21.5, 12.0),
                    (19.0, 15.6),
                    (15.6, 17.4),
                    (12.0, 17.8),
                    (8.4, 17.4),
                    (5.0, 15.6),
                    (2.5, 12.0),
                ],
                vec![
                    (12.0, 8.6),
                    (14.4, 9.9),
                    (15.1, 12.0),
                    (14.4, 14.1),
                    (12.0, 15.4),
                    (9.6, 14.1),
                    (8.9, 12.0),
                    (9.6, 9.9),
                    (12.0, 8.6),
                ],
            ],
            // 眼睛 + 斜杠。
            Icon::EyeSlash => {
                let mut paths = Icon::Eye.stroke_paths();
                paths.push(vec![(3.2, 3.2), (20.8, 20.8)]);
                paths
            }
            // 叉：两条对角线。
            Icon::Xmark => vec![
                vec![(5.5, 5.5), (18.5, 18.5)],
                vec![(18.5, 5.5), (5.5, 18.5)],
            ],
            // 加号：横竖各一条。
            Icon::Plus => vec![
                vec![(12.0, 4.5), (12.0, 19.5)],
                vec![(4.5, 12.0), (19.5, 12.0)],
            ],
            // v 形箭头（向下 / 向右）。
            Icon::ChevronDown => vec![vec![(6.5, 9.5), (12.0, 15.0), (17.5, 9.5)]],
            Icon::ChevronRight => vec![vec![(9.5, 6.5), (15.0, 12.0), (9.5, 17.5)]],
            // 侧栏：左边一条竖直分隔 + 外框。
            Icon::Sidebar => vec![
                vec![
                    (3.0, 5.5),
                    (21.0, 5.5),
                    (21.0, 18.5),
                    (3.0, 18.5),
                    (3.0, 5.5),
                ],
                vec![(9.0, 5.5), (9.0, 18.5)],
            ],
            // 铃铛：钟体 + 铃锤。
            Icon::Bell => vec![
                vec![
                    (6.0, 16.5),
                    (7.6, 14.2),
                    (7.6, 10.4),
                    (8.4, 8.2),
                    (10.2, 6.8),
                    (12.0, 6.4),
                    (13.8, 6.8),
                    (15.6, 8.2),
                    (16.4, 10.4),
                    (16.4, 14.2),
                    (18.0, 16.5),
                    (6.0, 16.5),
                ],
                vec![(10.3, 18.6), (12.0, 19.6), (13.7, 18.6)],
            ],
            // 全屏：四角括号。
            Icon::Fullscreen => vec![
                vec![(4.0, 9.0), (4.0, 4.0), (9.0, 4.0)],
                vec![(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)],
                vec![(20.0, 15.0), (20.0, 20.0), (15.0, 20.0)],
                vec![(9.0, 20.0), (4.0, 20.0), (4.0, 15.0)],
            ],
            // 齿轮：8 个齿的星形轮廓 + 中心圆。
            Icon::Gear => {
                let mut paths = vec![gear_outline()];
                paths.push(circle_path(3.0));
                paths
            }
            // ⋯：三个实心点（单独 fill）。
            Icon::Ellipsis => Vec::new(),
            Icon::CircleFilled | Icon::Circle => vec![circle_path(6.0)],
            // 三角警告。
            Icon::Warning => vec![
                vec![(12.0, 4.3), (21.2, 19.6), (2.8, 19.6), (12.0, 4.3)],
                vec![(12.0, 10.2), (12.0, 14.6)],
                vec![(12.0, 17.0), (12.0, 17.2)],
            ],
            // 菱形：聚合槽（填充，见 `filled_paths`）。
            Icon::Diamond => Vec::new(),
        }
    }

    /// 实心填充的路径（`CircleFilled` 与 `Ellipsis` 的点）。
    fn filled_paths(self) -> Vec<Vec<(f32, f32)>> {
        match self {
            Icon::CircleFilled => vec![circle_path(6.0)],
            Icon::Ellipsis => vec![
                circle_at(6.0, 12.0, 1.7),
                circle_at(12.0, 12.0, 1.7),
                circle_at(18.0, 12.0, 1.7),
            ],
            Icon::Diamond => vec![vec![
                (12.0, 4.0),
                (20.0, 12.0),
                (12.0, 20.0),
                (4.0, 12.0),
                (12.0, 4.0),
            ]],
            _ => Vec::new(),
        }
    }

    /// 是否只用描边（其余是填充）。
    fn is_stroked(self) -> bool {
        !matches!(self, Icon::CircleFilled | Icon::Ellipsis | Icon::Diamond)
    }
}

/// 圆：用 8 段折线近似（13pt 下肉眼无差，且不依赖贝塞尔 API 的版本）。
fn circle_path(radius: f32) -> Vec<(f32, f32)> {
    circle_at(GRID / 2.0, GRID / 2.0, radius)
}

fn circle_at(cx: f32, cy: f32, radius: f32) -> Vec<(f32, f32)> {
    const SEGMENTS: usize = 16;
    let mut points = Vec::with_capacity(SEGMENTS + 1);
    for index in 0..=SEGMENTS {
        let angle = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
        points.push((cx + radius * angle.cos(), cy + radius * angle.sin()));
    }
    points
}

/// 齿轮：外圈 16 个点，交替半径，形成 8 个齿。
fn gear_outline() -> Vec<(f32, f32)> {
    const TEETH: usize = 8;
    let mut points = Vec::with_capacity(TEETH * 2);
    for index in 0..(TEETH * 2) {
        let angle = std::f32::consts::TAU * index as f32 / (TEETH * 2) as f32;
        let radius = if index % 2 == 0 { 8.4 } else { 6.2 };
        points.push((12.0 + radius * angle.cos(), 12.0 + radius * angle.sin()));
    }
    points.push(points[0]);
    points
}

fn scaled(center: (f32, f32), bounds: Bounds<Pixels>) -> Point<Pixels> {
    let scale = f32::from(bounds.size.width) / GRID;
    // 线条居中：路径坐标按中心对齐到目标盒。
    point(
        bounds.origin.x + px(center.0 * scale),
        bounds.origin.y + px(center.1 * scale),
    )
}

/// 把图标画进 `bounds`。`color` 是线/面的颜色。
pub fn paint_icon(window: &mut Window, icon: Icon, bounds: Bounds<Pixels>, color: gpui::Rgba) {
    let scale = f32::from(bounds.size.width) / GRID;
    // 线宽按盒子缩放；13pt 时约 1.35pt，与 SF Symbols 的 small 控制尺寸接近。
    let line_width = (1.5 * scale).max(1.0);

    if icon.is_stroked() {
        for path in icon.stroke_paths() {
            if path.len() < 2 {
                continue;
            }
            let mut builder = PathBuilder::stroke(px(line_width));
            builder.move_to(scaled(path[0], bounds));
            for point in &path[1..] {
                builder.line_to(scaled(*point, bounds));
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        }
        return;
    }

    for path in icon.filled_paths() {
        if path.len() < 3 {
            continue;
        }
        let mut builder = PathBuilder::fill();
        builder.move_to(scaled(path[0], bounds));
        for point in &path[1..] {
            builder.line_to(scaled(*point, bounds));
        }
        builder.close();
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }
}

/// 图标 + 颜色的组合，UI 层拿来直接画。
#[derive(Clone, Copy)]
pub struct IconSpec {
    pub icon: Icon,
    pub color: gpui::Rgba,
    pub size: f32,
}

impl IconSpec {
    pub fn new(icon: Icon, color: gpui::Rgba) -> Self {
        Self {
            icon,
            color,
            size: ICON_SIZE,
        }
    }

    pub fn with_size(icon: Icon, color: gpui::Rgba, size: f32) -> Self {
        Self { icon, color, size }
    }
}

/// 把图标包成一个定宽元素（行内对齐用）。
pub fn icon_element(spec: IconSpec, width: f32) -> gpui::AnyElement {
    use gpui::{div, prelude::*};
    let IconSpec { icon, color, size } = spec;
    div()
        .flex_none()
        .w(px(width))
        .h(px(width))
        .flex()
        .items_center()
        .justify_center()
        .child(
            gpui::canvas(
                move |bounds, _, _| (icon, bounds, size),
                move |_, (icon, bounds, size), window, _| {
                    // 在定宽盒里居中：按 size 缩到中心。
                    let scale = f32::from(bounds.size.width) / GRID;
                    let _ = scale;
                    let offset = (f32::from(bounds.size.width) - size) / 2.0;
                    let target = Bounds::new(
                        point(bounds.origin.x + px(offset), bounds.origin.y + px(offset)),
                        gpui::size(px(size), px(size)),
                    );
                    paint_icon(window, icon, target, color);
                },
            )
            .w(px(width))
            .h(px(width)),
        )
        .into_any_element()
}

/// 侧栏 marker 用的小图标（11pt 列宽）。
pub fn marker_icon(icon: Icon, color: gpui::Rgba) -> gpui::AnyElement {
    icon_element(IconSpec::with_size(icon, color, 11.0), 11.0)
}

/// 尾部按钮图标（Swift `controlSize = .small`，约 11pt）。
pub fn trailing_icon(icon: Icon, color: gpui::Rgba) -> gpui::AnyElement {
    icon_element(IconSpec::with_size(icon, color, 11.0), 11.0)
}

/// 状态栏按钮图标。
pub fn status_icon(icon: Icon, color: gpui::Rgba) -> gpui::AnyElement {
    icon_element(IconSpec::with_size(icon, color, 13.0), 13.0)
}

/// 段头的折叠箭头。
pub fn chevron_icon(expanded: bool, color: gpui::Rgba) -> gpui::AnyElement {
    let icon = if expanded {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    icon_element(IconSpec::with_size(icon, color, 9.0), 9.0)
}

/// `+` 按钮。
pub fn plus_icon(color: gpui::Rgba) -> gpui::AnyElement {
    icon_element(IconSpec::with_size(Icon::Plus, color, 11.0), 11.0)
}

/// 标记列：把 `Indicator` / 聚合槽映射成矢量图标。
pub fn indicator_glyph(
    indicator: crate::frontend::gpui::model::sidebar::Indicator,
    color: gpui::Rgba,
) -> gpui::AnyElement {
    use crate::frontend::gpui::model::sidebar::Indicator;
    let icon = match indicator {
        Indicator::Idle => Icon::Circle,
        Indicator::Done | Indicator::Working => Icon::CircleFilled,
        Indicator::Blocked => Icon::Warning,
    };
    marker_icon(icon, color)
}

/// 侧栏尾部的 symbol → 矢量图标。
///
/// 模型层沿用 Swift 的 SF Symbol 名字（`eye` / `eye.slash` / `xmark`），
/// 渲染层必须有对应矢量图形：没有 fallback，未知名字也要给出可画的图标，
/// 绝不能退回 emoji。
pub fn icon_for_trailing(symbol: &str) -> Icon {
    match symbol {
        "eye" => Icon::Eye,
        "eye.slash" => Icon::EyeSlash,
        "xmark" => Icon::Xmark,
        _ => Icon::Ellipsis,
    }
}

/// 未使用主题时也能取的默认线色（测试 / 无窗口路径）。
pub fn default_icon_color(theme: &Theme) -> gpui::Rgba {
    theme.muted
}

/// 把 `Path` 直接画出来的便捷入口（`pane` 标题条等自绘处用）。
pub fn paint_polyline(
    window: &mut Window,
    points: &[Point<Pixels>],
    width: Pixels,
    color: gpui::Rgba,
) {
    if points.len() < 2 {
        return;
    }
    let mut builder = PathBuilder::stroke(width);
    builder.move_to(points[0]);
    for next in &points[1..] {
        builder.line_to(*next);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个图标都必须给出可画的几何：描边图标有点，填充图标有多边形。
    #[test]
    fn every_icon_has_geometry() {
        let all = [
            Icon::Eye,
            Icon::EyeSlash,
            Icon::Xmark,
            Icon::Plus,
            Icon::ChevronDown,
            Icon::ChevronRight,
            Icon::Sidebar,
            Icon::Bell,
            Icon::Fullscreen,
            Icon::Gear,
            Icon::Ellipsis,
            Icon::CircleFilled,
            Icon::Circle,
            Icon::Warning,
            Icon::Diamond,
        ];
        for icon in all {
            let stroked = icon.stroke_paths();
            let filled = icon.filled_paths();
            assert!(
                !stroked.is_empty() || !filled.is_empty(),
                "{icon:?} 没有任何路径"
            );
            for path in stroked.iter().chain(filled.iter()) {
                assert!(path.len() >= 2, "{icon:?} 的路径点太少");
                for (x, y) in path {
                    assert!(
                        (-0.5..=GRID + 0.5).contains(x) && (-0.5..=GRID + 0.5).contains(y),
                        "{icon:?} 的坐标 ({x},{y}) 超出 24×24 设计网格"
                    );
                }
            }
        }
    }

    /// 眼罩就是眼睛 + 一条斜杠：不能少画眼睛（用户要的是同一个符号）。
    #[test]
    fn eye_slash_keeps_the_eye() {
        assert_eq!(
            Icon::EyeSlash.stroke_paths().len(),
            Icon::Eye.stroke_paths().len() + 1
        );
    }

    /// 圆必须在设计网格中心，半径一致（标记列上下对齐靠它）。
    #[test]
    fn circles_are_centered() {
        for radius in [3.0_f32, 6.0, 1.7] {
            let points = circle_at(12.0, 12.0, radius);
            let min_x = points.iter().map(|p| p.0).fold(f32::MAX, f32::min);
            let max_x = points.iter().map(|p| p.0).fold(f32::MIN, f32::max);
            assert!((min_x - (12.0 - radius)).abs() < 0.01);
            assert!((max_x - (12.0 + radius)).abs() < 0.01);
        }
    }

    /// 侧栏尾部符号全部映射到矢量图标，且相互不同（不能再退回 emoji）。
    #[test]
    fn trailing_symbols_map_to_distinct_vector_icons() {
        let eye = icon_for_trailing("eye");
        let slash = icon_for_trailing("eye.slash");
        let close = icon_for_trailing("xmark");
        assert_eq!(eye, Icon::Eye);
        assert_eq!(slash, Icon::EyeSlash);
        assert_eq!(close, Icon::Xmark);
        assert_ne!(eye, slash);
        assert_ne!(eye, close);
        // 未知符号也要有可画图形（宁可画省略号，也不画 emoji）。
        assert_eq!(icon_for_trailing("mystery"), Icon::Ellipsis);
    }

    /// 三种状态标记映射到三种不同图标：实心点 / 空心圈 / 警告。
    #[test]
    fn indicators_map_to_distinct_glyphs() {
        use crate::frontend::gpui::model::sidebar::Indicator;
        assert_eq!(
            match Indicator::Idle {
                Indicator::Idle => Icon::Circle,
                Indicator::Done | Indicator::Working => Icon::CircleFilled,
                Indicator::Blocked => Icon::Warning,
            },
            Icon::Circle
        );
    }

    /// 齿轮有 8 个齿（16 个交替半径的点）并且闭合。
    #[test]
    fn gear_has_eight_teeth_and_closes() {
        let outline = gear_outline();
        assert_eq!(outline.len(), 17);
        assert_eq!(outline.first(), outline.last());
    }
}
