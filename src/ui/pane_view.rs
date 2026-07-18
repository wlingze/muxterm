//! 单个 pane 的渲染视图（vte4 Terminal）。
//!
//! 两种模式：
//! - [`PaneMode::Local`]：vte4 自己 spawn 子进程（本地 shell），用户键盘输入
//!   直接进 vte4（`input_enabled=true`）。类似 GNOME Terminal / iTerm2 启动即
//!   有 shell。
//! - [`PaneMode::Tmux`]：tmux `-CC` 的 `%output` 内容通过 `feed_output()` 喂给
//!   vte4 渲染；键盘输入走底部输入栏的 `send-keys`（`input_enabled=false`）。
//!
//! 两种模式共用 vte4 的 ANSI 颜色/样式/24-bit 真彩色/中文/emoji/自动滚动/scrollback。

use crate::config::Theme;
use gtk4::glib;
use gtk4::pango;
use vte4::prelude::*;
use vte4::{PtyFlags, Terminal};

/// pane 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMode {
    /// 本地 shell（vte4 自 spawn 子进程）。
    Local,
    /// tmux attach 的 pane（feed output，输入走 send-keys）。
    Tmux,
}

/// 一个 pane 的视图。
pub struct PaneView {
    pub terminal: Terminal,
    pub mode: PaneMode,
    /// 本地模式：无 pane id；tmux 模式：tmux 的 `@N`。
    pub pane_id: Option<crate::tmux::protocol::PaneId>,
    /// 本地模式下记录的子进程 PID（用于关闭时 kill）。
    pub child_pid: Option<glib::Pid>,
}

impl PaneView {
    /// 新建「本地 shell」pane：vte4 自 spawn 默认 shell。
    pub fn new_local(
        theme: &Theme,
        font_family: &str,
        font_size: u32,
        scrollback_lines: u32,
    ) -> Self {
        let terminal = build_terminal(theme, font_family, font_size, scrollback_lines, true);
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let argv = [shell.as_str()];
        let envv: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        let env_refs: Vec<&str> = envv.iter().map(|s| s.as_str()).collect();

        let pid_cell = std::cell::RefCell::new(None::<glib::Pid>);
        let pid_for_cb = pid_cell.borrow().clone();
        let _ = pid_for_cb;
        terminal.spawn_async(
            PtyFlags::DEFAULT,
            None,
            &argv,
            &env_refs,
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gtk4::gio::Cancellable>,
            {
                // 用一个共享 Cell 记录 PID —— 但闭包是 'static，只能用 Box 通道。
                // 简化：PID 不强制记录（关闭 tab 时直接靠 widget 销毁释放 pty）。
                move |_res| {}
            },
        );
        // vte4 spawn_async 的回调是异步的，PID 无法在构造时同步拿到；这里先置 None，
        // 关闭 tab 靠 widget destroy 触发 pty 关闭，子进程随之退出。

        Self {
            terminal,
            mode: PaneMode::Local,
            pane_id: None,
            child_pid: None,
        }
    }

    /// 新建「tmux attach」pane：渲染 `%output`，输入由输入栏走 send-keys。
    pub fn new_tmux(
        pane_id: crate::tmux::protocol::PaneId,
        theme: &Theme,
        font_family: &str,
        font_size: u32,
        scrollback_lines: u32,
    ) -> Self {
        let terminal = build_terminal(theme, font_family, font_size, scrollback_lines, false);
        Self {
            terminal,
            mode: PaneMode::Tmux,
            pane_id: Some(pane_id),
            child_pid: None,
        }
    }

    /// 喂入 pane 输出（tmux `%output` 的已解码字节）。仅 tmux 模式有意义。
    pub fn feed_output(&self, data: &[u8]) {
        self.terminal.feed(data);
    }

    /// 是否为 tmux attach 模式。
    pub fn is_tmux(&self) -> bool {
        self.mode == PaneMode::Tmux
    }
}

/// 构造 vte4 Terminal 并应用主题/字体。
fn build_terminal(
    theme: &Theme,
    font_family: &str,
    font_size: u32,
    scrollback_lines: u32,
    input_enabled: bool,
) -> Terminal {
    let terminal = Terminal::builder()
        .scrollback_lines(scrollback_lines)
        .scroll_on_output(true)
        .scroll_on_keystroke(true)
        .enable_bidi(true)
        .enable_shaping(true)
        .allow_hyperlink(true)
        .input_enabled(input_enabled)
        .build();
    apply_theme(&terminal, theme);
    apply_font(&terminal, font_family, font_size);
    terminal
}

/// 把主题应用到 vte4 Terminal。
pub fn apply_theme(term: &Terminal, theme: &Theme) {
    let fg = rgba(theme.foreground);
    let bg = rgba(theme.background);
    let cursor = rgba(theme.cursor);
    let palette: Vec<gtk4::gdk::RGBA> = theme.colors.iter().map(|c| rgba(*c)).collect();
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

fn rgba(c: crate::config::Rgb) -> gtk4::gdk::RGBA {
    gtk4::gdk::RGBA::new(
        c.0 as f32 / 255.0,
        c.1 as f32 / 255.0,
        c.2 as f32 / 255.0,
        1.0,
    )
}
