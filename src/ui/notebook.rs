//! Notebook tab 管理：每个 tmux pane 一个 tab。
//!
//! 维护 pane id → 页面索引/widget 的映射。tab 标题优先用 window/pane 名，
//! 缺失时用 pane id（`@N`）。PaneView 的实际持有由 AppWindow 负责，本结构
//! 只管 Notebook 的页面几何。

use std::collections::HashMap;

use gtk4::prelude::*;
use gtk4::{Label, Notebook, PositionType, Widget};

use crate::tmux::protocol::PaneId;

/// tab 管理器：持有 Notebook 和 pane→页面索引映射。
pub struct PaneNotebook {
    pub notebook: Notebook,
    /// pane id → (页面 index, 页面 widget)
    panes: HashMap<u32, (u32, Widget)>,
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
        }
    }

    /// 添加一个 pane tab（widget 通常是 vte4 Terminal 的 upcast）。
    pub fn add_pane(&mut self, pane: PaneId, widget: Widget, title: &str) {
        if self.panes.contains_key(&pane.0) {
            tracing::warn!(pane = pane.0, "重复添加 pane tab，忽略");
            return;
        }
        let tab_label = Label::new(Some(title));
        let idx = self.notebook.append_page(&widget, Some(&tab_label));
        self.panes.insert(pane.0, (idx as u32, widget));
    }

    /// 移除一个 pane 的 tab。
    pub fn remove_pane(&mut self, pane: PaneId) {
        if let Some((idx, _widget)) = self.panes.remove(&pane.0) {
            self.notebook.remove_page(Some(idx));
        }
    }

    /// 更新某个 pane tab 的标题。
    pub fn set_title(&self, pane: PaneId, title: &str) {
        if let Some((_, widget)) = self.panes.get(&pane.0) {
            if let Some(label) = self
                .notebook
                .tab_label(widget)
                .and_then(|w| w.downcast::<Label>().ok())
            {
                label.set_label(title);
            }
        }
    }

    /// 当前激活 tab 对应的 pane id（若有）。
    pub fn current_pane(&self) -> Option<PaneId> {
        let idx = self.notebook.current_page()?;
        self.panes.iter().find_map(|(n, (i, _))| {
            if *i == idx as u32 {
                Some(PaneId(*n))
            } else {
                None
            }
        })
    }

    /// 切换到某个 pane。
    pub fn select_pane(&self, pane: PaneId) {
        if let Some((idx, _)) = self.panes.get(&pane.0) {
            self.notebook.set_current_page(Some(*idx));
        }
    }

    /// pane 数量。
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// 返回所有 pane id 与其页面索引的快照（供外部反查）。
    pub fn panes(&self) -> Vec<(PaneId, u32)> {
        self.panes
            .iter()
            .map(|(p, (i, _))| (PaneId(*p), *i))
            .collect()
    }

    /// 构造默认 tab 标题：优先名字，否则用 `@N`。
    pub fn default_title(pane: PaneId, name: Option<&str>) -> String {
        match name {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => pane.as_str(),
        }
    }
}
