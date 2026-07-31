//! Notebook tab 管理。
//!
//! 术语（与 tmux 对照）：
//! - **Tab**：我们 app 底部/顶部的标签（一个可切换页面）
//! - **Pane**：Tab 内部的分割区域（vte4 Terminal）
//! - tmux **window** ↔ 我们的 **Tab**
//! - tmux **pane** ↔ 我们的 **Pane**（同一 window 内多个 pane 嵌套分割）
//!
//! 分割布局：嵌套 `GtkPaned`（每次在当前激活 pane 内分割，不是整 tab 平铺）。

use std::collections::HashMap;

use gtk4::prelude::*;
use gtk4::{Label, Notebook, Orientation, Paned, PositionType, Widget};

use crate::core::runtime::tmux::protocol::{PaneId, WindowId};
use crate::platform::linux::pane_view::PaneView;

/// 我们 app 的 Tab 标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabKey {
    /// 本地程序 tab。
    Local(u64),
    /// 对应一个 tmux window（不是 pane）。
    TmuxWindow(WindowId),
}

/// 一个本地 pane 在 tab 内的序号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalPaneId(pub u64);

/// 我们 app 的 Pane 标识（跨 local / tmux）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneKey {
    Local(LocalPaneId),
    Tmux(PaneId),
}

/// 分割方向（与 GTK Orientation 对应：Horizontal=左右，Vertical=上下）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitOrient {
    Horizontal,
    Vertical,
}

impl SplitOrient {
    pub fn to_gtk(self) -> Orientation {
        match self {
            SplitOrient::Horizontal => Orientation::Horizontal,
            SplitOrient::Vertical => Orientation::Vertical,
        }
    }

    pub fn from_gtk(o: Orientation) -> Self {
        match o {
            Orientation::Vertical => SplitOrient::Vertical,
            _ => SplitOrient::Horizontal,
        }
    }
}

