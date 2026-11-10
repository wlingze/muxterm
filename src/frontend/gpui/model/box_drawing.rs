//! 制表符 / 方块元素的**几何**绘制（不依赖字体）。
//!
//! 对照 iTerm2 `iTermBoxDrawingRenderer` 与 Ghostty 的 box-drawing/glyph
//! 特判：字体里的 `─│┌┐└┘` 各自带留白，按格宽摆出去必然断线、错位；
//! `█▀▄▌▐` 与八分块在多数等宽字体里宽度也不一致（用户看到的就是「表格
//! 断线、进度条参差」）。这里把这些码点换成对齐到格子的矩形：
//!
//! * 线段端点永远落在格子边界上，相邻格自然接上；
//! * 方块按 1/8 分辨率填充，`▏▎▍▌▋▊▉█` 连起来是一条直线；
//! * `░▒▓` 用透明度叠加，颜色永远跟着当前前景色。

/// 一格像素尺寸。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub width: f32,
    pub height: f32,
}

/// 一个待画矩形（相对格子左上角）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 0..1；`░▒▓` 用不到 1 的透明度叠加在前景上。
    pub alpha: f32,
}

/// 线段粗细：细线取格子高度的 8%（最少 1px），粗线翻倍。
fn stroke(cell: Cell) -> f32 {
    (cell.height * 0.08).round().max(1.0)
}

/// 线宽等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Weight {
    None,
    Light,
    Heavy,
    Double,
}

/// 一个制表符往四个方向的延伸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Arms {
    up: Weight,
    down: Weight,
    left: Weight,
    right: Weight,
}

const fn arms(up: Weight, down: Weight, left: Weight, right: Weight) -> Arms {
    Arms {
        up,
        down,
        left,
        right,
    }
}

use Weight::{Double, Heavy, Light, None as N};

/// 码点 → 四个方向的线型。表里没有的返回 `None`（交回字体绘制）。
fn arms_for(ch: char) -> Option<Arms> {
    let value = ch as u32;
    let arms = match ch {
        // 单线：横竖与各种转角 / 三叉 / 十字。
        '─' => arms(N, N, Light, Light),
        '│' => arms(Light, Light, N, N),
        '┌' => arms(N, Light, N, Light),
        '┐' => arms(N, Light, Light, N),
        '└' => arms(Light, N, N, Light),
        '┘' => arms(Light, N, Light, N),
        '├' => arms(Light, Light, N, Light),
        '┤' => arms(Light, Light, Light, N),
        '┬' => arms(N, Light, Light, Light),
        '┴' => arms(Light, N, Light, Light),
        '┼' => arms(Light, Light, Light, Light),
        // 粗线。
        '━' => arms(N, N, Heavy, Heavy),
        '┃' => arms(Heavy, Heavy, N, N),
        '┏' => arms(N, Heavy, N, Heavy),
        '┓' => arms(N, Heavy, Heavy, N),
        '┗' => arms(Heavy, N, N, Heavy),
        '┛' => arms(Heavy, N, Heavy, N),
        '┣' => arms(Heavy, Heavy, N, Heavy),
        '┫' => arms(Heavy, Heavy, Heavy, N),
        '┳' => arms(N, Heavy, Heavy, Heavy),
        '┻' => arms(Heavy, N, Heavy, Heavy),
        '╋' => arms(Heavy, Heavy, Heavy, Heavy),
        // 双线。
        '═' => arms(N, N, Double, Double),
        '║' => arms(Double, Double, N, N),
        '╔' => arms(N, Double, N, Double),
        '╗' => arms(N, Double, Double, N),
        '╚' => arms(Double, N, N, Double),
        '╝' => arms(Double, N, Double, N),
        '╠' => arms(Double, Double, N, Double),
        '╣' => arms(Double, Double, Double, N),
        '╦' => arms(N, Double, Double, Double),
        '╩' => arms(Double, N, Double, Double),
        '╬' => arms(Double, Double, Double, Double),
        // 圆角：两条臂在格子中心相接（半径小于半格，视觉上与直角一致），
        // 但端点仍然贴格边，保证和相邻格子接得上。
        '╭' => arms(N, Light, N, Light),
        '╮' => arms(N, Light, Light, N),
        '╰' => arms(Light, N, N, Light),
        '╯' => arms(Light, N, Light, N),
        // 半线。
        '╴' => arms(N, N, Light, N),
        '╵' => arms(Light, N, N, N),
        '╶' => arms(N, N, N, Light),
        '╷' => arms(N, Light, N, N),
        _ => return box_mixed(ch, value),
    };
    Some(arms)
}

