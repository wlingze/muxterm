//! DaemonBackend：TUI 作为 client 连接本地 daemon（unix socket IPC）。
//!
//! 生命周期：
//! - `connect()`：检查 socket 存在，拉取 DumpState 建立初始快照
//! - `execute(Task)`：映射为 CliCommand，经 IPC 发给 daemon，再同步快照
//! - `take_events()`：再次 DumpState，对输出/布局 diff 后产生 StateChange
//! - `shutdown()`：仅断开 client（不 kill daemon，detach 语义）

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;

use crate::core::model::backend::Backend;
use crate::core::model::layout::{SplitDir, TabLayout};
use crate::core::model::state::{
    BackendStatus, PaneInfo, SessionInfo, State, StateChange, TabInfo, WindowInfo,
};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::protocol::terminal::input::encode;
use crate::core::types::{PaneId, SessionId, TabId, WindowId};
use crate::platform::cli::client::send_command;
use crate::platform::cli::{CliCommand, OutputFormat, StateSnapshot};

/// 通过 unix socket 连接本地 daemon 的 Backend。
pub struct DaemonBackend {
    socket_path: PathBuf,
    session_name: String,
    sessions: Vec<SessionInfo>,
    windows: Vec<WindowInfo>,
    tabs: Vec<TabInfo>,
    panes: Vec<PaneInfo>,
    layouts: HashMap<TabId, TabLayout>,
    outputs: HashMap<PaneId, Vec<u8>>,
    status: BackendStatus,
    active_session: Option<SessionId>,
    active_window: Option<WindowId>,
    active_tab: Option<TabId>,
    active_pane: Option<PaneId>,
    events: VecDeque<StateChange>,
}

