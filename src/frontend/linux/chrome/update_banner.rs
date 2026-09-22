//! 更新提醒条（GTK）：有新版本时显示「一键更新」按钮。
//!
//! 这是纯渲染层：状态来自 Core（`EventPump::poll_update_status`），点击只调
//! Core 的 check/install FFI。前端不做版本比较、不下载、不落盘。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation};

use crate::frontend::utils::corebridge::{ClientUpdatePhase, ClientUpdateStatus};
use crate::frontend::utils::i18n;

type ActivateCb = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// 更新提醒条：一条水平 banner，按 Core 阶段切换文案与按钮。
pub struct UpdateBanner {
    pub(crate) container: GtkBox,
    message: Label,
    action: Button,
    dismiss: Button,
    on_action: ActivateCb,
    on_dismiss: ActivateCb,
    /// 用户主动关掉后不再显示同一版本。
    dismissed_version: RefCell<Option<String>>,
    last_rendered: RefCell<Option<ClientUpdateStatus>>,
}

impl UpdateBanner {
    pub fn new() -> Self {
        let container = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .build();
        container.set_widget_name("muxterm-update-banner");
        container.add_css_class("muxterm-update-banner");
        container.set_visible(false);

        let message = Label::new(None);
        message.set_widget_name("muxterm-update-message");
        message.set_halign(Align::Start);
        message.set_hexpand(true);
        message.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        message.set_xalign(0.0);
        container.append(&message);

        let action = Button::with_label(&i18n::tr(i18n::Key::UpdateInstallNow));
        action.set_widget_name("muxterm-update-action");
        action.add_css_class("muxterm-update-action");
        action.set_can_focus(false);
        container.append(&action);

        let dismiss = Button::with_label("✕");
        dismiss.set_widget_name("muxterm-update-dismiss");
        dismiss.set_has_frame(false);
        dismiss.set_can_focus(false);
        dismiss.set_tooltip_text(Some(&i18n::tr(i18n::Key::Cancel)));
        container.append(&dismiss);

        let on_action = Rc::new(RefCell::new(None::<Box<dyn Fn()>>));
        let on_dismiss = Rc::new(RefCell::new(None::<Box<dyn Fn()>>));
        {
            let callback = on_action.clone();
            action.connect_clicked(move |_| {
                if let Some(callback) = callback.borrow().as_ref() {
                    callback();
                }
            });
        }
        {
            let callback = on_dismiss.clone();
            dismiss.connect_clicked(move |_| {
                if let Some(callback) = callback.borrow().as_ref() {
                    callback();
                }
            });
        }

        Self {
            container,
            message,
            action,
            dismiss,
            on_action,
            on_dismiss,
            dismissed_version: RefCell::new(None),
            last_rendered: RefCell::new(None),
        }
    }

    /// 连接主操作（安装/重试）与关闭按钮。
    pub fn connect_actions<A, D>(&self, on_action: A, on_dismiss: D)
    where
        A: Fn() + 'static,
        D: Fn() + 'static,
    {
        *self.on_action.borrow_mut() = Some(Box::new(on_action));
        *self.on_dismiss.borrow_mut() = Some(Box::new(on_dismiss));
    }

