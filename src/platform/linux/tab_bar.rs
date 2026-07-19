//! 极简 tab 栏（类似 tmux status line）。
//!
//! 高度约 24px，无边框/无多余 padding；显示 `序号:名字`（如 `1:bash`）。
//! 点击切换 tab；当前 tab 用 CSS class `active` 高亮。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Orientation};

use crate::platform::linux::notebook::TabKey;

/// 一条 tab 的展示信息。
#[derive(Debug, Clone)]
pub struct TabBarItem {
    pub key: TabKey,
    /// 已格式化标题，如 `1:bash` 或 `2:vim · 2panes`。
    pub title: String,
    pub active: bool,
}

/// 生成 tab 栏按钮标题：`序号:显示名`。
pub fn format_tab_bar_title(index_1based: usize, display_name: &str) -> String {
    format!("{index_1based}:{display_name}")
}

/// 多 pane 时附加 ` · Npanes` 后缀；单 pane 原样返回。
pub fn format_tab_display_name(name: &str, n_panes: usize) -> String {
    if n_panes > 1 {
        format!("{name} · {n_panes}panes")
    } else {
        name.to_string()
    }
}

/// 过长名字截断（保留前缀 + …）。
pub fn truncate_tab_name(name: &str, max_chars: usize) -> String {
    let count = name.chars().count();
    if max_chars == 0 || count <= max_chars {
        return name.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let prefix: String = name.chars().take(keep).collect();
    format!("{prefix}…")
}

/// 极简 tab 栏。
pub struct TabBar {
    pub container: GtkBox,
    buttons: Rc<RefCell<Vec<(TabKey, Button)>>>,
    on_activate: Rc<RefCell<Option<Box<dyn Fn(TabKey)>>>>,
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
        container.set_margin_start(0);
        container.set_margin_end(0);
        container.set_margin_top(0);
        container.set_margin_bottom(0);
        Self {
            container,
            buttons: Rc::new(RefCell::new(Vec::new())),
            on_activate: Rc::new(RefCell::new(None)),
            height,
        }
    }

    /// 注册点击回调。
    pub fn connect_activate<F: Fn(TabKey) + 'static>(&self, f: F) {
        *self.on_activate.borrow_mut() = Some(Box::new(f));
    }

    /// 用完整列表重建按钮（顺序即显示顺序）。
    pub fn rebuild(&self, items: &[TabBarItem]) {
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        self.buttons.borrow_mut().clear();

        for item in items {
            let btn = Button::builder()
                .label(&item.title)
                .has_frame(false)
                .focus_on_click(false)
                .build();
            btn.add_css_class("tab-bar-item");
            btn.set_size_request(-1, self.height);
            if item.active {
                btn.add_css_class("active");
            }
            let key = item.key;
            let on_activate = self.on_activate.clone();
            btn.connect_clicked(move |_| {
                if let Some(cb) = on_activate.borrow().as_ref() {
                    cb(key);
                }
            });
            self.container.append(&btn);
            self.buttons.borrow_mut().push((item.key, btn));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对应：tab 显示名 `1:bash`。
    #[test]
    fn test_tab_bar_format_title_plain() {
        assert_eq!(format_tab_bar_title(1, "bash"), "1:bash");
        assert_eq!(format_tab_bar_title(3, "vim"), "3:vim");
    }

    /// 对应：多 pane tab 显示 `名 · Npanes`。
    #[test]
    fn test_tab_bar_multi_pane_suffix() {
        assert_eq!(format_tab_display_name("bash", 1), "bash");
        assert_eq!(format_tab_display_name("bash", 2), "bash · 2panes");
        assert_eq!(
            format_tab_bar_title(2, &format_tab_display_name("vim", 3)),
            "2:vim · 3panes"
        );
    }

    /// 对应：过长名字截断不撑爆 tab 栏。
    #[test]
    fn test_tab_bar_truncate_long_name() {
        let long = "a".repeat(40);
        let t = truncate_tab_name(&long, 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('…'));
        assert_eq!(truncate_tab_name("bash", 10), "bash");
    }

    /// 对应：激活 tab 标记（纯数据，UI 用 CSS class）。
    #[test]
    fn test_tab_bar_item_active_flag() {
        let item = TabBarItem {
            key: TabKey::Local(1),
            title: "1:shell".into(),
            active: true,
        };
        assert!(item.active);
        assert_eq!(item.title, "1:shell");
    }
}
