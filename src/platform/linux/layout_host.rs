//! 从 FFI 布局树构建 / 更新 GTK4 Paned。

use std::collections::HashMap;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Orientation, Paned, Widget};

use crate::core::config::Theme;
use crate::core::model::layout::{LayoutNode, SplitDir};
use crate::core::types::PaneId;
use crate::platform::linux::pane_view::PaneView;
use crate::platform::linux::quickconnect::font::FontSettings;

/// 布局根：持有 pane_id → PaneView，以及当前根 widget。
pub struct LayoutHost {
    pub root_box: gtk4::Box,
    panes: HashMap<u32, Rc<PaneView>>,
    theme: Theme,
    font: FontSettings,
    is_tmux_mirror: bool,
    scrollback_lines: u32,
    /// 当前布局签名，用于 damage tracking（只在变化时重建）。
    last_sig: String,
    /// 本地 shell 模式的全屏 pane（tmux 模式由 resize-pane -Z 处理）。
    fullscreen_pane: Option<u32>,
}

impl LayoutHost {
    pub fn new(
        theme: Theme,
        font: FontSettings,
        is_tmux_mirror: bool,
        scrollback_lines: u32,
    ) -> Self {
        let root_box = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        Self {
            root_box,
            panes: HashMap::new(),
            theme,
            font,
            is_tmux_mirror,
            scrollback_lines,
            last_sig: String::new(),
            fullscreen_pane: None,
        }
    }

    pub fn pane(&self, id: u32) -> Option<&Rc<PaneView>> {
        self.panes.get(&id)
    }

    pub fn panes_mut(&mut self) -> &mut HashMap<u32, Rc<PaneView>> {
        &mut self.panes
    }

    pub fn fullscreen_pane(&self) -> Option<u32> {
        self.fullscreen_pane
    }

    pub fn set_fullscreen_pane(&mut self, pane: Option<u32>) {
        if self.fullscreen_pane != pane {
            self.fullscreen_pane = pane;
            self.last_sig.clear(); // 强制重建
        }
    }

    pub fn ensure_pane<F>(&mut self, id: u32, on_input: &F) -> Rc<PaneView>
    where
        F: Fn(u32, &[u8]) + Clone + 'static,
    {
        if let Some(p) = self.panes.get(&id) {
            return p.clone();
        }
        let view = Rc::new(PaneView::new(
            id,
            &self.theme,
            &self.font,
            self.is_tmux_mirror,
            self.scrollback_lines,
        ));
        let cb = on_input.clone();
        view.connect_input(move |pid, data| cb(pid, data));
        self.panes.insert(id, view.clone());
        view
    }

    /// 若布局签名变化则重建 GTK 树。返回是否重建。
    ///
    /// W5：**不**因换 Tab 而 `retain` 掉其它 pane 控件——pane 是像素缓存，
    /// 切回时 VTE 内容与滚动位置必须还在。pane 只在工作区关闭时整体释放。
    pub fn apply_layout<F>(&mut self, layout: &LayoutNode, on_input: &F) -> bool
    where
        F: Fn(u32, &[u8]) + Clone + 'static,
    {
        let effective = match self.fullscreen_pane {
            Some(id) => LayoutNode::Leaf(PaneId(id)),
            None => layout.clone(),
        };
        let sig = layout_signature(&effective);
        if sig == self.last_sig {
            return false;
        }
        self.last_sig = sig;

        // 先把 pane widget 从旧 Paned 摘掉，再清空根（顺序反了会触发 unparent 断言）
        for view in self.panes.values() {
            let w = view.widget();
            if w.parent().is_some() {
                w.unparent();
            }
        }
        while let Some(child) = self.root_box.first_child() {
            self.root_box.remove(&child);
        }

        // 收集新树需要的 pane id，缺失的创建；已有的一律保留（跨 tab 像素缓存）。
        let mut needed = Vec::new();
        collect_pane_ids(&effective, &mut needed);
        for id in &needed {
            self.ensure_pane(*id, on_input);
        }

        let widget = self.build_widget(&effective);
        self.root_box.append(&widget);
        true
    }

