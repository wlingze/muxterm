//! 独立设置窗口的交互。项目列表只编辑草稿，不会顺带打开 workspace。
use super::GpuiApp;
use crate::frontend::gpui::{
    model::{quick_connect::TargetConfig, workspace_picker::ProjectList},
    terminal::KeyPress,
};
use gpui::{Context, KeyDownEvent, Window};

impl GpuiApp {
    pub fn settings_edit_project(&mut self, config: Option<&TargetConfig>, cx: &mut Context<Self>) {
        self.open_target_editor(config, cx);
        if config.is_none() {
            self.target_editor =
                Some(crate::frontend::gpui::model::target_editor::TargetEditor::new_project());
        }
        self.settings_project_editor = true;
        self.target_editor_focus =
            crate::frontend::gpui::model::target_editor::TargetEditorFocus::Name;
        cx.notify();
    }

    pub fn settings_delete_project(&mut self, config: &TargetConfig, cx: &mut Context<Self>) {
        let mut projects = ProjectList::new(
            &self
                .settings_value("/projects")
                .unwrap_or_else(|| serde_json::json!([])),
        );
        projects.remove(config);
        self.settings_set("/projects", projects.value(), cx);
    }

    /// 独立窗口有自己的按键入口，绝不把设置里的键交给终端全局路由。
    pub(crate) fn handle_settings_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_project_editor {
            self.handle_key(event, window, cx);
            cx.stop_propagation();
            return;
        }
        let key = &event.keystroke;
        if self.shortcut_editor.recording.is_some() {
            let canonical = crate::frontend::gpui::platform::physical_key_for_event(&key.key)
                .unwrap_or(&key.key);
            self.settings_record_shortcut(
                crate::frontend::gpui::model::shortcuts::KeyChord::new(
                    key.modifiers.platform,
                    key.modifiers.shift,
                    key.modifiers.alt,
                    key.modifiers.control,
                    canonical,
                ),
                cx,
            );
            cx.stop_propagation();
            return;
        }
        if key.modifiers.platform {
            match key.key.as_str() {
                "w" => self.settings_cancel(cx),
                "a" => {
                    self.settings_select_all = true;
                    cx.notify();
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.settings_type(&text, cx);
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        let press = KeyPress {
            key: key.key.clone(),
            key_char: key.key_char.clone(),
            ctrl: key.modifiers.control,
            alt: key.modifiers.alt,
            shift: key.modifiers.shift,
            cmd: false,
        };
        if crate::frontend::gpui::terminal::keymap::defers_to_ime(&press) {
            // 平台输入法拥有普通文字（包括组词的第一键）。
            return;
        }
        self.route_settings_key(key, cx);
        cx.stop_propagation();
    }

    /// 明确的多窗口回归入口：复用现有状态，不创建另一个 FFI handle。
    pub fn detach_settings_window(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        if let Some(handle) = self.settings_window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return Ok(());
            }
        }
        self.settings_detached = true;
        let owner = cx.entity();
        // open_window 可能立即渲染新窗口，必须先把 owner 交还 EntityMap，
        // 否则新窗口读同一 owner 会触发重入借用 panic。
        cx.defer(move |cx| {
            if !owner.read(cx).settings_open || owner.read(cx).settings_window.is_some() {
                return;
            }
            let result = crate::frontend::gpui::chrome::settings_window::SettingsWindow::open(
                owner.clone(),
                cx,
            );
            owner.update(cx, |app, cx| {
                match result {
                    Ok(handle) => app.settings_window = Some(handle),
                    Err(error) => {
                        app.settings_error = Some(format!("Unable to open Settings: {error}"))
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
        Ok(())
    }
}
