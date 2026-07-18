//! Notebook tab 管理。
//!
//! - 本地 tab（`TabKey::Local`）：内容是一个 [`TabContent`]，可包含多个 pane
//!   （水平/竖直分割，GtkPaned 线性链）。每个 pane 是一个本地 shell。
//! - tmux tab（`TabKey::Tmux(PaneId)`）：内容是单个 vte4 Terminal（tmux 的每个
//!   pane 渲染成我们的一个 tab，1:1）。

use std::collections::HashMap;

use gtk4::prelude::*;
use gtk4::{Label, Notebook, Orientation, Paned, PositionType, Widget};

use crate::tmux::protocol::PaneId;
use crate::ui::pane_view::{PaneMode, PaneView};

/// tab 标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabKey {
    Local(u64),
    Tmux(PaneId),
}

/// 一个本地 pane 在 tab 内的序号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalPaneId(pub u64);

/// pane 标识（跨 local/tmux）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneKey {
    Local(LocalPaneId),
    Tmux(PaneId),
}

/// 一个本地 tab 的内容：若干 pane 的线性分割。
#[derive(Clone)]
pub struct TabContent {
    /// 该 tab 内所有 pane 的 key（按分割顺序）。
    pub panes: Vec<PaneKey>,
    /// 当前激活 pane（焦点所在）。
    pub active: Option<PaneKey>,
    /// 根 widget（随分割变化重建）。
    pub root: Widget,
    /// 分割方向（首个 Paned 的方向）。
    pub orientation: Orientation,
}

impl TabContent {
    pub fn single(pane: PaneKey, terminal: &vte4::Terminal) -> Self {
        let root = terminal.clone().upcast::<Widget>();
        TabContent {
            panes: vec![pane],
            active: Some(pane),
            root,
            orientation: Orientation::Horizontal,
        }
    }
}

