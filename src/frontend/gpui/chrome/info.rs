//! 命令面板里的「Muxterm Info」：版本、commit、调试日志位置、GitHub issue。
//!
//! 对照 macOS `AboutWindow.swift` 的 `MuxtermInfoWindowController`。数据全部来自
//! Core 的 `muxterm_build_info_json`（经 `corebridge::ClientBuildInfo`），
//! 前端不自己拼版本号，也不猜日志路径。
use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};

use crate::frontend::gpui::state::GpuiApp;
use crate::frontend::gpui::theme::Theme;
use crate::frontend::utils::corebridge::ClientBuildInfo;

pub const INFO_WIDTH: f32 = 520.0;
pub const INFO_HEIGHT: f32 = 280.0;
/// 问题入口：与 macOS `submitIssue` 同一个仓库。
pub const ISSUE_URL: &str = "https://github.com/wlingze/muxterm/issues/new";

/// 调试日志没开时的说明：仍然告诉用户日志会落在哪个目录。
pub fn log_placeholder() -> String {
    let dir = std::env::temp_dir().join("muxterm-logs");
    format!(
        "off — enable Settings → Debug (files go to {})",
        dir.display()
    )
}

impl GpuiApp {
    pub(crate) fn render_info(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        let info = self.info()?;
        let theme = self.theme().clone();
        let log_path = info.log_file.clone();
        let log_text = log_path.clone().unwrap_or_else(log_placeholder);
        let rows = [
            ("Version", info.version.clone()),
            ("Commit", info.commit.clone()),
            ("Debug log", log_text),
        ];
        Some(
            div()
                .id("info-overlay")
                .debug_selector(|| "info-overlay".into())
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000033))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|app, _, _, cx| app.close_info(cx)),
                )
                .child(
                    div()
                        .id("info-panel")
                        .debug_selector(|| "info-panel".into())
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .w(px(INFO_WIDTH))
                        .h(px(INFO_HEIGHT))
                        .p(px(16.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.chrome_bg)
                        .text_size(px(13.0))
                        .text_color(theme.label())
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Muxterm Info"),
                        )
                        .children(rows.into_iter().map(|(label, value)| {
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(80.0))
                                        .text_color(theme.muted)
                                        .child(SharedString::from(label)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(theme.label())
                                        .child(SharedString::from(value)),
                                )
                        }))
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(8.0))
                                .child(info_button(
                                    "info-copy-log",
                                    "Copy log path",
                                    !log_path.is_some(),
                                    &theme,
                                    cx,
                                    move |app, _, cx| app.copy_log_path(cx),
                                ))
                                .child(info_button(
                                    "info-copy-commit",
                                    "Copy commit",
                                    false,
                                    &theme,
                                    cx,
                                    move |app, _, cx| app.copy_commit(cx),
                                ))
                                .child(info_button(
                                    "info-issue",
                                    "Submit GitHub issue",
                                    false,
                                    &theme,
                                    cx,
                                    move |_, _, cx| cx.open_url(ISSUE_URL),
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted)
                                .child("Esc closes this panel"),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn info_button(
    id: &'static str,
    title: &'static str,
    disabled: bool,
    theme: &Theme,
    cx: &mut gpui::Context<GpuiApp>,
    handler: impl Fn(&mut GpuiApp, &mut gpui::Window, &mut gpui::Context<GpuiApp>) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .debug_selector(move || format!("{id}-button"))
        .px(px(10.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .bg(if disabled {
            theme.chrome_bg
        } else {
            theme.active_bg
        })
        .text_color(if disabled { theme.muted } else { theme.label() })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, window, cx| {
                if disabled {
                    return;
                }
                cx.stop_propagation();
                handler(app, window, cx);
            }),
        )
        .child(title)
        .into_any_element()
}

/// 「现在还有什么信息可以复制」这类纯逻辑，便于无头测试。
pub fn copyable(info: &ClientBuildInfo) -> (Option<String>, String) {
    (info.log_file.clone(), info.commit.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 日志没开时也要给出目录，不让用户「找不到路径」。
    #[test]
    fn off_state_still_names_the_log_directory() {
        let text = log_placeholder();
        assert!(text.contains("off"));
        assert!(text.contains("muxterm-logs"));
        assert!(!text.contains("muxterm-0000"));
    }

    /// 日志开着时抄路径，没开时不提供（按钮由渲染层禁用）。
    #[test]
    fn copyable_follows_the_core_build_info() {
        let off = ClientBuildInfo {
            version: "0.1.0".into(),
            commit: "abc1234".into(),
            log_file: None,
        };
        assert_eq!(copyable(&off), (None, "abc1234".to_string()));
        let on = ClientBuildInfo {
            log_file: Some("/tmp/muxterm-logs/muxterm-2026-11-05.log".into()),
            ..off
        };
        assert_eq!(
            copyable(&on).0.as_deref(),
            Some("/tmp/muxterm-logs/muxterm-2026-11-05.log")
        );
    }
}
