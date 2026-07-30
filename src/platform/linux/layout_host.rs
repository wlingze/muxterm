//! 从 FFI 布局树构建 / 更新 GTK4 Paned。

use std::collections::HashMap;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Orientation, Paned, Widget};

use crate::config::Theme;
use crate::platform::linux::ffi_bridge::BridgeLayout;
use crate::platform::linux::pane_view::PaneView;

/// 布局根：持有 pane_id → PaneView，以及当前根 widget。
pub struct LayoutHost {
    pub root_box: gtk4::Box,
    panes: HashMap<u32, Rc<PaneView>>,
    theme: Theme,
    /// 当前布局签名，用于 damage tracking（只在变化时重建）。
    last_sig: String,
}

impl LayoutHost {
    pub fn new(theme: Theme) -> Self {
        let root_box = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        Self {
            root_box,
            panes: HashMap::new(),
            theme,
            last_sig: String::new(),
        }
    }

    pub fn pane(&self, id: u32) -> Option<&Rc<PaneView>> {
        self.panes.get(&id)
    }

    pub fn panes_mut(&mut self) -> &mut HashMap<u32, Rc<PaneView>> {
        &mut self.panes
    }

    pub fn ensure_pane<F>(&mut self, id: u32, on_input: &F) -> Rc<PaneView>
    where
        F: Fn(u32, &[u8]) + Clone + 'static,
    {
        if let Some(p) = self.panes.get(&id) {
            return p.clone();
        }
        let view = Rc::new(PaneView::new(id, &self.theme));
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
        let sig = layout_signature(layout);
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
        collect_pane_ids(layout, &mut needed);
        // 创建缺失 pane
        for id in &needed {
            self.ensure_pane(*id, on_input);
        }
        // 移除不再需要的
        self.panes.retain(|id, _| needed.contains(id));

        let widget = self.build_widget(layout);
        self.root_box.append(&widget);
        true
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
                // ratio 0..=1000 → position 百分比近似
                let pos = ((*ratio).min(1000) as f64) / 1000.0;
                paned.set_position(1); // 先设非零避免 0
                                       // GTK4：用 wide-handle；位置在 realize 后更准，这里近似
                let _ = pos;
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
