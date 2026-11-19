//! 设计 token：色板、字号、间距。只放平台无关的数值。
//!
//! 对照 macOS：
//! - 终端调色板来自 Core 的 `resolved_theme`（`[theme] name` → white / black / …），
//!   浅色是默认：白底黑字，深色是深底浅字；`cursor` 与 ANSI 16 色一起跟随。
//! - chrome（侧栏 / tab 栏 / 状态栏 / 浮层）用 AppKit 语义色的等价常量：
//!   浅色对应 aqua，深色对应 darkAqua（`NSColor.windowBackgroundColor` 等）。
//! - 字体家族/字号/行距/字距来自 `[font]`，缺省与 `MuxtermTerminalFont` 一致
//!   （Menlo 18、行距与字距 1.0）。

use gpui::{rgb, Rgba, SharedString};

use crate::frontend::gpui::platform;
use crate::frontend::gpui::terminal::grid::Color;
use crate::frontend::utils::corebridge::{ClientFontConfig, ClientTheme};

/// 终端字体缺省值：按平台取该平台自带的等宽字体。
///
/// Core 的配置缺省会写 `JetBrains Mono`（`assets/fonts/` 里带了这份字体，
/// 启动时会注册进 `TextSystem`）；这个常量只用于配置缺失与首帧。
pub const DEFAULT_FONT_FAMILY: &str = platform::os().default_monospace_family();
pub const DEFAULT_FONT_SIZE: f32 = 18.0;
/// 行距/字距的默认倍数（1.0 = 字体本来的行高/字宽）。
pub const DEFAULT_FONT_SPACING: f32 = 1.0;
/// 与 `[font] size` 的合法区间一致（`MuxtermTerminalFont.minSize/maxSize`）。
pub const MIN_FONT_SIZE: f32 = 9.0;
pub const MAX_FONT_SIZE: f32 = 36.0;
pub const MIN_FONT_SPACING: f32 = 0.5;
pub const MAX_FONT_SPACING: f32 = 2.0;

/// Chrome 字号：与 `UI/StatusBarView.swift` / `App/WorkspaceSidebar.swift` 一致。
pub const TAB_BAR_FONT_SIZE: f32 = 11.0;
pub const STATUS_BAR_FONT_SIZE: f32 = 11.0;
pub const SIDEBAR_SECTION_FONT_SIZE: f32 = 10.5;
pub const SIDEBAR_TITLE_FONT_SIZE: f32 = 12.0;
pub const SIDEBAR_DETAIL_FONT_SIZE: f32 = 10.0;
pub const SIDEBAR_SHORTCUT_FONT_SIZE: f32 = 9.5;

/// 一份主题。浅色与深色两套 chrome + 任意终端调色板。
#[derive(Debug, Clone)]
pub struct Theme {
    /// 终端背景（`Color::Default` 的底色）。
    pub bg: Rgba,
    /// 终端前景（`Color::Default` 的字色）。
    pub fg: Rgba,
    /// 光标色。Core 的 `resolved_theme.cursor`。
    pub cursor: Rgba,
    /// 侧栏底色（`NSColor.controlBackgroundColor`）。
    pub sidebar_bg: Rgba,
    /// tab 栏 / 状态栏 / 浮层底色（`NSColor.windowBackgroundColor`）。
    pub chrome_bg: Rgba,
    /// 选中行底色（`NSColor.selectedContentBackgroundColor` 的低饱和替代）。
    pub active_bg: Rgba,
    /// 分隔线（`NSColor.separatorColor`）。
    pub border: Rgba,
    /// 次要文字（`NSColor.secondaryLabelColor`）。
    pub muted: Rgba,
    /// 强调色（`NSColor.controlAccentColor`）。
    pub accent: Rgba,
    /// 成功/完成态（侧栏 done 指示、图片粘贴成功横幅）。
    pub success: Rgba,
    /// 危险/失败态（连接丢失、失败横幅）。
    pub danger: Rgba,
    /// 进行中（tab 上的 working 环）。
    pub warning: Rgba,
    /// Swift `NSColor.systemYellow`：侧栏 working 标记。
    pub working: Rgba,
    /// Swift `NSColor.systemPink`：blocked 标记（不是红色 danger）。
    pub blocked: Rgba,
    /// Swift `NSColor.systemTeal`：done 标记（不是绿色 success）。
    pub done: Rgba,
    /// Swift `NSColor.systemPurple`：Agents 聚合槽的身份色。
    pub purple: Rgba,
    /// 终端字体家族（`[font] family`）。
    pub font_family: SharedString,
    /// 用户配置的回退字体（`[font] fallback`），排在平台回退栈前面。
    pub font_fallback: Vec<SharedString>,
    /// 终端字号（`[font] size`）。
    pub font_size: f32,
    /// 行距倍数（`[font] line_spacing`）。
    pub line_spacing: f32,
    /// 字距倍数（`[font] column_spacing`）。
    pub column_spacing: f32,
    /// 深色 chrome？由终端背景亮度决定，不猜配置名。
    pub is_dark: bool,
    ansi: [Rgba; 16],
}

