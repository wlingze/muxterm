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

/// 调试日志的目录（没开日志时也能复制它）。
pub fn log_directory() -> String {
    std::env::temp_dir()
        .join("muxterm-logs")
        .display()
        .to_string()
}

/// 调试日志没开时的说明：仍然告诉用户日志会落在哪个目录。
pub fn log_placeholder() -> String {
    format!(
        "off — enable Settings → Debug (files go to {})",
        log_directory()
    )
}

/// 「Copy log」按钮实际复制什么、按钮叫什么。
///
/// 日志没开也必须能复制（复制目录），否则按钮点了没反应——用户看到的就是
/// 「copy 失败」。
pub fn log_copy_target(info: &ClientBuildInfo) -> (String, &'static str) {
    match info.log_file.as_deref().filter(|path| !path.is_empty()) {
        Some(path) => (path.to_string(), "Copy log path"),
        None => (log_directory(), "Copy log dir"),
    }
}

impl GpuiApp {
    pub(crate) fn render_info(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        let info = self.info()?;
        let theme = self.theme().clone();
        let log_text = info.log_file.clone().unwrap_or_else(log_placeholder);
        let (_, log_label) = log_copy_target(info);
        let status = self.info_status().map(str::to_string);
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
                                    log_label,
                                    false,
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
                                .debug_selector(|| "info-status".into())
                                .child(SharedString::from(
                                    status.unwrap_or_else(|| "Esc closes this panel".to_string()),
                                )),
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

    /// 日志没开时按钮改成「复制目录」，点了必须有东西可复制。
    #[test]
    fn log_copy_target_falls_back_to_the_directory() {
        let off = ClientBuildInfo {
            version: "0.1.0".into(),
            commit: "abc1234".into(),
            log_file: None,
        };
        let (value, label) = log_copy_target(&off);
        assert_eq!(label, "Copy log dir");
        assert!(value.contains("muxterm-logs"), "{value}");
        let on = ClientBuildInfo {
            log_file: Some("/tmp/muxterm-logs/muxterm-1.log".into()),
            ..off
        };
        let (value, label) = log_copy_target(&on);
        assert_eq!(label, "Copy log path");
        assert_eq!(value, "/tmp/muxterm-logs/muxterm-1.log");
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

/// 图片粘贴横幅：右下角浮一条，进度/成功/失败三种色调。
///
/// 对照 Swift `ImagePasteBanner`：位置固定在内容区右下，不挡终端首行，
/// 进行中显示转圈，结果可点叉关掉（`Esc` 也能关，见 `state.rs`）。
pub fn render_image_paste_banner(
    app: &crate::frontend::gpui::state::GpuiApp,
    theme: &crate::frontend::gpui::theme::Theme,
) -> Option<gpui::AnyElement> {
    use crate::frontend::gpui::model::image_paste::{ImagePasteStage, Tone};
    use gpui::{div, prelude::*, px, SharedString};
    let feedback = app.image_paste_feedback()?;
    let (tone_color, label) = match feedback.tone {
        Tone::Progress => (theme.accent, "Encoding image…"),
        Tone::Success => (theme.success, "Image pasted"),
        Tone::Failure => (theme.danger, "Image paste failed"),
    };
    let label = if matches!(feedback.stage, ImagePasteStage::Uploading) {
        "Uploading image…"
    } else {
        label
    };
    let detail = feedback.detail.clone().map(SharedString::from);
    Some(
        div()
            .id("image-paste-banner")
            .debug_selector(|| "image-paste-banner".into())
            .absolute()
            .right(px(16.0))
            .bottom(px(16.0))
            .max_w(px(420.0))
            .flex()
            .flex_col()
            .gap_1()
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .bg(theme.chrome_bg)
            .border_1()
            .border_color(tone_color)
            .text_size(px(11.5))
            .text_color(theme.fg)
            .shadow_md()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(tone_color))
                    .child(SharedString::from(label)),
            )
            .children(detail.map(|detail| {
                div()
                    .text_size(px(10.5))
                    .text_color(theme.muted)
                    .overflow_hidden()
                    .child(detail)
            }))
            .into_any_element(),
    )
}
