//! Tab 栏：从 FFI `get_tabs` 渲染，点击 → SwitchTab。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Orientation};

use crate::core::attention::state::PaneStatus;
use crate::platform::linux::attention_ui::tab_prefix;
use crate::platform::linux::ffi_bridge::BridgeTab;
use crate::platform::linux::lifecycle::tab_shortcut_label;

type TabActivateCb = Box<dyn Fn(u32)>;

/// 极简 tab 栏（基于 FFI tab id）。
pub struct TabBar {
    pub container: GtkBox,
    buttons: RefCell<Vec<(u32, Button)>>,
    on_activate: Rc<RefCell<Option<TabActivateCb>>>,
    height: i32,
}

impl TabBar {
    pub fn new(height: u32) -> Self {
        let height = height.max(18) as i32;
        let container = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(0)
            .hexpand(true)
            .vexpand(false)
            .build();
        container.add_css_class("tab-bar");
        container.set_size_request(-1, height);
        Self {
            container,
            buttons: RefCell::new(Vec::new()),
            on_activate: Rc::new(RefCell::new(None)),
            height,
        }
    }

    pub fn connect_activate<F: Fn(u32) + 'static>(&self, f: F) {
        *self.on_activate.borrow_mut() = Some(Box::new(f));
    }

    /// 用 FFI tab 列表刷新按钮。
    pub fn set_tabs(&self, tabs: &[BridgeTab]) {
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        self.buttons.borrow_mut().clear();

        for (index, t) in tabs.iter().enumerate() {
            let title = tab_button_title(index, &t.name);
            let btn = Button::with_label(&title);
            btn.set_has_frame(false);
            btn.set_can_focus(false);
            btn.add_css_class("tab-button");
            btn.add_css_class("flat");
            if t.is_active {
                btn.add_css_class("tab-active");
            }
            btn.set_size_request(-1, self.height);
            let id = t.id;
            let on_act = self.on_activate.clone();
            btn.connect_clicked(move |_| {
                if let Some(cb) = on_act.borrow().as_ref() {
                    cb(id);
                }
            });
            self.container.append(&btn);
            self.buttons.borrow_mut().push((t.id, btn));
        }
    }

    pub fn tab_count(&self) -> usize {
        self.buttons.borrow().len()
    }

    /// 给 tab 加注意力前缀（blocked `● ` / done `✓ `），并加 CSS 类。
    pub fn set_attention(&self, tab_id: u32, status: Option<PaneStatus>) {
        let prefix = tab_prefix(status);
        let buttons = self.buttons.borrow();
        for (id, btn) in buttons.iter() {
            if *id != tab_id {
                continue;
            }
            let label = btn.label().map(|l| l.to_string()).unwrap_or_default();
            let stripped = label
                .strip_prefix("● ")
                .or_else(|| label.strip_prefix("✓ "))
                .unwrap_or(&label);
            btn.set_label(&format!("{prefix}{stripped}"));
            btn.remove_css_class("tab-blocked");
            btn.remove_css_class("tab-done");
            match status {
                Some(PaneStatus::Blocked) => btn.add_css_class("tab-blocked"),
                Some(PaneStatus::Done) => btn.add_css_class("tab-done"),
                _ => {}
            }
        }
    }

    pub fn set_visible(&self, visible: bool) {
        self.container.set_visible(visible);
    }
}

fn tab_button_title(index: usize, name: &str) -> String {
    truncate_label(&tab_shortcut_label(index, name), 28)
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let prefix: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_title_uses_1based_index_like_macos() {
        assert_eq!(tab_button_title(0, "shell"), "1:shell");
        assert_eq!(tab_button_title(1, "build"), "2:build");
        assert_eq!(tab_button_title(0, ""), "1");
    }
}