impl Default for Theme {
    /// 浅色默认：Core `configs/themes/white.toml` + Menlo 18。
    fn default() -> Self {
        Self {
            bg: rgb(0xffffff),
            fg: rgb(0x1f2328),
            cursor: rgb(0x1f2328),
            sidebar_bg: rgb(0xffffff),
            chrome_bg: rgb(0xececec),
            active_bg: rgb(0xe3e3e3),
            border: rgb(0xd6d6d6),
            muted: rgb(0x6e7781),
            accent: rgb(0x007aff),
            success: rgb(0x28cd41),
            danger: rgb(0xff3b30),
            warning: rgb(0xff9500),
            // 与 macOS `WorkspaceSidebar.indicatorColor` / `TabActivityIndicatorView`
            // 一致：working = systemYellow、blocked = systemPink、done = systemTeal。
            working: rgb(0xffcc00),
            blocked: rgb(0xff2d55),
            done: rgb(0x30b0c7),
            purple: rgb(0xaf52de),
            font_family: SharedString::from(DEFAULT_FONT_FAMILY),
            font_fallback: Vec::new(),
            font_size: DEFAULT_FONT_SIZE,
            line_spacing: DEFAULT_FONT_SPACING,
            column_spacing: DEFAULT_FONT_SPACING,
            is_dark: false,
            // `configs/themes/white.toml` 的 16 色（GitHub Light）。
            ansi: [
                rgb(0x24292f),
                rgb(0xcf222e),
                rgb(0x116329),
                rgb(0x7d4e00),
                rgb(0x0969da),
                rgb(0x8250df),
                rgb(0x0a6b6b),
                rgb(0x6e7781),
                rgb(0x57606a),
                rgb(0xa40e26),
                rgb(0x1a7f37),
                rgb(0x9a6700),
                rgb(0x218bff),
                rgb(0xa475f9),
                rgb(0x3192aa),
                rgb(0x8c959f),
            ],
        }
    }
}

impl Theme {
    /// 从 Core 快照拼一份主题。
    ///
    /// `resolved` 为空时退回 `Theme::default()` 的浅色，前端不自己解析配置名。
    pub fn from_config(resolved: Option<&ClientTheme>, font: &ClientFontConfig) -> Self {
        let mut theme = match resolved {
            Some(resolved) => Self::from_palette(resolved),
            None => Self::default(),
        };
        theme.font_family = SharedString::from(font.family.trim().to_string());
        if theme.font_family.is_empty() {
            theme.font_family = SharedString::from(DEFAULT_FONT_FAMILY);
        }
        // 配置里的 `[font] fallback` 是用户显式要求的回退顺序，排在平台栈前面。
        theme.font_fallback = font
            .fallback
            .iter()
            .map(|family| SharedString::from(family.trim().to_string()))
            .filter(|family| !family.is_empty())
            .collect();
        theme.font_size = clamp_font_size(font.size);
        theme.line_spacing = DEFAULT_FONT_SPACING;
        theme.column_spacing = DEFAULT_FONT_SPACING;
        theme
    }

    /// Core 解析好的终端调色板 → 主题。chrome 颜色只按明暗分流。
    pub fn from_palette(resolved: &ClientTheme) -> Self {
        let bg = rgba_of(resolved.background);
        let dark = luminance(resolved.background) < 0.5;
        let mut theme = if dark { Self::dark() } else { Self::default() };
        theme.bg = bg;
        theme.fg = rgba_of(resolved.foreground);
        theme.cursor = rgba_of(resolved.cursor);
        theme.is_dark = dark;
        for (slot, color) in theme.ansi.iter_mut().zip(resolved.colors.iter()) {
            *slot = rgba_of(*color);
        }
        theme
    }

