//! 单个 pane 的渲染视图（vte4 Terminal）。
//!
//! 两种模式：
//! - [`PaneMode::Local`]：vte4 `spawn_async` 跑一个程序（默认 shell，可配置），
//!   键盘输入直接进 vte4（`input_enabled=true`）。子进程退出时 emit
//!   `child-exited`，上层据此关闭对应 pane（不是再开空 shell）。
//! - [`PaneMode::Tmux`]：tmux `-CC` 的 `%output` 内容通过 `feed_output()` 喂给
//!   vte4 渲染；`input_enabled=true`，由上层连接 `commit` 信号转发 `send-keys`
//!   （不再使用底部输入栏）。

use std::cell::Cell;
use std::path::{Path, PathBuf};

use crate::config::{program_basename, Rgb, Theme};
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

/// 本地 pane 的 spawn 参数。
#[derive(Debug, Clone)]
pub struct SpawnOpts {
    /// argv（至少一个元素）。
    pub argv: Vec<String>,
    /// 工作目录；None 则用进程 cwd。
    pub workdir: Option<PathBuf>,
}

impl SpawnOpts {
    pub fn program_name(&self) -> String {
        self.argv
            .first()
            .map(|a| program_basename(a))
            .unwrap_or_else(|| "shell".into())
    }
}

/// 一个 pane 的视图。
pub struct PaneView {
    pub terminal: Terminal,
    pub mode: PaneMode,
    pub pane_id: Option<crate::tmux::protocol::PaneId>,
    /// 默认名：本地为 argv[0] basename；tmux 为 pane id 字符串。
    pub program_name: String,
    /// 用户重命名（优先于 program_name）。
    pub custom_name: Option<String>,
    /// 本地子进程 pid（用于 close pane 发信号）。
    pub child_pid: Cell<Option<i32>>,
}

impl PaneView {
    /// 当前显示名（自定义名优先）。
    pub fn display_name(&self) -> String {
        self.custom_name
            .clone()
            .unwrap_or_else(|| self.program_name.clone())
    }

    /// 本地程序 pane：按 `opts` spawn。
    pub fn new_local(
        theme: &Theme,
        font_family: &str,
        font_size: f32,
        scrollback: u32,
        opts: &SpawnOpts,
    ) -> Self {
        let terminal = build_terminal(theme, font_family, font_size, scrollback, true);
        let program_name = opts.program_name();
        let argv_owned = opts.argv.clone();
        let argv_refs: Vec<&str> = argv_owned.iter().map(|s| s.as_str()).collect();
        let envv: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
        let env_refs: Vec<&str> = envv.iter().map(|s| s.as_str()).collect();
        let workdir = opts
            .workdir
            .as_ref()
            .and_then(|p| p.to_str().map(|s| s.to_string()));
        let workdir_ref = workdir.as_deref();
        let child_pid = Cell::new(None);
        let pid_slot = child_pid.clone();
        terminal.spawn_async(
            PtyFlags::DEFAULT,
            workdir_ref,
            &argv_refs,
            &env_refs,
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gtk4::gio::Cancellable>,
            move |res| {
                if let Ok(pid) = res {
                    pid_slot.set(Some(pid.0 as i32));
                }
            },
        );
        Self {
            terminal,
            mode: PaneMode::Local,
            pane_id: None,
            program_name,
            custom_name: None,
            child_pid,
        }
    }

    /// tmux attach 的 pane：仅 feed `%output`；键盘由上层 EventController → send-keys。
    ///
    /// `input_enabled=false`：避免 VTE 本地回显导致与 tmux echo 双重显示。
    pub fn new_tmux(
        pane_id: crate::tmux::protocol::PaneId,
        theme: &Theme,
        font_family: &str,
        font_size: f32,
        scrollback: u32,
    ) -> Self {
        let terminal = build_terminal(theme, font_family, font_size, scrollback, false);
        terminal.set_can_focus(true);
        terminal.set_focusable(true);
        let program_name = pane_id.as_str();
        Self {
            terminal,
            mode: PaneMode::Tmux,
            pane_id: Some(pane_id),
            program_name,
            custom_name: None,
            child_pid: Cell::new(None),
        }
    }

    /// 连接 commit：把用户输入交给回调（tmux 侧用于 send-keys）。
    pub fn connect_commit<F: Fn(&str) + 'static>(&self, f: F) {
        self.terminal.connect_commit(move |_t, text, _len| {
            f(text);
        });
    }

    /// 终止本地子进程（若有 pid）。
    pub fn kill_child(&self) -> bool {
        let Some(pid) = self.child_pid.get() else {
            return false;
        };
        #[cfg(unix)]
        {
            let r = unsafe { libc::kill(pid, libc::SIGTERM) };
            r == 0
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }

    pub fn feed_output(&self, data: &[u8]) {
        self.terminal.feed(data);
    }

    pub fn is_tmux(&self) -> bool {
        self.mode == PaneMode::Tmux
    }

    /// 注册 child-exited 回调（仅本地程序有意义）。
    pub fn connect_child_exited<F: Fn(&Terminal, i32) + 'static>(&self, f: F) {
        self.terminal.connect_child_exited(f);
    }

    /// 尝试读取该 terminal 的当前工作目录（OSC 7 / VTE）。
    pub fn current_workdir(&self) -> Option<PathBuf> {
        terminal_workdir(&self.terminal)
    }
}

/// 从 vte Terminal 读当前目录 URI。
pub fn terminal_workdir(term: &Terminal) -> Option<PathBuf> {
    let uri = term.current_directory_uri()?;
    let s = uri.as_str();
    let path = s.strip_prefix("file://").unwrap_or(s);
    let p = Path::new(path);
    if p.is_absolute() {
        Some(p.to_path_buf())
    } else {
        None
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
