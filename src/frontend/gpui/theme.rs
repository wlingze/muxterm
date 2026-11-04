//! Design token：色板、字号、间距。只放平台无关的数值。

use gpui::{rgb, Rgba};

use crate::frontend::gpui::terminal::grid::Color;

/// 一份主题。v1 只有 night 一套，后续从 Core 配置快照热替换。
#[derive(Debug, Clone)]
pub struct Theme {
    pub bg: Rgba,
    pub fg: Rgba,
    pub sidebar_bg: Rgba,
    pub chrome_bg: Rgba,
    pub active_bg: Rgba,
    pub border: Rgba,
    pub muted: Rgba,
    pub accent: Rgba,
    pub font_family: &'static str,
    pub font_size: f32,
    ansi: [Rgba; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: rgb(0x1a1b26),
            fg: rgb(0xc0caf5),
            sidebar_bg: rgb(0x16161e),
            chrome_bg: rgb(0x1f2335),
            active_bg: rgb(0x292e42),
            border: rgb(0x2f334d),
            muted: rgb(0x7f87a8),
            accent: rgb(0x7aa2f7),
            font_family: "Menlo",
            font_size: 13.0,
            ansi: [
                rgb(0x15161e),
                rgb(0xf7768e),
                rgb(0x9ece6a),
                rgb(0xe0af68),
                rgb(0x7aa2f7),
                rgb(0xbb9af7),
                rgb(0x7dcfff),
                rgb(0xa9b1d6),
                rgb(0x414868),
                rgb(0xff899d),
                rgb(0xb9f27c),
                rgb(0xff9e64),
                rgb(0x7da6ff),
                rgb(0xc7a9ff),
                rgb(0x0db9d7),
                rgb(0xacb0d0),
            ],
        }
    }
}

impl Theme {
    /// 前景色。`Color::Default` 跟随主题。
    pub fn foreground(&self, color: Color) -> Rgba {
        self.resolve(color, self.fg)
    }

    /// 背景色。`Color::Default` 跟随主题。
    pub fn background(&self, color: Color) -> Rgba {
        self.resolve(color, self.bg)
    }

    fn resolve(&self, color: Color, fallback: Rgba) -> Rgba {
        match color {
            Color::Default => fallback,
            Color::Indexed(index) => self.indexed(index),
            Color::Rgb(r, g, b) => rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)),
        }
    }

    /// xterm 256 色：0–15 主题色，16–231 六色立方，232–255 灰阶。
    pub fn indexed(&self, index: u8) -> Rgba {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let value = index - 16;
                let steps = [0u8, 95, 135, 175, 215, 255];
                let r = steps[(value / 36) as usize];
                let g = steps[((value % 36) / 6) as usize];
                let b = steps[(value % 6) as usize];
                rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
            }
            _ => {
                let level = 8 + (index - 232) * 10;
                rgb(u32::from(level) << 16 | u32::from(level) << 8 | u32::from(level))
            }
        }
    }

    /// 单元格像素尺寸：等宽字体按 0.6 字宽、1.35 行高估算。
    pub fn cell_width(&self) -> f32 {
        (self.font_size * 0.6).round()
    }

    pub fn cell_height(&self) -> f32 {
        (self.font_size * 1.35).round()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_color_cube_matches_xterm() {
        let theme = Theme::default();
        // 196 是纯红：16 + 36*5 + 0 + 0。
        assert_eq!(theme.indexed(196), rgb(0xff0000));
        // 46 是纯绿：16 + 0 + 6*5 + 0。
        assert_eq!(theme.indexed(46), rgb(0x00ff00));
        // 21 是纯蓝。
        assert_eq!(theme.indexed(21), rgb(0x0000ff));
    }

    #[test]
    fn grayscale_ramp() {
        let theme = Theme::default();
        assert_eq!(theme.indexed(232), rgb(0x080808));
        assert_eq!(theme.indexed(255), rgb(0xeeeeee));
    }

    #[test]
    fn default_colors_follow_theme() {
        let theme = Theme::default();
        assert_eq!(theme.foreground(Color::Default), theme.fg);
        assert_eq!(theme.background(Color::Default), theme.bg);
    }

    #[test]
    fn truecolor_is_passed_through() {
        let theme = Theme::default();
        assert_eq!(theme.foreground(Color::Rgb(1, 2, 3)), rgb(0x010203));
    }
}
