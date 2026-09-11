//! Workspace Scene 与其常驻 `LayoutHost` 的统一生命周期 owner。

use std::collections::HashMap;

use gtk4::prelude::*;

use muxterm_core::protocol::WorkspaceId;

use crate::linux::layout_host::LayoutHost;
use crate::linux::scene_stack::SceneStackView;

/// 持有一个已打开 workspace 的常驻布局和对应 GTK scene page。
///
/// `LayoutHost` 与 `SceneStackView` 必须在同一个 owner 内增删，避免关闭或
/// 切换 workspace 时只更新其中一份状态。
pub struct WorkspaceScenes {
    layouts: HashMap<WorkspaceId, LayoutHost>,
    stack: SceneStackView,
}

impl WorkspaceScenes {
    /// 创建启动 workspace，并把它作为第一个可见 scene。
    pub fn new(startup_id: WorkspaceId, layout: LayoutHost) -> Self {
        let startup_key = startup_id.as_str();
        let stack = SceneStackView::new(&startup_key, &layout.root_box);
        let mut layouts = HashMap::new();
        layouts.insert(startup_id, layout);
        Self { layouts, stack }
    }

    pub fn widget(&self) -> gtk4::Stack {
        self.stack.widget()
    }

    pub fn ensure(&mut self, workspace_id: &WorkspaceId) {
        let key = workspace_id.as_str();
        self.stack.ensure(&key);
    }

    pub fn show(&mut self, workspace_id: &WorkspaceId) -> bool {
        let key = workspace_id.as_str();
        self.stack.show(&key)
    }

    pub fn has_page(&self, workspace_id: &WorkspaceId) -> bool {
        let key = workspace_id.as_str();
        self.stack.has_page(&key)
    }

    pub fn add_page(&mut self, workspace_id: &WorkspaceId, root: &impl IsA<gtk4::Widget>) {
        let key = workspace_id.as_str();
        self.stack.add_page(&key, root);
    }

    /// 移除 workspace 的布局和 scene page；这是唯一的 workspace close 边界。
    pub fn remove(&mut self, workspace_id: &WorkspaceId) {
        self.layouts.remove(workspace_id);
        let key = workspace_id.as_str();
        self.stack.remove(&key);
    }

    pub fn contains(&self, workspace_id: &WorkspaceId) -> bool {
        self.layouts.contains_key(workspace_id)
    }

    pub fn insert(&mut self, workspace_id: WorkspaceId, layout: LayoutHost) {
        self.ensure(&workspace_id);
        self.layouts.insert(workspace_id, layout);
    }

    pub fn get(&self, workspace_id: &WorkspaceId) -> Option<&LayoutHost> {
        self.layouts.get(workspace_id)
    }

    pub fn get_mut(&mut self, workspace_id: &WorkspaceId) -> Option<&mut LayoutHost> {
        self.layouts.get_mut(workspace_id)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut LayoutHost> {
        self.layouts.values_mut()
    }

    /// 关闭窗口时释放所有 LayoutHost 和其中的 GTK/VTE 子树。
    pub fn shutdown(&mut self) {
        for layout in self.layouts.values_mut() {
            layout.reset(false);
            while let Some(child) = layout.root_box.first_child() {
                layout.root_box.remove(&child);
            }
        }
        self.layouts.clear();
    }
}
