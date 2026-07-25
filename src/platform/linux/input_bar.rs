//! 底部输入框 + 快捷键。
//!
//! - 输入文本 + Enter → 发送到当前 pane（`send-keys -l`，逐字，不解释特殊键）。
//! - Ctrl+Enter → 同样逐字发送（适合粘贴多行）。
//! - Ctrl+C → 发 `C-c`。
//! - Ctrl+D → 发 `C-d`。
//! - Tab 键 → 发 `Tab`（拦截 GTK 焦点切换）。
//!
//! 输入框旁显示当前目标 pane id，避免发错。

use std::sync::Arc;

use gtk4::gdk::ModifierType;
use gtk4::glib::Propagation;
use gtk4::prelude::*;
use gtk4::{Box, Entry, Label, Orientation};

use crate::core::tmux::command::{send_keys, Key};
use crate::core::tmux::protocol::PaneId;

/// 输入栏：水平布局 [pane 标签] [输入框] [发送按钮]。
pub struct InputBar {
    pub container: Box,
    entry: Entry,
    pane_label: Label,
}

impl Default for InputBar {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBar {
    pub fn new() -> Self {
        let container = Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(6)
            .margin_start(6)
            .margin_end(6)
            .margin_top(4)
            .margin_bottom(4)
            .build();

        let pane_label = Label::builder().label("@?").xalign(0.0).build();
        pane_label.add_css_class("pane-target");
        let entry = Entry::builder()
            .hexpand(true)
            .placeholder_text("输入命令，Enter 发送…")
            .build();
        let send_btn = gtk4::Button::with_label("发送");

        container.append(&pane_label);
        container.append(&entry);
        container.append(&send_btn);

        let entry_for_cb = entry.clone();
        send_btn.connect_clicked(move |_| {
            entry_for_cb.emit_activate();
        });

        Self {
            container,
            entry,
            pane_label,
        }
    }

    /// 设置当前目标 pane id（显示）。
    pub fn set_target(&self, pane: Option<PaneId>) {
        let text = match pane {
            Some(p) => p.as_str(),
            None => "@?".to_string(),
        };
        self.pane_label.set_label(&text);
    }

    /// 安装事件处理：注册发送回调（接收已构造好的命令字符串）。
    ///
    /// `dispatcher`：把 `send-keys` 命令行投递到 tmux client 的发送端。
    /// `current_pane`：返回当前激活 pane id 的闭包。
    pub fn wire(
        &self,
        dispatcher: Arc<dyn Fn(&str) + Send + Sync>,
        current_pane: Arc<dyn Fn() -> Option<PaneId> + Send + Sync>,
    ) {
        // Enter：逐字发送当前输入框文本
        {
            let dispatcher = dispatcher.clone();
            let current_pane = current_pane.clone();
            let entry = self.entry.clone();
            self.entry.connect_activate(move |_| {
                let text = entry.text().to_string();
                if text.is_empty() {
                    return;
                }
                let pane = (current_pane)();
                if let Some(p) = pane {
                    let cmd = send_keys(p, &[Key::literal(&text)]);
                    dispatcher(&cmd.to_line());
                }
                entry.set_text("");
            });
        }

        // 按键事件：Ctrl+C / Ctrl+D / Ctrl+Enter / Tab
        {
            let dispatcher = dispatcher.clone();
            let current_pane = current_pane.clone();
            let entry2 = self.entry.clone();
            let key_ctrl = gtk4::EventControllerKey::new();
            key_ctrl.connect_key_pressed(move |_c, key, _k, mods| {
                let ctrl = mods.contains(ModifierType::CONTROL_MASK);
                match (key, ctrl) {
                    (gtk4::gdk::Key::c, true) => {
                        if let Some(p) = (current_pane)() {
                            let cmd = send_keys(p, &[Key::ctrl('c')]);
                            dispatcher(&cmd.to_line());
                        }
                        Propagation::Stop
                    }
                    (gtk4::gdk::Key::d, true) => {
                        if let Some(p) = (current_pane)() {
                            let cmd = send_keys(p, &[Key::ctrl('d')]);
                            dispatcher(&cmd.to_line());
                        }
                        Propagation::Stop
                    }
                    (gtk4::gdk::Key::Return, true) => {
                        // Ctrl+Enter：逐字发送输入框内容（适合多行粘贴）
                        let text = entry2.text().to_string();
                        if !text.is_empty() {
                            if let Some(p) = (current_pane)() {
                                let cmd = send_keys(p, &[Key::literal(&text)]);
                                dispatcher(&cmd.to_line());
                            }
                            entry2.set_text("");
                        }
                        Propagation::Stop
                    }
                    (gtk4::gdk::Key::Tab, false) => {
                        if let Some(p) = (current_pane)() {
                            let cmd = send_keys(p, &[Key::tab()]);
                            dispatcher(&cmd.to_line());
                        }
                        Propagation::Stop
                    }
                    _ => Propagation::Proceed,
                }
            });
            self.entry.add_controller(key_ctrl);
        }
    }
}
