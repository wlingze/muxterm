//! GTK Application 启动（FFI 驱动的 Linux 前端）。
//!
//! 加载配置/主题 → 创建主窗口（内部 `muxterm_new` + connect）→ GTK 主循环。

use gtk4::prelude::*;
use gtk4::Application;

use crate::core::config::Theme;
use crate::core::config_service::{ConfigDocument, SettingsService};

pub const APP_ID: &str = "io.muxterm.Muxterm";

/// 启动 GTK 应用。
///
/// `socket` 对应 CLI `-L/--socket`：非空时写入配置，本地/tmux 后端统一使用。
pub fn run(socket: Option<String>) -> anyhow::Result<()> {
    let app = Application::builder()
        .application_id(APP_ID)
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    if let Err(error) = crate::platform::linux::font_registry::register_bundled_fonts() {
        tracing::warn!(
            target = "muxterm::app",
            "bundled font registration failed: {error}"
        );
    }

    app.connect_activate(move |a| {
        let default_document = ConfigDocument::default();
        let (mut cfg, shortcuts) = match SettingsService::default_user() {
            Ok(mut service) => {
                if let Err(error) = service.migrate_legacy_quickconnect() {
                    tracing::warn!(target = "muxterm::app", "QuickConnect 迁移未完成: {error}");
                }
                if let Err(error) = service.migrate_legacy_linux_preferences() {
                    tracing::warn!(
                        target = "muxterm::app",
                        "Linux preferences 迁移未完成: {error}"
                    );
                }
                let document = service.document();
                (document.config.clone(), document.shortcuts.clone())
            }
            Err(error) => {
                tracing::warn!(
                    target = "muxterm::app",
                    "加载配置失败，用现代默认值: {error}"
                );
                (
                    default_document.config.clone(),
                    default_document.shortcuts.clone(),
                )
            }
        };
        if let Some(ref sock) = socket {
            let sock = sock.trim();
            if !sock.is_empty() {
                cfg.tmux.socket = sock.to_string();
            }
        }
        let requested_theme = if cfg.theme.name.eq_ignore_ascii_case("system") {
            let resolved = Theme::resolve_name("system");
            if resolved == "black" {
                cfg.theme.dark.clone()
            } else {
                cfg.theme.light.clone()
            }
        } else {
            cfg.theme.name.clone()
        };
        let theme = match Theme::load(&requested_theme) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    target = "muxterm::app",
                    "加载主题 {} 失败，用内置 light: {e}",
                    requested_theme
                );
                Theme::load("white").unwrap_or_else(|_| fallback_theme())
            }
        };
        let win = crate::platform::linux::window::AppWindow::new_with_effective_keybindings(
            cfg, theme, &shortcuts,
        );
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

fn fallback_theme() -> crate::core::config::Theme {
    let raw = crate::core::config::Theme::embedded("white").expect("embedded white");
    crate::core::config::parse_theme_toml(raw).expect("embedded light 可解析")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_theme_is_embedded_white_not_black() {
        let t = fallback_theme();
        assert_eq!(
            t.background,
            crate::core::config::parse_hex("#ffffff").unwrap()
        );
        assert_ne!(
            t.background,
            crate::core::config::parse_hex("#0b0d10").unwrap()
        );
    }
}