    /// 深色 chrome（darkAqua 语义色）+ Black 终端调色板。
    pub fn dark() -> Self {
        Self {
            bg: rgb(0x0b0d10),
            fg: rgb(0xe6e8eb),
            cursor: rgb(0xffffff),
            sidebar_bg: rgb(0x1e1e1e),
            chrome_bg: rgb(0x262626),
            active_bg: rgb(0x353535),
            border: rgb(0x3a3a3c),
            muted: rgb(0x98989d),
            accent: rgb(0x0a84ff),
            success: rgb(0x32d74b),
            danger: rgb(0xff453a),
            warning: rgb(0xff9f0a),
            working: rgb(0xffd60a),
            blocked: rgb(0xff375f),
            done: rgb(0x40c8e0),
            purple: rgb(0xbf5af2),
            font_family: SharedString::from(DEFAULT_FONT_FAMILY),
            font_fallback: Vec::new(),
            font_size: DEFAULT_FONT_SIZE,
            line_spacing: DEFAULT_FONT_SPACING,
            column_spacing: DEFAULT_FONT_SPACING,
            is_dark: true,
            // `configs/themes/black.toml` 的 16 色。
            ansi: [
                rgb(0x161a1f),
                rgb(0xff6b6b),
                rgb(0x8bd49c),
                rgb(0xf2c879),
                rgb(0x78a9ff),
                rgb(0xd6a8ff),
                rgb(0x70d6d0),
                rgb(0xd9dde3),
                rgb(0x5b6572),
                rgb(0xff8585),
                rgb(0xa5e7b2),
                rgb(0xf7d99a),
                rgb(0x9cc1ff),
                rgb(0xe4c7ff),
                rgb(0x9ae8e2),
                rgb(0xffffff),
            ],
        }
    }

    /// 前景色。`Color::Default` 跟随主题。
    pub fn foreground(&self, color: Color) -> Rgba {
        self.resolve(color, self.fg)
    }

    /// 前景色，并把 SGR 粗体的提亮算进去。
    ///
    /// 对照 SwiftTerm 1.15 `AppleTerminalView.mapColor(isBold:)`：
    /// `midx = ansi < 7 ? ansi + 8 : ansi`。7 是源码里的例外（保持原色），
    /// 8–15 本来就是高亮色，不再上移。Truecolor / 默认色不受影响。
    pub fn foreground_with_bold(&self, color: Color, bold: bool) -> Rgba {
        if bold {
            if let Color::Indexed(index) = color {
                if index < 7 {
                    return self.indexed(index + 8);
                }
            }
        }
        self.foreground(color)
    }

    /// SGR 2（dim/faint）：前景往背景方向混合 50%。
    ///
    /// 对照 SwiftTerm 1.15 `NSColor.dimmedColor(towards:)`：逐通道
    /// `(fg + bg) * 0.5`，alpha 保持前景。
    pub fn dimmed(foreground: Rgba, background: Rgba) -> Rgba {
        Rgba {
            r: (foreground.r + background.r) * 0.5,
            g: (foreground.g + background.g) * 0.5,
            b: (foreground.b + background.b) * 0.5,
            a: foreground.a,
        }
    }

    /// 背景色。`Color::Default` 跟随主题。
    pub fn background(&self, color: Color) -> Rgba {
        self.resolve(color, self.bg)
    }

