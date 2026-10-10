//! Herdr API event subscription and wire normalization.
//!
//! Subscription names (`pane.agent_status_changed`) and global event names
//! (`pane_agent_status_changed`) are both contained here. Runtime/Workspace only
//! receive typed snapshots/layouts and never need to recognize either spelling.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use super::channel::{shutdown, ChannelIo, SharedChannel};
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

/// 同一波 topology 事件只打一次 `session.snapshot`。
///
/// test-2026-1010-1103：输入 grok 后 agent/pane 事件挤在一条订阅里。
/// 每条都新开一条 SSH snapshot，慢的时候一条 5 秒；失败再被当成订阅死亡，
/// poll 丢掉这条流时会把已经排上队的下一次 snapshot 一起等完。界面就冻住。
struct SnapshotCoalesce {
    pending: Option<PendingSnapshot>,
    not_before: Instant,
}

struct PendingSnapshot {
    cause: HerdrEventKind,
    started: Instant,
    latest: Instant,
}

impl SnapshotCoalesce {
    /// 这一波里没有新事件，就可以打 snapshot。
    const QUIET: Duration = Duration::from_millis(50);
    /// 事件一直在来时也不能无限推迟，到点打一次。
    const HOLD: Duration = Duration::from_millis(300);
    /// snapshot 读失败后先歇一拍。紧接着重试会把同一条 SSH 再堵死。
    const BACKOFF: Duration = Duration::from_secs(1);

    fn new(now: Instant) -> Self {
        Self {
            pending: None,
            not_before: now,
        }
    }

    fn push(&mut self, cause: HerdrEventKind, now: Instant) {
        match &mut self.pending {
            Some(pending) => {
                pending.cause = cause;
                pending.latest = now;
            }
            None => {
                self.pending = Some(PendingSnapshot {
                    cause,
                    started: now,
                    latest: now,
                });
            }
        }
    }

    /// 安静窗口过了，或者这一波已经攒到 HOLD，就取出一次刷新。
    fn take_ready(&mut self, now: Instant) -> Option<HerdrEventKind> {
        let pending = self.pending.as_ref()?;
        if now < self.not_before {
            return None;
        }
        let quiet = now.saturating_duration_since(pending.latest) >= Self::QUIET;
        let held = now.saturating_duration_since(pending.started) >= Self::HOLD;
        if quiet || held {
            self.pending.take().map(|pending| pending.cause)
        } else {
            None
        }
    }

    fn note_success(&mut self, now: Instant) {
        self.pending = None;
        self.not_before = now + Self::HOLD;
    }

    fn note_failure(&mut self, cause: HerdrEventKind, now: Instant) {
        self.not_before = now + Self::BACKOFF;
        self.pending = Some(PendingSnapshot {
            cause,
            started: now,
            latest: now,
        });
    }

    /// 有待刷新时，读订阅的最长等待。`None` 表示一直阻塞到下一行。
    fn poll_timeout(&self, now: Instant) -> Option<Duration> {
        let pending = self.pending.as_ref()?;
        if now < self.not_before {
            return Some(self.not_before.saturating_duration_since(now));
        }
        let quiet_left = Self::QUIET.saturating_sub(now.saturating_duration_since(pending.latest));
        let hold_left = Self::HOLD.saturating_sub(now.saturating_duration_since(pending.started));
        let wait = quiet_left.min(hold_left);
        Some(if wait.is_zero() {
            Duration::from_millis(1)
        } else {
            wait
        })
    }
}

/// Event reader thread → HerdrRuntime.
#[derive(Debug, Clone, PartialEq)]
pub enum EventStreamEvent {
    Snapshot {
        cause: HerdrEventKind,
        snapshot: SessionSnapshot,
    },
    Layout(LayoutRecord),
    /// snapshot RPC 失败。订阅 socket 还活着，不能据此拆掉 event stream。
    RefreshFailed(String),
    Closed,
    Error(String),
}

