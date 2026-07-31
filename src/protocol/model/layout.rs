//! 纯布局树：session / window / pane 嵌套分割模型。
//!
//! Terminal 层的纯数据结构，**无 I/O、无 GUI 依赖**。
//! 由 [`crate::protocol::model::state`] 引用，由各 Backend 构造/同步。
//!
//! 嵌套模型（非平铺）：每次分割只替换当前激活的叶子 pane，不重排其他 pane。
//! 参考 `ARCHITECTURE.md` §2.4。
use crate::core::types::PaneId;

/// 分割方向。
///
/// - `Horizontal`：左右分割（第一个在左，第二个在右）
/// - `Vertical`：上下分割（第一个在上，第二个在下）
///
/// 注意：与 GTK `Orientation` 命名相反（GTK Horizontal = 水平排列 = 左右），
/// 这里沿用 `notebook.rs` 既有语义，避免行为回归。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

/// 布局树节点（二叉嵌套）。
///
/// 与现有 `platform::linux::notebook::PaneNode` 同构，但用 `PaneId`（newtype）
/// 统一标识空间（local / tmux 共用）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LayoutNode {
    /// 叶子节点 = 一个 pane。
    Leaf(PaneId),
    /// 内部节点 = 一次分割。
    Split {
        dir: SplitDir,
        /// 分割比例（0..=1000，first 占比），用于字符格分配。
        /// tmux Backend 从 `window_layout` 推导；LocalBackend 默认 500（各半）。
        ratio: u16,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    /// 单叶子树。
    pub fn leaf(pane: PaneId) -> Self {
        Self::Leaf(pane)
    }

    /// 在 `target` 叶子位置原地分割：原叶子变 `first`，新 pane 作 `second`。
    /// 找不到 `target` 返回 false。
    pub fn split_at(&mut self, target: PaneId, new_pane: PaneId, dir: SplitDir) -> bool {
        match self {
            Self::Leaf(p) if *p == target => {
                *self = Self::Split {
                    dir,
                    ratio: 500,
                    first: Box::new(Self::Leaf(target)),
                    second: Box::new(Self::Leaf(new_pane)),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.split_at(target, new_pane, dir) || second.split_at(target, new_pane, dir)
            }
        }
    }

    /// 移除 `target` 叶子，并把其兄弟节点提升到父节点位置（保持嵌套结构）。
    /// 根节点被移除时返回 `Err(RemoveRoot)`，调用方需决定如何处理。
    pub fn remove(&mut self, target: PaneId) -> Result<(), RemoveRootError> {
        match self {
            Self::Leaf(p) if *p == target => Err(RemoveRootError),
            Self::Leaf(_) => Ok(()),
            Self::Split { first, second, .. } => {
                // first 是目标 → 用 second 替换当前节点
                if matches!(first.as_ref(), Self::Leaf(p) if *p == target) {
                    let replacement = std::mem::replace(
                        second.as_mut(),
                        Self::Leaf(PaneId(u32::MAX)), // 哨兵，立即被覆盖
                    );
                    *self = replacement;
                    return Ok(());
                }
                if matches!(second.as_ref(), Self::Leaf(p) if *p == target) {
                    let replacement =
                        std::mem::replace(first.as_mut(), Self::Leaf(PaneId(u32::MAX)));
                    *self = replacement;
                    return Ok(());
                }
                first.remove(target)?;
                second.remove(target)?;
                Ok(())
            }
        }
    }

    /// 深度优先后序遍历，返回所有叶子 pane id（从左/上到右/下）。
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Leaf(p) => out.push(*p),
            Self::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    /// 是否包含某 pane。
    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            Self::Leaf(p) => *p == pane,
            Self::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// 在叶子序列中找 `target` 的下一个 pane（循环），用于 Alt+]。
    pub fn next_leaf(&self, target: PaneId) -> Option<PaneId> {
        let leaves = self.leaves();
        leaves
            .iter()
            .position(|p| *p == target)
            .map(|i| leaves[(i + 1) % leaves.len()])
    }

    /// 在叶子序列中找 `target` 的上一个 pane（循环），用于 Alt+[。
    pub fn prev_leaf(&self, target: PaneId) -> Option<PaneId> {
        let leaves = self.leaves();
        leaves
            .iter()
            .position(|p| *p == target)
            .map(|i| leaves[(i + leaves.len() - 1) % leaves.len()])
    }

    /// 连续分割深度（叶子到根的最大边数）。
    pub fn depth(&self) -> usize {
        match self {
            Self::Leaf(_) => 0,
            Self::Split { first, second, .. } => 1 + first.depth().max(second.depth()),
        }
    }
}

/// 试图移除根（唯一的）叶子时返回的错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("不能移除布局树唯一的叶子（根）")]
pub struct RemoveRootError;

/// 一个 tab 的布局快照。
///
/// Terminal 层不关心 tab 的像素几何，只关心 pane 拓扑 + 每个 pane 的字符格大小
/// （由 Backend 从 tmux `window_layout` 或本地 vte4 尺寸同步）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TabLayout {
    pub tab: crate::core::types::TabId,
    pub tree: LayoutNode,
    /// 激活 pane。
    pub active: PaneId,
}

