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
