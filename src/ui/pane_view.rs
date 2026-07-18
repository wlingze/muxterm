//! 单个 pane 的渲染视图（vte4 Terminal）。
//!
//! 两种模式：
//! - [`PaneMode::Local`]：vte4 自己 spawn 子进程（本地 shell），键盘输入直接进
//!   vte4（`input_enabled=true`）。子进程退出时 emit `child-exited`，上层据此
//!   关闭对应 tab。
//! - [`PaneMode::Tmux`]：tmux `-CC` 的 `%output` 内容通过 `feed_output()` 喂给
//!   vte4 渲染；键盘输入走底部输入栏的 `send-keys`（`input_enabled=false`）。

use crate::config::{Rgb, Theme};
use gtk4::glib;
use gtk4::pango;
use vte4::prelude::*;
use vte4::{PtyFlags, Terminal};

/// pane 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMode {
    Local,
    Tmux,
}

/// 一个 pane 的视图。
pub struct PaneView {
    pub terminal: Terminal,
    pub mode: PaneMode,
    pub pane_id: Option<crate::tmux::protocol::PaneId>,
}

impl PaneView {
    /// 本地 shell pane：vte4 自 spawn 默认 shell。
    pub fn new_local(theme: &Theme, font_family: &str, font_size: f32, scrollback: u32) -> Self {
        let terminal = build_terminal(theme, font_family, font_size, scrollback, true);
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let argv = [shell.as_str()];
        let envv: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        let env_refs: Vec<&str> = envv.iter().map(|s| s.as_str()).collect();
        terminal.spawn_async(
            PtyFlags::DEFAULT,
            None,
            &argv,
            &env_refs,
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gtk4::gio::Cancellable>,
            move |_res| {},
        );
        Self {
            terminal,
            mode: PaneMode::Local,
            pane_id: None,
        }
    }

    /// tmux attach 的 pane：feed 输出，输入走 send-keys。
    pub fn new_tmux(
        pane_id: crate::tmux::protocol::PaneId,
        theme: &Theme,
        font_family: &str,
        font_size: f32,
        scrollback: u32,
    ) -> Self {
        let terminal = build_terminal(theme, font_family, font_size, scrollback, false);
        Self {
            terminal,
            mode: PaneMode::Tmux,
            pane_id: Some(pane_id),
        }
    }

    pub fn feed_output(&self, data: &[u8]) {
        self.terminal.feed(data);
    }

    pub fn is_tmux(&self) -> bool {
        self.mode == PaneMode::Tmux
    }

    /// 注册 child-exited 回调（仅本地 shell 有意义）。返回信号 id。
    pub fn connect_child_exited<F: Fn(&Terminal, i32) + 'static>(&self, f: F) {
        self.terminal.connect_child_exited(f);
    }
}

fn build_terminal(
    theme: &Theme,
    font_family: &str,
    font_size: f32,
    scrollback: u32,
    input_enabled: bool,
) -> Terminal {
    let terminal = Terminal::builder()
        .scrollback_lines(scrollback)
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

pub fn apply_theme(term: &Terminal, theme: &Theme) {
    let fg = rgba(theme.foreground);
    let bg = rgba(theme.background);
    let cursor = rgba(theme.cursor);
    let palette: Vec<gtk4::gdk::RGBA> = theme.colors.iter().map(|c| rgba(*c)).collect();
    let palette_refs: Vec<&gtk4::gdk::RGBA> = palette.iter().collect();
    term.set_colors(Some(&fg), Some(&bg), &palette_refs);
    term.set_color_cursor(Some(&cursor));
}

pub fn apply_font(term: &Terminal, family: &str, size: f32) {
    let mut desc = pango::FontDescription::new();
    desc.set_family(family);
    // pango 用 1/1024 点
    desc.set_size((size * pango::SCALE as f32) as i32);
    term.set_font_desc(Some(&desc));
}

fn rgba(c: Rgb) -> gtk4::gdk::RGBA {
    gtk4::gdk::RGBA::new(
        c.0 as f32 / 255.0,
        c.1 as f32 / 255.0,
        c.2 as f32 / 255.0,
        1.0,
    )
}