/// 兼容别名（过渡期）。
pub type WindowLayout = TabLayout;

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u32) -> PaneId {
        PaneId(n)
    }

    #[test]
    fn leaf_construction_and_leaves() {
        let t = LayoutNode::leaf(p(1));
        assert_eq!(t.leaves(), vec![p(1)]);
        assert!(t.contains(p(1)));
        assert!(!t.contains(p(2)));
        assert_eq!(t.depth(), 0);
    }

    #[test]
    fn split_at_creates_nested_binary() {
        let mut t = LayoutNode::leaf(p(1));
        assert!(t.split_at(p(1), p(2), SplitDir::Horizontal));
        assert_eq!(t.leaves(), vec![p(1), p(2)]);
        // 在 p(1) 上再竖直分割
        assert!(t.split_at(p(1), p(3), SplitDir::Vertical));
        assert_eq!(t.leaves(), vec![p(1), p(3), p(2)]);
        assert_eq!(t.depth(), 2);
    }

    #[test]
    fn split_at_missing_target_returns_false() {
        let mut t = LayoutNode::leaf(p(1));
        assert!(!t.split_at(p(99), p(2), SplitDir::Horizontal));
        assert_eq!(t.leaves(), vec![p(1)]);
    }

    #[test]
    fn remove_collapses_sibling() {
        // ((3|1)|2)
        let mut t = LayoutNode::leaf(p(1));
        t.split_at(p(1), p(2), SplitDir::Horizontal);
        t.split_at(p(1), p(3), SplitDir::Vertical);
        assert_eq!(t.leaves(), vec![p(1), p(3), p(2)]);
        // 移除 p(3) → 树变 (1|2)
        t.remove(p(3)).unwrap();
        assert_eq!(t.leaves(), vec![p(1), p(2)]);
        // 移除 p(2) → 树变 叶 1
        t.remove(p(2)).unwrap();
        assert_eq!(t.leaves(), vec![p(1)]);
    }

    #[test]
    fn remove_root_leaf_errors() {
        let mut t = LayoutNode::leaf(p(1));
        assert_eq!(t.remove(p(1)), Err(RemoveRootError));
    }

    #[test]
    fn remove_missing_is_noop() {
        let mut t = LayoutNode::leaf(p(1));
        t.split_at(p(1), p(2), SplitDir::Horizontal);
        assert!(t.remove(p(99)).is_ok());
        assert_eq!(t.leaves(), vec![p(1), p(2)]);
    }

    #[test]
    fn next_prev_leaf_circular() {
        // (1|(2|3))：先 1|2，再在 2 上竖直分割 → Split(1, Split(2,3))
        let mut t = LayoutNode::leaf(p(1));
        t.split_at(p(1), p(2), SplitDir::Horizontal);
        t.split_at(p(2), p(3), SplitDir::Vertical);
        // leaves = [1, 2, 3]
        let leaves = t.leaves();
        assert_eq!(leaves, vec![p(1), p(2), p(3)]);
        assert_eq!(t.next_leaf(p(1)), Some(p(2)));
        assert_eq!(t.next_leaf(p(2)), Some(p(3)));
        assert_eq!(t.next_leaf(p(3)), Some(p(1))); // 循环
        assert_eq!(t.prev_leaf(p(1)), Some(p(3))); // 循环
        assert_eq!(t.prev_leaf(p(2)), Some(p(1)));
        assert_eq!(t.prev_leaf(p(3)), Some(p(2)));
    }

    #[test]
    fn next_leaf_missing_target_none() {
        let t = LayoutNode::leaf(p(1));
        assert_eq!(t.next_leaf(p(99)), None);
    }

    #[test]
    fn depth_grows_with_splits() {
        let mut t = LayoutNode::leaf(p(1));
        for i in 1..=10u32 {
            let new = p(i + 1);
            assert!(t.split_at(p(i), new, SplitDir::Horizontal));
        }
        assert_eq!(t.depth(), 10); // ≥10 次连续分割不崩溃
        assert_eq!(t.leaves().len(), 11);
    }

    #[test]
    fn window_layout_fields() {
        let t = LayoutNode::leaf(p(1));
        let wl = TabLayout {
            tab: crate::core::types::TabId(1),
            tree: t,
            active: p(1),
        };
        assert_eq!(wl.tab, crate::core::types::TabId(1));
        assert_eq!(wl.active, p(1));
        assert_eq!(wl.tree.leaves(), vec![p(1)]);
    }
}
