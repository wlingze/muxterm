//! 主题 → ANSI 样式映射。
//!
//! 把 vte4 解析出的 SGR 参数映射成具体 RGB 颜色 + 样式位，复用 `config::Theme`
//! 的 ANSI 16 色与背景/前景。这部分是纯函数，便于单元测试。

use crate::config::{Rgb, Theme};

/// 终端字符样式（前景/背景/粗体/下划线等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

/// 解析一段 SGR 参数序列，产出新的 `CellStyle`。
///
/// 参数 `params` 是 vte 报上来的 SGR 数字序列（如 `[0]`=重置，`[1]`=粗体，
/// `[31]`=红前景，`[38;2;255;0;0]`=24位红前景，`[48;5;1]`=索引背景色 …）。
/// `current` 是当前样式，用于增量更新。
pub fn apply_sgr(params: &[u32], current: CellStyle, theme: &Theme) -> CellStyle {
    let mut s = current;
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => {
                // 重置
                s = CellStyle {
                    fg: theme.foreground,
                    bg: theme.background,
                    bold: false,
                    italic: false,
                    underline: false,
                    reverse: false,
                };
            }
            1 => s.bold = true,
            3 => s.italic = true,
            4 => s.underline = true,
            22 => s.bold = false,
            23 => s.italic = false,
            24 => s.underline = false,
            7 => s.reverse = true,
            27 => s.reverse = false,
            // 标准 8 前景色
            30..=37 => s.fg = theme.colors[(params[i] - 30) as usize],
            40..=47 => s.bg = theme.colors[(params[i] - 40) as usize],
            // 高亮 8 前景色
            90..=97 => s.fg = theme.colors[(params[i] - 90 + 8) as usize],
            100..=107 => s.bg = theme.colors[(params[i] - 100 + 8) as usize],
            38 => {
                // 前景扩展色
                if let Some(c) = parse_extended_color(params, i) {
                    s.fg = c;
                    i = skip_extended(params, i);
                    continue;
                }
            }
            48 => {
                if let Some(c) = parse_extended_color(params, i) {
                    s.bg = c;
                    i = skip_extended(params, i);
                    continue;
                }
            }
            39 => s.fg = theme.foreground,
            49 => s.bg = theme.background,
            _ => {}
        }
        i += 1;
    }
    s
}

/// 解析 38/48 扩展色（24 位或 256 色），返回 Rgb。
fn parse_extended_color(params: &[u32], i: usize) -> Option<Rgb> {
    let mode = params.get(i + 1)?;
    match mode {
        2 => {
            // `38;2;R;G;B` 或 `38;2;<colorspace>;R;G;B`（ITU-T T.416，多一个参数）
            // 兼容：跳过可能的 colorspace id
            if params.len() >= i + 5 && params[i + 2] <= 255 {
                let (r, g, b) = if params.len() >= i + 6
                    && params[i + 2] > 255.min(0)
                    && params.len() >= i + 6
                    && params.get(i + 5).is_some()
                {
                    (
                        params[i + 3] as u8,
                        params[i + 4] as u8,
                        params[i + 5] as u8,
                    )
                } else {
                    (
                        params[i + 2] as u8,
                        params[i + 3] as u8,
                        params[i + 4] as u8,
                    )
                };
                Some(Rgb(r, g, b))
            } else {
                None
            }
        }
        5 => {
            // `38;5;n` 256 色索引
            let n = *params.get(i + 2)?;
            Some(indexed_color(n))
        }
        _ => None,
    }
}

/// 跳过 38/48 扩展色参数，返回下一个待处理的索引。
fn skip_extended(params: &[u32], i: usize) -> usize {
    let mode = params.get(i + 1).copied().unwrap_or(0);
    match mode {
        2 => {
            // 标准 5 段：38;2;R;G;B；带 colorspace id 的 6 段也跳过同样数量
            i + 5
        }
        5 => i + 3,
        _ => i + 2,
    }
}

/// 256 色索引 → Rgb。
pub fn indexed_color(n: u32) -> Rgb {
    if n < 16 {
        // 标准 16 色（由主题定义，这里用近似回退；实际由 theme 提供）
        // 调用方对 0..16 已走主题映射，这里只兜底。
        standard16_fallback(n)
    } else if n >= 232 {
        // 灰阶 24 级：232..=255
        let v = 8 + (n - 232) * 10;
        let v = v.min(255) as u8;
        Rgb(v, v, v)
    } else {
        // 6x6x6 色立方 16..=231
        let n = n - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        let cv = |x: u32| -> u8 {
            if x == 0 {
                0
            } else {
                (55 + x * 40).min(255) as u8
            }
        };
        Rgb(cv(r), cv(g), cv(b))
    }
}

/// 标准前 16 色的回退表（仅用于无主题时的兜底）。
fn standard16_fallback(n: u32) -> Rgb {
    const TABLE: [Rgb; 16] = [
        Rgb(0, 0, 0),
        Rgb(205, 0, 0),
        Rgb(0, 205, 0),
        Rgb(205, 205, 0),
        Rgb(0, 0, 238),
        Rgb(205, 0, 205),
        Rgb(0, 205, 205),
        Rgb(229, 229, 229),
        Rgb(127, 127, 127),
        Rgb(255, 0, 0),
        Rgb(0, 255, 0),
        Rgb(255, 255, 0),
        Rgb(92, 92, 255),
        Rgb(255, 0, 255),
        Rgb(0, 255, 255),
        Rgb(255, 255, 255),
    ];
    TABLE[(n as usize).min(15)]
}

