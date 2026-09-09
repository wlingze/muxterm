//! GTK Application 启动（FFI 驱动的 Linux 前端）。
//!
//! 加载配置/主题 → 创建主窗口（内部 `muxterm_new` + connect）→ GTK 主循环。

use gtk4::prelude::*;
use gtk4::Application;

use crate::core::config_service::ConfigDocument;
use crate::platform::ffi_client::FfiClient;
use crate::platform::linux::theme::fallback_theme;
#[cfg(test)]
use crate::platform::linux::theme::Rgb;

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
        let (mut cfg, shortcuts, theme) = match FfiClient::new_catalog()
            .and_then(|client| client.config_describe())
            .and_then(|snapshot| {
                let theme = snapshot.resolved_theme.unwrap_or_else(fallback_theme);
                serde_json::from_value::<ConfigDocument>(snapshot.values)
                    .map(|document| (document.config, document.shortcuts, theme))
                    .map_err(anyhow::Error::from)
            }) {
            Ok(document) => document,
            Err(error) => {
                tracing::warn!(
                    target = "muxterm::app",
                    "通过 Core FFI 加载配置失败，用现代默认值: {error}"
                );
                (
                    default_document.config.clone(),
                    default_document.shortcuts.clone(),
                    fallback_theme(),
                )
            }
        };
        if let Some(ref sock) = socket {
            let sock = sock.trim();
            if !sock.is_empty() {
                cfg.tmux.socket = sock.to_string();
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_theme_is_embedded_white_not_black() {
        let t = fallback_theme();
        assert_eq!(t.background, Rgb(0xff, 0xff, 0xff));
        assert_ne!(t.background, Rgb(0x0b, 0x0d, 0x10));
    }
}
