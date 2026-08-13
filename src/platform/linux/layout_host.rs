//! 从 FFI 布局树构建 / 更新 GTK4 Paned。

use std::collections::HashMap;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Orientation, Paned, Widget};

use crate::core::config::Theme;
use crate::platform::linux::ffi_bridge::BridgeLayout;
use crate::platform::linux::pane_view::PaneView;
use crate::platform::linux::quickconnect::font::FontSettings;

/// 布局根：持有 pane_id → PaneView，以及当前根 widget。
pub struct LayoutHost {
    pub root_box: gtk4::Box,
    panes: HashMap<u32, Rc<PaneView>>,
    theme: Theme,
    font: FontSettings,
    is_tmux_mirror: bool,
    /// 当前布局签名，用于 damage tracking（只在变化时重建）。
    last_sig: String,
    /// 本地 shell 模式的全屏 pane（tmux 模式由 resize-pane -Z 处理）。
    fullscreen_pane: Option<u32>,
}

impl LayoutHost {
    pub fn new(theme: Theme, font: FontSettings, is_tmux_mirror: bool) -> Self {
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
        ));
        let cb = on_input.clone();
        view.connect_input(move |pid, data| cb(pid, data));
        self.panes.insert(id, view.clone());
        view
    }

    /// 若布局签名变化则重建 GTK 树。返回是否重建。
    pub fn apply_layout<F>(&mut self, layout: &BridgeLayout, on_input: &F) -> bool
    where
        F: Fn(u32, &[u8]) + Clone + 'static,
    {
        let effective = match self.fullscreen_pane {
            Some(id) => BridgeLayout::Leaf { pane_id: id },
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

        // 收集新树需要的 pane id
        let mut needed = Vec::new();
        collect_pane_ids(&effective, &mut needed);
        // 创建缺失 pane
        for id in &needed {
            self.ensure_pane(*id, on_input);
        }
        // 移除不再需要的
        self.panes.retain(|id, _| needed.contains(id));

        let widget = self.build_widget(&effective);
        self.root_box.append(&widget);
        true
    }

    /// 运行期切换主题：所有已有 pane 的 VTE 调色板同步更新。
    pub fn apply_theme(&self, theme: &Theme) {
        for view in self.panes.values() {
            view.apply_theme(theme);
        }
    }

    /// 运行期修改字号（所有已有 pane）。
    pub fn set_font_size(&self, size: f32) {
        for view in self.panes.values() {
            view.set_font_size(size);
        }
    }

    /// 运行期修改字体 family + size。
    pub fn set_font(&self, font: &FontSettings) {
        for view in self.panes.values() {
            view.set_font(font);
        }
    }

    fn build_widget(&self, layout: &BridgeLayout) -> Widget {
        match layout {
            BridgeLayout::Leaf { pane_id } => self
                .panes
                .get(pane_id)
                .map(|p| p.widget())
                .unwrap_or_else(|| {
                    gtk4::Label::new(Some(&format!("?{pane_id}"))).upcast::<Widget>()
                }),
            BridgeLayout::Split {
                horizontal,
                ratio,
                first,
                second,
            } => {
                let orient = if *horizontal {
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
                // ratio 0..=1000 → 位置百分比近似
                let _ratio = (*ratio).min(1000) as f64 / 1000.0;
                paned.set_position(1);
                let _ = _ratio;
                paned.upcast()
            }
        }
    }
}

fn collect_pane_ids(layout: &BridgeLayout, out: &mut Vec<u32>) {
    match layout {
        BridgeLayout::Leaf { pane_id } => out.push(*pane_id),
        BridgeLayout::Split { first, second, .. } => {
            collect_pane_ids(first, out);
            collect_pane_ids(second, out);
        }
    }
}

fn layout_signature(layout: &BridgeLayout) -> String {
    match layout {
        BridgeLayout::Leaf { pane_id } => format!("L{pane_id}"),
        BridgeLayout::Split {
            horizontal,
            ratio,
            first,
            second,
        } => format!(
            "S{}:{}:{}:{}",
            if *horizontal { "H" } else { "V" },
            ratio,
            layout_signature(first),
            layout_signature(second)
        ),
    }
}
