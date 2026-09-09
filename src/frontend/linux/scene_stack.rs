//! 常驻 Workspace Scene 的身份栈。
//!
//! GTK widget 树仍由 `LayoutHost` 承载；本模块只拥有场景生命周期和可见场景
//! 的产品状态。切换场景不会调用 Core，关闭 workspace 才会移除 Scene。

use std::collections::HashSet;

/// Workspace 场景的常驻索引。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SceneStack {
    order: Vec<String>,
    visible: Option<String>,
}

impl SceneStack {
    /// Create an empty stack with no visible scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a stack whose first scene is already visible.
    pub fn with_visible(workspace_id: impl Into<String>) -> Self {
        let workspace_id = workspace_id.into();
        Self {
            order: vec![workspace_id.clone()],
            visible: Some(workspace_id),
        }
    }

    /// Keep one workspace scene resident for the lifetime of its workspace.
    pub fn ensure(&mut self, workspace_id: &str) {
        if !self.order.iter().any(|id| id == workspace_id) {
            self.order.push(workspace_id.to_string());
        }
    }

    /// Change only the frontend-visible scene.  No Core operation is implied.
    pub fn show(&mut self, workspace_id: &str) -> bool {
        if !self.order.iter().any(|id| id == workspace_id) {
            return false;
        }
        self.visible = Some(workspace_id.to_string());
        true
    }

    /// Remove a scene at the single workspace-close lifecycle boundary.
    pub fn remove(&mut self, workspace_id: &str) {
        self.order.retain(|id| id != workspace_id);
        if self.visible.as_deref() == Some(workspace_id) {
            self.visible = self.order.last().cloned();
        }
    }

    pub fn contains(&self, workspace_id: &str) -> bool {
        self.order.iter().any(|id| id == workspace_id)
    }

    pub fn visible_id(&self) -> Option<&str> {
        self.visible.as_deref()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Return the stable scene ids as a set for diagnostics/tests.
    pub fn id_set(&self) -> HashSet<&str> {
        self.order.iter().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::SceneStack;

    #[test]
    fn scenes_stay_resident_until_explicitly_removed() {
        let mut stack = SceneStack::with_visible("one");
        stack.ensure("two");
        assert_eq!(stack.visible_id(), Some("one"));
        assert_eq!(stack.len(), 2);

        assert!(stack.show("two"));
        assert_eq!(stack.visible_id(), Some("two"));
        assert_eq!(stack.len(), 2);

        stack.remove("two");
        assert_eq!(stack.visible_id(), Some("one"));
        assert!(!stack.contains("two"));
    }

    #[test]
    fn showing_unknown_scene_does_not_change_visibility() {
        let mut stack = SceneStack::with_visible("one");
        assert!(!stack.show("missing"));
        assert_eq!(stack.visible_id(), Some("one"));
    }
}
