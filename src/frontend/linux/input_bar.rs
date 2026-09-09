//! 底部输入框（保留控件，正常 UI 中隐藏）。
//!
//! 所有发送逻辑已迁移到 FFI muxterm_send_input；本模块仅保留 UI 控件壳。

use gtk4::prelude::*;
use gtk4::{Box, Label, Orientation};

/// 输入栏：水平布局 [pane 标签] [输入框]。
///
/// 保留控件以便调试，正常 UI 中隐藏（参见 lifecycle.rs）。
pub struct InputBar {
    pub container: Box,
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
        container.append(&pane_label);

        Self {
            container,
            pane_label,
        }
    }

    /// 设置当前目标 pane id（显示）。
    pub fn set_target(&self, pane: Option<u32>) {
        let text = match pane {
            Some(id) => format!("@{id}"),
            None => "@?".to_string(),
        };
        self.pane_label.set_label(&text);
    }
}