/// tab 管理器。
pub struct PaneNotebook {
    pub notebook: Notebook,
    /// tab key → (页面索引, TabContent 或 None for tmux 单 pane)
    pub(crate) tabs: HashMap<TabKey, (u32, Option<TabContent>)>,
    /// tmux pane：tab key → terminal widget（直接持有，无分割）
    pub(crate) tmux_widgets: HashMap<TabKey, Widget>,
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
            tabs: HashMap::new(),
            tmux_widgets: HashMap::new(),
            next_local: 0,
        }
    }

    /// 新建一个本地 tab，内含一个 pane。返回 (tab key, pane key)。
    pub fn add_local_tab(&mut self, view: &PaneView, title: &str) -> (TabKey, PaneKey) {
        let tab_key = TabKey::Local(self.next_local);
        self.next_local += 1;
        let pane_key = PaneKey::Local(LocalPaneId(0));
        let content = TabContent::single(pane_key, &view.terminal);
        let widget = content.root.clone();
        let idx = self.append(&widget, title);
        self.tabs.insert(tab_key, (idx, Some(content)));
        self.notebook.set_current_page(Some(idx));
        (tab_key, pane_key)
    }

    /// 新建一个 tmux pane tab（1:1）。
    pub fn add_tmux_tab(&mut self, view: &PaneView, title: &str) -> TabKey {
        let pane_id = view.pane_id.expect("tmux pane 必须有 pane id");
        let key = TabKey::Tmux(pane_id);
        if self.tabs.contains_key(&key) {
            return key;
        }
        let widget = view.terminal.clone().upcast::<Widget>();
        let idx = self.append(&widget, title);
        self.tabs.insert(key, (idx, None));
        self.tmux_widgets.insert(key, widget);
        self.notebook.set_current_page(Some(idx));
        key
    }

    fn append(&self, widget: &Widget, title: &str) -> u32 {
        let label = Label::new(Some(title));
        self.notebook.append_page(widget, Some(&label))
    }

    /// 在指定本地 tab 的当前激活 pane 旁分割出一个新 pane，返回新 pane key。
    /// orientation: Horizontal = 左右分（vte 术语），Vertical = 上下分。
    pub fn split_local_pane(&mut self, tab: TabKey, orientation: Orientation) -> Option<PaneKey> {
        let (_idx, content_opt) = self.tabs.get_mut(&tab)?;
        let content = content_opt.as_mut()?;
        // 新 pane id = 当前最大 + 1
        let next_id = content
            .panes
            .iter()
            .filter_map(|p| match p {
                PaneKey::Local(LocalPaneId(n)) => Some(*n),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1;
        let new_pane = PaneKey::Local(LocalPaneId(next_id));
        content.panes.push(new_pane);
        content.orientation = orientation;
        Some(new_pane)
    }

    /// 重建本地 tab 的根 widget（分割变化后调用）。
    /// `terminals` 提供 pane key → Terminal 的映射。
    pub fn rebuild_local_root(
        &mut self,
        tab: TabKey,
        terminals: &HashMap<PaneKey, vte4::Terminal>,
    ) {
        let Some((_idx, content_opt)) = self.tabs.get_mut(&tab) else {
            return;
        };
        let Some(content) = content_opt.as_mut() else {
            return;
        };
        if content.panes.is_empty() {
            return;
        }
        // 线性链：第一个 pane 作左/上，依次往右/下加。
        let first = terminals.get(&content.panes[0]).cloned();
        let Some(mut acc) = first.map(|t| t.upcast::<Widget>()) else {
            return;
        };
        for p in content.panes.iter().skip(1) {
            let Some(t) = terminals.get(p) else {
                continue;
            };
            let paned = Paned::builder()
                .orientation(content.orientation)
                .wide_handle(true)
                .build();
            paned.set_start_child(Some(&acc));
            paned.set_end_child(Some(&t.clone().upcast::<gtk4::Widget>()));
            paned.set_position(400);
            acc = paned.upcast::<Widget>();
        }
        content.root = acc;
    }

    /// 替换 tab 的页面 widget（rebuild 后旧 widget 需要换成新 root）。
    /// Notebook 没有 replace page，用 remove + insert（保持索引）。
    pub fn relayout_local_tab(&mut self, tab: TabKey, title: &str) {
        let idx = match self.tabs.get(&tab) {
            Some((i, _)) => *i,
            None => return,
        };
        let content = match self.tabs.get(&tab).and_then(|(_, c)| c.clone()) {
            Some(c) => c,
            None => return,
        };
        // remove 旧 page
        self.notebook.remove_page(Some(idx));
        // insert 新 root 到原位置
        let label = Label::new(Some(title));
        let new_idx = self
            .notebook
            .insert_page(&content.root, Some(&label), Some(idx));
        self.reindex();
        if let Some(entry) = self.tabs.get_mut(&tab) {
            entry.0 = new_idx;
        }
        self.notebook.set_current_page(Some(new_idx));
    }

    /// 移除一个 tab。
    pub fn remove(&mut self, key: TabKey) {
        if let Some((idx, _)) = self.tabs.remove(&key) {
            self.notebook.remove_page(Some(idx));
            self.tmux_widgets.remove(&key);
            self.reindex();
        }
    }

    fn reindex(&mut self) {
        let n = self.notebook.n_pages();
        let mut new_tabs: HashMap<TabKey, (u32, Option<TabContent>)> = HashMap::new();
        for i in 0..n {
            if let Some(page) = self.notebook.nth_page(Some(i)) {
                let ptr = page.as_ptr() as usize;
                // 本地 tab：匹配 content.root
                let found = self.tabs.iter().find_map(|(k, (_, c))| {
                    let matches = match c {
                        Some(content) => content.root.as_ptr() as usize == ptr,
                        None => self
                            .tmux_widgets
                            .get(k)
                            .map(|w| w.as_ptr() as usize == ptr)
                            .unwrap_or(false),
                    };
                    if matches {
                        Some(*k)
                    } else {
                        None
                    }
                });
                if let Some(k) = found {
                    if let Some((_, c)) = self.tabs.remove(&k) {
                        new_tabs.insert(k, (i, c));
                    }
                }
            }
        }
        self.tabs = new_tabs;
    }

    pub fn set_title(&self, key: TabKey, title: &str) {
        let widget = match self.tabs.get(&key) {
            Some((_, Some(c))) => Some(&c.root),
            Some((_, None)) => self.tmux_widgets.get(&key),
            None => None,
        };
        if let Some(w) = widget {
            if let Some(label) = self
                .notebook
                .tab_label(w)
                .and_then(|l| l.downcast::<Label>().ok())
            {
                label.set_label(title);
            }
        }
    }

    pub fn current_key(&self) -> Option<TabKey> {
        let idx = self.notebook.current_page()?;
        self.tabs
            .iter()
            .find_map(|(k, (i, _))| if *i == idx { Some(*k) } else { None })
    }

    pub fn find_key_by_index(&self, idx: u32) -> Option<TabKey> {
        self.tabs
            .iter()
            .find_map(|(k, (i, _))| if *i == idx { Some(*k) } else { None })
    }

    pub fn select(&self, key: TabKey) {
        if let Some((idx, _)) = self.tabs.get(&key) {
            self.notebook.set_current_page(Some(*idx));
        }
    }

    pub fn select_by_index(&self, idx: u32) {
        self.notebook.set_current_page(Some(idx));
    }

    pub fn n_tabs(&self) -> u32 {
        self.notebook.n_pages()
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn default_title(key: TabKey, name: Option<&str>) -> String {
        match key {
            TabKey::Tmux(pane) => match name {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => pane.as_str(),
            },
            TabKey::Local(_) => "shell".into(),
        }
    }

    pub fn next_local_tab_key(&self) -> TabKey {
        TabKey::Local(self.next_local)
    }
}
