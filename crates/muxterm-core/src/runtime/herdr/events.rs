//! Herdr API event subscription and wire normalization.
//!
//! Subscription names (`pane.agent_status_changed`) and global event names
//! (`pane_agent_status_changed`) are both contained here. Runtime/Workspace only
//! receive typed snapshots/layouts and never need to recognize either spelling.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use super::session::{HerdrSession, LayoutRecord, SessionSnapshot};

/// Parameterless protocol-19 subscriptions. Pane-scoped agent status is added
/// separately for every pane owned by the bound Runtime.
pub const GLOBAL_SUBSCRIPTIONS: &[&str] = &[
    "workspace.created",
    "workspace.updated",
    "workspace.metadata_updated",
    "workspace.renamed",
    "workspace.moved",
    "workspace.reordered",
    "workspace.focused",
    "workspace.closed",
    "worktree.created",
    "worktree.opened",
    "worktree.removed",
    "tab.created",
    "tab.renamed",
    "tab.moved",
    "tab.focused",
    "tab.closed",
    "pane.created",
    "pane.updated",
    "pane.moved",
    "pane.focused",
    "pane.closed",
    "pane.exited",
    "pane.agent_detected",
    "layout.updated",
];

/// All Herdr event kinds Muxterm understands, including kinds emitted by wait
/// APIs but not available as a parameterless subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrEventKind {
    WorkspaceCreated,
    WorkspaceUpdated,
    WorkspaceMetadataUpdated,
    WorkspaceClosed,
    WorkspaceRenamed,
    WorkspaceMoved,
    WorkspaceReordered,
    WorkspaceFocused,
    WorktreeCreated,
    WorktreeOpened,
    WorktreeRemoved,
    TabCreated,
    TabClosed,
    TabRenamed,
    TabMoved,
    TabFocused,
    PaneCreated,
    PaneClosed,
    PaneUpdated,
    PaneFocused,
    PaneMoved,
    PaneOutputChanged,
    PaneExited,
    PaneAgentDetected,
    PaneAgentStatusChanged,
    LayoutUpdated,
    Unknown(String),
}

impl HerdrEventKind {
    fn from_wire(kind: &str) -> Self {
        match kind {
            "workspace.created" | "workspace_created" => Self::WorkspaceCreated,
            "workspace.updated" | "workspace_updated" => Self::WorkspaceUpdated,
            "workspace.metadata_updated" | "workspace_metadata_updated" => {
                Self::WorkspaceMetadataUpdated
            }
            "workspace.closed" | "workspace_closed" => Self::WorkspaceClosed,
            "workspace.renamed" | "workspace_renamed" => Self::WorkspaceRenamed,
            "workspace.moved" | "workspace_moved" => Self::WorkspaceMoved,
            "workspace.reordered" | "workspace_reordered" => Self::WorkspaceReordered,
            "workspace.focused" | "workspace_focused" => Self::WorkspaceFocused,
            "worktree.created" | "worktree_created" => Self::WorktreeCreated,
            "worktree.opened" | "worktree_opened" => Self::WorktreeOpened,
            "worktree.removed" | "worktree_removed" => Self::WorktreeRemoved,
            "tab.created" | "tab_created" => Self::TabCreated,
            "tab.closed" | "tab_closed" => Self::TabClosed,
            "tab.renamed" | "tab_renamed" => Self::TabRenamed,
            "tab.moved" | "tab_moved" => Self::TabMoved,
            "tab.focused" | "tab_focused" => Self::TabFocused,
            "pane.created" | "pane_created" => Self::PaneCreated,
            "pane.closed" | "pane_closed" => Self::PaneClosed,
            "pane.updated" | "pane_updated" => Self::PaneUpdated,
            "pane.focused" | "pane_focused" => Self::PaneFocused,
            "pane.moved" | "pane_moved" => Self::PaneMoved,
            "pane.output_changed" | "pane_output_changed" => Self::PaneOutputChanged,
            "pane.exited" | "pane_exited" => Self::PaneExited,
            "pane.agent_detected" | "pane_agent_detected" => Self::PaneAgentDetected,
            "pane.agent_status_changed" | "pane_agent_status_changed" => {
                Self::PaneAgentStatusChanged
            }
            "layout.updated" | "layout_updated" => Self::LayoutUpdated,
            other => Self::Unknown(other.to_string()),
        }
    }

