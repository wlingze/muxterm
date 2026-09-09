//! DaemonRuntime：TUI 作为 client 连接本地 daemon（unix socket IPC）。
//!
//! 生命周期：
//! - `connect()`：检查 socket 存在，消费 daemon 的初始拓扑/事件批次
//! - `execute(Task)`：映射为 CliCommand，经 IPC 发给 daemon，再消费事件批次
//! - `take_events()`：轮询 semantic event wire，不重建累计状态快照
//! - `shutdown()`：释放 client；显式 `Task::Shutdown` 才终止 daemon 宿主

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;

use crate::buffer_cap::{append_capped, MAX_PANE_OUTPUT_BYTES};
use crate::protocol::layout::{SplitDir, TabLayout};
use crate::protocol::state::{
    BackendStatus, MutationKind, MutationResult, PaneAgentInfo, PaneInfo, State, StateChange,
    TabInfo,
};
use crate::protocol::task::{Task, TaskOutcome};
use crate::protocol::terminal::input::encode;
use crate::runtime::shell::daemon_client::send_command;
use crate::runtime::{Runtime, RuntimeCapability};
use muxterm_protocol::command::CliCommand;
use muxterm_protocol::daemon::{OutputFormat, TopologySnapshot};
use muxterm_protocol::{PaneId, TabId};

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

    fn poll_from_daemon(&mut self) -> Result<()> {
        let resp = send_command(
            &self.socket_path,
            &CliCommand::PollEvents,
            OutputFormat::Json,
        )
        .with_context(|| {
            format!(
                "轮询 daemon 事件失败（session={} socket={}）",
                self.session_name,
                self.socket_path.display()
            )
        })?;
        if !resp.ok {
            bail!("daemon PollEvents 失败: {}", resp.error);
        }
        self.enqueue_wire_events(resp.events);
        Ok(())
    }

    fn apply_topology_event(&mut self, value: &serde_json::Value) -> bool {
        if value.get("kind").and_then(serde_json::Value::as_str) != Some("workspace_topology") {
            return false;
        }
        let Some(snapshot) = value
            .get("snapshot")
            .cloned()
            .and_then(|snapshot| serde_json::from_value::<TopologySnapshot>(snapshot).ok())
        else {
            return false;
        };
        self.session_name = snapshot.workspace_name;
        self.workspace_runtime = snapshot.workspace_runtime;
        self.tabs = snapshot.tabs;
        self.panes = snapshot.panes;
        self.layouts = snapshot.layouts.into_iter().map(|l| (l.tab, l)).collect();
        self.active_tab = snapshot.active_tab.map(TabId);
        self.active_pane = snapshot.active_pane.map(PaneId);
        true
    }

    fn apply_render_cache(&mut self, value: &serde_json::Value) {
        let Some(kind) = value.get("kind").and_then(serde_json::Value::as_str) else {
            return;
        };
        if kind == "backend_status" {
            if let Some(status) = wire_backend_status(value) {
                self.status = status;
            }
            return;
        }
        let Some(pane) = wire_u32(value, "pane_id").map(PaneId) else {
            return;
        };
        match kind {
            "pane_output" | "pane_history" => {
                if let Some(data) = wire_bytes(value) {
                    append_capped(
                        self.outputs.entry(pane).or_default(),
                        &data,
                        MAX_PANE_OUTPUT_BYTES,
                    );
                }
            }
            "pane_snapshot" => {
                if let Some(data) = wire_bytes(value) {
                    self.outputs.insert(pane, data);
                }
            }
            "pane_closed" => {
                self.outputs.remove(&pane);
            }
            _ => {}
        }
    }

    /// Decode the semantic daemon event wire at the Core runtime boundary.
    ///
    /// The daemon response deliberately carries JSON values instead of Core
    /// `StateChange` values: the CLI host owns the FFI DTOs, while this client
    /// owns the product event vocabulary. Unknown events are ignored so a
    /// newer FFI producer can still be consumed by an older daemon client.
    fn enqueue_wire_events(&mut self, events: impl IntoIterator<Item = serde_json::Value>) {
        for value in events {
            if self.apply_topology_event(&value) {
                continue;
            }
            self.apply_render_cache(&value);
            if let Some(event) = self.decode_wire_event(&value) {
                self.events.push_back(event);
            } else {
                tracing::debug!(
                    target = "muxterm::daemon",
                    kind = value.get("kind").and_then(serde_json::Value::as_str),
                    "忽略无法解码的 daemon event"
                );
            }
        }
    }

    fn decode_wire_event(&self, value: &serde_json::Value) -> Option<StateChange> {
        let kind = value.get("kind")?.as_str()?;
        match kind {
            "pane_output" => Some(StateChange::PaneOutput {
                pane: PaneId(wire_u32(value, "pane_id")?),
                data: wire_bytes(value)?,
            }),
            "pane_frame" => Some(StateChange::PaneFrame {
                pane: PaneId(wire_u32(value, "pane_id")?),
                data: wire_bytes(value)?,
            }),
            "pane_snapshot" => Some(StateChange::PaneSnapshot {
                pane: PaneId(wire_u32(value, "pane_id")?),
                data: wire_bytes(value)?,
            }),
            "pane_history" => Some(StateChange::PaneHistory {
                pane: PaneId(wire_u32(value, "pane_id")?),
                data: wire_bytes(value)?,
            }),
            "tab_added" => Some(StateChange::TabAdded {
                tab: TabId(wire_u32(value, "tab_id")?),
            }),
            "tab_closed" => Some(StateChange::TabClosed {
                tab: TabId(wire_u32(value, "tab_id")?),
            }),
            "layout_changed" => {
                let tab = TabId(wire_u32(value, "tab_id")?);
                self.layouts
                    .get(&tab)
                    .cloned()
                    .map(|layout| StateChange::LayoutChanged { tab, layout })
            }
            "pane_added" => Some(StateChange::PaneAdded {
                pane: PaneId(wire_u32(value, "pane_id")?),
                tab: TabId(wire_u32(value, "tab_id")?),
            }),
            "pane_closed" => Some(StateChange::PaneClosed {
                pane: PaneId(wire_u32(value, "pane_id")?),
            }),
            "active_tab_changed" => Some(StateChange::ActiveTabChanged {
                tab: TabId(wire_u32(value, "tab_id")?),
            }),
            "active_pane_changed" => Some(StateChange::ActivePaneChanged {
                tab: TabId(wire_u32(value, "tab_id")?),
                pane: PaneId(wire_u32(value, "pane_id")?),
            }),
            "tab_renamed" => Some(StateChange::TabRenamed {
                tab: TabId(wire_u32(value, "tab_id")?),
                name: wire_string(value, "name")?,
            }),
            "tab_order_changed" => Some(StateChange::TabOrderChanged),
            "pane_resized" => {
                let pane = PaneId(wire_u32(value, "pane_id")?);
                let (cols, rows) = wire_resize(value)?;
                Some(StateChange::PaneResized { pane, cols, rows })
            }
            "pane_agent_changed" => {
                let payload = wire_payload(value)?;
                let initial = payload
                    .get("initial")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let agent = payload
                    .get("agent")
                    .cloned()
                    .and_then(|agent| {
                        serde_json::from_value::<Option<Box<PaneAgentInfo>>>(agent).ok()
                    })
                    .flatten();
                Some(StateChange::PaneAgentChanged {
                    pane: PaneId(wire_u32(value, "pane_id")?),
                    agent,
                    initial,
                })
            }
            "status_subscription" => Some(StateChange::StatusBarSubscription {
                name: wire_string(value, "name")?,
                value: value
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                pane: nonzero_pane(value, "pane_id"),
            }),
            "workspace_renamed" => Some(StateChange::WorkspaceRenamed {
                name: wire_string(value, "name")?,
            }),
            "pool_changed" => Some(StateChange::PoolChanged),
            "backend_status" => Some(StateChange::BackendStatusChanged(wire_backend_status(
                value,
            )?)),
            "mutation_settled" => {
                let payload = wire_payload(value)?;
                Some(StateChange::MutationSettled {
                    operation_id: payload.get("operation_id")?.as_u64()?,
                    kind: serde_json::from_value::<MutationKind>(payload.get("kind")?.clone())
                        .ok()?,
                    result: serde_json::from_value::<MutationResult>(
                        payload.get("result")?.clone(),
                    )
                    .ok()?,
                })
            }
            // `STATE_OTHER` is used by the C ABI for PaneTitleChanged.  The
            // only other producer, PaneIndexSnapshot, is filtered before FFI
            // export, so a non-empty title is safe to recover here.
            "other"
                if wire_u32(value, "pane_id").is_some() && wire_string(value, "name").is_some() =>
            {
                Some(StateChange::PaneTitleChanged {
                    pane: PaneId(wire_u32(value, "pane_id")?),
                    title: wire_string(value, "name")?,
                })
            }
            _ => None,
        }
    }

    fn send_cli(&mut self, cmd: CliCommand) -> Result<()> {
        let closes_daemon = matches!(&cmd, CliCommand::CloseWorkspace { .. });
        let resp = send_command(&self.socket_path, &cmd, OutputFormat::Json)
            .with_context(|| format!("发送命令到 daemon 失败: {cmd:?}"))?;
        if !resp.ok {
            bail!("daemon 执行失败: {}", resp.error);
        }
        self.enqueue_wire_events(resp.events);
        if closes_daemon {
            self.status = BackendStatus::Disconnected;
        } else {
            self.poll_from_daemon()?;
        }
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
            Task::Detach => None, // detach：不向 daemon 发 KillSession
            Task::Shutdown => Some(CliCommand::CloseWorkspace { target: None }),
            Task::NextPane
            | Task::PrevPane
            | Task::ResizePaneStep { .. }
            | Task::ReportPaneColours { .. }
            | Task::RequestPaneSnapshot { .. } => None,
        }
    }
}