/// 嵌套 pane 布局树。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneNode {
    Leaf(PaneKey),
    Split {
        orientation: SplitOrient,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    pub fn leaf(pane: PaneKey) -> Self {
        PaneNode::Leaf(pane)
    }

    /// 前序遍历收集所有叶节点。
    pub fn leaves(&self) -> Vec<PaneKey> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneKey>) {
        match self {
            PaneNode::Leaf(k) => out.push(*k),
            PaneNode::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    /// 在 `target` 叶节点处嵌套分割：原 pane 为 first，新 pane 为 second。
    pub fn split_leaf(
        &mut self,
        target: PaneKey,
        new_pane: PaneKey,
        orientation: SplitOrient,
    ) -> bool {
        match self {
            PaneNode::Leaf(k) if *k == target => {
                *self = PaneNode::Split {
                    orientation,
                    first: Box::new(PaneNode::Leaf(target)),
                    second: Box::new(PaneNode::Leaf(new_pane)),
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { first, second, .. } => {
                first.split_leaf(target, new_pane, orientation)
                    || second.split_leaf(target, new_pane, orientation)
            }
        }
    }

    /// 移除叶节点并折叠：若某侧被移除，整棵 Split 收缩为另一侧。
    /// 返回 false 表示未找到；若移除后树为空则不应发生（至少留一个 leaf）。
    pub fn remove_leaf(&mut self, target: PaneKey) -> bool {
        match self {
            PaneNode::Leaf(k) => *k == target, // 调用方处理「根就是该叶」
            PaneNode::Split { first, second, .. } => {
                if matches!(first.as_ref(), PaneNode::Leaf(k) if *k == target) {
                    *self = second.as_ref().clone();
                    return true;
                }
                if matches!(second.as_ref(), PaneNode::Leaf(k) if *k == target) {
                    *self = first.as_ref().clone();
                    return true;
                }
                if first.remove_leaf(target) {
                    // 若子树变成「空」不可能；但若 first 自身是被删的 leaf 已在上面处理
                    return true;
                }
                second.remove_leaf(target)
            }
        }
    }

    /// 根是目标叶时返回 true（整 tab 应关闭）。
    pub fn is_leaf(&self, key: PaneKey) -> bool {
        matches!(self, PaneNode::Leaf(k) if *k == key)
    }
}

/// tmux window_layout 解析出的树（纯数据，便于单测）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutNode {
    Leaf(u32),
    /// `vertical=false` → `{...}` 左右；`true` → `[...]` 上下。
    Split {
        vertical: bool,
        children: Vec<LayoutNode>,
    },
}

impl LayoutNode {
    pub fn leaves(&self) -> Vec<u32> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<u32>) {
        match self {
            LayoutNode::Leaf(id) => out.push(*id),
            LayoutNode::Split { children, .. } => {
                for c in children {
                    c.collect_leaves(out);
                }
            }
        }
    }

    /// 转成二叉 PaneNode（多子节点时左结合嵌套）。
    pub fn to_pane_node(&self) -> PaneNode {
        match self {
            LayoutNode::Leaf(id) => PaneNode::Leaf(PaneKey::Tmux(PaneId(*id))),
            LayoutNode::Split { vertical, children } => {
                let orient = if *vertical {
                    SplitOrient::Vertical
                } else {
                    SplitOrient::Horizontal
                };
                let mut nodes: Vec<PaneNode> = children.iter().map(|c| c.to_pane_node()).collect();
                if nodes.is_empty() {
                    return PaneNode::Leaf(PaneKey::Tmux(PaneId(0)));
                }
                let mut acc = nodes.remove(0);
                for n in nodes {
                    acc = PaneNode::Split {
                        orientation: orient,
                        first: Box::new(acc),
                        second: Box::new(n),
                    };
                }
                acc
            }
        }
    }
}

/// 解析 tmux `window_layout` 字符串为嵌套树。
///
/// 格式：`[<checksum>,]<WxH>,<X>,<Y>,<pane_id>` 或
/// `...{child,child}`（左右）/ `...[child,child]`（上下）。
pub fn parse_layout_tree(layout: &str) -> Option<LayoutNode> {
    let bytes = layout.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'x' {
            let mut k = j + 1;
            if k < bytes.len() && bytes[k].is_ascii_digit() {
                while k < bytes.len() && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                if let Some((node, _)) = parse_layout_node(&layout[i..]) {
                    return Some(node);
                }
            }
        }
        i += 1;
    }
    None
}

/// 从 `WxH,X,Y...` 起解析一个节点，返回 (节点, 剩余)。
fn parse_layout_node(s: &str) -> Option<(LayoutNode, &str)> {
    let (w, rest) = split_digits(s)?;
    let rest = rest.strip_prefix('x')?;
    let (_h, rest) = split_digits(rest)?;
    let rest = rest.strip_prefix(',')?;
    let (_x, rest) = split_digits(rest)?;
    let rest = rest.strip_prefix(',')?;
    let (_y, rest) = split_digits(rest)?;

    if let Some(rest) = rest.strip_prefix('{') {
        let (children, rest) = parse_layout_children(rest, '}')?;
        return Some((
            LayoutNode::Split {
                vertical: false,
                children,
            },
            rest,
        ));
    }
    if let Some(rest) = rest.strip_prefix('[') {
        let (children, rest) = parse_layout_children(rest, ']')?;
        return Some((
            LayoutNode::Split {
                vertical: true,
                children,
            },
            rest,
        ));
    }
    // 叶：,pane_id
    let rest = rest.strip_prefix(',')?;
    let (id, rest) = split_digits(rest)?;
    let id: u32 = id.parse().ok()?;
    let _ = (w,); // 几何宽仅用于跳过
    Some((LayoutNode::Leaf(id), rest))
}

fn parse_layout_children(mut s: &str, end: char) -> Option<(Vec<LayoutNode>, &str)> {
    let mut children = Vec::new();
    loop {
        if s.starts_with(end) {
            return Some((children, &s[end.len_utf8()..]));
        }
        if children.is_empty() || s.starts_with(',') {
            if s.starts_with(',') {
                s = &s[1..];
            }
            let (node, rest) = parse_layout_node(s)?;
            children.push(node);
            s = rest;
        } else {
            return None;
        }
    }
}

