//! Notebook tab 管理：每个 pane 一个 tab。
//!
//! 维护 pane id（tmux 模式）或序号（本地 shell 模式）→ 页面索引/widget 的映射。
//! tab 标题：本地 shell 用 `shell` / 用户自定义，tmux pane 用 window 名或 `@N`。
//! Notebook 自带 tab 切换；新建 tab 通过 `add_local_pane` 触发（由上层「新建 tab」
//! 按钮调用）。

use std::collections::HashMap;

use gtk4::prelude::*;
use gtk4::{Label, Notebook, PositionType, Widget};

use crate::tmux::protocol::PaneId;
use crate::ui::pane_view::PaneView;

/// tab key：本地 shell 用自增序号，tmux pane 用 pane id。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabKey {
    Local(u64),
    Tmux(PaneId),
}

/// tab 管理器。
pub struct PaneNotebook {
    pub notebook: Notebook,
    panes: HashMap<TabKey, (u32, Widget)>,
    next_local: u64,
}

impl PaneNotebook {
    pub fn new() -> Self {
        let nb = Notebook::new();
        nb.set_show_tabs(true);
        nb.set_tab_pos(PositionType::Top);
        nb.set_scrollable(true);
        Self {
            notebook: nb,
            panes: HashMap::new(),
            next_local: 0,
        }
    }

    /// 添加一个 tab（widget 通常是 vte4 Terminal 的 upcast），返回它的 key。
    pub fn add_pane(&mut self, view: &PaneView, title: &str) -> TabKey {
        let key = match view.mode {
            crate::ui::pane_view::PaneMode::Local => {
                let k = TabKey::Local(self.next_local);
                self.next_local += 1;
                k
            }
            crate::ui::pane_view::PaneMode::Tmux => {
                TabKey::Tmux(view.pane_id.expect("tmux pane 必须有 pane id"))
            }
        };
        if self.panes.contains_key(&key) {
            tracing::warn!(?key, "重复添加 tab，忽略");
            return key;
        }
        let widget = view.terminal.clone().upcast::<Widget>();
        let tab_label = Label::new(Some(title));
        let idx = self.notebook.append_page(&widget, Some(&tab_label));
        self.panes.insert(key, (idx as u32, widget));
        // 切到新 tab
        self.notebook.set_current_page(Some(idx));
        key
    }

    /// 移除一个 tab。
    pub fn remove(&mut self, key: TabKey) {
        if let Some((idx, widget)) = self.panes.remove(&key) {
            self.notebook.remove_page(Some(idx));
            // widget 引用由 notebook 释放
            let _ = widget;
            // 重排剩余页面索引（Notebook 的 index 在 remove 后会变化）
            self.reindex();
        }
    }

    /// 移除当前激活 tab。
    pub fn remove_current(&mut self) -> Option<TabKey> {
        let key = self.current_key()?;
        self.remove(key);
        Some(key)
    }

    /// 重新计算所有 pane 的页面索引（基于 widget 指针，稳健）。
    fn reindex(&mut self) {
        let n = self.notebook.n_pages();
        let mut new_map: HashMap<TabKey, (u32, Widget)> = HashMap::new();
        for i in 0..n {
            if let Some(page) = self.notebook.nth_page(Some(i)) {
                let ptr = page.as_ptr() as usize;
                if let Some(k) = self.panes.iter().find_map(|(k, (_, w))| {
                    if w.as_ptr() as usize == ptr {
                        Some(*k)
                    } else {
                        None
                    }
                }) {
                    if let Some((_, w)) = self.panes.remove(&k) {
                        new_map.insert(k, (i, w));
                    }
                }
            }
        }
        self.panes = new_map;
    }

    /// 更新某个 tab 的标题。
    pub fn set_title(&self, key: TabKey, title: &str) {
        if let Some((_, widget)) = self.panes.get(&key) {
            if let Some(label) = self
                .notebook
                .tab_label(widget)
                .and_then(|w| w.downcast::<Label>().ok())
            {
                label.set_label(title);
            }
        }
    }

    /// 当前激活 tab 的 key。
    pub fn current_key(&self) -> Option<TabKey> {
        let idx = self.notebook.current_page()?;
        self.panes
            .iter()
            .find_map(|(k, (i, _))| if *i == idx as u32 { Some(*k) } else { None })
    }

    /// 按页面索引反查 key（switch-page 信号用）。
    pub fn find_key_by_index(&self, idx: u32) -> Option<TabKey> {
        self.panes
            .iter()
            .find_map(|(k, (i, _))| if *i == idx { Some(*k) } else { None })
    }

    /// 切换到某个 tab。
    pub fn select(&self, key: TabKey) {
        if let Some((idx, _)) = self.panes.get(&key) {
            self.notebook.set_current_page(Some(*idx));
        }
    }

    /// tab 数量。
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// 构造默认 tab 标题：tmux pane 优先名字否则 `@N`；本地 shell 用 `shell`。
    pub fn default_title(key: TabKey, name: Option<&str>) -> String {
        match key {
            TabKey::Tmux(pane) => match name {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => pane.as_str(),
            },
            TabKey::Local(_) => "shell".into(),
        }
    }
}