fn wire_u32(value: &serde_json::Value, key: &str) -> Option<u32> {
    value.get(key)?.as_u64()?.try_into().ok()
}

fn wire_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToOwned::to_owned)
}

fn wire_bytes(value: &serde_json::Value) -> Option<Vec<u8>> {
    serde_json::from_value(value.get("data")?.clone()).ok()
}

fn wire_payload(value: &serde_json::Value) -> Option<serde_json::Value> {
    value
        .get("payload")
        .cloned()
        .or_else(|| serde_json::from_value(value.get("data")?.clone()).ok())
}

fn wire_resize(value: &serde_json::Value) -> Option<(u16, u16)> {
    if let (Some(cols), Some(rows)) = (
        value.get("cols").and_then(serde_json::Value::as_u64),
        value.get("rows").and_then(serde_json::Value::as_u64),
    ) {
        return Some((cols.try_into().ok()?, rows.try_into().ok()?));
    }

    let data: Vec<u8> = wire_bytes(value)?;
    let [cols_lo, cols_hi, rows_lo, rows_hi, ..] = data.as_slice() else {
        return None;
    };
    Some((
        u16::from_le_bytes([*cols_lo, *cols_hi]),
        u16::from_le_bytes([*rows_lo, *rows_hi]),
    ))
}

