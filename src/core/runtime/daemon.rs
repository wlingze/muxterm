//! DaemonRuntime：TUI 作为 client 连接本地 daemon（unix socket IPC）。
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

use crate::core::model::backend::{Runtime, RuntimeCapability};
use crate::core::model::layout::{SplitDir, TabLayout};
use crate::core::model::state::{BackendStatus, PaneInfo, State, StateChange, TabInfo};
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::protocol::terminal::input::encode;
use crate::core::types::{PaneId, TabId};
use crate::platform::cli::client::send_command;
use crate::platform::cli::{CliCommand, OutputFormat, StateSnapshot};

/// 通过 unix socket 连接本地 daemon 的 Runtime。
pub struct DaemonRuntime {
    socket_path: PathBuf,
    session_name: String,
    workspace_runtime: String,
    tabs: Vec<TabInfo>,
    panes: Vec<PaneInfo>,
    layouts: HashMap<TabId, TabLayout>,
    outputs: HashMap<PaneId, Vec<u8>>,
    status: BackendStatus,
    active_tab: Option<TabId>,
    active_pane: Option<PaneId>,
    events: VecDeque<StateChange>,
}

impl DaemonRuntime {
    /// 默认 unix socket 路径（$XDG_RUNTIME_DIR 或 /tmp，与 platform CLI 一致）。
    ///
    /// W12：core 不再 `use crate::platform::cli` 推导 daemon socket 路径；
    /// 默认路径挪到 Runtime 构造处，platform CLI 的 `session_socket_path`
    /// 只是这层的薄包装。
    pub fn default_socket_path(name: &str) -> PathBuf {
        let dir = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        dir.join(format!("muxterm-{safe}.sock"))
    }

    /// 创建尚未 connect 的 backend。
    pub fn new(socket_path: impl Into<PathBuf>, session_name: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            session_name: session_name.into(),
            workspace_runtime: String::new(),
            tabs: vec![],
            panes: vec![],
            layouts: HashMap::new(),
            outputs: HashMap::new(),
            status: BackendStatus::Disconnected,
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

        self.session_name = snap.workspace_name;
        self.workspace_runtime = snap.workspace_runtime;
        self.tabs = snap.tabs;
        self.panes = snap.panes;
        self.layouts = snap.layouts.into_iter().map(|l| (l.tab, l)).collect();
        self.outputs = snap
            .outputs
            .into_iter()
            .map(|(id, s)| (PaneId(id), s.into_bytes()))
            .collect();
        self.status = snap.status;
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
            if let Some(t) = self.active_tab {
                self.events
                    .push_back(StateChange::ActiveTabChanged { tab: t });
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
            Task::TogglePaneFullscreen { .. }
            | Task::MoveTab { .. }
            | Task::BreakPane { .. }
            | Task::RefreshTabs => None, // daemon CLI 暂不支持 zoom
            Task::NewTab { name, .. } => Some(CliCommand::NewTab { name: name.clone() }),
            Task::RenameWorkspace { name } => Some(CliCommand::RenameWorkspace {
                new_name: name.clone(),
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
            Task::Detach | Task::Shutdown => None, // detach：不向 daemon 发 KillSession
            Task::NextPane
            | Task::PrevPane
            | Task::ResizePaneStep { .. }
            | Task::ReportPaneColours { .. } => None,
        }
    }
}

impl State for DaemonRuntime {
    fn workspace_name(&self) -> &str {
        &self.session_name
    }

    fn workspace_runtime(&self) -> &str {
        &self.workspace_runtime
    }

    fn active_tab(&self) -> Option<&TabInfo> {
        let id = self.active_tab?;
        self.tabs.iter().find(|t| t.id == id)
    }

    fn active_pane(&self) -> Option<&PaneInfo> {
        let id = self.active_pane?;
        self.panes.iter().find(|p| p.id == id)
    }

    fn tabs(&self) -> Vec<&TabInfo> {
        self.tabs.iter().collect()
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
impl Runtime for DaemonRuntime {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn support(&self) -> &'static [RuntimeCapability] {
        // IPC 客户端：能力等于背后那个 Runtime；自己绝不谎报 worktree。
        match self.workspace_runtime.as_str() {
            "tmux" => &[
                RuntimeCapability::PersistDetach,
                RuntimeCapability::Discover,
                RuntimeCapability::MultiTab,
                RuntimeCapability::SplitPane,
            ],
            "shell" => &[RuntimeCapability::MultiTab, RuntimeCapability::SplitPane],
            _ => &[],
        }
    }

    async fn connect(&mut self) -> Result<()> {
        self.status = BackendStatus::Connecting;
        tracing::debug!(
            target = "muxterm::daemon",
            session = %self.session_name,
            socket = %self.socket_path.display(),
            "daemon connect"
        );
        if !Path::new(&self.socket_path).exists() {
            tracing::debug!(target = "muxterm::daemon", "daemon socket 不存在");
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
        if matches!(task, Task::Detach | Task::Shutdown) {
            // detach：不向 daemon 发 KillSession
            self.status = BackendStatus::Disconnected;
            self.events.push_back(StateChange::BackendStatusChanged(
                BackendStatus::Disconnected,
            ));
            return Ok(TaskOutcome::Done);
        }
        let Some(cmd) = Self::task_to_cli(task) else {
            return Ok(TaskOutcome::Rejected {
                reason: format!("DaemonRuntime 不支持任务: {task:?}"),
            });
        };
        tracing::debug!(target = "muxterm::daemon", task = ?task, cli = ?cmd, "daemon execute");
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
        let cmd = DaemonRuntime::task_to_cli(&Task::SendKeys {
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
        assert!(DaemonRuntime::task_to_cli(&Task::Shutdown).is_none());
    }

    #[test]
    fn task_detach_maps_to_none() {
        assert!(DaemonRuntime::task_to_cli(&Task::Detach).is_none());
    }
}