    fn refreshes_snapshot(&self) -> bool {
        !matches!(
            self,
            Self::LayoutUpdated
                | Self::PaneOutputChanged
                | Self::WorktreeCreated
                | Self::WorktreeOpened
                | Self::WorktreeRemoved
                | Self::Unknown(_)
        )
    }
}

/// Parsed Herdr event envelope. `data` is retained so newly added optional
/// fields remain inspectable even before Muxterm assigns them product meaning.
#[derive(Debug, Clone, PartialEq)]
pub struct HerdrEvent {
    pub kind: HerdrEventKind,
    pub data: Value,
}

impl HerdrEvent {
    pub fn parse_line(line: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("解析 Herdr event JSON 失败: {line}"))?;
        let kind = value
            .get("event")
            .and_then(|event| {
                event
                    .as_str()
                    .or_else(|| event.get("type").and_then(Value::as_str))
            })
            .or_else(|| value.get("type").and_then(Value::as_str))
            .or_else(|| value.get("kind").and_then(Value::as_str))
            .ok_or_else(|| anyhow!("Herdr event 缺 event/type/kind: {value}"))?;
        let data = value
            .get("data")
            .cloned()
            .or_else(|| {
                value
                    .get("event")
                    .and_then(|event| event.get("data"))
                    .cloned()
            })
            .unwrap_or_else(|| Value::Object(Default::default()));
        Ok(Self {
            kind: HerdrEventKind::from_wire(kind),
            data,
        })
    }

    /// EventData 是否影响指定 workspace。`pane.moved` 同时影响旧/新两格，
    /// 不能只取第一个 workspace id；reorder 的数组也必须完整检查。
    pub fn affects_workspace(&self, workspace_id: &str) -> bool {
        let direct = self
            .data
            .get("workspace_id")
            .and_then(Value::as_str)
            .into_iter()
            .chain(
                self.data
                    .get("previous_workspace_id")
                    .and_then(Value::as_str),
            )
            .chain(self.data.get("closed_workspace_id").and_then(Value::as_str))
            .chain(nested_id(&self.data, "workspace", "workspace_id"))
            .chain(nested_id(&self.data, "created_workspace", "workspace_id"))
            .chain(nested_id(&self.data, "tab", "workspace_id"))
            .chain(nested_id(&self.data, "pane", "workspace_id"))
            .chain(nested_id(&self.data, "layout", "workspace_id"))
            .any(|candidate| candidate == workspace_id);
        direct
            || self
                .data
                .get("workspace_ids")
                .and_then(Value::as_array)
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(workspace_id)))
    }

    fn layout(&self) -> Option<LayoutRecord> {
        matches!(self.kind, HerdrEventKind::LayoutUpdated)
            .then(|| self.data.get("layout"))
            .flatten()
            .and_then(LayoutRecord::from_json)
    }
}

fn nested_id<'a>(value: &'a Value, object: &str, id: &str) -> Option<&'a str> {
    value.get(object)?.get(id)?.as_str()
}

/// Event reader thread → HerdrRuntime.
#[derive(Debug, Clone, PartialEq)]
pub enum EventStreamEvent {
    Snapshot {
        cause: HerdrEventKind,
        snapshot: SessionSnapshot,
    },
    Layout(LayoutRecord),
    Closed,
    Error(String),
}

