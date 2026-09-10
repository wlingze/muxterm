//! TUI 主题：ratatui 颜色 / 样式常量。
//!
//! 集中定义配色，避免在渲染代码里散落 magic number。所有颜色用 16 色 + 基础
//! 修饰，确保在常见终端（truecolor/256/16）上都有可用回退。

use ratatui::style::{Color, Modifier, Style};

/// 主题结构：集中所有用到的 Style。
#[derive(Debug, Clone)]
pub struct Theme {
    /// 全局背景色。
    pub bg: Color,
    /// 前景文本色。
    pub fg: Color,
    /// 强调色（活动 tab / 活动 pane 边框 / 状态栏高亮）。
    pub accent: Color,
    /// 次要文本（分隔线 / pane id / 状态栏次要信息）。
    pub dim: Color,
    /// 危险色（关闭 pane/tab / 错误状态）。
    pub danger: Color,
    /// 成功色（connected / new）。
    pub success: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::Black,
            fg: Color::Gray,
            accent: Color::Cyan,
            dim: Color::DarkGray,
            danger: Color::Red,
            success: Color::Green,
        }
    }
}

impl Theme {
    /// 普通文本样式。
    pub fn text(&self) -> Style {
        Style::default().fg(self.fg)
    }

    /// 强调 / 高亮样式。
    pub fn accent_style(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// 次要文本样式。
    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.dim)
    }

    /// 活动 pane / tab 的背景高亮。
    pub fn active_bg(&self) -> Style {
        Style::default()
            .bg(Color::DarkGray)
            .fg(self.fg)
            .add_modifier(Modifier::BOLD)
    }

    /// 危险操作样式（关闭）。
    pub fn danger_style(&self) -> Style {
        Style::default().fg(self.danger)
    }

    /// 成功 / 在线样式。
    pub fn success_style(&self) -> Style {
        Style::default().fg(self.success)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_has_distinct_colors() {
        let t = Theme::default();
        assert_ne!(t.accent, t.fg);
        assert_ne!(t.dim, t.fg);
        assert_ne!(t.danger, t.success);
    }
}
