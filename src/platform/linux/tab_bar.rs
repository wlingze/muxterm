//! Tab 栏：从 FFI `get_tabs` 渲染，点击 → SwitchTab。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Orientation};

use crate::platform::linux::ffi_bridge::BridgeTab;

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

        for t in tabs {
            let label = if t.name.is_empty() {
                format!("t{}", t.id)
            } else {
                truncate_label(&t.name, 24)
            };
            let btn = Button::with_label(&label);
            btn.add_css_class("tab-button");
            if t.is_active {
                btn.add_css_class("active");
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
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let prefix: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{prefix}…")
}