/// One API-socket events.subscribe stream for a bound Herdr workspace.
pub struct EventStream {
    shutdown_stream: Option<UnixStream>,
    handle: Option<JoinHandle<()>>,
    /// Drop 时置位：reader 据此把「主动 shutdown 造成的 EOF/Error」静默
    /// 掉，不向 Runtime 误报订阅死亡（否则 restart 替换订阅会残留一个
    /// 假的 Closed/Error，触发无意义的二次重启）。
    dropping: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl EventStream {
    pub fn start(
        session: Arc<HerdrSession>,
        workspace_id: &str,
        pane_ids: &[String],
        tx: Sender<EventStreamEvent>,
    ) -> Result<Self> {
        let mut stream = UnixStream::connect(session.socket_path()).with_context(|| {
            format!(
                "连接 Herdr event socket 失败: {}",
                session.socket_path().display()
            )
        })?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .context("设置 Herdr event ack 超时失败")?;

        let mut subscriptions = GLOBAL_SUBSCRIPTIONS
            .iter()
            .map(|kind| serde_json::json!({ "type": kind }))
            .collect::<Vec<_>>();
        subscriptions.extend(pane_ids.iter().map(|pane_id| {
            serde_json::json!({
                "type": "pane.agent_status_changed",
                "pane_id": pane_id,
            })
        }));
        let request = serde_json::json!({
            "id": format!("muxterm-events-{workspace_id}"),
            "method": "events.subscribe",
            "params": { "subscriptions": subscriptions },
        });
        stream
            .write_all((request.to_string() + "\n").as_bytes())
            .context("写 Herdr events.subscribe 失败")?;
        stream.flush().ok();

        let shutdown_stream = stream.try_clone().context("复制 Herdr event socket 失败")?;
        let mut reader = BufReader::new(stream);
        let mut ack = String::new();
        reader
            .read_line(&mut ack)
            .context("读取 Herdr events.subscribe ack 失败")?;
        let ack: Value = serde_json::from_str(&ack)
            .with_context(|| format!("解析 Herdr events.subscribe ack 失败: {ack}"))?;
        if let Some(error) = ack.get("error") {
            bail!("Herdr events.subscribe 失败: {error}");
        }
        if ack.get("result").is_none() {
            bail!("Herdr events.subscribe ack 缺 result: {ack}");
        }
        reader
            .get_ref()
            .set_read_timeout(None)
            .context("清除 Herdr event 读超时失败")?;

        let workspace_id = workspace_id.to_string();
        let dropping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_dropping = std::sync::Arc::clone(&dropping);
        let handle = std::thread::spawn(move || loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    if !reader_dropping.load(std::sync::atomic::Ordering::Acquire) {
                        let _ = tx.send(EventStreamEvent::Closed);
                    }
                    return;
                }
                Ok(_) if line.trim().is_empty() => continue,
                Ok(_) => {
                    let event = match HerdrEvent::parse_line(line.trim_end()) {
                        Ok(event) => event,
                        Err(err) => {
                            if !reader_dropping.load(std::sync::atomic::Ordering::Acquire) {
                                let _ = tx.send(EventStreamEvent::Error(err.to_string()));
                            }
                            continue;
                        }
                    };
                    if let Some(layout) = event.layout() {
                        if layout.workspace_id == workspace_id
                            && tx.send(EventStreamEvent::Layout(layout)).is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    if !event.kind.refreshes_snapshot() || !event.affects_workspace(&workspace_id) {
                        continue;
                    }
                    match session.snapshot() {
                        Ok(snapshot) => {
                            if tx
                                .send(EventStreamEvent::Snapshot {
                                    cause: event.kind,
                                    snapshot,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(err) => {
                            if tx
                                .send(EventStreamEvent::Error(format!(
                                    "Herdr event 后刷新 snapshot 失败: {err}"
                                )))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
                Err(err) => {
                    if !reader_dropping.load(std::sync::atomic::Ordering::Acquire) {
                        let _ = tx.send(EventStreamEvent::Error(err.to_string()));
                    }
                    return;
                }
            }
        });

        Ok(Self {
            shutdown_stream: Some(shutdown_stream),
            handle: Some(handle),
            dropping,
        })
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        // 先置位：reader 不再上报 EOF/Error，否则 restart 替换订阅会
        // 残留一个假的 Closed/Error，触发无限重建循环。
        self.dropping
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(stream) = self.shutdown_stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_global_subscription_has_a_known_kind() {
        for kind in GLOBAL_SUBSCRIPTIONS {
            assert!(
                !matches!(HerdrEventKind::from_wire(kind), HerdrEventKind::Unknown(_)),
                "未解析 subscription kind {kind}"
            );
        }
    }

    #[test]
    fn every_protocol_19_event_kind_accepts_dot_and_snake_names() {
        let cases = [
            ("workspace.created", HerdrEventKind::WorkspaceCreated),
            ("workspace.updated", HerdrEventKind::WorkspaceUpdated),
            (
                "workspace.metadata_updated",
                HerdrEventKind::WorkspaceMetadataUpdated,
            ),
            ("workspace.closed", HerdrEventKind::WorkspaceClosed),
            ("workspace.renamed", HerdrEventKind::WorkspaceRenamed),
            ("workspace.moved", HerdrEventKind::WorkspaceMoved),
            ("workspace.reordered", HerdrEventKind::WorkspaceReordered),
            ("workspace.focused", HerdrEventKind::WorkspaceFocused),
            ("worktree.created", HerdrEventKind::WorktreeCreated),
            ("worktree.opened", HerdrEventKind::WorktreeOpened),
            ("worktree.removed", HerdrEventKind::WorktreeRemoved),
            ("tab.created", HerdrEventKind::TabCreated),
            ("tab.closed", HerdrEventKind::TabClosed),
            ("tab.renamed", HerdrEventKind::TabRenamed),
            ("tab.moved", HerdrEventKind::TabMoved),
            ("tab.focused", HerdrEventKind::TabFocused),
            ("pane.created", HerdrEventKind::PaneCreated),
            ("pane.closed", HerdrEventKind::PaneClosed),
            ("pane.updated", HerdrEventKind::PaneUpdated),
            ("pane.focused", HerdrEventKind::PaneFocused),
            ("pane.moved", HerdrEventKind::PaneMoved),
            ("pane.output_changed", HerdrEventKind::PaneOutputChanged),
            ("pane.exited", HerdrEventKind::PaneExited),
            ("pane.agent_detected", HerdrEventKind::PaneAgentDetected),
            (
                "pane.agent_status_changed",
                HerdrEventKind::PaneAgentStatusChanged,
            ),
            ("layout.updated", HerdrEventKind::LayoutUpdated),
        ];

        for (dot, expected) in cases {
            assert_eq!(HerdrEventKind::from_wire(dot), expected, "dot={dot}");
            let snake = dot.replace('.', "_");
            assert_eq!(HerdrEventKind::from_wire(&snake), expected, "snake={snake}");
        }
    }

    #[test]
    fn agent_event_accepts_scoped_dot_and_global_snake_names() {
        for kind in ["pane.agent_status_changed", "pane_agent_status_changed"] {
            let event = HerdrEvent::parse_line(&format!(
                r#"{{"event":"{kind}","data":{{"pane_id":"w1:p1","workspace_id":"w1","agent_status":"blocked","agent":"codex","title":"Approve","display_agent":"Codex","state_labels":{{"blocked":"Needs approval"}}}}}}"#
            ))
            .unwrap();
            assert_eq!(event.kind, HerdrEventKind::PaneAgentStatusChanged);
            assert!(event.affects_workspace("w1"));
            assert_eq!(event.data["state_labels"]["blocked"], "Needs approval");
        }
    }

    #[test]
    fn unknown_event_is_preserved_instead_of_misclassified() {
        let event = HerdrEvent::parse_line(
            r#"{"event":"pane.future_signal","data":{"workspace_id":"w1","extra":7}}"#,
        )
        .unwrap();
        assert_eq!(
            event.kind,
            HerdrEventKind::Unknown("pane.future_signal".into())
        );
        assert_eq!(event.data["extra"], 7);
    }

    #[test]
    fn pane_move_affects_both_previous_and_current_workspaces() {
        let event = HerdrEvent::parse_line(
            r#"{"event":"pane_moved","data":{"type":"pane_moved","previous_workspace_id":"w1","pane":{"workspace_id":"w2"}}}"#,
        )
        .unwrap();
        assert!(event.affects_workspace("w1"));
        assert!(event.affects_workspace("w2"));
        assert!(!event.affects_workspace("w3"));
    }

    #[test]
    fn workspace_filter_covers_direct_nested_reordered_and_wait_envelopes() {
        let lines = [
            r#"{"event":"workspace_closed","data":{"workspace_id":"w1"}}"#,
            r#"{"event":"tab_created","data":{"tab":{"workspace_id":"w1"}}}"#,
            r#"{"event":"pane_updated","data":{"pane":{"workspace_id":"w1"}}}"#,
            r#"{"event":"layout_updated","data":{"layout":{"workspace_id":"w1"}}}"#,
            r#"{"event":"workspace_reordered","data":{"workspace_ids":["w2","w1"]}}"#,
            r#"{"event":{"type":"pane.agent_status_changed","data":{"workspace_id":"w1","pane_id":"w1:p1","agent_status":"done"}}}"#,
        ];
        for line in lines {
            let event = HerdrEvent::parse_line(line).unwrap();
            assert!(event.affects_workspace("w1"), "line={line}");
            assert!(!event.affects_workspace("w9"), "line={line}");
        }
    }
}