fn nonzero_pane(value: &serde_json::Value, key: &str) -> Option<PaneId> {
    let pane = wire_u32(value, key)?;
    (pane != 0).then_some(PaneId(pane))
}

fn wire_backend_status(value: &serde_json::Value) -> Option<BackendStatus> {
    match value.get("status").and_then(serde_json::Value::as_str) {
        Some("disconnected" | "Disconnected") => Some(BackendStatus::Disconnected),
        Some("connecting" | "Connecting") => Some(BackendStatus::Connecting),
        Some("connected" | "Connected") => Some(BackendStatus::Connected),
        Some("error" | "Error") => Some(BackendStatus::Error),
        Some("exited" | "Exited") => Some(BackendStatus::Exited),
        Some(_) => None,
        None => match wire_u32(value, "pane_id")? {
            0 => Some(BackendStatus::Disconnected),
            1 => Some(BackendStatus::Connecting),
            2 => Some(BackendStatus::Connected),
            3 => Some(BackendStatus::Error),
            4 => Some(BackendStatus::Exited),
            _ => None,
        },
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
                RuntimeCapability::SharedClientResize,
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
        self.poll_from_daemon()?;
        self.status = BackendStatus::Connected;
        self.events
            .push_back(StateChange::BackendStatusChanged(BackendStatus::Connected));
        Ok(())
    }

    fn execute(&mut self, task: &Task) -> Result<TaskOutcome> {
        if matches!(task, Task::Detach) {
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
        if matches!(task, Task::Shutdown) {
            self.events.push_back(StateChange::BackendStatusChanged(
                BackendStatus::Disconnected,
            ));
        }
        Ok(TaskOutcome::Done)
    }

    fn take_events(&mut self) -> Vec<StateChange> {
        // 每次拉取前只消费事件流；拓扑 baseline 由 daemon 作为 control
        // event 提供，render bytes 不通过累计状态查询回读。
        if self.status == BackendStatus::Connected {
            let _ = self.poll_from_daemon();
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
    use crate::protocol::terminal::input::KeyEvent;

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
    fn task_shutdown_maps_to_close_workspace() {
        assert!(matches!(
            DaemonRuntime::task_to_cli(&Task::Shutdown),
            Some(CliCommand::CloseWorkspace { target: None })
        ));
    }

    #[test]
    fn task_detach_maps_to_none() {
        assert!(DaemonRuntime::task_to_cli(&Task::Detach).is_none());
    }

    #[test]
    fn topology_event_updates_state_without_reading_render_snapshot() {
        let mut runtime = DaemonRuntime::new("/tmp/muxterm-test-daemon.sock", "test");
        let topology = TopologySnapshot {
            workspace_name: "workspace".into(),
            workspace_runtime: "shell".into(),
            tabs: vec![TabInfo {
                id: TabId(1),
                name: "main".into(),
                active: true,
            }],
            panes: vec![PaneInfo {
                id: PaneId(7),
                tab: TabId(1),
                active: true,
                title: "bash".into(),
                cols: 120,
                rows: 40,
            }],
            layouts: vec![TabLayout {
                tab: TabId(1),
                tree: crate::protocol::layout::LayoutNode::Leaf(PaneId(7)),
                active: PaneId(7),
            }],
            active_tab: Some(1),
            active_pane: Some(7),
        };
        runtime.enqueue_wire_events([
            serde_json::json!({
                "kind": "workspace_topology",
                "workspace_id": "ws",
                "snapshot": serde_json::to_value(topology).unwrap(),
            }),
            serde_json::json!({
                "kind": "pane_output",
                "pane_id": 7,
                "data": [99, 117, 109, 117, 108, 97, 116, 105, 118, 101]
            }),
        ]);

        assert_eq!(runtime.workspace_name(), "workspace");
        assert_eq!(runtime.workspace_runtime(), "shell");
        assert_eq!(runtime.tabs().len(), 1);
        assert_eq!(runtime.panes(&TabId(1)).len(), 1);
        assert_eq!(
            runtime.pane_output(&PaneId(7)),
            Some(b"cumulative".as_slice())
        );
        assert_eq!(runtime.events.len(), 1);
        assert!(matches!(
            runtime.events.front(),
            Some(StateChange::PaneOutput {
                pane: PaneId(7),
                ..
            })
        ));
    }

    #[test]
    fn pane_snapshot_replaces_only_event_derived_render_cache() {
        let mut runtime = DaemonRuntime::new("/tmp/muxterm-test-daemon.sock", "test");
        runtime.enqueue_wire_events([
            serde_json::json!({
                "kind": "pane_output",
                "pane_id": 1,
                "data": [111, 108, 100]
            }),
            serde_json::json!({
                "kind": "pane_snapshot",
                "pane_id": 1,
                "data": [110, 101, 119]
            }),
        ]);

        assert_eq!(runtime.pane_output(&PaneId(1)), Some(b"new".as_slice()));
        assert_eq!(runtime.events.len(), 2);
    }

    #[test]
    fn semantic_wire_decodes_render_resize_and_backend_events() {
        let mut runtime = DaemonRuntime::new("/tmp/muxterm-test-daemon.sock", "test");
        runtime.enqueue_wire_events([
            serde_json::json!({
                "workspace_id": "ws",
                "kind": "pane_output",
                "pane_id": 7,
                "data": [0, 255, 27]
            }),
            serde_json::json!({
                "workspace_id": "ws",
                "kind": "pane_resized",
                "pane_id": 7,
                "cols": 120,
                "rows": 40
            }),
            serde_json::json!({
                "workspace_id": "ws",
                "kind": "backend_status",
                "status": "connected"
            }),
        ]);

        let events: Vec<_> = runtime.events.drain(..).collect();
        assert!(matches!(
            &events[0],
            StateChange::PaneOutput { pane: PaneId(7), data }
                if data == &[0, 255, 27]
        ));
        assert!(matches!(
            events[1],
            StateChange::PaneResized {
                pane: PaneId(7),
                cols: 120,
                rows: 40,
            }
        ));
        assert_eq!(
            events[2],
            StateChange::BackendStatusChanged(BackendStatus::Connected)
        );
    }

    #[test]
    fn semantic_wire_decodes_agent_and_mutation_payloads() {
        let mut runtime = DaemonRuntime::new("/tmp/muxterm-test-daemon.sock", "test");
        runtime.enqueue_wire_events([
            serde_json::json!({
                "kind": "pane_agent_changed",
                "pane_id": 3,
                "payload": {"initial": true, "agent": null}
            }),
            serde_json::json!({
                "kind": "mutation_settled",
                "payload": {
                    "operation_id": 42,
                    "kind": "new_tab",
                    "result": "completed"
                }
            }),
            serde_json::json!({"kind": "future_event"}),
        ]);

        let events: Vec<_> = runtime.events.drain(..).collect();
        assert!(matches!(
            events[0],
            StateChange::PaneAgentChanged {
                pane: PaneId(3),
                agent: None,
                initial: true,
            }
        ));
        assert!(matches!(
            events[1],
            StateChange::MutationSettled {
                operation_id: 42,
                kind: crate::protocol::state::MutationKind::NewTab,
                result: crate::protocol::state::MutationResult::Completed,
            }
        ));
        assert_eq!(events.len(), 2);
    }
}