/// 处理订阅里的一行。`false`：channel 已关，reader 该退出。
fn accept_event_line(
    line: &str,
    workspace_id: &str,
    dropping: &AtomicBool,
    tx: &Sender<EventStreamEvent>,
    coalesce: &mut SnapshotCoalesce,
) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return true;
    }
    let event = match HerdrEvent::parse_line(line) {
        Ok(event) => event,
        Err(err) => {
            if !dropping.load(Ordering::Acquire) {
                let _ = tx.send(EventStreamEvent::Error(err.to_string()));
            }
            return true;
        }
    };
    if let Some(layout) = event.layout() {
        if layout.workspace_id == workspace_id && tx.send(EventStreamEvent::Layout(layout)).is_err()
        {
            return false;
        }
        return true;
    }
    if event.kind.refreshes_snapshot() && event.affects_workspace(workspace_id) {
        coalesce.push(event.kind, Instant::now());
    }
    true
}

/// One API-socket events.subscribe stream for a bound Herdr workspace.
pub struct EventStream {
    shutdown_channel: Option<SharedChannel>,
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
        let channel = session.open_socket_channel(session.socket_path())?;
        let mut stream = ChannelIo::with_read_timeout(channel.clone(), Duration::from_secs(5));

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
        reader.get_mut().clear_read_timeout();

