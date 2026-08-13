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

    /// 应用主题色到 VTE。
    pub fn apply_theme(&self, theme: &Theme) {
        let fg = rgba(theme.foreground);
        let bg = rgba(theme.background);
        let cursor = rgba(theme.cursor);
        self.terminal.set_colors(Some(&fg), Some(&bg), &[]);
        let palette: Vec<gtk4::gdk::RGBA> = theme.colors.iter().map(|c| rgba(*c)).collect();
        let refs: Vec<&gtk4::gdk::RGBA> = palette.iter().collect();
        self.terminal.set_colors(Some(&fg), Some(&bg), &refs);
        let _ = cursor;
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