impl DaemonBackend {
    /// 创建尚未 connect 的 backend。
    pub fn new(socket_path: impl Into<PathBuf>, session_name: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            session_name: session_name.into(),
            sessions: vec![],
            windows: vec![],
            tabs: vec![],
            panes: vec![],
            layouts: HashMap::new(),
            outputs: HashMap::new(),
            status: BackendStatus::Disconnected,
            active_session: None,
            active_window: None,
            active_tab: None,
            active_pane: None,
            events: VecDeque::new(),
        }
    }

    fn sync_from_daemon(&mut self) -> Result<()> {
        let resp = send_command(
            &self.socket_path,
            &CliCommand::DumpState,
            OutputFormat::Json,
        )
        .with_context(|| {
            format!(
                "同步 daemon 状态失败（session={} socket={}）",
                self.session_name,
                self.socket_path.display()
            )
        })?;
        if !resp.ok {
            bail!("daemon DumpState 失败: {}", resp.error);
        }
        let snap: StateSnapshot =
            serde_json::from_str(&resp.output).context("反序列化 DumpState 失败")?;
        self.apply_snapshot(snap);
        Ok(())
    }

    fn apply_snapshot(&mut self, snap: StateSnapshot) {
        let old_outputs = std::mem::take(&mut self.outputs);
        let old_layouts = std::mem::take(&mut self.layouts);
        let old_active_tab = self.active_tab;
        let old_active_pane = self.active_pane;
        let old_status = self.status;

        self.sessions = snap.sessions;
        self.windows = snap.windows;
        self.tabs = snap.tabs;
        self.panes = snap.panes;
        self.layouts = snap.layouts.into_iter().map(|l| (l.tab, l)).collect();
        self.outputs = snap
            .outputs
            .into_iter()
            .map(|(id, s)| (PaneId(id), s.into_bytes()))
            .collect();
        self.status = snap.status;
        self.active_session = snap.active_session.map(SessionId);
        self.active_window = snap.active_window.map(WindowId);
        self.active_tab = snap.active_tab.map(TabId);
        self.active_pane = snap.active_pane.map(PaneId);

        // 输出增量 → PaneOutput
        for (pid, new_out) in &self.outputs {
            let old = old_outputs.get(pid).map(|v| v.as_slice()).unwrap_or(&[]);
            if new_out.len() > old.len() && new_out.starts_with(old) {
                let delta = new_out[old.len()..].to_vec();
                if !delta.is_empty() {
                    self.events.push_back(StateChange::PaneOutput {
                        pane: *pid,
                        data: delta,
                    });
                }
            } else if new_out.as_slice() != old {
                // 非前缀增长（重置等）：整段当作新输出
                self.events.push_back(StateChange::PaneOutput {
                    pane: *pid,
                    data: new_out.clone(),
                });
            }
        }

        // 布局变化
        for (tid, layout) in &self.layouts {
            let changed = match old_layouts.get(tid) {
                Some(old) => old != layout,
                None => true,
            };
            if changed {
                self.events.push_back(StateChange::LayoutChanged {
                    tab: *tid,
                    layout: layout.clone(),
                });
            }
        }

        if self.active_tab != old_active_tab {
            if let (Some(w), Some(t)) = (self.active_window, self.active_tab) {
                self.events
                    .push_back(StateChange::ActiveTabChanged { window: w, tab: t });
            }
        }
        if self.active_pane != old_active_pane {
            if let (Some(t), Some(p)) = (self.active_tab, self.active_pane) {
                self.events
                    .push_back(StateChange::ActivePaneChanged { tab: t, pane: p });
            }
        }
        if self.status != old_status {
            self.events
                .push_back(StateChange::BackendStatusChanged(self.status));
        }
    }

    fn send_cli(&mut self, cmd: CliCommand) -> Result<()> {
        let resp = send_command(&self.socket_path, &cmd, OutputFormat::Json)
            .with_context(|| format!("发送命令到 daemon 失败: {cmd:?}"))?;
        if !resp.ok {
            bail!("daemon 执行失败: {}", resp.error);
        }
        self.sync_from_daemon()?;
        Ok(())
    }

    fn task_to_cli(task: &Task) -> Option<CliCommand> {
        match task {
            Task::SplitPane { target, dir, .. } => Some(CliCommand::SplitPane {
                horizontal: matches!(dir, SplitDir::Horizontal),
                target: *target,
                size: None,
            }),
            Task::ClosePane { target } => Some(CliCommand::KillPane {
                target: Some(*target),
            }),
            Task::SwitchPane { target } => Some(CliCommand::SelectPane { target: *target }),
            Task::NewWindow { name, .. } => Some(CliCommand::NewWindow {
                name: name.clone(),
                session: None,
            }),
            Task::CloseWindow { target } => Some(CliCommand::KillWindow {
                target: Some(*target),
            }),
            Task::SwitchWindow { target } => Some(CliCommand::SelectWindow { target: *target }),
            Task::RenameWindow { name, .. } => Some(CliCommand::RenameWindow {
                new_name: name.clone(),
            }),
            Task::NewTab { window, name, .. } => Some(CliCommand::NewTab {
                window: Some(*window),
                name: name.clone(),
            }),
            Task::CloseTab { target } => Some(CliCommand::KillTab {
                target: Some(*target),
            }),
            Task::SwitchTab { target } => Some(CliCommand::SelectTab { target: *target }),
            Task::RenameTab { name, .. } => Some(CliCommand::RenameTab {
                new_name: name.clone(),
            }),
            Task::SendKeys { target, keys } => {
                // 全部编码为原始字节，经 WriteRaw 发给 daemon（支持 Ctrl/方向键等）
                let mut data = Vec::new();
                for k in keys {
                    data.extend(encode(k));
                }
                Some(CliCommand::WriteRaw {
                    target: Some(*target),
                    data,
                })
            }
            Task::WriteRaw { target, data } => Some(CliCommand::WriteRaw {
                target: Some(*target),
                data: data.clone(),
            }),
            Task::ResizePane { target, cols, rows } => Some(CliCommand::ResizePane {
                target: *target,
                width: Some(*cols),
                height: Some(*rows),
            }),
            Task::ResizeClient { cols, rows } => Some(CliCommand::ResizeClient {
                width: *cols,
                height: *rows,
            }),
            Task::ResizePaneAxis { target, dir, size } => Some(CliCommand::ResizePane {
                target: *target,
                width: matches!(dir, SplitDir::Horizontal).then_some(*size),
                height: matches!(dir, SplitDir::Vertical).then_some(*size),
            }),
            Task::Shutdown => None, // detach：不 kill daemon
            Task::NextPane
            | Task::PrevPane
            | Task::SwitchSession { .. }
            | Task::RenameSession { .. }
            | Task::ResizePaneStep { .. } => None,
        }
    }
}

