//! 单个 tmux pane 的渲染视图。
//!
//! 用 vte4 `Terminal` 作为输出渲染器（它本身就是一个完整的终端模拟器，自带
//! ANSI 颜色/样式/光标/滚动/Unicode/emoji 支持），但不让它 spawn 自己的子进程
//! ——tmux 已经替我们管理了底层 shell。我们把 `tmux -CC` 的 `%output` 内容
//! 通过 `Terminal::feed()` 喂进去渲染即可。
//!
//! 这样做的优点：
//! - ANSI 颜色/样式/24-bit 真彩色/中文/emoji 全部由 vte4 处理，零额外代码。
//! - 自动滚动到底 + 用户手动上滚不强制拉回（vte4 自带行为）。
//! - scrollback 由 vte4 管理，按配置设置 `scrollback-lines`。
//!
//! 输入：tmux → UI 的输出字节流；UI → tmux 的按键由 `input_bar` 走命令通道。

use crate::config::Theme;
use gtk4::prelude::*;
use gtk4::{gdk, pango};
use vte4::prelude::*;
use vte4::Terminal;

/// 一个 pane 的视图：vte4 Terminal + 它的 pane id。
pub struct PaneView {
    pub terminal: Terminal,
    pub pane_id: crate::tmux::protocol::PaneId,
}

impl PaneView {
    /// 新建一个 pane 视图。
    pub fn new(
        pane_id: crate::tmux::protocol::PaneId,
        theme: &Theme,
        font_family: &str,
        font_size: u32,
        scrollback_lines: u32,
    ) -> Self {
        let terminal = Terminal::builder()
            .scrollback_lines(scrollback_lines)
            .scroll_on_output(true)
            .scroll_on_keystroke(true)
            .enable_bidi(true)
            .enable_shaping(true)
            .allow_hyperlink(true)
            // 我们不通过 vte4 的 pty spawn 子进程，输入由 input_bar 走 tmux 命令。
            // input_enabled=false 阻止 vte4 自己消费键盘事件把字符送进不存在的 pty。
            .input_enabled(false)
            .build();

        apply_theme(&terminal, theme);
        apply_font(&terminal, font_family, font_size);

        Self { terminal, pane_id }
    }

    /// 喂入 pane 输出（tmux `%output` 的已解码字节）。
    pub fn feed_output(&self, data: &[u8]) {
        self.terminal.feed(data);
    }

    /// 重置终端（清屏 + 复位状态），用于 pane 重连场景。
    pub fn reset(&self) {
        // ESC c = RIS（复位到初始状态）
        self.terminal.feed(b"\x1bc");
    }
}

/// 把主题应用到 vte4 Terminal。
pub fn apply_theme(term: &Terminal, theme: &Theme) {
    let fg = gtk4::gdk::RGBA::new(
        theme.foreground.0 as f32 / 255.0,
        theme.foreground.1 as f32 / 255.0,
        theme.foreground.2 as f32 / 255.0,
        1.0,
    );
    let bg = gtk4::gdk::RGBA::new(
        theme.background.0 as f32 / 255.0,
        theme.background.1 as f32 / 255.0,
        theme.background.2 as f32 / 255.0,
        1.0,
    );
    let cursor = gtk4::gdk::RGBA::new(
        theme.cursor.0 as f32 / 255.0,
        theme.cursor.1 as f32 / 255.0,
        theme.cursor.2 as f32 / 255.0,
        1.0,
    );
    // ANSI 16 色调色板
    let palette: Vec<gtk4::gdk::RGBA> = theme
        .colors
        .iter()
        .map(|c| {
            gtk4::gdk::RGBA::new(
                c.0 as f32 / 255.0,
                c.1 as f32 / 255.0,
                c.2 as f32 / 255.0,
                1.0,
            )
        })
        .collect();
    let palette_refs: Vec<&gtk4::gdk::RGBA> = palette.iter().collect();
    term.set_colors(Some(&fg), Some(&bg), &palette_refs);
    term.set_color_cursor(Some(&cursor));
}

/// 把字体应用到 vte4 Terminal。
pub fn apply_font(term: &Terminal, family: &str, size: u32) {
    let mut desc = pango::FontDescription::new();
    desc.set_family(family);
    desc.set_size((size as i32) * pango::SCALE);
    term.set_font_desc(Some(&desc));
}
