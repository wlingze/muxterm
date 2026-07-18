//! GTK Application 启动与主入口。
//!
//! 加载配置/主题，构造主窗口，进入 GTK 主循环。启动即一个本地 shell tab，
//! tmux 是可选的 attach 功能（点工具栏「tmux」按钮）。

use gtk4::prelude::*;
use gtk4::Application;

use crate::config::Config;

/// 应用 ID。
pub const APP_ID: &str = "io.muxterm.Muxterm";

/// 启动 GTK 应用。阻塞直到窗口关闭。
pub fn run() -> anyhow::Result<()> {
    // NON_UNIQUE 允许同时跑多个实例（开发期方便）
    let app = Application::builder()
        .application_id(APP_ID)
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |a| {
        let cfg = Config::load().unwrap_or_else(|e| {
            tracing::warn!(target = "muxterm::app", "加载配置失败，用默认: {e}");
            Config::default()
        });
        let theme = match crate::config::Theme::load(&cfg.terminal.theme) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    target = "muxterm::app",
                    "加载主题 {} 失败，用默认 dark: {e}",
                    cfg.terminal.theme
                );
                crate::config::Theme::load("dark").unwrap_or_else(|_| fallback_theme())
            }
        };

        let win = crate::ui::window::AppWindow::new(
            theme,
            &cfg.terminal.font_family,
            cfg.terminal.font_size,
            cfg.terminal.scrollback_lines,
        );
        a.add_window(&win.window);
        win.window.show();
    });

    // run() 阻塞直到窗口关闭。GApplication 需要 argv[0]（程序名）才能 emit
    // activate 信号——空 argv 会导致不触发 activate（窗口不出现）。
    // 但不能把原始 argv 全传给 GApplication：它不认识 --verbose 等业务参数会报
    // "Unknown option" 退出。--verbose 已被 clap 在 main() 解析，这里只传
    // argv[0]（程序名），让 GApplication 走默认流程触发 activate。
    let argv0 = std::env::args().next().unwrap_or_else(|| "muxterm".into());
    let exit = app.run_with_args(&[argv0]);
    let code: i32 = exit.into();
    if code != 0 {
        anyhow::bail!("GTK 应用退出码非零: {code}");
    }
    Ok(())
}

/// 极端兜底主题。
fn fallback_theme() -> crate::config::Theme {
    use crate::config::Rgb;
    let colors = [
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
    crate::config::Theme {
        name: "fallback".into(),
        background: Rgb(0x1e, 0x1e, 0x2e),
        foreground: Rgb(0xcd, 0xd6, 0xf4),
        cursor: Rgb(0xf5, 0xe0, 0xdc),
        colors,
    }
}