fn split_digits(s: &str) -> Option<(&str, &str)> {
    let n = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 {
        return None;
    }
    Some((&s[..n], &s[n..]))
}

/// 一个 Tab 的内容：嵌套 pane 树。
#[derive(Clone)]
pub struct TabContent {
    pub tree: PaneNode,
    /// 当前激活 pane（焦点所在）。
    pub active: Option<PaneKey>,
    /// 根 widget（随分割变化重建）。
    pub root: Widget,
}

impl TabContent {
    pub fn single(pane: PaneKey, terminal: &vte4::Terminal) -> Self {
        let root = terminal.clone().upcast::<Widget>();
        TabContent {
            tree: PaneNode::leaf(pane),
            active: Some(pane),
            root,
        }
    }

    pub fn panes(&self) -> Vec<PaneKey> {
        self.tree.leaves()
    }
}

/// tab 管理器。
pub struct PaneNotebook {
    pub notebook: Notebook,
    /// tab key → (页面索引, TabContent)
    pub(crate) tabs: HashMap<TabKey, (u32, TabContent)>,
    next_local: u64,
}

impl Default for PaneNotebook {
    fn default() -> Self {
        Self::new()
    }
}

impl PaneNotebook {
    pub fn new() -> Self {
        let nb = Notebook::new();
        nb.set_show_tabs(false);
        nb.set_show_border(false);
        nb.set_tab_pos(PositionType::Bottom);
        nb.set_scrollable(false);
        nb.set_margin_start(0);
        nb.set_margin_end(0);
        nb.set_margin_top(0);
        nb.set_margin_bottom(0);
        Self {
            notebook: nb,
            tabs: HashMap::new(),
            next_local: 0,
        }
    }