    /// 反显（`SGR 7`）：前景背景互换。
    pub fn inverse(&self, color: Color) -> (Rgba, Rgba) {
        match color {
            Color::Default => (self.bg, self.fg),
            other => (self.background(other), self.foreground(other)),
        }
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

    /// 单元格像素尺寸的**估计值**。
    ///
    /// 真实值由渲染期从 `TextSystem` 量出来（见 `GpuiApp::measure_cell`）；
    /// 这里只给无头测试和首帧之前用，按 Menlo 的 0.6em / 1.16em 估算。
    pub fn cell_width(&self) -> f32 {
        (self.font_size * 0.6 * self.column_spacing)
            .round()
            .max(1.0)
    }

    pub fn cell_height(&self) -> f32 {
        (self.font_size * 1.16 * self.line_spacing).round().max(1.0)
    }

    /// Cmd +/- 缩放后的主题（字号夹在合法区间内）。
    pub fn zoomed(&self, direction: i32) -> Self {
        let mut next = self.clone();
        next.font_size = clamp_font_size(self.font_size + direction as f32);
        next
    }

    /// chrome 主文字色（`NSColor.labelColor`）。
    pub fn label(&self) -> Rgba {
        if self.is_dark {
            rgb(0xffffff)
        } else {
            rgb(0x000000)
        }
    }
}

/// `[font] size` 的夹取范围与 macOS `MuxtermTerminalFont.clamp` 一致。
pub fn clamp_font_size(size: f32) -> f32 {
    if !size.is_finite() {
        return DEFAULT_FONT_SIZE;
    }
    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

/// `[font] spacing` 的夹取范围与 macOS 一致。
pub fn clamp_font_spacing(value: f32) -> f32 {
    if !value.is_finite() {
        return DEFAULT_FONT_SPACING;
    }
    value.clamp(MIN_FONT_SPACING, MAX_FONT_SPACING)
}

/// 终端字体的回退栈：用户配置先、平台栈后。
///
/// 为什么要显式给：Linux / Windows 上没有 Menlo，配置里写了一个本机不存在的
/// 族名时，gpui 会落到比例字体，格子宽度立刻漂。内置的 JetBrains Mono
/// （`assets/fonts/`）与 Noto Symbols 在启动时注册，这里保证它们进回退栈。
pub fn font_fallbacks(theme: &Theme) -> gpui::FontFallbacks {
    let mut families: Vec<String> = theme
        .font_fallback
        .iter()
        .map(|family| family.to_string())
        .collect();
    for family in platform::os().monospace_fallbacks() {
        if !families.iter().any(|known| known == family) {
            families.push((*family).to_string());
        }
    }
    gpui::FontFallbacks::from_fonts(families)
}

/// 终端正文用的 `Font`（带回退栈）。测量格子与绘制字形必须用同一份。
pub fn terminal_font(theme: &Theme) -> gpui::Font {
    let mut font = gpui::font(theme.font_family.clone());
    font.fallbacks = Some(font_fallbacks(theme));
    font
}

fn rgba_of(color: crate::frontend::utils::corebridge::ClientRgb) -> Rgba {
    rgb(color.to_u32())
}

/// 相对亮度（sRGB → 线性），只用来判断明暗。
fn luminance(color: crate::frontend::utils::corebridge::ClientRgb) -> f32 {
    let channel = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.0) + 0.7152 * channel(color.1) + 0.0722 * channel(color.2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::utils::corebridge::ClientRgb;

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

    /// 对照 SwiftTerm 1.15 `AppleTerminalView.mapColor(isBold:)`：
    /// 粗体把 ANSI 0–6 往高亮 8–14 推，7 和 8–15 维持原色。
    #[test]
    fn bold_brightens_only_the_first_seven_ansi_colors() {
        let theme = Theme::default();
        for index in 0u8..7 {
            assert_eq!(
                theme.foreground_with_bold(Color::Indexed(index), true),
                theme.indexed(index + 8),
                "ANSI {index} 粗体要提亮"
            );
            assert_eq!(
                theme.foreground_with_bold(Color::Indexed(index), false),
                theme.indexed(index),
                "不加粗保持原色"
            );
        }
        assert_eq!(
            theme.foreground_with_bold(Color::Indexed(7), true),
            theme.indexed(7),
            "7 是 SwiftTerm 源码里的例外"
        );
        assert_eq!(
            theme.foreground_with_bold(Color::Indexed(11), true),
            theme.indexed(11),
            "8–15 已经是高亮色"
        );
        assert_eq!(
            theme.foreground_with_bold(Color::Rgb(1, 2, 3), true),
            rgb(0x010203),
            "Truecolor 不参与提亮"
        );
        assert_eq!(
            theme.foreground_with_bold(Color::Default, true),
            theme.fg,
            "默认前景色不参与提亮"
        );
    }

    /// 对照 SwiftTerm 1.15 `NSColor.dimmedColor(towards:)`：与背景对半混合。
    #[test]
    fn dim_blends_halfway_towards_the_background() {
        let fg = rgb(0x000000);
        let bg = rgb(0xffffff);
        let dimmed = Theme::dimmed(fg, bg);
        assert!((dimmed.r - 0.5).abs() < 1e-6, "{dimmed:?}");
        assert!((dimmed.g - 0.5).abs() < 1e-6, "{dimmed:?}");
        assert!((dimmed.b - 0.5).abs() < 1e-6, "{dimmed:?}");
        assert!((dimmed.a - 1.0).abs() < 1e-6, "{dimmed:?}");

        let colored = Theme::dimmed(rgb(0x204060), rgb(0x000000));
        assert!(
            (colored.r - 0x20 as f32 / 255.0 / 2.0).abs() < 1e-6,
            "{colored:?}"
        );
    }

    #[test]
    fn truecolor_is_passed_through() {
        let theme = Theme::default();
        assert_eq!(theme.foreground(Color::Rgb(1, 2, 3)), rgb(0x010203));
    }

    #[test]
    fn inverse_swaps_default_colors() {
        let theme = Theme::default();
        assert_eq!(theme.inverse(Color::Default), (theme.bg, theme.fg));
        assert_eq!(
            theme.inverse(Color::Rgb(1, 2, 3)),
            (rgb(0x010203), rgb(0x010203))
        );
    }

    #[test]
    fn default_theme_is_light_with_menlo_18() {
        // 对照 macOS `MuxtermTerminalColors.activePalette = .light` 与
        // `MuxtermTerminalFont.defaultFamily/defaultSize`。
        let theme = Theme::default();
        assert!(!theme.is_dark);
        assert_eq!(theme.font_family.as_ref(), "Menlo");
        assert_eq!(theme.font_size, 18.0);
        assert_eq!(theme.bg, rgb(0xffffff));
    }

    #[test]
    fn white_palette_resolves_to_light_chrome() {
        // 对照 `configs/themes/white.toml`。
        let resolved = ClientTheme {
            name: "white".into(),
            background: ClientRgb(0xff, 0xff, 0xff),
            foreground: ClientRgb(0x1f, 0x23, 0x28),
            cursor: ClientRgb(0x1f, 0x23, 0x28),
            colors: [ClientRgb(0, 0, 0); 16],
        };
        let theme = Theme::from_palette(&resolved);
        assert!(!theme.is_dark);
        assert_eq!(theme.bg, rgb(0xffffff));
        assert_eq!(theme.fg, rgb(0x1f2328));
    }

    #[test]
    fn black_palette_resolves_to_dark_chrome() {
        // 对照 `configs/themes/black.toml`。
        let resolved = ClientTheme {
            name: "black".into(),
            background: ClientRgb(0x0b, 0x0d, 0x10),
            foreground: ClientRgb(0xe6, 0xe8, 0xeb),
            cursor: ClientRgb(0xff, 0xff, 0xff),
            colors: [ClientRgb(0, 0, 0); 16],
        };
        let theme = Theme::from_palette(&resolved);
        assert!(theme.is_dark);
        assert_eq!(theme.bg, rgb(0x0b0d10));
        assert_eq!(theme.label(), rgb(0xffffff));
    }

    #[test]
    fn font_config_clamps_size_and_keeps_family() {
        // 对照 `MuxtermTerminalFont.settings(from:)`：非法字号夹回区间。
        let font = ClientFontConfig {
            family: "monaco".into(),
            size: 400.0,
            fallback: Vec::new(),
        };
        let theme = Theme::from_config(None, &font);
        assert_eq!(theme.font_family.as_ref(), "monaco");
        assert_eq!(theme.font_size, MAX_FONT_SIZE);
        assert!(!theme.is_dark);
    }

    #[test]
    fn empty_family_falls_back_to_menlo() {
        let font = ClientFontConfig {
            family: "  ".into(),
            size: 18.0,
            fallback: Vec::new(),
        };
        let theme = Theme::from_config(None, &font);
        assert_eq!(theme.font_family.as_ref(), "Menlo");
    }

    #[test]
    fn zoom_clamps_to_range() {
        let theme = Theme::default();
        let mut zoomed = theme.clone();
        for _ in 0..40 {
            zoomed = zoomed.zoomed(1);
        }
        assert_eq!(zoomed.font_size, MAX_FONT_SIZE);
        for _ in 0..40 {
            zoomed = zoomed.zoomed(-1);
        }
        assert_eq!(zoomed.font_size, MIN_FONT_SIZE);
    }
}
