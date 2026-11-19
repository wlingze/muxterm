//! 快捷键设置与原生边界的共享接线。所有写入只改变草稿，Apply 才经 CommandQueue。
use super::GpuiApp;
use crate::frontend::gpui::model::{
    shortcut_editor::{self, ShortcutMap, ShortcutRow},
    shortcuts::{KeyAction, KeyChord},
};
use gpui::Context;
use serde_json::Value;

impl GpuiApp {
    pub fn apply_shortcut_values(&mut self, values: &Value) {
        // `[shortcuts] primary_key` 决定 Linux / Windows 上哪个键是「命令键」
        // （`auto`：macOS = Command，其它平台 = Alt）。平台层持有它，
        // 子视图（命令面板 / 设置窗）不必各自传一遍配置。
        crate::frontend::gpui::platform::set_primary_key(
            values
                .pointer("/shortcuts/primary_key")
                .and_then(Value::as_str)
                .unwrap_or("auto"),
        );
        self.shortcuts = ShortcutMap::from_values(values);
        self.sync_native_shortcuts();
    }

    pub(crate) fn sync_native_shortcuts(&self) {
        crate::frontend::gpui::platform::configure_shortcuts(
            &self.shortcuts,
            self.panel.open
                || self.palette.open
                || self.composer_open
                || self.target_editor.is_some()
                || self.prompt.is_some(),
            self.shortcut_editor.recording.is_some(),
            self.qwerty_labels,
        );
    }

    pub fn shortcut_rows(&self) -> Vec<ShortcutRow> {
        let query = self.shortcut_editor.query.trim().to_lowercase();
        shortcut_editor::catalog()
            .into_iter()
            .filter(|row| {
                query.is_empty()
                    || row.title.to_lowercase().contains(&query)
                    || row.id.contains(&query)
            })
            .collect()
    }

    pub fn settings_start_recording(&mut self, action: KeyAction, cx: &mut Context<Self>) {
        self.shortcut_editor.recording = Some(action);
        self.shortcut_editor.error = None;
        self.sync_native_shortcuts();
        cx.notify();
    }

    pub fn settings_record_shortcut(&mut self, chord: KeyChord, cx: &mut Context<Self>) {
        let Some(action) = self.shortcut_editor.recording else {
            return;
        };
        if chord.key == "escape" {
            self.shortcut_editor.recording = None;
            self.sync_native_shortcuts();
            cx.notify();
            return;
        }
        if !chord.command && !chord.control && !chord.option {
            self.shortcut_editor.error = Some("Use Command, Control or Option with a key.".into());
            cx.notify();
            return;
        }
        let map = ShortcutMap::from_values(self.settings_draft.values());
        if let Some(conflict) = map.resolve(&chord).filter(|bound| *bound != action) {
            self.shortcut_editor.error = Some(format!(
                "{} is already used by {conflict:?}.",
                shortcut_editor::label(&chord, false)
            ));
            cx.notify();
            return;
        }
        let value =
            shortcut_editor::set_override(self.settings_draft.values(), action, Some(&chord));
        self.settings_set("/shortcuts/overrides", value, cx);
        self.shortcut_editor.recording = None;
        self.shortcut_editor.error = None;
        self.sync_native_shortcuts();
        cx.notify();
    }

    pub fn settings_disable_shortcut(&mut self, action: KeyAction, cx: &mut Context<Self>) {
        let value = shortcut_editor::set_override(self.settings_draft.values(), action, None);
        self.settings_set("/shortcuts/overrides", value, cx);
    }

    pub fn settings_restore_shortcut(&mut self, action: KeyAction, cx: &mut Context<Self>) {
        let value = shortcut_editor::restore(self.settings_draft.values(), action);
        self.settings_set("/shortcuts/overrides", value, cx);
    }
}
