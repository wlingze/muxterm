//! 终端渲染抽象：默认 VTE4，预留 GPU（GtkGLArea）实现。
//!
//! 参考 alacritty 的 glyph cache / texture atlas 思路；当前 Phase 用 VTE4。

use gtk4::prelude::*;
use vte4::prelude::*;
use vte4::Terminal;

use crate::core::config::{Rgb, Theme};

/// 终端渲染 trait（平台无关接口，便于日后换 GPU 实现）。
pub trait TerminalRenderer {
    /// 创建渲染器。
    fn new() -> Self
    where
        Self: Sized;

    /// 渲染一帧：追加/刷新终端输出。`cursor_pos` 供 GPU 实现使用；VTE 自管光标。
    fn render(&mut self, output: &[u8], cursor_pos: (u16, u16));

    /// 字符格 resize。
    fn resize(&mut self, cols: u16, rows: u16);

    /// 取出可嵌入 GTK 布局的 widget。
    fn widget(&self) -> gtk4::Widget;
}

/// 默认渲染器：VTE4 widget（Pango + cairo）。
pub struct VteRenderer {
    terminal: Terminal,
}

impl TerminalRenderer for VteRenderer {
    fn new() -> Self {
        let terminal = Terminal::new();
        terminal.set_hexpand(true);
        terminal.set_vexpand(true);
        terminal.set_enable_fallback_scrolling(true);
        Self { terminal }
    }

    fn render(&mut self, output: &[u8], _cursor_pos: (u16, u16)) {
        if output.is_empty() {
            return;
        }
        // feed：把核心输出写进 VTE 显示缓冲（不走本地 pty）
        self.terminal.feed(output);
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1) as i64;
        let rows = rows.max(1) as i64;
        self.terminal.set_size(cols, rows);
    }

    fn widget(&self) -> gtk4::Widget {
        self.terminal.clone().upcast()
    }
}

impl VteRenderer {
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    /// tmux/SSH 镜像：VTE 没有 PTY。htop/codex 用 CUP 画全屏，必须关掉
    /// rewrap 和 scroll-on-output，否则表头被卷走、CPU 条折行叠在一起。
    /// scrollback 由用户 prefs 决定（F5：不再强制 0）。
    pub fn apply_mirror_policy(&self, is_tmux_mirror: bool) {
        if !is_tmux_mirror {
            return;
        }
        self.terminal.set_enable_fallback_scrolling(false);
        self.terminal.set_scroll_on_output(false);
        self.terminal.set_scroll_on_insert(false);
        self.terminal.set_enable_bidi(false);
        // vte4 0.8 未导出 set_rewrap_on_resize；属性仍在（VTE 3.91）。
        self.terminal.set_property("rewrap-on-resize", false);
    }

    /// 应用主题色到 VTE。显式设 default fg/bg/cursor/highlight：
    /// Codex 浅色输入框用 SGR 39（默认前景）画字，若只靠 GTK CSS，Linux 上
    /// 字色会跟 48;2;216;216;216 背景糊在一起，选中也看不见。
    pub fn apply_theme(&self, theme: &Theme) {
        let fg = rgba(theme.foreground);
        let bg = rgba(theme.background);
        let cursor = rgba(theme.cursor);
        let palette: Vec<gtk4::gdk::RGBA> = theme.colors.iter().map(|c| rgba(*c)).collect();
        let refs: Vec<&gtk4::gdk::RGBA> = palette.iter().collect();
        self.terminal.set_color_foreground(&fg);
        self.terminal.set_color_background(&bg);
        self.terminal.set_color_bold(Some(&fg));
        self.terminal.set_color_cursor(Some(&cursor));
        self.terminal.set_color_cursor_foreground(Some(&bg));
        self.terminal.set_color_highlight(Some(&cursor));
        self.terminal.set_color_highlight_foreground(Some(&bg));
        self.terminal.set_colors(Some(&fg), Some(&bg), &refs);
    }

    /// 应用字体（family + size，size 以 pt 为单位）。
    pub fn apply_font(&self, font: &crate::platform::linux::quickconnect::font::FontSettings) {
        use gtk4::pango;
        let mut desc = pango::FontDescription::new();
        if !font.family.is_empty() {
            desc.set_family(&font.family);
        }
        desc.set_size((font.size * pango::SCALE as f32) as i32);
        self.terminal.set_font_desc(Some(&desc));
    }
}

fn rgba(c: Rgb) -> gtk4::gdk::RGBA {
    gtk4::gdk::RGBA::new(
        c.0 as f32 / 255.0,
        c.1 as f32 / 255.0,
        c.2 as f32 / 255.0,
        1.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_converts_byte_colors_to_unit_range() {
        let c = rgba(Rgb(0xaa, 0xbb, 0xcc));
        assert!((c.red() - 170.0 / 255.0).abs() < 1e-6, "{}", c.red());
        assert!((c.green() - 187.0 / 255.0).abs() < 1e-6, "{}", c.green());
        assert!((c.blue() - 204.0 / 255.0).abs() < 1e-6, "{}", c.blue());
        assert!((c.alpha() - 1.0).abs() < 1e-6, "{}", c.alpha());
    }
}

// ── GPU 加速预留（TODO）──────────────────────────────────────
//
// /// 参考 alacritty OpenGL renderer：
// /// GtkGLArea + glyph atlas + texture atlas + damage tracking
// ///
// /// struct GlRenderer {
// ///     gl_area: gtk4::GLArea,
// ///     glyph_cache: GlyphCache,
// ///     atlas: TextureAtlas,
// /// }
// ///
// /// impl TerminalRenderer for GlRenderer { ... }