    /// 按页面索引顺序返回所有 tab key。
    pub fn keys_in_order(&self) -> Vec<TabKey> {
        let n = self.notebook.n_pages();
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            if let Some(k) = self.find_key_by_index(i) {
                out.push(k);
            }
        }
        out
    }

    /// 新建一个本地 tab，内含一个 pane。返回 (tab key, pane key)。
    pub fn add_local_tab(&mut self, view: &PaneView, title: &str) -> (TabKey, PaneKey) {
        let tab_key = TabKey::Local(self.next_local);
        self.next_local += 1;
        let pane_key = PaneKey::Local(LocalPaneId(0));
        let content = TabContent::single(pane_key, view.terminal());
        let widget = content.root.clone();
        let idx = self.append(&widget, title);
        self.tabs.insert(tab_key, (idx, content));
        self.notebook.set_current_page(Some(idx));
        (tab_key, pane_key)
    }

    /// 确保存在对应 tmux window 的 tab；若无则用首个 pane 创建。
    pub fn ensure_tmux_window_tab(
        &mut self,
        window: WindowId,
        first_pane: &PaneView,
        title: &str,
    ) -> TabKey {
        let key = TabKey::TmuxWindow(window);
        if self.tabs.contains_key(&key) {
            return key;
        }
        let pane_key = PaneKey::Tmux(PaneId(first_pane.pane_id()));
        let content = TabContent::single(pane_key, first_pane.terminal());
        let widget = content.root.clone();
        let idx = self.append(&widget, title);
        self.tabs.insert(key, (idx, content));
        self.notebook.set_current_page(Some(idx));
        key
    }

    fn append(&self, widget: &Widget, title: &str) -> u32 {
        let label = Label::new(Some(title));
        self.notebook.append_page(widget, Some(&label))
    }

    /// 用完整布局树替换 tab 内容并安全重建嵌套 Paned。
    pub fn set_tree_and_relayout(
        &mut self,
        tab: TabKey,
        tree: PaneNode,
        active: Option<PaneKey>,
        terminals: &HashMap<PaneKey, vte4::Terminal>,
        title: &str,
    ) {
        if let Some((_, content)) = self.tabs.get_mut(&tab) {
            content.tree = tree;
            let leaves = content.tree.leaves();
            content.active = active
                .filter(|a| leaves.contains(a))
                .or_else(|| leaves.first().copied());
        }
        self.relayout_tab(tab, terminals, title);
    }

    /// 在当前激活叶上嵌套分割后重建。
    pub fn split_and_relayout(
        &mut self,
        tab: TabKey,
        target: PaneKey,
        new_pane: PaneKey,
        orientation: SplitOrient,
        terminals: &HashMap<PaneKey, vte4::Terminal>,
        title: &str,
    ) -> bool {
        let ok = if let Some((_, content)) = self.tabs.get_mut(&tab) {
            content.tree.split_leaf(target, new_pane, orientation)
        } else {
            false
        };
        if ok {
            if let Some((_, content)) = self.tabs.get_mut(&tab) {
                // 焦点留在原 pane（first / 左或上）
                content.active = Some(target);
            }
            self.relayout_tab(tab, terminals, title);
        }
        ok
    }

    /// 从树中移除 pane 并重建；若 tab 变空返回 true。
    pub fn remove_pane_and_relayout(
        &mut self,
        tab: TabKey,
        pane: PaneKey,
        terminals: &HashMap<PaneKey, vte4::Terminal>,
        title: &str,
    ) -> bool {
        let Some((_, content)) = self.tabs.get_mut(&tab) else {
            return true;
        };
        if content.tree.is_leaf(pane) {
            return true; // 调用方应关 tab
        }
        if !content.tree.remove_leaf(pane) {
            return false;
        }
        let leaves = content.tree.leaves();
        content.active = content
            .active
            .filter(|a| *a != pane && leaves.contains(a))
            .or_else(|| leaves.first().copied());
        self.relayout_tab(tab, terminals, title);
        false
    }

    /// 安全重建：先 `remove_page` → `unparent` 每个 terminal → 嵌套 Paned → `insert_page`。
    pub fn relayout_tab(
        &mut self,
        tab: TabKey,
        terminals: &HashMap<PaneKey, vte4::Terminal>,
        title: &str,
    ) {
        let Some(idx) = self.tabs.get(&tab).map(|(i, _)| *i) else {
            return;
        };
        let tree = match self.tabs.get(&tab) {
            Some((_, c)) => c.tree.clone(),
            None => return,
        };
        let leaves = tree.leaves();
        if leaves.is_empty() {
            return;
        }

        self.notebook.remove_page(Some(idx));

        for pk in &leaves {
            if let Some(t) = terminals.get(pk) {
                if t.parent().is_some() {
                    t.unparent();
                }
            }
        }

        let root = build_pane_paned(&tree, terminals);
        let active = self
            .tabs
            .get(&tab)
            .and_then(|(_, c)| c.active)
            .or_else(|| leaves.first().copied());
        let content = TabContent {
            tree,
            active,
            root: root.clone(),
        };

        let label = Label::new(Some(title));
        let new_idx = self.notebook.insert_page(&root, Some(&label), Some(idx));
        self.tabs.insert(tab, (new_idx, content));
        self.reindex();
        self.notebook.set_current_page(Some(new_idx));
    }

    /// 移除一个 tab。
    pub fn remove(&mut self, key: TabKey) {
        if let Some((idx, _)) = self.tabs.remove(&key) {
            self.notebook.remove_page(Some(idx));
            self.reindex();
        }
    }

    fn reindex(&mut self) {
        let n = self.notebook.n_pages();
        let mut new_tabs: HashMap<TabKey, (u32, TabContent)> = HashMap::new();
        for i in 0..n {
            if let Some(page) = self.notebook.nth_page(Some(i)) {
                let ptr = page.as_ptr() as usize;
                let found = self.tabs.iter().find_map(|(k, (_, c))| {
                    if c.root.as_ptr() as usize == ptr {
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
        if let Some((_, c)) = self.tabs.get(&key) {
            if let Some(label) = self
                .notebook
                .tab_label(&c.root)
                .and_then(|l| l.downcast::<Label>().ok())
            {
                label.set_label(title);
            }
        }
    }

    pub fn current_key(&self) -> Option<TabKey> {
        let idx = self.notebook.current_page()?;
        self.find_key_by_index(idx)
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

    pub fn contains(&self, key: TabKey) -> bool {
        self.tabs.contains_key(&key)
    }

    pub fn default_title(key: TabKey, name: Option<&str>) -> String {
        match key {
            TabKey::TmuxWindow(w) => match name {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => format!("@{}", w.0),
            },
            TabKey::Local(_) => "shell".into(),
        }
    }
}

/// 用嵌套 GtkPaned 构建布局。先 unparent 再挂载。
fn build_pane_paned(node: &PaneNode, terminals: &HashMap<PaneKey, vte4::Terminal>) -> Widget {
    match node {
        PaneNode::Leaf(pk) => {
            let term = terminals
                .get(pk)
                .expect("build_pane_paned: 缺 terminal")
                .clone();
            term.set_hexpand(true);
            term.set_vexpand(true);
            term.add_css_class("pane-terminal");
            term.upcast()
        }
        PaneNode::Split {
            orientation,
            first,
            second,
        } => {
            let orient = *orientation;
            let paned = Paned::new(orient.to_gtk());
            paned.add_css_class("pane-split");
            paned.set_wide_handle(true);
            paned.set_hexpand(true);
            paned.set_vexpand(true);
            let a = build_pane_paned(first, terminals);
            let b = build_pane_paned(second, terminals);
            paned.set_start_child(Some(&a));
            paned.set_end_child(Some(&b));
            paned.set_resize_start_child(true);
            paned.set_resize_end_child(true);
            paned.set_shrink_start_child(false);
            paned.set_shrink_end_child(false);
            // 默认对半；realize 后按分配尺寸设 position
            let paned_pos = paned.clone();
            paned.connect_realize(move |p| {
                let alloc = p.allocation();
                let mid = match orient.to_gtk() {
                    Orientation::Vertical => alloc.height() / 2,
                    _ => alloc.width() / 2,
                };
                if mid > 0 {
                    paned_pos.set_position(mid);
                }
            });
            paned.upcast()
        }
    }
}

/// 从 tmux window_layout 原始字符串提取 pane id 列表（兼容旧调用）。
pub fn extract_pane_ids_from_layout(layout: &str) -> Vec<u32> {
    parse_layout_tree(layout)
        .map(|n| n.leaves())
        .unwrap_or_default()
}

/// 根据 layout 字符串猜测根分割方向。
pub fn layout_orientation(layout: &str) -> Orientation {
    match parse_layout_tree(layout) {
        Some(LayoutNode::Split { vertical: true, .. }) => Orientation::Vertical,
        _ => Orientation::Horizontal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_single_pane() {
        assert_eq!(extract_pane_ids_from_layout("a87e,100x30,0,0,1"), vec![1]);
        assert_eq!(extract_pane_ids_from_layout("80x24,0,0,0"), vec![0]);
    }

    #[test]
    fn extract_split_panes() {
        let layout = "abc,100x30,0,0{50x30,0,0,2,50x30,50,0,3}";
        assert_eq!(extract_pane_ids_from_layout(layout), vec![2, 3]);
    }

    #[test]
    fn parse_nested_layout() {
        // 左：上下两 pane；右：一个 pane
        let layout = "x,100x30,0,0{50x30,0,0[50x15,0,0,1,50x15,0,15,2],50x30,50,0,3}";
        let tree = parse_layout_tree(layout).expect("parse");
        assert_eq!(tree.leaves(), vec![1, 2, 3]);
        match &tree {
            LayoutNode::Split {
                vertical: false,
                children,
            } => {
                assert_eq!(children.len(), 2);
                assert!(matches!(
                    &children[0],
                    LayoutNode::Split { vertical: true, .. }
                ));
                assert_eq!(children[1], LayoutNode::Leaf(3));
            }
            other => panic!("unexpected {other:?}"),
        }
        let pane_tree = tree.to_pane_node();
        assert_eq!(
            pane_tree.leaves(),
            vec![
                PaneKey::Tmux(PaneId(1)),
                PaneKey::Tmux(PaneId(2)),
                PaneKey::Tmux(PaneId(3))
            ]
        );
    }

    #[test]
    fn orientation_brackets() {
        assert_eq!(
            layout_orientation("a,10x10,0,0{5x10,0,0,1,5x10,5,0,2}"),
            Orientation::Horizontal
        );
        assert_eq!(
            layout_orientation("a,10x10,0,0[10x5,0,0,1,10x5,0,5,2]"),
            Orientation::Vertical
        );
    }

    #[test]
    fn nested_split_leaf() {
        let a = PaneKey::Local(LocalPaneId(0));
        let b = PaneKey::Local(LocalPaneId(1));
        let c = PaneKey::Local(LocalPaneId(2));
        let mut tree = PaneNode::leaf(a);
        assert!(tree.split_leaf(a, b, SplitOrient::Horizontal));
        // 再在左边（a）竖直分割
        assert!(tree.split_leaf(a, c, SplitOrient::Vertical));
        assert_eq!(tree.leaves(), vec![a, c, b]);
        match &tree {
            PaneNode::Split {
                orientation: SplitOrient::Horizontal,
                first,
                second,
            } => {
                assert_eq!(**second, PaneNode::Leaf(b));
                match first.as_ref() {
                    PaneNode::Split {
                        orientation: SplitOrient::Vertical,
                        first: f2,
                        second: s2,
                    } => {
                        assert_eq!(**f2, PaneNode::Leaf(a));
                        assert_eq!(**s2, PaneNode::Leaf(c));
                    }
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn remove_leaf_collapses() {
        let a = PaneKey::Local(LocalPaneId(0));
        let b = PaneKey::Local(LocalPaneId(1));
        let c = PaneKey::Local(LocalPaneId(2));
        let mut tree = PaneNode::leaf(a);
        tree.split_leaf(a, b, SplitOrient::Horizontal);
        tree.split_leaf(a, c, SplitOrient::Vertical);
        assert!(tree.remove_leaf(c));
        assert_eq!(tree.leaves(), vec![a, b]);
        assert!(tree.remove_leaf(b));
        assert_eq!(tree, PaneNode::Leaf(a));
    }

    fn lp(n: u64) -> PaneKey {
        PaneKey::Local(LocalPaneId(n))
    }

    /// 对应：嵌套分割——在激活 pane 内分割，另一侧不变（Bug #2）。
    #[test]
    fn test_panenode_nested_split_keeps_sibling() {
        let a = lp(0);
        let b = lp(1);
        let c = lp(2);
        let mut tree = PaneNode::leaf(a);
        assert!(tree.split_leaf(a, b, SplitOrient::Horizontal));
        assert_eq!(tree.leaves().len(), 2);
        // 焦点在 a，再竖直分割 → 只有 a 侧变，b 不变
        assert!(tree.split_leaf(a, c, SplitOrient::Vertical));
        assert_eq!(tree.leaves(), vec![a, c, b]);
        match &tree {
            PaneNode::Split {
                orientation: SplitOrient::Horizontal,
                second,
                ..
            } => assert_eq!(**second, PaneNode::Leaf(b)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn test_panenode_split_horizontal_root() {
        let a = lp(0);
        let b = lp(1);
        let mut tree = PaneNode::leaf(a);
        assert!(tree.split_leaf(a, b, SplitOrient::Horizontal));
        assert!(matches!(
            tree,
            PaneNode::Split {
                orientation: SplitOrient::Horizontal,
                ..
            }
        ));
    }

    #[test]
    fn test_panenode_split_vertical_root() {
        let a = lp(0);
        let b = lp(1);
        let mut tree = PaneNode::leaf(a);
        assert!(tree.split_leaf(a, b, SplitOrient::Vertical));
        assert!(matches!(
            tree,
            PaneNode::Split {
                orientation: SplitOrient::Vertical,
                ..
            }
        ));
    }

    #[test]
    fn test_panenode_split_missing_target_false() {
        let mut tree = PaneNode::leaf(lp(0));
        assert!(!tree.split_leaf(lp(9), lp(1), SplitOrient::Horizontal));
        assert_eq!(tree.leaves().len(), 1);
    }

    #[test]
    fn test_panenode_remove_from_nested_three() {
        let a = lp(0);
        let b = lp(1);
        let c = lp(2);
        let mut tree = PaneNode::leaf(a);
        tree.split_leaf(a, b, SplitOrient::Horizontal);
        tree.split_leaf(b, c, SplitOrient::Vertical);
        // 删中间层一侧的 b：树应折叠
        assert!(tree.remove_leaf(b));
        assert_eq!(tree.leaves(), vec![a, c]);
    }

    #[test]
    fn test_panenode_pane_count_1_to_4() {
        let mut tree = PaneNode::leaf(lp(0));
        assert_eq!(tree.leaves().len(), 1);
        tree.split_leaf(lp(0), lp(1), SplitOrient::Horizontal);
        assert_eq!(tree.leaves().len(), 2);
        tree.split_leaf(lp(0), lp(2), SplitOrient::Vertical);
        assert_eq!(tree.leaves().len(), 3);
        tree.split_leaf(lp(1), lp(3), SplitOrient::Horizontal);
        assert_eq!(tree.leaves().len(), 4);
    }

    /// 对应：连续多次不同方向分割不崩，树深度可用。
    #[test]
    fn test_panenode_five_alternating_splits() {
        let mut tree = PaneNode::leaf(lp(0));
        let orients = [
            SplitOrient::Horizontal,
            SplitOrient::Vertical,
            SplitOrient::Horizontal,
            SplitOrient::Vertical,
            SplitOrient::Horizontal,
        ];
        for (i, o) in orients.iter().enumerate() {
            let target = tree.leaves()[0];
            assert!(tree.split_leaf(target, lp((i + 1) as u64), *o));
        }
        assert_eq!(tree.leaves().len(), 6);
    }

    /// 对应：大量 pane（10 次分割）后 leaves 数量正确。
    #[test]
    fn test_panenode_ten_splits_count() {
        let mut tree = PaneNode::leaf(lp(0));
        for i in 0..10 {
            let target = *tree.leaves().last().unwrap();
            assert!(tree.split_leaf(target, lp((i + 1) as u64), SplitOrient::Horizontal));
        }
        assert_eq!(tree.leaves().len(), 11);
    }

    #[test]
    fn test_panenode_is_leaf() {
        let a = lp(0);
        let tree = PaneNode::leaf(a);
        assert!(tree.is_leaf(a));
        assert!(!tree.is_leaf(lp(1)));
    }

    #[test]
    fn test_panenode_switch_index_helpers() {
        // 模拟 switch_pane 循环索引（window 侧逻辑的纯版）
        let panes = [lp(0), lp(1), lp(2)];
        let next = |idx: usize| (idx + 1) % panes.len();
        let prev = |idx: usize| if idx == 0 { panes.len() - 1 } else { idx - 1 };
        assert_eq!(next(0), 1);
        assert_eq!(next(2), 0);
        assert_eq!(prev(0), 2);
        assert_eq!(prev(1), 0);
        // 单 pane：无操作
        let single = [lp(0)];
        assert_eq!(single.len(), 1);
    }

    #[test]
    fn test_notebook_default_title() {
        assert_eq!(PaneNotebook::default_title(TabKey::Local(1), None), "shell");
        assert_eq!(
            PaneNotebook::default_title(TabKey::TmuxWindow(WindowId(3)), Some("bash")),
            "bash"
        );
        assert_eq!(
            PaneNotebook::default_title(TabKey::TmuxWindow(WindowId(3)), None),
            "@3"
        );
    }

    #[test]
    fn test_panenode_remove_missing_false() {
        let mut tree = PaneNode::leaf(lp(0));
        tree.split_leaf(lp(0), lp(1), SplitOrient::Horizontal);
        assert!(!tree.remove_leaf(lp(99)));
    }
}