/// 粗细混合 / 双线混合区（U+250D–U+257F）的近似：取出现过的线型里最粗的那个。
///
/// 混合线的视觉目标是「能接上、不歪」，精确到每条边粗细没有意义；粗/双线
/// 已经单独成表。
fn box_mixed(_ch: char, value: u32) -> Option<Arms> {
    // 粗线的起止与混合：U+2501–U+2503、U+250D–U+254B 中带粗边的码点。
    let heavy = |arms: Arms| {
        let up = if arms.up != N { Heavy } else { N };
        let down = if arms.down != N { Heavy } else { N };
        let left = if arms.left != N { Heavy } else { N };
        let right = if arms.right != N { Heavy } else { N };
        Arms {
            up,
            down,
            left,
            right,
        }
    };
    // U+250D..=U+2513 / U+2515..=U+251B / U+251D..=U+2523 / U+2525..=U+252B /
    // U+252D..=U+2533 / U+2535..=U+253B / U+253D..=U+254B 里「奇数」码点带粗边。
    let (light_equiv, is_heavy) = match value {
        0x2504..=0x250B => ('─', false),
        0x250D..=0x2513 => ('┌', value % 2 == 1),
        0x2515..=0x251B => ('└', value % 2 == 1),
        0x251D..=0x2523 => ('├', value % 2 == 1),
        0x2525..=0x252B => ('┤', value % 2 == 1),
        0x252D..=0x2533 => ('┬', value % 2 == 1),
        0x2535..=0x253B => ('┴', value % 2 == 1),
        0x253D..=0x254B => ('┼', value % 2 == 1),
        0x254C..=0x254F => ('─', false),
        0x2552..=0x255D => ('╔', false),
        0x2561..=0x256C => ('╬', false),
        0x2574..=0x257F => ('╴', false),
        _ => return None,
    };
    // 混合线按「起点形状」近似：`├` 系沿左上、`┤` 系沿右上等等。
    let base = match light_equiv {
        '┌' => arms(N, Light, N, Light),
        '└' => arms(Light, N, N, Light),
        '├' => arms(Light, Light, N, Light),
        '┤' => arms(Light, Light, Light, N),
        '┬' => arms(N, Light, Light, Light),
        '┴' => arms(Light, N, Light, Light),
        '┼' => arms(Light, Light, Light, Light),
        '─' => arms(N, N, Light, Light),
        '╔' => arms(N, Double, N, Double),
        '╬' => arms(Double, Double, Double, Double),
        '╴' => arms(N, N, Light, N),
        _ => return None,
    };
    Some(if is_heavy { heavy(base) } else { base })
}

/// 一个字符要画的矩形；`None` 表示交回字体。
pub fn shapes(ch: char, cell: Cell) -> Option<Vec<Rect>> {
    if let Some(arms) = arms_for(ch) {
        return Some(line_shapes(arms, cell));
    }
    block_shapes(ch, cell)
}

/// 这个码点是否由几何绘制接管（渲染时不再画字体字形）。
pub fn is_custom(ch: char) -> bool {
    arms_for(ch).is_some()
        || block_shapes(
            ch,
            Cell {
                width: 1.0,
                height: 1.0,
            },
        )
        .is_some()
}

/// 线宽 → 像素。
fn width_of(weight: Weight, cell: Cell) -> f32 {
    match weight {
        Weight::None => 0.0,
        Weight::Light => stroke(cell),
        Weight::Heavy => (stroke(cell) * 2.0).max(2.0),
        Weight::Double => stroke(cell),
    }
}

/// 四条臂 → 矩形。所有端点对齐到格子边界，保证相邻格接得上。
fn line_shapes(arms: Arms, cell: Cell) -> Vec<Rect> {
    let mut rects = Vec::new();
    // 水平方向：左臂 / 右臂。
    for (weight, from_left) in [(arms.left, true), (arms.right, false)] {
        push_arm(&mut rects, weight, cell, from_left, true);
    }
    // 垂直方向：上臂 / 下臂。
    for (weight, from_top) in [(arms.up, true), (arms.down, false)] {
        push_arm(&mut rects, weight, cell, from_top, false);
    }
    rects
}

/// 单侧臂。`positive` 表示从中心朝左 / 朝上。
fn push_arm(rects: &mut Vec<Rect>, weight: Weight, cell: Cell, positive: bool, horizontal: bool) {
    let t = width_of(weight, cell);
    if t <= 0.0 {
        return;
    }
    // 双线：两条细线夹一条同宽缝隙，整体仍在格子中线两侧。
    let offsets: Vec<f32> = if weight == Weight::Double {
        vec![-(t + t) / 2.0, (t + t) / 2.0]
    } else {
        vec![0.0]
    };
    let cross = if horizontal { cell.height } else { cell.width };
    // 臂从格子边界画到中心（两端都到中线），转角处自然重叠不留缝。
    let length = if horizontal { cell.width } else { cell.height };
    let start = if positive { 0.0 } else { length / 2.0 };
    let end = if positive { length / 2.0 } else { length };
    let center = cross / 2.0;
    for offset in offsets {
        let rect = if horizontal {
            Rect {
                x: start,
                y: center - t / 2.0 + offset,
                w: (end - start).max(t),
                h: t,
                alpha: 1.0,
            }
        } else {
            let x = center - t / 2.0 + offset;
            Rect {
                x,
                y: start,
                w: t,
                h: (end - start).max(t),
                alpha: 1.0,
            }
        };
        rects.push(rect);
    }
}