    /// 按 Core 状态渲染。状态未变化时不动 widget（避免每 16ms 重排）。
    pub fn apply(&self, status: &ClientUpdateStatus) {
        if self
            .last_rendered
            .borrow()
            .as_ref()
            .is_some_and(|last| last == status)
        {
            return;
        }
        *self.last_rendered.borrow_mut() = Some(status.clone());

        let version = status.version.clone().unwrap_or_default();
        if !version.is_empty() && *self.dismissed_version.borrow() == Some(version.clone()) {
            self.container.set_visible(false);
            return;
        }
        if version.is_empty() {
            *self.dismissed_version.borrow_mut() = None;
        }

        let (visible, text, action_label, action_sensitive) = match status.phase {
            ClientUpdatePhase::Available => (
                true,
                i18n::tr_args(
                    i18n::Key::UpdateAvailableMessage,
                    &[("version", version.as_str())],
                ),
                i18n::tr(i18n::Key::UpdateInstallNow),
                true,
            ),
            ClientUpdatePhase::Installing => (
                true,
                i18n::tr(i18n::Key::UpdateInstalling),
                i18n::tr(i18n::Key::UpdateInstalling),
                false,
            ),
            ClientUpdatePhase::Installed => (
                true,
                i18n::tr(i18n::Key::UpdateInstalledRestart),
                i18n::tr(i18n::Key::UpdateInstalledRestart),
                false,
            ),
            ClientUpdatePhase::Failed => {
                let message = status.message.clone().unwrap_or_default();
                (
                    true,
                    i18n::tr_args(i18n::Key::UpdateFailed, &[("message", message.as_str())]),
                    i18n::tr(i18n::Key::UpdateRetry),
                    true,
                )
            }
            ClientUpdatePhase::Checking => (
                true,
                i18n::tr(i18n::Key::UpdateChecking),
                i18n::tr(i18n::Key::UpdateChecking),
                false,
            ),
            // 已是最新只在用户主动检查后提示一次；空闲时不占位。
            ClientUpdatePhase::UpToDate | ClientUpdatePhase::Idle => {
                (false, String::new(), String::new(), false)
            }
        };

        self.container.set_visible(visible);
        if !visible {
            return;
        }
        self.message.set_text(&text);
        self.action.set_label(&action_label);
        self.action.set_sensitive(action_sensitive);
        self.action.set_visible(action_sensitive);
        // 安装中/已完成时不需要「关闭」打扰用户。
        self.dismiss.set_visible(action_sensitive);
    }

    /// 用户点了关闭：记住这个版本不再提醒。
    pub fn dismiss(&self, version: Option<String>) {
        *self.dismissed_version.borrow_mut() = version;
        self.container.set_visible(false);
    }

    /// 状态 JSON 里的版本号（测试与关闭逻辑共用）。
    pub fn current_version(&self) -> Option<String> {
        self.last_rendered
            .borrow()
            .as_ref()
            .and_then(|status| status.version.clone())
    }

    pub fn widget(&self) -> &GtkBox {
        &self.container
    }

    pub fn is_visible(&self) -> bool {
        self.container.is_visible()
    }
}

impl Default for UpdateBanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(phase: ClientUpdatePhase, version: Option<&str>) -> ClientUpdateStatus {
        ClientUpdateStatus {
            phase,
            current_version: "v1.0.0".into(),
            version: version.map(str::to_string),
            message: Some("offline".into()),
            release_url: None,
            download_url: None,
            asset_name: None,
            restart_required: false,
        }
    }

    #[test]
    fn banner_copy_targets_the_available_phase() {
        // 纯逻辑断言：文案 key 必须在 catalog 中存在（parity 测试会全局校验）。
        assert!(!i18n::tr(i18n::Key::UpdateInstallNow).is_empty());
        assert!(
            i18n::tr_args(i18n::Key::UpdateAvailableMessage, &[("version", "v2.0.0")])
                .contains("v2.0.0")
        );
        assert!(!i18n::tr_args(i18n::Key::UpdateFailed, &[("message", "boom")]).is_empty());
        assert!(!i18n::tr(i18n::Key::UpdateInstalledRestart).is_empty());
    }

    #[test]
    fn idle_status_does_not_claim_an_update() {
        let idle = status(ClientUpdatePhase::Idle, None);
        assert!(!idle.has_update());
        assert!(!idle.needs_restart());
        let available = status(ClientUpdatePhase::Available, Some("v2.0.0"));
        assert!(available.has_update());
    }

    #[test]
    fn installed_status_requires_a_restart() {
        let installed = status(ClientUpdatePhase::Installed, Some("v2.0.0"));
        assert!(installed.needs_restart());
    }
}