        let workspace_id = workspace_id.to_string();
        let dropping = std::sync::Arc::new(AtomicBool::new(false));
        let reader_dropping = std::sync::Arc::clone(&dropping);
        let handle = std::thread::spawn(move || {
            let mut coalesce = SnapshotCoalesce::new(Instant::now());
            loop {
                if reader_dropping.load(Ordering::Acquire) {
                    return;
                }
                // 安静窗口到了才打一次 snapshot。替换订阅时这张过期快照
                // 不能再送回去，否则分屏结果会被旧拓扑盖回去。
                if let Some(cause) = coalesce.take_ready(Instant::now()) {
                    if reader_dropping.load(Ordering::Acquire) {
                        return;
                    }
                    match session.snapshot() {
                        Ok(snapshot) => {
                            if reader_dropping.load(Ordering::Acquire) {
                                return;
                            }
                            if tx
                                .send(EventStreamEvent::Snapshot { cause, snapshot })
                                .is_err()
                            {
                                return;
                            }
                            coalesce.note_success(Instant::now());
                        }
                        Err(err) => {
                            if reader_dropping.load(Ordering::Acquire) {
                                return;
                            }
                            // 订阅 socket 还在读。这次 RPC 失败不能报 Error，
                            // 否则 Runtime 会拆掉订阅再重启，慢 SSH 上又堵一轮。
                            if tx
                                .send(EventStreamEvent::RefreshFailed(format!(
                                    "Herdr event 后刷新 snapshot 失败: {err}"
                                )))
                                .is_err()
                            {
                                return;
                            }
                            coalesce.note_failure(cause, Instant::now());
                        }
                    }
                    continue;
                }

                match coalesce.poll_timeout(Instant::now()) {
                    Some(timeout) => reader.get_mut().set_read_timeout(Some(timeout)),
                    None => reader.get_mut().clear_read_timeout(),
                }
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        if !reader_dropping.load(Ordering::Acquire) {
                            let _ = tx.send(EventStreamEvent::Closed);
                        }
                        return;
                    }
                    Err(err) if err.kind() == ErrorKind::TimedOut => continue,
                    Err(err) => {
                        if !reader_dropping.load(Ordering::Acquire) {
                            let _ = tx.send(EventStreamEvent::Error(err.to_string()));
                        }
                        return;
                    }
                    Ok(_) => {
                        if !accept_event_line(
                            &line,
                            &workspace_id,
                            &reader_dropping,
                            &tx,
                            &mut coalesce,
                        ) {
                            return;
                        }
                        // 同一次读进来的后续行并成这一波，不要每行一条 snapshot。
                        while reader.buffer().contains(&b'\n') {
                            let mut extra = String::new();
                            match reader.read_line(&mut extra) {
                                Ok(0) => {
                                    if !reader_dropping.load(Ordering::Acquire) {
                                        let _ = tx.send(EventStreamEvent::Closed);
                                    }
                                    return;
                                }
                                Ok(_) => {
                                    if !accept_event_line(
                                        &extra,
                                        &workspace_id,
                                        &reader_dropping,
                                        &tx,
                                        &mut coalesce,
                                    ) {
                                        return;
                                    }
                                }
                                Err(err) if err.kind() == ErrorKind::TimedOut => break,
                                Err(err) => {
                                    if !reader_dropping.load(Ordering::Acquire) {
                                        let _ = tx.send(EventStreamEvent::Error(err.to_string()));
                                    }
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            shutdown_channel: Some(channel),
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
        if let Some(channel) = self.shutdown_channel.take() {
            let _ = shutdown(&channel);
        }
        // 不 join。reader 可能正卡在 session.snapshot()：分屏时这次 RPC
        // 和 pane.split 抢同一条 SSH，墙钟能到十几秒。join 会把 poll
        // 线程冻住，界面在这段时间里不能画、也不能收输入。
        // 与 ObserveStream 一样只 detach，让慢 RPC 自己结束。
        self.handle.take();
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

    /// test-2026-1010-1103：分屏替换订阅时，旧 reader 卡在 session.snapshot()。
    /// Drop 若 join，poll 线程会跟着冻住十几秒。
    #[test]
    fn dropping_event_stream_does_not_wait_for_inflight_snapshot() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::time::Instant;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("muxterm-evtdrop-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("api.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let entered = std::sync::Arc::new(AtomicBool::new(false));
        let entered_flag = std::sync::Arc::clone(&entered);
        let server = std::thread::spawn(move || {
            let (subscribe, _) = listener.accept().unwrap();
            let mut subscribe = BufReader::new(subscribe);
            let mut line = String::new();
            subscribe.read_line(&mut line).unwrap();
            assert!(
                line.contains("events.subscribe"),
                "first connection must be the event stream: {line}"
            );
            subscribe.get_mut().write_all(b"{\"result\":{}}\n").unwrap();
            subscribe
                .get_mut()
                .write_all(b"{\"event\":\"pane.created\",\"data\":{\"workspace_id\":\"w4\"}}\n")
                .unwrap();
            subscribe.get_mut().flush().ok();

            let (snapshot, _) = listener.accept().unwrap();
            let mut snapshot = BufReader::new(snapshot);
            line.clear();
            snapshot.read_line(&mut line).unwrap();
            assert!(
                line.contains("session.snapshot"),
                "second connection must be the snapshot refresh: {line}"
            );
            entered_flag.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(700));
            snapshot
                .get_mut()
                .write_all(b"{\"result\":{\"snapshot\":{\"version\":\"0.8.0\",\"protocol\":19}}}\n")
                .unwrap();
            snapshot.get_mut().flush().ok();
            // snapshot() 返回后才会决定要不要把结果送进 Runtime。留一点
            // 时间让 reader 走完这条路径，再看 channel 里有没有晚到的快照。
            std::thread::sleep(Duration::from_millis(100));
        });

        let session = std::sync::Arc::new(HerdrSession::new("test", &socket));
        let (tx, rx) = mpsc::channel();
        let stream = EventStream::start(session, "w4", &[], tx).expect("start event stream");
        let wait_started = Instant::now();
        while !entered.load(Ordering::Acquire) {
            assert!(
                wait_started.elapsed() < Duration::from_secs(2),
                "snapshot RPC did not start"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        let dropped_at = Instant::now();
        drop(stream);
        let elapsed = dropped_at.elapsed();
        assert!(
            elapsed < Duration::from_millis(200),
            "drop waited for the in-flight snapshot: {elapsed:?}"
        );
        server.join().unwrap();
        match rx.try_recv() {
            Err(_) => {}
            Ok(event) => {
                panic!("a snapshot that finishes after drop must not reach the runtime: {event:?}")
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshot_coalesce_collapses_a_burst_and_backs_off_after_failure() {
        let start = Instant::now();
        let mut coalesce = SnapshotCoalesce::new(start);
        for _ in 0..5 {
            coalesce.push(HerdrEventKind::PaneUpdated, start);
        }
        coalesce.push(
            HerdrEventKind::PaneAgentStatusChanged,
            start + Duration::from_millis(10),
        );
        assert!(coalesce
            .take_ready(start + Duration::from_millis(10))
            .is_none());
        assert_eq!(
            coalesce.take_ready(start + Duration::from_millis(10) + SnapshotCoalesce::QUIET),
            Some(HerdrEventKind::PaneAgentStatusChanged)
        );
        assert!(coalesce
            .take_ready(start + Duration::from_millis(10) + SnapshotCoalesce::QUIET)
            .is_none());

        // 事件不停地来，安静窗口永远不满，到 HOLD 也必须打一次，而且是最后一条。
        let mut held = SnapshotCoalesce::new(start);
        held.push(HerdrEventKind::PaneUpdated, start);
        held.push(
            HerdrEventKind::PaneAgentStatusChanged,
            start + SnapshotCoalesce::HOLD - Duration::from_millis(10),
        );
        assert!(held
            .take_ready(start + SnapshotCoalesce::HOLD - Duration::from_millis(10))
            .is_none());
        assert_eq!(
            held.take_ready(start + SnapshotCoalesce::HOLD),
            Some(HerdrEventKind::PaneAgentStatusChanged)
        );

        let failed_at = start + Duration::from_millis(80);
        coalesce.note_failure(HerdrEventKind::PaneAgentStatusChanged, failed_at);
        assert!(coalesce
            .take_ready(failed_at + SnapshotCoalesce::BACKOFF - Duration::from_millis(1))
            .is_none());
        assert_eq!(
            coalesce.take_ready(failed_at + SnapshotCoalesce::BACKOFF),
            Some(HerdrEventKind::PaneAgentStatusChanged)
        );

        let ok_at = failed_at + SnapshotCoalesce::BACKOFF;
        coalesce.note_success(ok_at);
        coalesce.push(
            HerdrEventKind::PaneUpdated,
            ok_at + Duration::from_millis(10),
        );
        assert!(coalesce
            .take_ready(ok_at + SnapshotCoalesce::QUIET)
            .is_none());
        assert_eq!(
            coalesce.take_ready(ok_at + SnapshotCoalesce::HOLD + SnapshotCoalesce::QUIET),
            Some(HerdrEventKind::PaneUpdated)
        );
    }

    /// test-2026-1010-1103：grok 启动连发 agent status。一波只开一条 snapshot。
    #[test]
    fn agent_status_burst_opens_one_snapshot() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc;

        const OK: &[u8] = b"{\"result\":{\"snapshot\":{\"version\":\"0.8.0\",\"protocol\":19}}}\n";
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("muxterm-evtburst-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("api.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let phase = std::sync::Arc::new(AtomicUsize::new(0));
        let count_srv = std::sync::Arc::clone(&count);
        let phase_srv = std::sync::Arc::clone(&phase);
        let server = std::thread::spawn(move || {
            let (subscribe, _) = listener.accept().unwrap();
            let mut subscribe = BufReader::new(subscribe);
            let mut line = String::new();
            subscribe.read_line(&mut line).unwrap();
            assert!(line.contains("events.subscribe"), "{line}");
            let mut burst = String::from("{\"result\":{}}\n");
            for _ in 0..6 {
                burst.push_str(
                    "{\"event\":\"pane.agent_status_changed\",\"data\":{\"workspace_id\":\"w4\",\"pane_id\":\"w4:p1Z\",\"agent_status\":\"working\"}}\n",
                );
            }
            subscribe.get_mut().write_all(burst.as_bytes()).unwrap();
            subscribe.get_mut().flush().ok();

            let phase_snap = std::sync::Arc::clone(&phase_srv);
            let snapshots = std::thread::spawn(move || {
                answer_snapshot_connections(listener, count_srv, phase_snap, OK, OK);
            });
            while phase_srv.load(Ordering::Acquire) == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            if phase_srv.load(Ordering::Acquire) == 1 {
                subscribe
                    .get_mut()
                    .write_all(b"{\"event\":\"pane.updated\",\"data\":{\"pane\":{\"workspace_id\":\"w4\"}}}\n")
                    .unwrap();
                subscribe.get_mut().flush().ok();
                while phase_srv.load(Ordering::Acquire) == 1 {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            snapshots.join().unwrap();
        });

        let session = std::sync::Arc::new(HerdrSession::new("test", &socket));
        let (tx, _rx) = mpsc::channel();
        let stream = EventStream::start(session, "w4", &[], tx).expect("start event stream");
        let started = Instant::now();
        while count.load(Ordering::Acquire) < 1 {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "snapshot RPC did not start"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            count.load(Ordering::Acquire),
            1,
            "a grok status burst must not open one snapshot per event"
        );
        phase.store(1, Ordering::Release);
        while count.load(Ordering::Acquire) < 2 {
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "the event after the quiet window did not refresh"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(count.load(Ordering::Acquire), 2);
        drop(stream);
        phase.store(2, Ordering::Release);
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// snapshot 读失败只记一笔，订阅继续读。下一次刷新仍然走同一条流。
    #[test]
    fn snapshot_refresh_failure_keeps_the_event_subscription() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc;

        const OK: &[u8] = b"{\"result\":{\"snapshot\":{\"version\":\"0.8.0\",\"protocol\":19}}}\n";
        const ERR: &[u8] = b"{\"error\":{\"message\":\"busy\"}}\n";
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("muxterm-evtfail-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("api.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let phase = std::sync::Arc::new(AtomicUsize::new(0));
        let count_srv = std::sync::Arc::clone(&count);
        let phase_srv = std::sync::Arc::clone(&phase);
        let server = std::thread::spawn(move || {
            let (subscribe, _) = listener.accept().unwrap();
            let mut subscribe = BufReader::new(subscribe);
            let mut line = String::new();
            subscribe.read_line(&mut line).unwrap();
            assert!(line.contains("events.subscribe"), "{line}");
            let mut burst = String::from("{\"result\":{}}\n");
            for _ in 0..4 {
                burst.push_str(
                    "{\"event\":\"pane.agent_status_changed\",\"data\":{\"workspace_id\":\"w4\",\"pane_id\":\"w4:p1Z\",\"agent_status\":\"working\"}}\n",
                );
            }
            subscribe.get_mut().write_all(burst.as_bytes()).unwrap();
            subscribe.get_mut().flush().ok();
            let phase_snap = std::sync::Arc::clone(&phase_srv);
            let snapshots = std::thread::spawn(move || {
                answer_snapshot_connections(listener, count_srv, phase_snap, ERR, OK);
            });
            while phase_srv.load(Ordering::Acquire) < 2 {
                std::thread::sleep(Duration::from_millis(5));
            }
            snapshots.join().unwrap();
        });

        let session = std::sync::Arc::new(HerdrSession::new("test", &socket));
        let (tx, rx) = mpsc::channel();
        let stream = EventStream::start(session, "w4", &[], tx).expect("start event stream");
        let started = Instant::now();
        while count.load(Ordering::Acquire) < 1 {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "snapshot RPC did not start"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            count.load(Ordering::Acquire),
            1,
            "a failed refresh must not immediately open another snapshot"
        );
        while count.load(Ordering::Acquire) < 2 {
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "backoff did not retry the refresh on the same subscription"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut saw_failure = false;
        let mut saw_snapshot = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && !(saw_failure && saw_snapshot) {
            match rx.try_recv() {
                Ok(EventStreamEvent::RefreshFailed(message)) => {
                    assert!(
                        message.contains("session.snapshot"),
                        "failure must name the snapshot RPC: {message}"
                    );
                    saw_failure = true;
                }
                Ok(EventStreamEvent::Snapshot { .. }) => saw_snapshot = true,
                Ok(EventStreamEvent::Error(message)) => {
                    panic!("snapshot failure closed the subscription: {message}")
                }
                Ok(EventStreamEvent::Closed) => panic!("snapshot failure closed the subscription"),
                Ok(other) => panic!("unexpected event: {other:?}"),
                Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
                Err(mpsc::TryRecvError::Disconnected) => panic!("event reader exited"),
            }
        }
        assert!(saw_failure, "runtime must hear that the refresh failed");
        assert!(saw_snapshot, "the same subscription must deliver the retry");
        assert_eq!(
            count.load(Ordering::Acquire),
            2,
            "retry is one refresh, not another burst"
        );
        drop(stream);
        phase.store(2, Ordering::Release);
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn answer_snapshot_connections(
        listener: std::os::unix::net::UnixListener,
        count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        phase: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        first: &'static [u8],
        later: &'static [u8],
    ) {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::atomic::Ordering;

        listener.set_nonblocking(true).unwrap();
        loop {
            if phase.load(Ordering::Acquire) >= 2 {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    // 非阻塞 listener accept 出来的 fd 会带上 O_NONBLOCK。
                    stream.set_nonblocking(false).unwrap();
                    let n = count.fetch_add(1, Ordering::AcqRel) + 1;
                    let mut stream = BufReader::new(stream);
                    stream
                        .get_ref()
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .ok();
                    let mut line = String::new();
                    stream.read_line(&mut line).unwrap();
                    assert!(
                        line.contains("session.snapshot"),
                        "snapshot connection must be session.snapshot: {line}"
                    );
                    let body = if n == 1 { first } else { later };
                    stream.get_mut().write_all(body).unwrap();
                    stream.get_mut().flush().ok();
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => panic!("accept snapshot: {err}"),
            }
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