/// 反转前后景色（处理 SGR 7 reverse）。
pub fn resolve(s: CellStyle, theme: &Theme) -> (Rgb, Rgb) {
    if s.reverse {
        (s.bg, s.fg)
    } else {
        (s.fg, s.bg)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        let colors = (0..16)
            .map(|i| Rgb(i as u8, i as u8, i as u8))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        Theme {
            name: "test".into(),
            background: Rgb(0x1e, 0x1e, 0x2e),
            foreground: Rgb(0xcd, 0xd6, 0xf4),
            cursor: Rgb(0xf5, 0xe0, 0xdc),
            colors,
        }
    }

    fn base(t: &Theme) -> CellStyle {
        CellStyle {
            fg: t.foreground,
            bg: t.background,
            bold: false,
            italic: false,
            underline: false,
            reverse: false,
        }
    }

    #[test]
    fn reset_returns_theme_defaults() {
        let t = theme();
        let cur = CellStyle {
            fg: Rgb(1, 2, 3),
            bg: Rgb(4, 5, 6),
            bold: true,
            italic: true,
            underline: true,
            reverse: true,
        };
        let s = apply_sgr(&[0], cur, &t);
        assert_eq!(s, base(&t));
    }

    #[test]
    fn bold_italic_underline_flags() {
        let t = theme();
        let s = apply_sgr(&[1, 3, 4], base(&t), &t);
        assert!(s.bold && s.italic && s.underline);

        let s2 = apply_sgr(&[22, 23, 24], s, &t);
        assert!(!s2.bold && !s2.italic && !s2.underline);
    }

    #[test]
    fn reverse_flag() {
        let t = theme();
        let s = apply_sgr(&[7], base(&t), &t);
        assert!(s.reverse);
        let (fg, bg) = resolve(s, &t);
        // 反转后 fg=旧bg, bg=旧fg
        assert_eq!(fg, t.background);
        assert_eq!(bg, t.foreground);

        let s2 = apply_sgr(&[27], s, &t);
        assert!(!s2.reverse);
    }

    #[test]
    fn standard_fg_colors() {
        let t = theme();
        let s = apply_sgr(&[31], base(&t), &t);
        assert_eq!(s.fg, t.colors[1]);
        let s = apply_sgr(&[97], base(&t), &t);
        assert_eq!(s.fg, t.colors[15]);
    }

    #[test]
    fn standard_bg_colors() {
        let t = theme();
        let s = apply_sgr(&[42], base(&t), &t);
        assert_eq!(s.bg, t.colors[2]);
        let s = apply_sgr(&[107], base(&t), &t);
        assert_eq!(s.bg, t.colors[15]);
    }

    #[test]
    fn truecolor_24bit_fg() {
        let t = theme();
        let s = apply_sgr(&[38, 2, 255, 0, 128], base(&t), &t);
        assert_eq!(s.fg, Rgb(255, 0, 128));
    }

    #[test]
    fn truecolor_24bit_bg() {
        let t = theme();
        let s = apply_sgr(&[48, 2, 10, 20, 30], base(&t), &t);
        assert_eq!(s.bg, Rgb(10, 20, 30));
    }

    #[test]
    fn indexed_256_fg_black() {
        let t = theme();
        let s = apply_sgr(&[38, 5, 0], base(&t), &t);
        // 0..16 走回退表
        assert_eq!(s.fg, standard16_fallback(0));
    }

    #[test]
    fn indexed_256_fg_cubed() {
        let t = theme();
        // 196 = 16 + 5*36 + 0*6 + 0 → 红
        let s = apply_sgr(&[38, 5, 196], base(&t), &t);
        // 红: x=5 → 255
        assert_eq!(s.fg, Rgb(255, 0, 0));
    }

    #[test]
    fn indexed_256_greyscale() {
        let t = theme();
        let s = apply_sgr(&[38, 5, 255], base(&t), &t);
        let v = (8 + (255 - 232) * 10).min(255) as u8;
        assert_eq!(s.fg, Rgb(v, v, v));
    }

    #[test]
    fn default_fg_bg_reset() {
        let t = theme();
        let s = apply_sgr(&[31, 42], base(&t), &t);
        assert_ne!(s.fg, t.foreground);
        assert_ne!(s.bg, t.background);
        let s2 = apply_sgr(&[39, 49], s, &t);
        assert_eq!(s2.fg, t.foreground);
        assert_eq!(s2.bg, t.background);
    }

    #[test]
    fn multiple_sgr_in_one_sequence() {
        let t = theme();
        let s = apply_sgr(&[1, 31, 4], base(&t), &t);
        assert!(s.bold && s.underline);
        assert_eq!(s.fg, t.colors[1]);
    }

    #[test]
    fn cubed_color_formula() {
        // 验证 6x6x6 立方的标准公式
        assert_eq!(indexed_color(16), Rgb(0, 0, 0));
        assert_eq!(indexed_color(21), Rgb(0, 0, 255)); // 16+5 → b=255
        assert_eq!(indexed_color(196), Rgb(255, 0, 0)); // 16 + 5*36
    }
}