/// 方块元素：1/8 网格填充。
fn block_shapes(ch: char, cell: Cell) -> Option<Vec<Rect>> {
    let w = cell.width;
    let h = cell.height;
    let full = |alpha: f32| {
        vec![Rect {
            x: 0.0,
            y: 0.0,
            w,
            h,
            alpha,
        }]
    };
    let bottom = |n: f32| {
        vec![Rect {
            x: 0.0,
            y: h * (1.0 - n),
            w,
            h: h * n,
            alpha: 1.0,
        }]
    };
    let left = |n: f32| {
        vec![Rect {
            x: 0.0,
            y: 0.0,
            w: w * n,
            h,
            alpha: 1.0,
        }]
    };
    let right = |n: f32| {
        vec![Rect {
            x: w * (1.0 - n),
            y: 0.0,
            w: w * n,
            h,
            alpha: 1.0,
        }]
    };
    let top = |n: f32| {
        vec![Rect {
            x: 0.0,
            y: 0.0,
            w,
            h: h * n,
            alpha: 1.0,
        }]
    };
    // 四个象限（左上、右上、左下、右下）。
    let quadrants = |lu: bool, ru: bool, ld: bool, rd: bool| {
        let mut rects = Vec::new();
        if lu {
            rects.push(Rect {
                x: 0.0,
                y: 0.0,
                w: w / 2.0,
                h: h / 2.0,
                alpha: 1.0,
            });
        }
        if ru {
            rects.push(Rect {
                x: w / 2.0,
                y: 0.0,
                w: w / 2.0,
                h: h / 2.0,
                alpha: 1.0,
            });
        }
        if ld {
            rects.push(Rect {
                x: 0.0,
                y: h / 2.0,
                w: w / 2.0,
                h: h / 2.0,
                alpha: 1.0,
            });
        }
        if rd {
            rects.push(Rect {
                x: w / 2.0,
                y: h / 2.0,
                w: w / 2.0,
                h: h / 2.0,
                alpha: 1.0,
            });
        }
        rects
    };
    Some(match ch {
        '█' => full(1.0),
        '▀' => top(0.5),
        '▄' => bottom(0.5),
        '▌' => left(0.5),
        '▐' => right(0.5),
        '▔' => top(0.125),
        '▕' => right(0.125),
        '▁' => bottom(0.125),
        '▂' => bottom(0.25),
        '▃' => bottom(0.375),
        '▅' => bottom(0.625),
        '▆' => bottom(0.75),
        '▇' => bottom(0.875),
        '▏' => left(0.125),
        '▎' => left(0.25),
        '▍' => left(0.375),
        '▋' => left(0.625),
        '▊' => left(0.75),
        '▉' => left(0.875),
        '░' => full(0.25),
        '▒' => full(0.5),
        '▓' => full(0.75),
        '▖' => quadrants(false, false, true, false),
        '▗' => quadrants(false, false, false, true),
        '▘' => quadrants(true, false, false, false),
        '▝' => quadrants(false, true, false, false),
        '▙' => quadrants(true, false, true, true),
        '▚' => quadrants(true, false, false, true),
        '▛' => quadrants(true, true, true, false),
        '▜' => quadrants(true, true, false, true),
        '▞' => quadrants(false, true, true, false),
        '▟' => quadrants(false, true, true, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Cell {
        Cell {
            width: 10.0,
            height: 20.0,
        }
    }

    /// 横线必须左右都贴格边：否则相邻两格之间出现 1px 缝。
    #[test]
    fn horizontal_line_spans_the_whole_cell() {
        let rects = shapes('─', cell()).unwrap();
        assert_eq!(rects.len(), 2, "左右两段");
        let left = rects[0];
        let right = rects[1];
        assert_eq!(left.x, 0.0);
        assert_eq!(left.x + left.w, 5.0);
        assert_eq!(right.x, 5.0);
        assert_eq!(right.x + right.w, 10.0);
        assert_eq!(left.y, right.y);
    }

    /// 竖线上下贴边，且和横线中线对齐（转角处不留缺口）。
    #[test]
    fn vertical_line_meets_the_horizontal_center() {
        let cell = cell();
        let vertical = shapes('│', cell).unwrap();
        let horizontal = shapes('─', cell).unwrap();
        assert_eq!(vertical[0].y, 0.0);
        assert_eq!(vertical[0].y + vertical[0].h, cell.height / 2.0);
        assert_eq!(vertical[0].x + vertical[0].w / 2.0, cell.width / 2.0);
        assert_eq!(horizontal[0].y + horizontal[0].h / 2.0, cell.height / 2.0);
        // 两臂都覆盖到格子中线：转角处必然是实心的。
        assert_eq!(vertical[0].y + vertical[0].h, cell.height / 2.0);
        assert_eq!(horizontal[0].x + horizontal[0].w, cell.width / 2.0);
    }

    /// 十字的四个方向都要有，缺一段就会在表格里露白。
    #[test]
    fn cross_draws_four_arms() {
        let rects = shapes('┼', cell()).unwrap();
        assert_eq!(rects.len(), 4);
        assert!(rects.iter().any(|rect| rect.x == 0.0));
        assert!(rects.iter().any(|rect| rect.x + rect.w == 10.0));
        assert!(rects.iter().any(|rect| rect.y == 0.0));
        assert!(rects.iter().any(|rect| rect.y + rect.h == 20.0));
    }

    /// 双线是两条细线，中间留缝。
    #[test]
    fn double_line_has_a_gap() {
        let rects = shapes('═', cell()).unwrap();
        assert_eq!(rects.len(), 4, "左右各两条");
        let mut ys: Vec<f32> = rects.iter().map(|rect| rect.y).collect();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.dedup();
        assert_eq!(ys.len(), 2, "两条不同的水平线");
        assert!(ys[1] - ys[0] > rects[0].h, "两条线之间要有缝");
    }

    /// 八分块：相邻两个块叠起来正好是整格，进度条才不会锯齿。
    #[test]
    fn eighth_blocks_stack_to_a_full_cell() {
        let cell = cell();
        let bar: f32 = (1..=8)
            .map(|n| {
                let ch = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][n - 1];
                shapes(ch, cell).unwrap()[0].h / cell.height
            })
            .sum::<f32>();
        assert!((bar - 4.5).abs() < 0.001, "1/8..8/8 之和 = 4.5 格：{bar}");
        for (index, ch) in ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'].iter().enumerate()
        {
            let rect = shapes(*ch, cell).unwrap()[0];
            assert!(
                (rect.h / cell.height - (index + 1) as f32 / 8.0).abs() < 0.001,
                "{ch} 的高度必须是 {}/8",
                index + 1
            );
            assert_eq!(rect.y + rect.h, cell.height, "底边对齐格底");
        }
    }

    /// 左八分块：宽度递增且都贴左边。
    #[test]
    fn left_eighth_blocks_grow_to_the_right() {
        for (index, ch) in ['▏', '▎', '▍', '▌', '▋', '▊', '▉'].iter().enumerate() {
            let rect = shapes(*ch, cell()).unwrap()[0];
            assert_eq!(rect.x, 0.0);
            assert!(
                (rect.w / 10.0 - (index + 1) as f32 / 8.0).abs() < 0.001,
                "{ch} 宽度错误"
            );
        }
    }

    /// 阴影块用透明度叠加，颜色跟随前景色。
    #[test]
    fn shades_use_alpha() {
        assert_eq!(shapes('░', cell()).unwrap()[0].alpha, 0.25);
        assert_eq!(shapes('▒', cell()).unwrap()[0].alpha, 0.5);
        assert_eq!(shapes('▓', cell()).unwrap()[0].alpha, 0.75);
        assert_eq!(shapes('█', cell()).unwrap()[0].alpha, 1.0);
    }

    /// 四个象限拼起来等于整格，互不重叠。
    #[test]
    fn quadrant_blocks_tile_the_cell() {
        let cell = cell();
        let rects = shapes('▟', cell).unwrap();
        assert_eq!(rects.len(), 3, "▟ = 右上 + 左下 + 右下");
        let area: f32 = rects.iter().map(|rect| rect.w * rect.h).sum();
        assert!((area - cell.width * cell.height * 0.75).abs() < 0.001);
    }

    /// 普通字符不能被接管，否则会画出空白。
    #[test]
    fn ordinary_text_stays_with_the_font() {
        for ch in ['a', 'A', '中', '😀', ' ', '~'] {
            assert!(!is_custom(ch), "{ch} 不该被几何绘制接管");
            assert!(shapes(ch, cell()).is_none());
        }
        assert!(is_custom('─'));
        assert!(is_custom('█'));
        assert!(is_custom('╰'));
    }
}
