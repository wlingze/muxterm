//! TUI Scene：每个已打开 Workspace 一组常驻 pane buffer。
//!
//! 切换 workspace / tab 只改可见场景，不销毁 TerminalManager，也不向 Core
//! 拉帧。EventPump 仍把事件写入所有 Scene；隐藏 Scene 继续 feed。

use std::collections::HashMap;

use crate::frontend::ffi_client::{ClientEvent, ClientEventKind, ClientWorkspaceEvent};
use crate::frontend::tui::terminal::TerminalManager;
use crate::frontend::view_store::{ViewStore, WorkspaceView};

/// 一个已打开 Workspace 的 TUI 场景：常驻 VT + 本地可见 tab。
pub struct WorkspaceScene {
    pub id: String,
    pub terminals: TerminalManager,
    visible_tab: Option<u32>,
}

impl WorkspaceScene {
    pub fn new(id: impl Into<String>, forward_replies: bool) -> Self {
        let mut terminals = TerminalManager::new();
        terminals.forward_replies = forward_replies;
        Self {
            id: id.into(),
            terminals,
            visible_tab: None,
        }
    }

    pub fn visible_tab(&self, view: &WorkspaceView) -> Option<u32> {
        self.visible_tab.or_else(|| view.active_tab_id())
    }

    /// 本地切 tab；Core SwitchTab 由 CommandQueue 另行入队。
    pub fn set_visible_tab(&mut self, tab_id: u32) {
        self.visible_tab = Some(tab_id);
    }

    pub fn apply_render(&mut self, event: &ClientEvent) {
        match event.kind() {
            ClientEventKind::PaneOutput => self.terminals.feed_event(event.pane_id, &event.data),
            ClientEventKind::PaneFrame => {
                self.terminals.feed_frame_event(event.pane_id, &event.data)
            }
            ClientEventKind::PaneSnapshot | ClientEventKind::PaneHistory => {
                self.terminals.replace_snapshot(event.pane_id, &event.data)
            }
            ClientEventKind::PaneClosed => self.terminals.remove(event.pane_id),
            ClientEventKind::PaneResized if event.data.len() >= 4 => {
                let cols = u16::from_le_bytes([event.data[0], event.data[1]]);
                let rows = u16::from_le_bytes([event.data[2], event.data[3]]);
                self.terminals.resize_pane(event.pane_id, cols, rows);
            }
            _ => {}
        }
    }
}

/// 持有全部 WorkspaceScene；`show` 只改可见 id。
pub struct SceneStack {
    scenes: HashMap<String, WorkspaceScene>,
    visible: Option<String>,
    forward_replies: bool,
}

impl SceneStack {
    pub fn new(forward_replies: bool) -> Self {
        Self {
            scenes: HashMap::new(),
            visible: None,
            forward_replies,
        }
    }

    pub fn visible_id(&self) -> Option<&str> {
        self.visible.as_deref()
    }

    pub fn visible(&self) -> Option<&WorkspaceScene> {
        self.visible.as_ref().and_then(|id| self.scenes.get(id))
    }

    pub fn visible_mut(&mut self) -> Option<&mut WorkspaceScene> {
        let id = self.visible.as_ref()?;
        self.scenes.get_mut(id)
    }

    pub fn scene_mut(&mut self, id: &str) -> Option<&mut WorkspaceScene> {
        self.scenes.get_mut(id)
    }

    pub fn ensure(&mut self, id: &str) {
        if !self.scenes.contains_key(id) {
            self.scenes.insert(
                id.to_string(),
                WorkspaceScene::new(id, self.forward_replies),
            );
        }
    }

    /// 切换可见 Scene。不调用 Core，不 reset 其他 Scene 的 buffer。
    pub fn show(&mut self, id: &str) -> bool {
        self.ensure(id);
        let changed = self.visible.as_deref() != Some(id);
        self.visible = Some(id.to_string());
        changed
    }

    pub fn clear(&mut self) {
        self.scenes.clear();
        self.visible = None;
    }

    pub fn set_forward_replies(&mut self, forward_replies: bool) {
        self.forward_replies = forward_replies;
        for scene in self.scenes.values_mut() {
            scene.terminals.forward_replies = forward_replies;
        }
    }

    pub fn apply_workspace_event(&mut self, event: &ClientWorkspaceEvent) {
        self.ensure(&event.workspace_id);
        if let Some(scene) = self.scenes.get_mut(&event.workspace_id) {
            scene.apply_render(&event.event);
        }
    }
}

/// 把 ViewStore 里排队的 render mailbox 喂进对应 Scene，含隐藏 workspace。
pub fn drain_store_into_scenes(store: &mut ViewStore, scenes: &mut SceneStack) {
    let ids: Vec<String> = store.workspace_ids().map(str::to_owned).collect();
    for id in ids {
        scenes.ensure(&id);
        let pane_ids: Vec<u32> = store
            .workspace(&id)
            .map(|view| view.all_pane_ids().collect())
            .unwrap_or_default();
        for pane_id in pane_ids {
            let events = store.take_pane_render_events(&id, pane_id);
            if let Some(scene) = scenes.scene_mut(&id) {
                for event in &events {
                    scene.apply_render(event);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ffi_client::{ClientEvent, ClientPane, ClientTab, ClientWorkspace};
    use crate::frontend::view_store::ViewStore;
    use crate::protocol::ffi::types;

    fn output_event(pane_id: u32, byte: u8) -> ClientEvent {
        ClientEvent {
            type_: types::STATE_PANE_OUTPUT,
            pane_id,
            tab_id: 1,
            window_id: 0,
            data: vec![byte],
            name: String::new(),
        }
    }

    #[test]
    fn show_switches_visible_scene_without_dropping_hidden_buffers() {
        let mut stack = SceneStack::new(false);
        stack.show("ws-a");
        stack
            .visible_mut()
            .expect("visible")
            .apply_render(&output_event(1, b'A'));
        stack.show("ws-b");
        stack
            .visible_mut()
            .expect("visible")
            .apply_render(&output_event(2, b'B'));

        assert_eq!(stack.visible_id(), Some("ws-b"));
        assert!(stack.scene_mut("ws-a").expect("a").terminals.has(1));
        assert!(stack.scene_mut("ws-b").expect("b").terminals.has(2));
        stack.show("ws-a");
        assert_eq!(stack.visible_id(), Some("ws-a"));
        assert!(stack.visible().expect("a").terminals.has(1));
    }

    #[test]
    fn hidden_workspace_mailboxes_still_feed_their_scene() {
        let mut store = ViewStore::default();
        store.replace_topology(
            ClientWorkspace {
                id: "hidden".into(),
                name: "hidden".into(),
                runtime: "shell".into(),
                active: false,
                resolved_target: None,
            },
            vec![ClientTab {
                id: 1,
                name: "tab".into(),
                is_active: true,
            }],
            vec![(
                1,
                vec![ClientPane {
                    id: 9,
                    cols: 80,
                    rows: 24,
                    is_active: true,
                    title: "bash".into(),
                }],
            )],
        );
        store.push_render_event("hidden", output_event(9, b'x'));
        let mut scenes = SceneStack::new(false);
        scenes.show("visible");
        drain_store_into_scenes(&mut store, &mut scenes);
        assert!(scenes.scene_mut("hidden").expect("hidden").terminals.has(9));
        assert_eq!(scenes.visible_id(), Some("visible"));
    }
}
