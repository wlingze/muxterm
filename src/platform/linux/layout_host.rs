//! 从 FFI 布局树构建 / 更新 GTK4 Paned。

use std::cell::Cell;
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
    /// 当前布局结构签名（不含 ratio），用于区分「结构变化」与「仅 ratio 变化」。
    last_structure_sig: String,
    /// 每个 Paned 当前 ratio（permille），供 resize 绑定与 in-place 更新共享。
    split_ratios: HashMap<Paned, Rc<Cell<u32>>>,
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
            last_structure_sig: String::new(),
            split_ratios: HashMap::new(),
            fullscreen_pane: None,
        }
    }

    pub fn pane(&self, id: u32) -> Option<&Rc<PaneView>> {
        self.panes.get(&id)
    }

    /// 测试用：已创建的 pane id（含后台 tab 像素缓存）。
    pub fn pane_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.panes.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// 测试用：立刻 flush 所有 VTE 合并缓冲。
    pub fn flush_all_feeds(&self) {
        for view in self.panes.values() {
            view.flush_pending_feed();
        }
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
            self.last_structure_sig.clear();
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

    /// 若布局结构变化则重建 GTK 树；仅 ratio 变化时 in-place 更新 Paned 位置。
    /// 返回是否重建。
    ///
    /// W5：**不**因换 Tab 而 `retain` 掉其它 pane 控件——pane 是像素缓存，
    /// 切回时 VTE 内容与滚动位置必须还在。pane 只在工作区关闭时整体释放。
    ///
    /// 重建会 unparent/reparent VTE widget（unrealize → VTE 停止处理已排队
    /// 的 feed），所以 ratio 变化不能走重建：tmux 每次 ResizeClient 都会微调
    /// split ratio，重建会让 attach 快照永远停在 VTE 队列里（1820 白屏）。
    pub fn apply_layout<F>(&mut self, layout: &LayoutNode, on_input: &F) -> bool
    where
        F: Fn(u32, &[u8]) + Clone + 'static,
    {
        let effective = match self.fullscreen_pane {
            Some(id) => LayoutNode::Leaf(PaneId(id)),
            None => layout.clone(),
        };
        // GtkWidget 同一时刻只能有一个 parent。Runtime 给出重复 leaf 时保留
        // 当前好布局并拒绝这帧，避免 gtk_paned_set_end_child critical。
        let mut leaf_ids = Vec::new();
        collect_pane_ids(&effective, &mut leaf_ids);
        let unique_leaf_count = leaf_ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len();
        if unique_leaf_count != leaf_ids.len() {
            tracing::error!(
                target: "muxterm::layout",
                leaves = ?leaf_ids,
                "reject layout with duplicate pane leaves"
            );
            return false;
        }
        let sig = layout_signature(&effective);
        let structure_sig = layout_structure_signature(&effective);
        if structure_sig == self.last_structure_sig {
            if sig != self.last_sig {
                // 仅 ratio 变化：更新 Paned 位置，不重建 GTK 树。
                tracing::info!(
                    target: "muxterm::layout",
                    sig = %sig,
                    "layout ratio update (no rebuild)"
                );
                self.last_sig = sig;
                self.update_split_positions(&effective);
            }
            return false;
        }
        tracing::info!(
            target: "muxterm::layout",
            sig = %sig,
            "layout rebuild"
        );
        self.last_sig = sig;
        self.last_structure_sig = structure_sig;
        self.split_ratios.clear();

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
        // 后挂载的布局（SSH attach 切工作区）也要让 VTE 撑满 root_box：
        // 显式 expand + queue_resize，避免新 child 在已分配 Box 里拿 0 尺寸。
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        self.root_box.append(&widget);
        self.root_box.queue_resize();
        true
    }

    /// 切换连接时清空布局树，保留 root_box 在窗口里的位置。
    pub fn reset(&mut self, is_tmux_mirror: bool) {
        self.is_tmux_mirror = is_tmux_mirror;
        self.fullscreen_pane = None;
        self.last_sig.clear();
        self.last_structure_sig.clear();
        self.split_ratios.clear();
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

    /// 当前字号（C8：activate 时按尺寸差补后台 cache）。
    pub fn font_size(&self) -> f32 {
        self.font.size
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

    /// 仅 ratio 变化时，沿现有 GTK 树更新 Paned 位置（不重建、不 unparent）。
    fn update_split_positions(&self, layout: &LayoutNode) {
        if let Some(root) = self.root_box.first_child() {
            update_split_positions_walk(&root, layout, &self.split_ratios);
        }
    }

    fn build_widget(&mut self, layout: &LayoutNode) -> Widget {
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
                let ratio_cell = Rc::new(Cell::new(u32::from(*ratio)));
                self.split_ratios.insert(paned.clone(), ratio_cell.clone());
                bind_split_position(&paned, horizontal, ratio_cell);
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

fn bind_split_position(paned: &Paned, horizontal: bool, ratio: Rc<Cell<u32>>) {
    apply_split_position(paned, horizontal, ratio.get());
    let p = paned.clone();
    let ratio2 = ratio.clone();
    paned.connect_notify_local(Some("width"), move |_, _| {
        apply_split_position(&p, horizontal, ratio2.get());
    });
    let p = paned.clone();
    let ratio3 = ratio.clone();
    paned.connect_notify_local(Some("height"), move |_, _| {
        apply_split_position(&p, horizontal, ratio3.get());
    });
}

fn update_split_positions_walk(
    widget: &Widget,
    layout: &LayoutNode,
    ratios: &HashMap<Paned, Rc<Cell<u32>>>,
) {
    match layout {
        LayoutNode::Leaf(_) => {}
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            if let Ok(paned) = widget.clone().downcast::<Paned>() {
                if let Some(cell) = ratios.get(&paned) {
                    cell.set(u32::from(*ratio));
                    let horizontal = matches!(dir, SplitDir::Horizontal);
                    apply_split_position(&paned, horizontal, cell.get());
                }
                if let Some(start) = paned.start_child() {
                    update_split_positions_walk(&start, first, ratios);
                }
                if let Some(end) = paned.end_child() {
                    update_split_positions_walk(&end, second, ratios);
                }
            }
        }
    }
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

/// 布局结构签名：只含树形与 pane id，不含 ratio。
///
/// ratio 变化（tmux ResizeClient 后的微调）不应触发 GTK 树重建，否则
/// VTE widget 被 unparent/reparent 后停止处理已排队的 feed（1820 白屏）。
fn layout_structure_signature(layout: &LayoutNode) -> String {
    match layout {
        LayoutNode::Leaf(pane_id) => format!("L{}", pane_id.0),
        LayoutNode::Split {
            dir, first, second, ..
        } => format!(
            "S{}:{}:{}",
            if matches!(dir, SplitDir::Horizontal) {
                "H"
            } else {
                "V"
            },
            layout_structure_signature(first),
            layout_structure_signature(second)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Rgb;

    #[test]
    fn split_position_uses_ratio_not_one_pixel() {
        assert_eq!(split_position_px(1000, 500), 500);
        assert_eq!(split_position_px(800, 250), 200);
        assert_eq!(split_position_px(100, 0), 1);
        assert_eq!(split_position_px(100, 1000), 99);
        assert_ne!(split_position_px(640, 500), 1);
    }

    /// Runtime 边界若意外给出重复 leaf，LayoutHost 必须保留旧树并拒绝，
    /// 不能把同一个 VTE 同时塞进 GtkPaned 两边触发 GTK critical。
    #[test]
    fn duplicate_pane_layout_is_rejected_before_gtk_parenting() {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            eprintln!("skip: 无 DISPLAY");
            return;
        }
        gtk4::test_synced(|| {
            let theme = Theme {
                name: "test".into(),
                background: Rgb(0, 0, 0),
                foreground: Rgb(255, 255, 255),
                cursor: Rgb(255, 255, 255),
                colors: [Rgb(0, 0, 0); 16],
            };
            let mut host = LayoutHost::new(theme, FontSettings::default(), true, 100);
            let duplicate = LayoutNode::Split {
                dir: SplitDir::Horizontal,
                ratio: 500,
                first: Box::new(LayoutNode::Leaf(PaneId(7))),
                second: Box::new(LayoutNode::Leaf(PaneId(7))),
            };
            let rebuilt = host.apply_layout(&duplicate, &|_, _| {});
            assert!(!rebuilt, "重复 pane leaf 必须在 GTK parenting 前被拒绝");
            assert!(
                host.root_box.first_child().is_none(),
                "无旧布局时拒绝重复 leaf 后 root 必须保持空"
            );
        });
    }
}
