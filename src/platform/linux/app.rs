//! GTK Application 启动（FFI 驱动的 Linux 前端）。
//!
//! 加载配置/主题 → 创建主窗口（内部 `muxterm_new` + connect）→ GTK 主循环。

use gtk4::prelude::*;
use gtk4::Application;

use crate::config::Config;

pub const APP_ID: &str = "io.muxterm.Muxterm";

/// 启动 GTK 应用。
///
/// `socket` 对应 CLI `-L/--socket`：非空时写入配置，本地/tmux 后端统一使用。
pub fn run(socket: Option<String>) -> anyhow::Result<()> {
    let app = Application::builder()
        .application_id(APP_ID)
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |a| {
        let mut cfg = Config::load().unwrap_or_else(|e| {
            tracing::warn!(target = "muxterm::app", "加载配置失败，用默认: {e}");
            Config::default()
        });
        if let Some(ref sock) = socket {
            let sock = sock.trim();
            if !sock.is_empty() {
                cfg.tmux.socket = sock.to_string();
            }
        }
        let theme = match crate::config::Theme::load(&cfg.theme.name) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    target = "muxterm::app",
                    "加载主题 {} 失败，用默认 light: {e}",
                    cfg.theme.name
                );
                crate::config::Theme::load("light").unwrap_or_else(|_| fallback_theme())
            }
        };
        let win = crate::platform::linux::window::AppWindow::new(cfg, theme);
        a.add_window(&win.window);
        win.window.present();
    });

    let argv0 = std::env::args().next().unwrap_or_else(|| "muxterm".into());
    let exit = app.run_with_args(&[argv0]);
    let code: i32 = exit.into();
    if code != 0 {
        anyhow::bail!("GTK 应用退出码非零: {code}");
    }
    Ok(())
}

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