    /// 切换连接时清空布局树，保留 root_box 在窗口里的位置。
    pub fn reset(&mut self, is_tmux_mirror: bool) {
        self.is_tmux_mirror = is_tmux_mirror;
        self.fullscreen_pane = None;
        self.last_sig.clear();
        for view in self.panes.values() {
            let w = view.widget();
            if w.parent().is_some() {
                w.unparent();
            }
        }
        self.panes.clear();
        while let Some(child) = self.root_box.first_child() {
            self.root_box.remove(&child);
        }
    }

    /// 运行期切换主题：所有已有 pane 的 VTE 调色板同步更新。
    pub fn apply_theme(&mut self, theme: &Theme) {
        self.theme = theme.clone();
        for view in self.panes.values() {
            view.apply_theme(theme);
        }
    }

    /// 运行期修改字号（所有已有 pane，保留 family）。
    pub fn set_font_size(&mut self, size: f32) {
        self.font.size = size;
        let font = self.font.clone();
        self.set_font(&font);
    }

    /// 运行期修改字体 family + size。
    pub fn set_font(&mut self, font: &FontSettings) {
        self.font = font.clone();
        for view in self.panes.values() {
            view.set_font(font);
        }
    }

    fn build_widget(&self, layout: &LayoutNode) -> Widget {
        match layout {
            LayoutNode::Leaf(pane_id) => self
                .panes
                .get(&pane_id.0)
                .map(|p| p.widget())
                .unwrap_or_else(|| {
                    gtk4::Label::new(Some(&format!("?{}", pane_id.0))).upcast::<Widget>()
                }),
            LayoutNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let horizontal = matches!(dir, SplitDir::Horizontal);
                let orient = if horizontal {
                    Orientation::Horizontal
                } else {
                    Orientation::Vertical
                };
                let paned = Paned::new(orient);
                paned.set_hexpand(true);
                paned.set_vexpand(true);
                let w1 = self.build_widget(first);
                let w2 = self.build_widget(second);
                paned.set_start_child(Some(&w1));
                paned.set_end_child(Some(&w2));
                paned.set_resize_start_child(true);
                paned.set_resize_end_child(true);
                paned.set_shrink_start_child(false);
                paned.set_shrink_end_child(false);
                bind_split_position(&paned, horizontal, u32::from(*ratio));
                paned.upcast()
            }
        }
    }
}

fn collect_pane_ids(layout: &LayoutNode, out: &mut Vec<u32>) {
    match layout {
        LayoutNode::Leaf(pane_id) => out.push(pane_id.0),
        LayoutNode::Split { first, second, .. } => {
            collect_pane_ids(first, out);
            collect_pane_ids(second, out);
        }
    }
}

/// GtkPaned 位置（像素）。`ratio_permille` 为 0..=1000。
pub(crate) fn split_position_px(total: i32, ratio_permille: u32) -> i32 {
    if total <= 2 {
        return 1;
    }
    let pos = i64::from(ratio_permille.min(1000)) * i64::from(total) / 1000;
    pos.clamp(1, i64::from(total - 1)) as i32
}

fn apply_split_position(paned: &Paned, horizontal: bool, ratio_permille: u32) {
    let total = if horizontal {
        paned.width()
    } else {
        paned.height()
    };
    let want = split_position_px(total, ratio_permille);
    if (paned.position() - want).abs() > 2 {
        paned.set_position(want);
    }
}

fn bind_split_position(paned: &Paned, horizontal: bool, ratio_permille: u32) {
    apply_split_position(paned, horizontal, ratio_permille);
    let p = paned.clone();
    paned.connect_notify_local(Some("width"), move |_, _| {
        apply_split_position(&p, horizontal, ratio_permille);
    });
    let p = paned.clone();
    paned.connect_notify_local(Some("height"), move |_, _| {
        apply_split_position(&p, horizontal, ratio_permille);
    });
}

fn layout_signature(layout: &LayoutNode) -> String {
    match layout {
        LayoutNode::Leaf(pane_id) => format!("L{}", pane_id.0),
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => format!(
            "S{}:{}:{}:{}",
            if matches!(dir, SplitDir::Horizontal) {
                "H"
            } else {
                "V"
            },
            ratio,
            layout_signature(first),
            layout_signature(second)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_position_uses_ratio_not_one_pixel() {
        assert_eq!(split_position_px(1000, 500), 500);
        assert_eq!(split_position_px(800, 250), 200);
        assert_eq!(split_position_px(100, 0), 1);
        assert_eq!(split_position_px(100, 1000), 99);
        assert_ne!(split_position_px(640, 500), 1);
    }
}
