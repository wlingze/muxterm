//! GTK Application 启动与主入口。
//!
//! 负责加载配置、主题，构造主窗口，连本地 tmux，进入 GTK 主循环。

use std::sync::Arc;

use gtk4::prelude::*;
use gtk4::Application;

use crate::config::Config;
use crate::tmux::client::{ConnectMode, TmuxClientConfig};
use crate::ui::theme::CellStyle;
use crate::ui::window::AppWindow;

/// 应用 ID（GTK Application 的唯一标识，用于实例单例/桌面集成）。
pub const APP_ID: &str = "io.muxterm.Muxterm";

/// 启动 GTK 应用。阻塞直到窗口关闭。
pub fn run() -> anyhow::Result<()> {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |a| {
        // 加载配置与主题
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

        let win = AppWindow::new(
            theme,
            &cfg.terminal.font_family,
            cfg.terminal.font_size,
            cfg.terminal.scrollback_lines,
        );
        a.add_window(&win.window);

        // 构造 tmux 连接配置
        let mode = if cfg.tmux.session_name.is_empty() {
            ConnectMode::NewSession { name: None }
        } else {
            ConnectMode::NewSession {
                name: Some(cfg.tmux.session_name.clone()),
            }
        };
        let tmux_cfg = TmuxClientConfig {
            mode: Some(mode),
            cols: Some(100),
            rows: Some(30),
            ..Default::default()
        };

        win.connect(tmux_cfg);
        win.window.show();
    });

    // run() 阻塞；退出码非零视为错误
    let exit = app.run_with_args::<&str>(&[]);
    let code: i32 = exit.into();
    if code != 0 {
        anyhow::bail!("GTK 应用退出码非零: {code}");
    }
    Ok(())
}

/// 极端兜底主题（连内置 dark.toml 都读不到时用）。
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