impl State for DaemonBackend {
    fn sessions(&self) -> &[SessionInfo] {
        &self.sessions
    }

    fn active_session(&self) -> Option<&SessionInfo> {
        let id = self.active_session?;
        self.sessions.iter().find(|s| s.id == id)
    }

    fn active_window(&self) -> Option<&WindowInfo> {
        let id = self.active_window?;
        self.windows.iter().find(|w| w.id == id)
    }

    fn all_windows(&self) -> Vec<&WindowInfo> {
        self.windows.iter().collect()
    }

    fn active_tab(&self) -> Option<&TabInfo> {
        let id = self.active_tab?;
        self.tabs.iter().find(|t| t.id == id)
    }

    fn active_pane(&self) -> Option<&PaneInfo> {
        let id = self.active_pane?;
        self.panes.iter().find(|p| p.id == id)
    }

    fn tabs(&self, window: &WindowId) -> Vec<&TabInfo> {
        self.tabs.iter().filter(|t| t.window == *window).collect()
    }

    fn tab(&self, tab: &TabId) -> Option<&TabInfo> {
        self.tabs.iter().find(|t| t.id == *tab)
    }

    fn layout(&self, tab: &TabId) -> Option<&TabLayout> {
        self.layouts.get(tab)
    }

    fn panes(&self, tab: &TabId) -> Vec<&PaneInfo> {
        self.panes.iter().filter(|p| p.tab == *tab).collect()
    }

    fn pane(&self, pane: &PaneId) -> Option<&PaneInfo> {
        self.panes.iter().find(|p| p.id == *pane)
    }

    fn pane_output(&self, pane: &PaneId) -> Option<&[u8]> {
        self.outputs.get(pane).map(|v| v.as_slice())
    }

    fn status(&self) -> BackendStatus {
        self.status
    }
}

#[async_trait]
impl Backend for DaemonBackend {
    async fn connect(&mut self) -> Result<()> {
        self.status = BackendStatus::Connecting;
        if !Path::new(&self.socket_path).exists() {
            bail!(
                "session '{}' 不存在（socket: {}）。用 `muxterm new-session -s {}` 创建。",
                self.session_name,
                self.socket_path.display(),
                self.session_name
            );
        }
        self.sync_from_daemon()?;
        self.status = BackendStatus::Connected;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connected));
        Ok(())
    }

    fn execute(&mut self, task: &Task) -> Result<TaskOutcome> {
        if matches!(task, Task::Shutdown) {
            // detach：不向 daemon 发 KillSession
            self.status = BackendStatus::Disconnected;
            self.events.push_back(StateChange::BackendStatusChanged(
                BackendStatus::Disconnected,
            ));
            return Ok(TaskOutcome::Done);
        }
        let Some(cmd) = Self::task_to_cli(task) else {
            return Ok(TaskOutcome::Rejected {
                reason: format!("DaemonBackend 不支持任务: {task:?}"),
            });
        };
        self.send_cli(cmd)?;
        Ok(TaskOutcome::Done)
    }

    fn take_events(&mut self) -> Vec<StateChange> {
        // 每次拉取前先同步 daemon（pty 输出等）
        if self.status == BackendStatus::Connected {
            let _ = self.sync_from_daemon();
        }
        self.events.drain(..).collect()
    }

    async fn shutdown(&mut self) -> Result<()> {
        // detach：不断开 daemon
        self.status = BackendStatus::Disconnected;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::terminal::input::KeyEvent;

    #[test]
    fn task_send_keys_maps_to_write_raw() {
        let cmd = DaemonBackend::task_to_cli(&Task::SendKeys {
            target: PaneId(1),
            keys: vec![KeyEvent::Char('a'), KeyEvent::Enter],
        });
        match cmd {
            Some(CliCommand::WriteRaw { target, data }) => {
                assert_eq!(target, Some(PaneId(1)));
                assert_eq!(data, b"a\r");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn task_shutdown_maps_to_none() {
        assert!(DaemonBackend::task_to_cli(&Task::Shutdown).is_none());
    }
}
