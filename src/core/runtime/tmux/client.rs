#![allow(clippy::while_let_loop)]
//! 异步 tmux `-CC` 客户端。
//!
//! 封装与 `tmux -CC` 子进程的通信：
//!
//! - [`TmuxClient::spawn`]：spawn tmux 子进程到一对 pty（tmux 在 `-CC` 模式下
//!   仍需要 tty，否则 `tcgetattr failed` 立即退出）。
//! - 后台 task 读 pty master 端，按**真换行**切行（`%output` content 里的 `\n`
//!   是 C 转义后的两个字符，不是真换行），逐行喂给
//!   [`parse_line_bytes`](super::protocol::parse_line_bytes)，产出 `Message` 事件流。
//! - [`TmuxClientHandle::send_command`]：把命令字符串写到 tmux stdin（pty）。
//! - 通过非阻塞的 [`tokio::sync::mpsc`] 输出 `TmuxEvent` 事件；命令响应正文在
//!   reader 内聚合成一个 `ResponseBlock`，避免大 `capture-pane` 按行堵塞 reader。
//! - 优雅关闭：`detach` / `kill`。
//!
//! 半行 buffer 处理：tmux 一次 write 到 pty 可能只写半行，必须按真换行符
//! （`\n`，tmux 实际用 `\r\n`）切包，把不完整的尾段留到下次。

use super::command::TmuxCommand;
use super::protocol::{parse_line_bytes, Message, NotificationKind};
use super::pty::{self, split_master, PtyChild, PtyReader, PtyWriter};
use crate::core::buffer_cap::{trim_incomplete_line, MAX_INCOMPLETE_LINE_BYTES};
use crate::core::types::PaneId;
use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::process::{Child, ChildStdout};
use tokio::sync::mpsc;

/// tmux reader → runtime 的事件发送端。
///
/// 控制事件（响应边界、结构通知、错误和退出）走无界 lane，不能因为 UI
/// 暂时没 poll 就丢失；pane output 走有界 lane，满时丢弃增量并发出一次
/// [`TmuxEvent::OutputGap`]。这样 reader 仍然持续排空 tmux control stream，
/// 而不会把持续输出无限堆在内存里。
#[derive(Debug)]
struct EventEnvelope {
    /// Number of accepted output events that were written before this event on
    /// the wire. Control events use this as a delivery fence; output events use
    /// `output_ordinal` below.
    output_watermark: u64,
    output_ordinal: Option<u64>,
    event: TmuxEvent,
}

#[derive(Debug, Default)]
struct SenderState {
    accepted_output: u64,
}

#[derive(Clone)]
pub struct TmuxEventSender {
    control_tx: mpsc::UnboundedSender<EventEnvelope>,
    output_tx: mpsc::Sender<EventEnvelope>,
    output_gaps: Arc<Mutex<HashSet<PaneId>>>,
    state: Arc<Mutex<SenderState>>,
}

/// tmux reader → runtime 的事件接收端。
///
/// `try_recv`/`recv` 用 output watermark 恢复两条 lane 的 wire 顺序：控制事件
/// 只有在它之前已经接受的 output 被交付后才会返回。output lane 允许丢增量；
/// 收到 OutputGap 后由 backend 发起有界 authoritative resync。
pub struct TmuxEventReceiver {
    control_rx: mpsc::UnboundedReceiver<EventEnvelope>,
    output_rx: mpsc::Receiver<EventEnvelope>,
    /// Output events drained from the bounded channel while discarding a pane's
    /// pre-gap suffix. Kept here so other panes retain their FIFO ordering.
    output_pending: VecDeque<EventEnvelope>,
    output_gaps: Arc<Mutex<HashSet<PaneId>>>,
    control_pending: Option<EventEnvelope>,
    consumed_output: u64,
    control_open: bool,
    output_open: bool,
}

// Keep enough burst headroom for normal repaint traffic while ensuring a
// stalled UI reaches an explicit OutputGap quickly instead of accumulating an
// unbounded tail. The gap is recovered by an authoritative pane snapshot.
const OUTPUT_EVENT_BUFFER: usize = 64;

/// 创建拆分后的 control/output 事件通道。
pub fn event_channel() -> (TmuxEventSender, TmuxEventReceiver) {
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::channel(OUTPUT_EVENT_BUFFER);
    let output_gaps = Arc::new(Mutex::new(HashSet::new()));
    let state = Arc::new(Mutex::new(SenderState::default()));
    (
        TmuxEventSender {
            control_tx,
            output_tx,
            output_gaps: output_gaps.clone(),
            state: state.clone(),
        },
        TmuxEventReceiver {
            control_rx,
            output_rx,
            output_pending: VecDeque::new(),
            output_gaps,
            control_pending: None,
            consumed_output: 0,
            control_open: true,
            output_open: true,
        },
    )
}

impl TmuxEventSender {
    fn send_event(&self, event: TmuxEvent) {
        match event {
            TmuxEvent::Message(
                message @ (Message::Output { pane, .. } | Message::ExtendedOutput { pane, .. }),
            ) => self.send_output(pane, TmuxEvent::Message(message)),
            other => self.send_control(other),
        }
    }

    fn send_message(&self, message: Message) {
        match message {
            output @ (Message::Output { pane, .. } | Message::ExtendedOutput { pane, .. }) => {
                self.send_output(pane, TmuxEvent::Message(output));
            }
            message => self.send_control(TmuxEvent::Message(message)),
        }
    }

    fn send_control(&self, event: TmuxEvent) {
        // Keep the watermark read and control send under the same mutex as
        // output acceptance. This preserves the wire order even if a cloned
        // sender is ever used by more than one parser task.
        let state = self
            .state
            .lock()
            .expect("event sender state mutex poisoned");
        let _ = self.control_tx.send(EventEnvelope {
            output_watermark: state.accepted_output,
            output_ordinal: None,
            event,
        });
    }

    fn send_output(&self, pane: PaneId, event: TmuxEvent) {
        // Once a pane has an outstanding gap, its bounded lane is no longer a
        // useful source of state: the backend will replace it with an
        // authoritative snapshot.  Drop the suffix immediately instead of
        // continuing to accept output and advancing the control watermark.
        // Otherwise a continuously-chatty pane can keep every resync response
        // behind an ever-growing output fence and make the UI appear hung.
        if self
            .output_gaps
            .lock()
            .expect("output gap mutex poisoned")
            .contains(&pane)
        {
            return;
        }
        let mut state = self
            .state
            .lock()
            .expect("event sender state mutex poisoned");
        let ordinal = state.accepted_output.saturating_add(1);
        let envelope = EventEnvelope {
            output_watermark: ordinal,
            output_ordinal: Some(ordinal),
            event,
        };
        match self.output_tx.try_send(envelope) {
            Ok(()) => state.accepted_output = ordinal,
            Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
                let mut gaps = self.output_gaps.lock().expect("output gap mutex poisoned");
                if gaps.insert(pane) {
                    let _ = self.control_tx.send(EventEnvelope {
                        output_watermark: state.accepted_output,
                        output_ordinal: None,
                        event: TmuxEvent::OutputGap { pane },
                    });
                }
            }
        }
    }
}

/// Internal abstraction keeps parser unit tests on a plain unbounded channel while
/// production readers use the split bounded channel above.
pub(crate) trait TmuxEventSink {
    fn emit(&self, event: TmuxEvent);
    fn emit_message(&self, message: Message) {
        self.emit(TmuxEvent::Message(message));
    }
    /// 把同一 pane 尚未发出的合并 %output 刷进下游。生产 reader 在每个
    /// pty chunk 结束时调用，避免连续 TUI 刷新被扣在 coalescer 里。
    fn flush(&self) {}
}

impl TmuxEventSink for TmuxEventSender {
    fn emit(&self, event: TmuxEvent) {
        self.send_event(event);
    }

    fn emit_message(&self, message: Message) {
        self.send_message(message);
    }
}

impl TmuxEventSink for mpsc::UnboundedSender<TmuxEvent> {
    fn emit(&self, event: TmuxEvent) {
        let _ = self.send(event);
    }
}

/// 同一 pane 连续 `%output` 在进入有界 lane 前合并成一块。
///
/// tmux 控制协议按行推送；Codex/htop 一次重绘可以产生远超
/// [`OUTPUT_EVENT_BUFFER`] 条 `%output`。若不合并，共享 64 槽会立刻
/// OutputGap，backend 再 pause + 大 capture，表现为卡死后再整屏重绘。
const OUTPUT_COALESCE_MAX_BYTES: usize = 32 * 1024;

pub(crate) struct OutputBatcher<S: TmuxEventSink> {
    inner: S,
    pending: Mutex<Option<TmuxEvent>>,
}

impl<S: TmuxEventSink> OutputBatcher<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self {
            inner,
            pending: Mutex::new(None),
        }
    }
}

impl<S: TmuxEventSink> TmuxEventSink for OutputBatcher<S> {
    fn emit(&self, event: TmuxEvent) {
        if output_event_pane(&event).is_some() {
            let mut pending = self.pending.lock().expect("output batcher mutex poisoned");
            if let Some(existing) = pending.as_mut() {
                if try_append_output(existing, &event) {
                    if output_event_bytes(existing) >= OUTPUT_COALESCE_MAX_BYTES {
                        let flushed = pending.take().expect("coalesced output just checked");
                        drop(pending);
                        self.inner.emit(flushed);
                    }
                    return;
                }
            }
            let previous = pending.replace(event);
            drop(pending);
            if let Some(previous) = previous {
                self.inner.emit(previous);
            }
            return;
        }
        self.flush();
        self.inner.emit(event);
    }

    fn flush(&self) {
        let event = self
            .pending
            .lock()
            .expect("output batcher mutex poisoned")
            .take();
        if let Some(event) = event {
            self.inner.emit(event);
        }
    }
}

fn output_event_pane(event: &TmuxEvent) -> Option<PaneId> {
    match event {
        TmuxEvent::Message(Message::Output { pane, .. } | Message::ExtendedOutput { pane, .. }) => {
            Some(*pane)
        }
        _ => None,
    }
}

fn output_event_bytes(event: &TmuxEvent) -> usize {
    match event {
        TmuxEvent::Message(
            Message::Output { content, .. } | Message::ExtendedOutput { content, .. },
        ) => content.len(),
        _ => 0,
    }
}

fn try_append_output(dst: &mut TmuxEvent, src: &TmuxEvent) -> bool {
    match (dst, src) {
        (
            TmuxEvent::Message(Message::Output {
                pane: dst_pane,
                content,
                raw_content,
            }),
            TmuxEvent::Message(Message::Output {
                pane: src_pane,
                content: more,
                raw_content: more_raw,
            }),
        ) if dst_pane == src_pane => {
            content.extend_from_slice(more);
            raw_content.push_str(more_raw);
            true
        }
        (
            TmuxEvent::Message(Message::ExtendedOutput {
                pane: dst_pane,
                content,
                raw_content,
                age_ms,
            }),
            TmuxEvent::Message(Message::ExtendedOutput {
                pane: src_pane,
                content: more,
                raw_content: more_raw,
                age_ms: src_age,
            }),
        ) if dst_pane == src_pane => {
            content.extend_from_slice(more);
            raw_content.push_str(more_raw);
            if *src_age > *age_ms {
                *age_ms = *src_age;
            }
            true
        }
        _ => false,
    }
}

impl TmuxEventReceiver {
    fn consume_output(&mut self, envelope: EventEnvelope) -> TmuxEvent {
        if let Some(ordinal) = envelope.output_ordinal {
            self.consumed_output = self.consumed_output.max(ordinal);
        }
        envelope.event
    }

    fn poll_control(&mut self) {
        if self.control_pending.is_some() || !self.control_open {
            return;
        }
        match self.control_rx.try_recv() {
            Ok(envelope) => self.control_pending = Some(envelope),
            Err(mpsc::error::TryRecvError::Disconnected) => self.control_open = false,
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
    }

    fn poll_output(&mut self) -> Result<TmuxEvent, mpsc::error::TryRecvError> {
        let envelope = if let Some(envelope) = self.output_pending.pop_front() {
            envelope
        } else {
            match self.output_rx.try_recv() {
                Ok(envelope) => envelope,
                Err(err) => {
                    if matches!(err, mpsc::error::TryRecvError::Disconnected) {
                        self.output_open = false;
                    }
                    return Err(err);
                }
            }
        };
        Ok(self.consume_output(envelope))
    }

    pub fn try_recv(&mut self) -> Result<TmuxEvent, mpsc::error::TryRecvError> {
        loop {
            self.poll_control();
            if let Some(envelope) = self.control_pending.take() {
                if envelope.output_watermark <= self.consumed_output
                    || (!self.output_open && self.output_pending.is_empty())
                {
                    return Ok(envelope.event);
                }
                self.control_pending = Some(envelope);
                match self.poll_output() {
                    Ok(event) => return Ok(event),
                    Err(mpsc::error::TryRecvError::Disconnected) => continue,
                    Err(mpsc::error::TryRecvError::Empty) => {
                        return Err(mpsc::error::TryRecvError::Empty)
                    }
                }
            }
            if self.output_open {
                match self.poll_output() {
                    Ok(event) => return Ok(event),
                    Err(mpsc::error::TryRecvError::Disconnected)
                    | Err(mpsc::error::TryRecvError::Empty) => {}
                }
            }
            self.poll_control();
            if self.control_pending.is_some() {
                continue;
            }
            if !self.control_open && !self.output_open && self.output_pending.is_empty() {
                return Err(mpsc::error::TryRecvError::Disconnected);
            }
            return Err(mpsc::error::TryRecvError::Empty);
        }
    }

    pub async fn recv(&mut self) -> Option<TmuxEvent> {
        loop {
            if let Ok(event) = self.try_recv() {
                return Some(event);
            }
            if !self.control_open && !self.output_open {
                return None;
            }
            if self.control_pending.is_some() {
                if let Some(envelope) = self.output_pending.pop_front() {
                    return Some(self.consume_output(envelope));
                }
                if self.output_open {
                    if let Some(envelope) = self.output_rx.recv().await {
                        return Some(self.consume_output(envelope));
                    }
                    self.output_open = false;
                }
                continue;
            }
            match (self.control_open, self.output_open) {
                (true, true) => {
                    tokio::select! {
                        biased;
                        envelope = self.control_rx.recv() => match envelope {
                            Some(envelope) => {
                                self.control_pending = Some(envelope);
                            }
                            None => self.control_open = false,
                        },
                        envelope = self.output_rx.recv() => match envelope {
                            Some(envelope) => return Some(self.consume_output(envelope)),
                            None => self.output_open = false,
                        },
                    }
                }
                (true, false) => match self.control_rx.recv().await {
                    Some(envelope) => self.control_pending = Some(envelope),
                    None => self.control_open = false,
                },
                (false, true) => match self.output_rx.recv().await {
                    Some(envelope) => return Some(self.consume_output(envelope)),
                    None => self.output_open = false,
                },
                (false, false) => return None,
            }
        }
    }

    /// Drop queued output for a pane at an authoritative resync boundary.
    ///
    /// The output watermark makes `OutputGap` a barrier after all output that
    /// was accepted before the loss. Bytes accepted after that barrier may
    /// still be buffered in the bounded lane; they must not be appended after a
    /// new snapshot for the affected pane. Output belonging to other panes is
    /// retained.
    pub fn discard_output_pane(&mut self, pane: PaneId) {
        let mut retained = VecDeque::new();
        while let Some(envelope) = self.output_pending.pop_front() {
            if event_is_output_for_pane(&envelope.event, pane) {
                if let Some(ordinal) = envelope.output_ordinal {
                    self.consumed_output = self.consumed_output.max(ordinal);
                }
            } else {
                retained.push_back(envelope);
            }
        }
        loop {
            match self.output_rx.try_recv() {
                Ok(envelope) => {
                    if event_is_output_for_pane(&envelope.event, pane) {
                        if let Some(ordinal) = envelope.output_ordinal {
                            self.consumed_output = self.consumed_output.max(ordinal);
                        }
                    } else {
                        retained.push_back(envelope);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.output_open = false;
                    break;
                }
            }
        }
        self.output_pending = retained;
    }

    /// Resume accepting incremental output after the backend has installed an
    /// authoritative snapshot (or released a timed-out resync fallback).
    ///
    /// The sender deliberately keeps the pane marked as gapped while the
    /// snapshot query is in flight; clearing it here is the single recovery
    /// boundary that lets new output enter the bounded lane again.
    pub fn resume_output_pane(&mut self, pane: PaneId) {
        if let Ok(mut gaps) = self.output_gaps.lock() {
            gaps.remove(&pane);
        }
    }
}

fn event_is_output_for_pane(event: &TmuxEvent, pane: PaneId) -> bool {
    matches!(
        event,
        TmuxEvent::Message(
            Message::Output { pane: event_pane, .. }
                | Message::ExtendedOutput {
                    pane: event_pane, ..
                }
        ) if *event_pane == pane
    )
}

const MAX_RESPONSE_BYTES: usize = crate::core::buffer_cap::MAX_PANE_OUTPUT_BYTES;

/// 客户端连接模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectMode {
    /// `tmux -CC new-session`（创建新 session）。
    NewSession {
        name: Option<String>,
        /// 起始工作目录（`-c <dir>`）；None 用 tmux 默认（当前目录）。
        start_directory: Option<String>,
    },
    /// `tmux -CC attach -t <target>`（附加已有 session）。
    Attach { target: Option<String> },
}

impl Default for ConnectMode {
    fn default() -> Self {
        ConnectMode::NewSession {
            name: None,
            start_directory: None,
        }
    }
}

/// 启动参数。
#[derive(Debug, Clone, Default)]
pub struct TmuxClientConfig {
    /// 连接模式，默认 NewSession。
    pub mode: Option<ConnectMode>,
    /// 额外传给 tmux 的参数（如 `-L socket_name`）。
    pub extra_args: Vec<String>,
    /// tmux 可执行路径，默认 `tmux`。
    pub tmux_bin: Option<String>,
    /// 初始窗口几何 -x。
    pub cols: Option<u32>,
    /// 初始窗口几何 -y。
    pub rows: Option<u32>,
    /// 事件通道容量。
    pub event_buffer: usize,
    /// SSH alias：非空时通过 SSH transport 在远端启动 tmux -CC。
    pub ssh_alias: Option<String>,
}

/// tmux `-CC` 客户端工厂。
///
/// 仅提供静态构造方法，真正的句柄是 [`TmuxClientHandle`]。
pub struct TmuxClient;

/// tmux -CC 客户端句柄。
pub struct TmuxClientHandle {
    /// pty 写端（pty 模式）。
    pty_writer: Option<PtyWriter>,
    /// 直 spawn 模式的 stdin。
    stdin: Option<tokio::process::ChildStdin>,
    /// 直 spawn 模式的子进程。
    child: Option<Child>,
    /// pty 模式的子进程。
    pty_child: Option<PtyChild>,
    /// SSH 读写字节计数（本地 pty 模式为 None）。
    pub traffic: Option<crate::core::transport::TrafficCounters>,
}

/// 事件：tmux → 客户端。
#[derive(Debug, Clone)]
pub enum TmuxEvent {
    /// 一条已解析的通知消息。
    Message(Message),
    /// 一条命令的完整响应正文。reader 在 `%end`/`%error` 前聚合，避免
    /// `capture-pane -S -10000` 产生上万条 channel 事件。
    ResponseBlock {
        number: i64,
        is_error: bool,
        lines: Vec<String>,
        /// 超出响应上限时只丢弃完整的前缀行，保留尾部。
        truncated_prefix: bool,
    },
    /// 有界 pane-output lane 满时丢弃了增量；backend 必须用 authoritative
    /// snapshot 恢复该 pane，控制事件本身仍然完整保留。
    OutputGap { pane: PaneId },
    /// tmux 子进程退出。
    Exit { code: Option<i32> },
}

impl TmuxClient {
    /// spawn 一个 tmux -CC 进程并启动后台读循环。
    ///
    /// 默认走 pty 模式（tmux -CC 需要 tty）。
    pub async fn spawn(config: TmuxClientConfig) -> Result<(TmuxClientHandle, TmuxEventReceiver)> {
        if config.ssh_alias.is_some() {
            Self::spawn_ssh(config).await
        } else {
            Self::spawn_pty(config).await
        }
    }

    /// SSH 模式：通过 SSH transport 在远端启动 `tmux -CC`。
    ///
    /// 用 `SshProcessTransport` spawn `ssh <alias> -- tmux -CC ...`，
    /// 取其 reader/writer 替代本地 pty。读循环与本地 pty 模式相同。
    pub async fn spawn_ssh(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, TmuxEventReceiver)> {
        use crate::core::transport::ssh::{build_ssh_command, SshProcessTransport};
        use crate::core::transport::Transport;

        let alias = config
            .ssh_alias
            .as_ref()
            .ok_or_else(|| anyhow!("spawn_ssh 需要 ssh_alias"))?;

        // 构造远端 tmux 命令字符串。
        // 注意 argv 开头是 `-L socket` 等二进制级选项；远端经 shell 执行，必须
        // 以 `tmux` 开头（否则 shell 会把 `-L ...` 当成 shell 自身选项报错）。
        let remote_tmux = build_remote_tmux_command(&config);
        // 复用 CLI 的 MUXTERM_SSH_CONFIG_PATH 约定：显式 -F 指定 ssh config
        // （测试/CI 用生成 config，不读用户真实 ~/.ssh/config）。
        let ssh_config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
        let (program, ssh_args) = build_ssh_command(alias, &remote_tmux, ssh_config.as_deref());
        let arg_refs: Vec<&str> = ssh_args.iter().map(|s| s.as_str()).collect();

        let pty_size = crate::core::transport::PtySize::new(
            config.cols.unwrap_or(80) as u16,
            config.rows.unwrap_or(24) as u16,
        );

        tracing::info!(
            target = "muxterm::client",
            alias = %alias,
            remote = %remote_tmux,
            "spawn tmux -CC via SSH"
        );

        let traffic = crate::core::transport::TrafficCounters::new();
        let mut transport = SshProcessTransport::new();
        transport.set_traffic(traffic.clone());
        transport
            .spawn_exec(&program, &arg_refs, pty_size)
            .context("SSH transport spawn 失败")?;

        // 先取 writer（take_pty_writer 消费 master 的 writer 端）
        let writer = transport
            .take_pty_writer()
            .context("SSH transport take_writer 失败")?;
        let writer = PtyWriter::with_traffic(writer, traffic.clone());

        // 再把 transport 移入读线程（read 是非阻塞，用后台线程桥接到 mpsc）
        let (read_tx, read_rx) = mpsc::channel::<std::io::Result<Vec<u8>>>(4096);
        let mut read_transport = transport;
        std::thread::Builder::new()
            .name("muxterm-ssh-read".into())
            .spawn(move || loop {
                match read_transport.read() {
                    Ok(Some(data)) => {
                        if read_tx.blocking_send(Ok(data)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            })
            .expect("spawn ssh read thread");

        // 用 PtyReader::from_channel 包装 read_rx，复用 read_pty_loop
        let reader = PtyReader::from_channel(read_rx);
        let (tx, rx) = event_channel();
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            read_pty_loop(reader, OutputBatcher::new(tx_clone)).await;
        });

        let handle = TmuxClientHandle {
            pty_writer: Some(writer),
            stdin: None,
            child: None,
            pty_child: None,
            traffic: Some(traffic),
        };
        Ok((handle, rx))
    }

    /// pty 模式 spawn（推荐，tmux -CC 需要 tty）。
    pub async fn spawn_pty(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, TmuxEventReceiver)> {
        let bin = config
            .tmux_bin
            .clone()
            .unwrap_or_else(crate::core::executable::resolve_tmux_binary);
        let argv = build_argv(&config);
        let arg_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let cols = config.cols.unwrap_or(80) as u16;
        let rows = config.rows.unwrap_or(24) as u16;
        tracing::info!(target = "muxterm::client", bin = %bin, args = ?argv, "spawn tmux -CC (pty)");
        let mut pty_child = pty::spawn_pty(&bin, &arg_refs, cols, rows)
            .with_context(|| format!("spawn tmux 失败: {bin}"))?;

        let (reader, writer) = split_master(&mut pty_child.master)?;
        let (tx, rx) = event_channel();
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            read_pty_loop(reader, OutputBatcher::new(tx_clone)).await;
        });

        let handle = TmuxClientHandle {
            pty_writer: Some(writer),
            stdin: None,
            child: None,
            pty_child: Some(pty_child),
            traffic: None,
        };
        // 配置消费标记（避免未使用）
        let _ = config.extra_args.len();
        Ok((handle, rx))
    }

    /// 直 spawn 模式（不用 pty）。tmux 在无 tty 下通常会立即退出，仅作兜底/测试。
    pub async fn spawn_direct(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, TmuxEventReceiver)> {
        let bin = config
            .tmux_bin
            .clone()
            .unwrap_or_else(crate::core::executable::resolve_tmux_binary);
        let mut cmd = tokio::process::Command::new(&bin);
        let argv = build_argv(&config);
        for a in &argv {
            cmd.arg(a);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        tracing::info!(target = "muxterm::client", bin = %bin, "spawn tmux -CC (direct)");
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn tmux 失败: {bin}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("tmux 没有 stdout"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("tmux 没有 stdin"))?;

        let (tx, rx) = event_channel();
        let tx_clone = tx.clone();
        tokio::spawn(read_stream_loop(stdout, OutputBatcher::new(tx_clone)));

        let handle = TmuxClientHandle {
            pty_writer: None,
            stdin: Some(stdin),
            child: Some(child),
            pty_child: None,
            traffic: None,
        };
        Ok((handle, rx))
    }

    /// 便捷：new-session。
    pub async fn new_session(
        config: TmuxClientConfig,
    ) -> Result<(TmuxClientHandle, TmuxEventReceiver)> {
        Self::spawn(config).await
    }

    /// 便捷：attach。
    pub async fn attach(config: TmuxClientConfig) -> Result<(TmuxClientHandle, TmuxEventReceiver)> {
        Self::spawn(config).await
    }
}

/// 构造 tmux 命令参数（不含 bin）。
///
/// 构造传给 tmux 可执行文件的 argv（不含二进制名本身）。
///
/// 顺序：`[extra_args] -CC <command> <command args>`。extra_args（如 `-L socket`）
/// 是 tmux **二进制级**选项，必须放在 `-CC` 之前；`-CC` 之后的第一个 token 才是
/// tmux 命令（new-session/attach），否则会被 tmux 当成命令参数解析报错。
pub(crate) fn build_argv(config: &TmuxClientConfig) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    // 二进制级选项（如 -L socket_name）放在 -CC 前
    for a in &config.extra_args {
        argv.push(a.clone());
    }
    argv.push("-CC".into());
    match config.mode.clone().unwrap_or_default() {
        ConnectMode::NewSession {
            name,
            start_directory,
        } => {
            argv.push("new-session".into());
            if let Some(n) = &name {
                argv.push("-s".into());
                argv.push(n.clone());
            }
            if let Some(dir) = &start_directory {
                argv.push("-c".into());
                argv.push(dir.clone());
            }
            if let Some(x) = config.cols {
                argv.push("-x".into());
                argv.push(x.to_string());
            }
            if let Some(y) = config.rows {
                argv.push("-y".into());
                argv.push(y.to_string());
            }
        }
        ConnectMode::Attach { target } => {
            argv.push("attach".into());
            if let Some(t) = &target {
                argv.push("-t".into());
                argv.push(t.clone());
            }
        }
    }
    argv
}

/// Build the single command string interpreted by the remote POSIX shell used
/// by `ssh`. Every tmux argument is shell-quoted; `-c` paths additionally keep
/// the conventional remote `~/...` expansion while quoting their suffix.
pub(crate) fn build_remote_tmux_command(config: &TmuxClientConfig) -> String {
    let argv = build_argv(config);
    let mut words = Vec::with_capacity(argv.len() + 1);
    words.push("tmux".to_string());
    let mut previous_was_c = false;
    for arg in argv {
        let quoted = if previous_was_c {
            crate::core::discovery::shell_quote_remote_path(&arg)
        } else {
            crate::core::discovery::shell_quote(&arg)
        };
        previous_was_c = arg == "-c";
        words.push(quoted);
    }
    words.join(" ")
}

impl TmuxClientHandle {
    /// 发送一条已构造好的命令。
    pub async fn send_command(&mut self, cmd: &TmuxCommand) -> Result<()> {
        let line = cmd.to_line();
        self.send_raw(&line).await
    }

    /// 发送任意原始文本到 tmux stdin（应自带末尾换行）。
    pub async fn send_raw(&mut self, raw: &str) -> Result<()> {
        tracing::debug!(target = "muxterm::client", "send: {:?}", raw);
        if let Some(w) = &self.pty_writer {
            w.write_all(raw.as_bytes().to_vec())
                .await
                .map_err(|e| anyhow!("写 tmux pty 失败: {e}"))?;
            return Ok(());
        }
        if let Some(stdin) = &mut self.stdin {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(raw.as_bytes())
                .await
                .map_err(|e| anyhow!("写 tmux stdin 失败: {e}"))?;
            stdin
                .flush()
                .await
                .map_err(|e| anyhow!("flush tmux stdin 失败: {e}"))?;
            return Ok(());
        }
        Err(anyhow!("tmux 写端已关闭"))
    }

    /// 优雅 detach：发 `detach-client`，tmux 会输出 %exit 然后进程退出。
    pub async fn detach(&mut self) -> Result<()> {
        self.send_raw("detach-client\n").await
    }

    /// 强制 kill：先关写端并杀子进程。
    ///
    /// 不先 `detach`：pty 写可能阻塞，导致 sender task / `shutdown` 永远等不到。
    pub async fn kill(&mut self) -> Result<()> {
        self.close_writer();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        if let Some(mut pty_child) = self.pty_child.take() {
            pty_child.kill_and_wait();
        }
        Ok(())
    }

    /// 等待子进程退出，返回退出码。
    pub async fn wait(mut self) -> Result<Option<i32>> {
        self.close_writer();
        if let Some(mut child) = self.child {
            let status = child.wait().await.context("等待 tmux 退出失败")?;
            Ok(status.code())
        } else if let Some(mut pty_child) = self.pty_child {
            pty_child
                .child
                .wait()
                .map(|s| s.success().then_some(0).or(None))
                .map_err(|e| anyhow!("等待 tmux 退出失败: {e}"))
        } else {
            Ok(None)
        }
    }

    /// 关闭写端（用于 detach 后让 tmux 自然退出）。
    pub fn close_writer(&mut self) {
        self.pty_writer.take();
        self.stdin.take();
    }
}

// ============================================================================
// 读循环
// ============================================================================

/// 把字节块渲染成可读的 debug 字符串：可打印 ASCII/UTF-8 保留，控制字节转义。
///
/// 用于 debug 模式把 tmux 原始收发数据落盘，方便排查渲染/输入问题；内容过大时
/// 截断，避免日志刷屏。
fn hex_debug(bytes: &[u8]) -> String {
    const MAX: usize = 1024;
    let show = if bytes.len() > MAX {
        &bytes[..MAX]
    } else {
        bytes
    };
    let mut out = String::with_capacity(show.len() + 16);
    for &b in show {
        match b {
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x1b => out.push_str("\\e"),
            0x20..=0x7e => out.push(b as char),
            other => {
                use std::fmt::Write;
                let _ = write!(out, "\\x{other:02x}");
            }
        }
    }
    if bytes.len() > MAX {
        out.push_str("...(truncated)");
    }
    out
}

/// DCS passthrough 前缀：tmux 3.3+ 在 CC 模式下用 `ESC P 1 0 0 0 p` 把第一条
/// `%begin` 包起来。我们识别并剥离它。
pub(crate) const DCS_PREFIX: &[u8] = b"\x1bP1000p";

/// 从 pty/stdout 读到的字节块中提取「完整行」。
///
/// 这是 read loop 的核心纯逻辑，抽出来便于单元测试（分包/拼包/长行/UTF-8 边界）。
/// 它把 `chunk` 追加进 `buf`，按**真换行** `\n` 切出完整行（去掉行尾 `\n` 与
/// `\r`，并剥离 DCS 前缀），返回提取出的行；未闭合的尾段留在 `buf` 等下次。
/// 若 `buf` 过长仍无换行，会丢弃最旧前缀（见 [`trim_incomplete_line`]）。
///
/// 注意：`%output` content 里的 `\n` 是 C 转义后的两个字符（`\\` + `n`），
/// 不是真换行符，所以这里只按真 `\n` 字节切，不会把 content 内部切碎。
pub(crate) fn feed_bytes_to_lines(buf: &mut Vec<u8>, chunk: &[u8]) -> Vec<Vec<u8>> {
    if chunk.is_empty() {
        return Vec::new();
    }
    buf.extend_from_slice(chunk);
    let mut lines = Vec::new();
    loop {
        let Some(nl) = buf.iter().position(|&b| b == b'\n') else {
            // 只限制仍未闭合的尾段；同一 chunk 中已经闭合的完整行不能被
            // 大小限制提前丢掉。
            trim_incomplete_line(buf, MAX_INCOMPLETE_LINE_BYTES);
            break;
        };
        let mut line_bytes: Vec<u8> = buf.drain(..=nl).collect();
        if line_bytes.last() == Some(&b'\n') {
            line_bytes.pop();
        }
        if line_bytes.last() == Some(&b'\r') {
            line_bytes.pop();
        }
        // 完整行也不能让单次输入绕过上限；保留最近的字节，明确表示这是一条
        // 被截断的行（通常会因此无法作为协议通知解析），但不会无限增长。
        if line_bytes.len() > MAX_INCOMPLETE_LINE_BYTES {
            let start = line_bytes.len() - MAX_INCOMPLETE_LINE_BYTES;
            line_bytes.drain(..start);
        }
        if line_bytes.starts_with(DCS_PREFIX) {
            line_bytes.drain(..DCS_PREFIX.len());
        }
        lines.push(line_bytes);
    }
    lines
}

/// pty 模式读循环：用 `PtyReader::read_chunk` 异步取字节块，按真换行切行。
async fn read_pty_loop<S: TmuxEventSink>(mut reader: PtyReader, tx: S) {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut response = None;

    while let Some(chunk_res) = reader.read_chunk().await {
        let chunk = match chunk_res {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(target = "muxterm::client", "读 tmux pty 失败: {e}");
                break;
            }
        };
        if chunk.is_empty() {
            continue;
        }
        tracing::trace!(
            target = "muxterm::client",
            len = chunk.len(),
            hex = %hex_debug(&chunk),
            "recv tmux chunk"
        );
        for line in feed_bytes_to_lines(&mut buf, &chunk) {
            tracing::trace!(
                target = "muxterm::client",
                line = %String::from_utf8_lossy(&line),
                "recv line"
            );
            process_line(&line, &tx, &mut response).await;
        }
        tx.flush();
    }
    tracing::info!(target = "muxterm::client", "tmux pty EOF");
    tx.flush();
    tx.emit(TmuxEvent::Exit { code: None });
}

/// 直 spawn 模式读循环：ChildStdout 是 AsyncRead。
async fn read_stream_loop<S: TmuxEventSink>(stdout: ChildStdout, tx: S) {
    use tokio::io::AsyncReadExt;
    let mut reader = stdout;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let mut response = None;

    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                for line in feed_bytes_to_lines(&mut buf, &chunk[..n]) {
                    process_line(&line, &tx, &mut response).await;
                }
                tx.flush();
            }
            Err(e) => {
                tracing::error!(target = "muxterm::client", "读 tmux stdout 失败: {e}");
                break;
            }
        }
    }
    tracing::info!(target = "muxterm::client", "tmux stdout EOF");
    tx.flush();
    tx.emit(TmuxEvent::Exit { code: None });
}

/// 命令响应正文的有界累积器。大 capture 只保留完整的尾部行，reader 不等待
/// runtime 消费，也不会把 UTF-8/ANSI 序列从任意字节偏移截断。
#[derive(Debug, Default)]
pub(crate) struct ResponseBuffer {
    number: i64,
    lines: VecDeque<String>,
    bytes: usize,
    truncated_prefix: bool,
}

impl ResponseBuffer {
    fn new(number: i64) -> Self {
        Self {
            number,
            ..Self::default()
        }
    }

    fn push(&mut self, line: String) {
        self.bytes = self.bytes.saturating_add(line.len().saturating_add(1));
        self.lines.push_back(line);
        while self.bytes > MAX_RESPONSE_BYTES {
            let Some(old) = self.lines.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(old.len().saturating_add(1));
            self.truncated_prefix = true;
        }
    }

    fn finish(self, is_error: bool) -> TmuxEvent {
        TmuxEvent::ResponseBlock {
            number: self.number,
            is_error,
            lines: self.lines.into_iter().collect(),
            truncated_prefix: self.truncated_prefix,
        }
    }
}

/// 处理单行：解析为 Message 或 ResponseBlock，并维护响应状态机。
///
/// `pub(crate)`：SSH 远程 client 复用同一套行状态机。
pub(crate) async fn process_line<S: TmuxEventSink>(
    line: &[u8],
    tx: &S,
    response: &mut Option<ResponseBuffer>,
) {
    if let Some(msg) = parse_line_bytes(line) {
        if let Message::ResponseBoundary(b) = &msg {
            match b.kind {
                NotificationKind::Begin => {
                    *response = Some(ResponseBuffer::new(b.number));
                    tx.emit(TmuxEvent::Message(msg));
                }
                NotificationKind::End | NotificationKind::Error => {
                    let block = response
                        .take()
                        .unwrap_or_else(|| ResponseBuffer::new(b.number))
                        .finish(matches!(b.kind, NotificationKind::Error));
                    // Runtime 在收到 boundary 时立即 dispatch；block 必须先到。
                    tx.emit(block);
                    tx.emit(TmuxEvent::Message(msg));
                }
            }
            return;
        }
        // tmux may defer a known notification until after a response, but if it
        // appears on the wire here it must remain a notification (not response
        // text). Unknown `%0 ...` rows are handled by the accumulator below.
        if response.is_none() || !matches!(&msg, Message::Unknown { .. }) {
            tx.emit(TmuxEvent::Message(msg));
            return;
        }
    }

    if response.is_some() {
        let line = String::from_utf8_lossy(line).into_owned();
        response.as_mut().expect("response checked").push(line);
    } else {
        // 响应边界之外的普通行不是 control protocol 消息。
        if let Some(msg) = parse_line_bytes(line) {
            tx.emit(TmuxEvent::Message(msg));
        } else {
            tracing::trace!(
                target = "muxterm::client",
                line = %String::from_utf8_lossy(line),
                "响应外普通行被忽略"
            );
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn ssh_attach_argv_without_socket_has_no_dash_l() {
        let config = TmuxClientConfig {
            mode: Some(ConnectMode::Attach {
                target: Some("yaklang-workspace".into()),
            }),
            extra_args: Vec::new(),
            ssh_alias: Some("ryzen".into()),
            cols: Some(80),
            rows: Some(24),
            ..TmuxClientConfig::default()
        };
        let argv = build_argv(&config);
        assert_eq!(
            argv,
            vec!["-CC", "attach", "-t", "yaklang-workspace"],
            "SSH attach 默认远端 socket 必须是 `tmux -CC attach -t <session>`，不能带 -L <alias>"
        );
        assert!(!argv.iter().any(|a| a == "ryzen"));
    }

    #[test]
    fn ssh_attach_argv_isolated_socket_is_not_alias() {
        let config = TmuxClientConfig {
            mode: Some(ConnectMode::Attach {
                target: Some("featssh".into()),
            }),
            extra_args: vec!["-L".into(), "muxterm-test-remote-x".into()],
            ssh_alias: Some("test-feat".into()),
            ..TmuxClientConfig::default()
        };
        let argv = build_argv(&config);
        assert_eq!(
            argv.windows(2)
                .find(|w| w[0] == "-L")
                .map(|w| w[1].as_str()),
            Some("muxterm-test-remote-x")
        );
        assert!(!argv.iter().any(|a| a == "test-feat"));
    }

    #[test]
    fn remote_tmux_command_quotes_cwd_and_preserves_remote_home_expansion() {
        let config = TmuxClientConfig {
            mode: Some(ConnectMode::NewSession {
                name: Some("project name".into()),
                start_directory: Some("~/Project/my repo".into()),
            }),
            extra_args: vec!["-L".into(), "socket with space".into()],
            ..TmuxClientConfig::default()
        };
        assert_eq!(
            build_remote_tmux_command(&config),
            "tmux '-L' 'socket with space' '-CC' 'new-session' '-s' 'project name' '-c' $HOME/'Project/my repo'"
        );
    }

    /// 每次调用递增的全局计数器，保证同一进程内并行线程拿到的测试 socket 名唯一。
    /// 旧实现只用了 `std::process::id()`，在默认并行下多个真实 tmux E2E 会共用
    /// 同一个 `-L` socket，互相 kill-server 导致 CI 卡死（end_to_end_real_tmux 30m 超时）。
    static TEST_SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_socket(prefix: &str) -> String {
        let n = TEST_SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{}-{n}", std::process::id())
    }

    /// 清理指定 tmux server；与测试体解耦，保证即使测试体在 15s 超时被取消，
    /// 残留的 tmux server 也能被回收，避免污染下一次 CI 运行。
    /// 接收 owned `String`，使返回的 future 拥有其数据，可被 `Fn` 闭包安全返回。
    async fn kill_tmux_server(socket: String) {
        let _ = tokio::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output()
            .await;
    }

    /// 在一个 15s 有界 tokio timeout 内运行闭包；无论闭包成功、panic 还是超时，
    /// 返回前都会调用 cleanup 清理资源。用于把 `spawn/send/kill` 包进有界窗口，
    /// 防止 `handle.kill().await` 在 Linux PTY 上无限阻塞导致整个测试挂死。
    /// `BodyFut` 与 `CleanupFut` 是独立泛型，避免二者 future 类型耦合。
    async fn run_bounded<B, Cleanup, BodyFut, CleanupFut, T>(
        timeout_s: u64,
        fut: B,
        cleanup: &Cleanup,
    ) -> std::result::Result<T, ()>
    where
        B: FnOnce() -> BodyFut,
        BodyFut: std::future::Future<Output = T>,
        Cleanup: Fn() -> CleanupFut,
        CleanupFut: std::future::Future<Output = ()>,
    {
        let res =
            match tokio::time::timeout(tokio::time::Duration::from_secs(timeout_s), fut()).await {
                Ok(t) => Ok(t),
                Err(_) => Err(()),
            };
        cleanup().await;
        res
    }

    #[tokio::test]
    async fn process_line_dcs_and_response() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut response = None;

        process_line(b"\x1bP1000p%begin 1784356613 286 1", &tx, &mut response).await;
        assert_eq!(response.as_ref().map(|r| r.number), Some(286));

        process_line(b"cmd: 1 windows (created ...)", &tx, &mut response).await;
        process_line(b"%end 1784356613 286 1", &tx, &mut response).await;
        assert!(response.is_none());

        let mut msgs = Vec::new();
        let mut lines = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            match ev {
                TmuxEvent::Message(m) => msgs.push(m),
                TmuxEvent::ResponseBlock { lines: block, .. } => lines.extend(block),
                _ => {}
            }
        }
        assert!(msgs
            .iter()
            .any(|m| matches!(m, Message::ResponseBoundary(_))));
        assert!(lines.contains(&"cmd: 1 windows (created ...)".to_string()));
    }

    #[tokio::test]
    async fn process_line_output_with_escaped_newline() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut response = None;
        process_line(br#"%output %0 a\nb"#, &tx, &mut response).await;
        let ev = rx.recv().await.unwrap();
        match ev {
            TmuxEvent::Message(Message::Output { content, .. }) => {
                assert_eq!(&content, b"a\nb");
            }
            _ => panic!("应为 Output"),
        }
    }

    #[tokio::test]
    async fn process_line_error_boundary_closes_response() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut response = None;
        process_line(b"%begin 1 5 0", &tx, &mut response).await;
        assert!(response.is_some());
        process_line(b"some error text", &tx, &mut response).await;
        process_line(b"%error 1 5 0", &tx, &mut response).await;
        assert!(response.is_none());
        let mut any_lines = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let TmuxEvent::ResponseBlock { lines, .. } = ev {
                any_lines.extend(lines);
            }
        }
        assert!(any_lines.contains(&"some error text".to_string()));
    }

    #[tokio::test]
    async fn process_line_keeps_percent_prefixed_response_rows_in_response_block() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut response = None;

        process_line(b"%begin 1 7 0", &tx, &mut response).await;
        process_line(b"%0 @0 0", &tx, &mut response).await;
        process_line(b"%end 1 7 0", &tx, &mut response).await;

        let mut response_rows = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let TmuxEvent::ResponseBlock {
                number: 7, lines, ..
            } = event
            {
                response_rows.extend(lines);
            }
        }
        assert_eq!(response_rows, vec!["%0 @0 0"]);
    }

    #[tokio::test]
    async fn large_response_is_aggregated_without_reader_backpressure() {
        let (tx, mut rx) = event_channel();
        let mut response = None;
        let run = async {
            process_line(b"%begin 1 99 0", &tx, &mut response).await;
            for row in 0..10_000 {
                process_line(
                    format!("capture-row-{row}-{}", "x".repeat(256)).as_bytes(),
                    &tx,
                    &mut response,
                )
                .await;
            }
            process_line(b"%end 1 99 0", &tx, &mut response).await;
        };
        tokio::time::timeout(tokio::time::Duration::from_secs(1), run)
            .await
            .expect("大 capture 不得因为 UI 未消费而阻塞 tmux reader");

        let mut block = None;
        while let Ok(event) = rx.try_recv() {
            if let TmuxEvent::ResponseBlock {
                number,
                lines,
                truncated_prefix,
                ..
            } = event
            {
                block = Some((number, lines, truncated_prefix));
            }
        }
        let (number, lines, truncated) = block.expect("应收到单个 ResponseBlock");
        assert_eq!(number, 99);
        assert!(truncated, "超过响应上限时应标记被裁掉的完整前缀");
        assert!(lines
            .last()
            .is_some_and(|line| line.starts_with("capture-row-9999-")));
    }

    #[tokio::test]
    async fn output_lane_overflow_preserves_control_gap_event() {
        let (tx, mut rx) = event_channel();
        let pane = PaneId(77);
        for _ in 0..(OUTPUT_EVENT_BUFFER + 32) {
            tx.send_event(TmuxEvent::Message(Message::Output {
                pane,
                content: b"x".to_vec(),
                raw_content: "x".into(),
            }));
        }
        // The control lane remains writable even when output is full, but its
        // watermark must still preserve the accepted output that preceded it.
        tx.send_event(TmuxEvent::ResponseBlock {
            number: 42,
            is_error: false,
            lines: vec!["ok".into()],
            truncated_prefix: false,
        });

        let mut output_count = 0;
        let mut saw_gap = false;
        let mut saw_response = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                TmuxEvent::Message(Message::Output { .. }) => output_count += 1,
                TmuxEvent::OutputGap { pane: p } => {
                    assert_eq!(p, pane);
                    saw_gap = true;
                }
                TmuxEvent::ResponseBlock { number: 42, .. } => saw_response = true,
                _ => {}
            }
        }
        assert_eq!(output_count, OUTPUT_EVENT_BUFFER);
        assert!(saw_gap && saw_response);
    }

    fn pane_output_event(pane: u32, text: &str) -> TmuxEvent {
        TmuxEvent::Message(Message::Output {
            pane: PaneId(pane),
            content: text.as_bytes().to_vec(),
            raw_content: text.to_string(),
        })
    }

    #[test]
    fn coalesced_tui_burst_does_not_gap_or_block_control() {
        let (tx, mut rx) = event_channel();
        let batcher = OutputBatcher::new(tx);
        let pane = PaneId(2);
        let burst = OUTPUT_EVENT_BUFFER * 4;
        for i in 0..burst {
            batcher.emit(pane_output_event(2, &format!("\x1b[Hframe-{i}")));
        }
        batcher.emit(TmuxEvent::ResponseBlock {
            number: 7,
            is_error: false,
            lines: vec!["layout-ok".into()],
            truncated_prefix: false,
        });
        batcher.flush();

        let mut output_events = 0usize;
        let mut output_bytes = 0usize;
        let mut saw_gap = false;
        let mut saw_response = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                TmuxEvent::Message(Message::Output {
                    content, pane: p, ..
                }) => {
                    assert_eq!(p, pane);
                    output_events += 1;
                    output_bytes += content.len();
                }
                TmuxEvent::OutputGap { .. } => saw_gap = true,
                TmuxEvent::ResponseBlock {
                    number: 7, lines, ..
                } => {
                    assert_eq!(lines, vec!["layout-ok".to_string()]);
                    saw_response = true;
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(
            !saw_gap,
            "Codex 式连续 %output 必须合并，不能打满 64 槽再 OutputGap"
        );
        assert!(saw_response, "控制响应不能被未合并的 output 水位挡住");
        assert!(
            (1..OUTPUT_EVENT_BUFFER).contains(&output_events),
            "burst 应合并成少量 output 事件，实际 {output_events}"
        );
        assert!(output_bytes > burst, "合并后仍要保留全部帧字节");
    }

    #[test]
    fn coalescer_does_not_merge_or_gap_a_quiet_pane() {
        let (tx, mut rx) = event_channel();
        let batcher = OutputBatcher::new(tx);
        for i in 0..(OUTPUT_EVENT_BUFFER * 3) {
            batcher.emit(pane_output_event(42, &format!("codex-{i}")));
        }
        batcher.emit(pane_output_event(0, "quiet-shell"));
        batcher.flush();

        let mut saw_gap = false;
        let mut quiet = None;
        let mut chatty_bytes = 0usize;
        while let Ok(event) = rx.try_recv() {
            match event {
                TmuxEvent::OutputGap { .. } => saw_gap = true,
                TmuxEvent::Message(Message::Output {
                    pane: PaneId(0),
                    content,
                    ..
                }) => quiet = Some(content),
                TmuxEvent::Message(Message::Output {
                    pane: PaneId(42),
                    content,
                    ..
                }) => chatty_bytes += content.len(),
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(!saw_gap);
        assert_eq!(quiet.as_deref(), Some(b"quiet-shell".as_slice()));
        assert!(chatty_bytes > OUTPUT_EVENT_BUFFER);
    }

    #[test]
    fn coalescer_flushes_when_pane_changes() {
        let (tx, mut rx) = event_channel();
        let batcher = OutputBatcher::new(tx);
        batcher.emit(pane_output_event(1, "aaa"));
        batcher.emit(pane_output_event(1, "bbb"));
        batcher.emit(pane_output_event(2, "ccc"));
        batcher.flush();

        let mut chunks = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                TmuxEvent::Message(Message::Output { pane, content, .. }) => {
                    chunks.push((pane.0, content))
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(chunks, vec![(1, b"aaabbb".to_vec()), (2, b"ccc".to_vec())]);
    }

    #[tokio::test]
    async fn output_gap_suppresses_same_pane_suffix_until_resumed() {
        let (tx, mut rx) = event_channel();
        let pane = PaneId(76);
        for _ in 0..=OUTPUT_EVENT_BUFFER {
            tx.send_event(TmuxEvent::Message(Message::Output {
                pane,
                content: b"x".to_vec(),
                raw_content: "x".into(),
            }));
        }

        // The first failed send marks the pane gapped. Further output must not
        // advance the global control watermark while resync is pending.
        let accepted_before_suffix = tx
            .state
            .lock()
            .expect("event sender state mutex poisoned")
            .accepted_output;
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane,
            content: b"dropped".to_vec(),
            raw_content: "dropped".into(),
        }));
        assert_eq!(
            tx.state
                .lock()
                .expect("event sender state mutex poisoned")
                .accepted_output,
            accepted_before_suffix
        );

        loop {
            match rx.try_recv() {
                Ok(TmuxEvent::OutputGap { pane: p }) if p == pane => break,
                Ok(_) => {}
                Err(error) => panic!("gap must remain deliverable: {error:?}"),
            }
        }
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane,
            content: b"still-dropped".to_vec(),
            raw_content: "still-dropped".into(),
        }));
        assert_eq!(
            tx.state
                .lock()
                .expect("event sender state mutex poisoned")
                .accepted_output,
            accepted_before_suffix
        );

        rx.resume_output_pane(pane);
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane,
            content: b"resumed".to_vec(),
            raw_content: "resumed".into(),
        }));
        assert_eq!(
            tx.state
                .lock()
                .expect("event sender state mutex poisoned")
                .accepted_output,
            accepted_before_suffix + 1
        );
    }

    #[tokio::test]
    async fn output_gap_discards_queued_stale_suffix_for_only_that_pane() {
        let (tx, mut rx) = event_channel();
        let stale_pane = PaneId(78);
        let other_pane = PaneId(79);
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane: stale_pane,
            content: b"stale".to_vec(),
            raw_content: "stale".into(),
        }));
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane: other_pane,
            content: b"keep".to_vec(),
            raw_content: "keep".into(),
        }));
        tx.send_event(TmuxEvent::OutputGap { pane: stale_pane });

        assert!(matches!(
            rx.try_recv(),
            Ok(TmuxEvent::Message(Message::Output { pane, .. })) if pane == stale_pane
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(TmuxEvent::Message(Message::Output { pane, .. })) if pane == other_pane
        ));
        assert!(matches!(rx.try_recv(), Ok(TmuxEvent::OutputGap { pane }) if pane == stale_pane));
        // Output accepted after the gap is the stale suffix that must be
        // discarded for this pane, while another pane remains FIFO-visible.
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane: stale_pane,
            content: b"stale-after-gap".to_vec(),
            raw_content: "stale-after-gap".into(),
        }));
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane: other_pane,
            content: b"keep-after-gap".to_vec(),
            raw_content: "keep-after-gap".into(),
        }));
        rx.discard_output_pane(stale_pane);
        assert!(matches!(
            rx.try_recv(),
            Ok(TmuxEvent::Message(Message::Output { pane, content, .. }))
                if pane == other_pane && content == b"keep-after-gap"
        ));
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn control_watermark_preserves_wire_order_across_lanes() {
        let (tx, mut rx) = event_channel();
        let pane = PaneId(80);
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane,
            content: b"before".to_vec(),
            raw_content: "before".into(),
        }));
        tx.send_event(TmuxEvent::ResponseBlock {
            number: 81,
            is_error: false,
            lines: Vec::new(),
            truncated_prefix: false,
        });
        tx.send_event(TmuxEvent::Message(Message::Output {
            pane,
            content: b"after".to_vec(),
            raw_content: "after".into(),
        }));

        assert!(matches!(
            rx.try_recv(),
            Ok(TmuxEvent::Message(Message::Output { content, .. })) if content == b"before"
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(TmuxEvent::ResponseBlock { number: 81, .. })
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(TmuxEvent::Message(Message::Output { content, .. })) if content == b"after"
        ));
    }

    /// 端到端：真实 spawn tmux -CC（pty），收事件，发命令，验证响应。
    ///
    /// 隔离回归：socket 名用 PID+Atomic 计数器保证唯一（旧实现只用 PID，
    /// 默认并行下两个真实 tmux E2E 共用同一 `-L` socket，互相 kill-server
    /// 会卡死 CI——end_to_end_real_tmux 曾 30 分钟超时）。整个测试体包在
    /// 15s 有界 timeout 内；无论成功/panic/超时，timeout 之外都执行
    /// `tmux -L <socket> kill-server` 回收残留，杜绝 `handle.kill().await`
    /// 在 Linux PTY 上无限阻塞导致 kill-server 永远执行不到。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn end_to_end_real_tmux() {
        let socket = unique_test_socket("muxterm-test");
        let cleanup_socket = socket.clone();
        let result = run_bounded(
            15,
            || async {
                let config = TmuxClientConfig {
                    mode: Some(ConnectMode::NewSession {
                        name: Some("mxtest".into()),
                        start_directory: None,
                    }),
                    extra_args: vec!["-L".into(), socket.clone()],
                    cols: Some(80),
                    rows: Some(24),
                    ..Default::default()
                };
                let (mut handle, mut rx) = TmuxClient::spawn(config)
                    .await
                    .expect("end_to_end_real_tmux 应能启动 tmux");

                let mut got_window_add = false;
                let mut got_session_changed = false;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(4);
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        ev = rx.recv() => match ev {
                            Some(TmuxEvent::Message(m)) => match &m {
                                Message::WindowAdd { .. } => got_window_add = true,
                                Message::SessionChanged { .. } => got_session_changed = true,
                                _ => {}
                            },
                            Some(TmuxEvent::Exit { .. }) | None => break,
                            _ => {}
                        }
                    }
                    if got_window_add && got_session_changed {
                        break;
                    }
                }
                assert!(got_window_add, "应收到 window-add");

                let cmd = super::super::command::display_message(
                    super::super::command::PaneId(0),
                    "#{session_name}",
                );
                handle.send_command(&cmd).await.unwrap();

                let mut got_response = false;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        ev = rx.recv() => match ev {
                            Some(TmuxEvent::ResponseBlock { lines, .. }) => {
                                if lines.iter().any(|line| line.trim() == "mxtest") {
                                    got_response = true;
                                    break;
                                }
                            }
                            Some(TmuxEvent::Exit { .. }) | None => break,
                            _ => {}
                        }
                    }
                }
                assert!(got_response, "display-message 应返回 mxtest");

                let _ = handle.kill().await;
                let _ = got_session_changed;
            },
            &move || kill_tmux_server(cleanup_socket.clone()),
        )
        .await;
        // 无论成功或超时，都已完成 kill-server 清理。
        assert!(result.is_ok(), "end_to_end_real_tmux 应在 15s 内完成");
    }

    /// 端到端（P0）：detach 后 tmux 应输出 `%exit`，验证程序退出 → %exit 顺序。
    ///
    /// 与其他真实 tmux E2E 一样使用唯一 socket + 15s 有界 timeout +
    /// timeout 之外 kill-server，防止并行 socket 冲突与 PTY kill 阻塞。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn end_to_end_detach_yields_exit_event() {
        let socket = unique_test_socket("muxterm-exit");
        let cleanup_socket = socket.clone();
        let result = run_bounded(
            15,
            || async {
                let config = TmuxClientConfig {
                    mode: Some(ConnectMode::NewSession {
                        name: Some("mexit".into()),
                        start_directory: None,
                    }),
                    extra_args: vec!["-L".into(), socket.clone()],
                    cols: Some(80),
                    rows: Some(24),
                    ..Default::default()
                };
                let (mut handle, mut rx) = TmuxClient::spawn(config).await.expect("应能启动 tmux");

                // 等 tmux 就绪（window-add）
                let ready = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
                    while let Some(ev) = rx.recv().await {
                        if matches!(ev, TmuxEvent::Message(Message::WindowAdd { .. })) {
                            return true;
                        }
                    }
                    false
                })
                .await
                .unwrap_or(false);
                assert!(ready, "应收到 window-add 表示就绪");

                // detach → tmux 输出 %exit 后进程退出
                let _ = handle.detach().await;
                let got_exit = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
                    while let Some(ev) = rx.recv().await {
                        if matches!(ev, TmuxEvent::Exit { .. }) {
                            return true;
                        }
                    }
                    false
                })
                .await
                .unwrap_or(false);
                assert!(got_exit, "detach 后应收到 %exit 事件");

                let _ = handle.kill().await;
            },
            &move || kill_tmux_server(cleanup_socket.clone()),
        )
        .await;
        assert!(
            result.is_ok(),
            "end_to_end_detach_yields_exit_event 应在 15s 内完成"
        );
    }

    /// 端到端：验证半行 buffer 正确拼包——发一个会被 tmux 分多次输出的命令
    /// （list-windows 的多行响应），确认所有响应行都被收到。
    ///
    /// 隔离回归：与 end_to_end_real_tmux 相同，使用唯一 socket + 15s 有界
    /// timeout + timeout 之外 kill-server，防止并行 socket 冲突与 PTY kill 阻塞。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn end_to_end_multi_line_response() {
        let socket = unique_test_socket("muxterm-ml");
        let cleanup_socket = socket.clone();
        let result = run_bounded(
            15,
            || async {
                let config = TmuxClientConfig {
                    mode: Some(ConnectMode::NewSession {
                        name: Some("mxml".into()),
                        start_directory: None,
                    }),
                    extra_args: vec!["-L".into(), socket.clone()],
                    cols: Some(80),
                    rows: Some(24),
                    ..Default::default()
                };
                let (mut handle, mut rx) = TmuxClient::spawn(config)
                    .await
                    .expect("end_to_end_multi_line_response 应能启动 tmux");

                // 等启动
                let mut started = false;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
                while !started {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        ev = rx.recv() => match ev {
                            Some(TmuxEvent::Message(Message::WindowAdd { .. })) => started = true,
                            Some(TmuxEvent::Exit { .. }) | None => break,
                            _ => {}
                        }
                    }
                }
                assert!(started, "end_to_end_multi_line_response 应收到 window-add");

                // 创建第二个窗口，再 list-windows，应得到 2 行响应
                handle
                    .send_command(&super::super::command::new_window(
                        super::super::command::TmuxSessionId(0),
                        Some("second"),
                    ))
                    .await
                    .unwrap();
                // 给一点时间让 window-add 到达
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                handle
                    .send_command(&super::super::command::list_windows(
                        super::super::command::TmuxSessionId(0),
                    ))
                    .await
                    .unwrap();

                let mut lines = Vec::new();
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline) => break,
                        ev = rx.recv() => match ev {
                            Some(TmuxEvent::ResponseBlock { lines: block, .. }) => {
                                lines.extend(block)
                            }
                            Some(TmuxEvent::Exit { .. }) | None => break,
                            _ => {}
                        }
                    }
                    if lines.len() >= 2 {
                        break;
                    }
                }
                assert!(lines.len() >= 2, "list-windows 应返回至少 2 行: {lines:?}");
                // 每行应是 `<n>: ...` 形式的窗口列表行。窗口序号受 tmux `base-index`
                // 全局设置影响（用户 `~/.tmux.conf` 可能设为 1），所以不硬编码 0/1，
                // 只校验至少出现两个**不同**的窗口序号。
                let window_idxs: Vec<&str> =
                    lines.iter().filter_map(|l| l.split(':').next()).collect();
                let distinct: std::collections::HashSet<&str> =
                    window_idxs.iter().copied().collect();
                assert!(
                    distinct.len() >= 2,
                    "应至少出现 2 个不同窗口序号: {lines:?}"
                );

                let _ = handle.kill().await;
            },
            &move || kill_tmux_server(cleanup_socket.clone()),
        )
        .await;
        assert!(
            result.is_ok(),
            "end_to_end_multi_line_response 应在 15s 内完成"
        );
    }

    #[test]
    fn unique_test_socket_is_unique_within_process() {
        let a = unique_test_socket("muxterm-test");
        let b = unique_test_socket("muxterm-test");
        let c = unique_test_socket("muxterm-ml");
        // 同一进程内每次调用必须唯一，且不同前缀不冲突。
        assert_ne!(a, b, "同一前缀两次调用应不同");
        assert_ne!(a, c);
        assert_ne!(b, c);
        // 都含进程 id 且不为空。
        assert!(!a.is_empty());
        assert!(a.starts_with("muxterm-test-"));
    }

    #[tokio::test]
    async fn run_bounded_cleans_up_on_success() {
        let cleaned = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = cleaned.clone();
        let res = run_bounded(2, || async { 42u32 }, &move || {
            flag.set(true);
            std::future::ready(())
        })
        .await;
        assert_eq!(res, Ok(42));
        assert!(cleaned.get(), "cleanup 应在成功后执行");
    }

    #[tokio::test]
    async fn run_bounded_times_out_and_still_cleans_up() {
        let cleaned = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = cleaned.clone();
        let res = run_bounded(
            1,
            || async {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                0u32
            },
            &move || {
                flag.set(true);
                std::future::ready(())
            },
        )
        .await;
        assert!(res.is_err(), "应超时返回 Err");
        assert!(cleaned.get(), "超时后 cleanup 也应执行");
    }

    #[test]
    fn config_default_and_mode() {
        let _c = TmuxClientConfig::default();
        assert_eq!(
            ConnectMode::default(),
            ConnectMode::NewSession {
                name: None,
                start_directory: None,
            }
        );
    }

    // ---------- feed_bytes_to_lines：分包/拼包/长行/UTF-8 边界 ----------

    #[test]
    fn feed_bytes_one_full_line() {
        let mut buf = Vec::new();
        let lines = feed_bytes_to_lines(&mut buf, b"%window-add @0\r\n");
        assert_eq!(lines, vec![b"%window-add @0".to_vec()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_split_across_chunks() {
        // 一条 %output 被多次 read 拆开
        let mut buf = Vec::new();
        // 三次 read 才拼出完整一行：前两段是半行，第三段带换行收尾
        assert_eq!(
            feed_bytes_to_lines(&mut buf, b"%output %0 \"ab"),
            Vec::<Vec<u8>>::new()
        );
        assert_eq!(
            feed_bytes_to_lines(&mut buf, b"cd\""),
            Vec::<Vec<u8>>::new()
        );
        assert_eq!(
            feed_bytes_to_lines(&mut buf, b"\r\n"),
            vec![br#"%output %0 "abcd""#.to_vec()]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_many_lines_in_one_chunk() {
        // 一次 read 含多条 %output
        let mut buf = Vec::new();
        let lines = feed_bytes_to_lines(
            &mut buf,
            b"%output %0 a\r\n%output %1 b\r\n%window-add @1\n",
        );
        assert_eq!(
            lines,
            vec![
                br#"%output %0 a"#.to_vec(),
                br#"%output %1 b"#.to_vec(),
                b"%window-add @1".to_vec(),
            ]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_long_line_across_buffers() {
        // 单条很长的 %output 跨多个内部 buffer 拼起来
        let mut buf = Vec::new();
        let long = format!("%output %0 \"{}x\"\r\n", "a".repeat(8192));
        let bytes = long.as_bytes();
        let mut got = Vec::new();
        for chunk in bytes.chunks(1000) {
            got.extend(feed_bytes_to_lines(&mut buf, chunk));
        }
        assert_eq!(got.len(), 1);
        assert!(got[0].starts_with(b"%output %0".as_slice()));
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_incomplete_utf8_kept_in_buf() {
        // 非完整 UTF-8 chunk：多字节字符被拆在两个 chunk 里
        let mut buf = Vec::new();
        assert_eq!(
            feed_bytes_to_lines(&mut buf, "中文".as_bytes()[..3].to_vec().as_slice()),
            Vec::<Vec<u8>>::new()
        );
        let lines = feed_bytes_to_lines(&mut buf, &"中文".as_bytes()[3..]);
        // 无换行时不产出行
        assert_eq!(lines, Vec::<Vec<u8>>::new());
        assert!(!buf.is_empty());
    }

    #[test]
    fn feed_bytes_incomplete_utf8_flushed_on_newline() {
        let mut buf = Vec::new();
        assert_eq!(
            feed_bytes_to_lines(&mut buf, "中文".as_bytes()[..3].to_vec().as_slice()),
            Vec::<Vec<u8>>::new()
        );
        let lines = feed_bytes_to_lines(&mut buf, "文\r\n".as_bytes());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "中文".as_bytes().to_vec());
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_dcs_prefix_stripped() {
        let mut buf = Vec::new();
        let dcs = b"\x1bP1000p%begin 123 1 0\r\n";
        let lines = feed_bytes_to_lines(&mut buf, dcs);
        assert_eq!(lines, vec![b"%begin 123 1 0".to_vec()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_crlf_and_lf() {
        let mut buf = Vec::new();
        // 同时处理 CRLF 与 LF
        let lines = feed_bytes_to_lines(&mut buf, b"%begin 1 2 0\r\n%end 1 2 0\n");
        assert_eq!(
            lines,
            vec![b"%begin 1 2 0".to_vec(), b"%end 1 2 0".to_vec()]
        );
        assert!(buf.is_empty());
    }

    /// RED 回归（来自 b.log 真实 chunk）：`%output` 的 content 可能含原始
    /// 非 UTF-8 字节（如 0x94 0x80），行切分必须逐字节保留，不能先经过
    /// `String::from_utf8_lossy` 替换成 U+FFFD（0xEF 0xBF 0xBD）。
    #[test]
    fn feed_bytes_preserves_raw_non_utf8_output_content() {
        let mut buf = Vec::new();
        // b.log 中真实出现的字节序列：%output content 以原始字节内嵌，
        // 不是 C 转义文本 `\x94\x80`。
        let chunk: &[u8] = b"%output %25 \x94\x80\r\n";
        let lines = feed_bytes_to_lines(&mut buf, chunk);
        assert_eq!(lines.len(), 1, "应切出 1 行");
        assert_eq!(lines[0], b"%output %25 \x94\x80".to_vec());
        assert!(
            !lines[0].windows(3).any(|w| w == b"\xef\xbf\xbd"),
            "行切分不得提前用 lossy 替换非 UTF-8 字节: {:?}",
            lines[0]
        );
        assert!(buf.is_empty());
    }

    /// 真实样例（用户 SSH a.log）：把 htop / git-lg 的 `%output` 行按小 chunk 喂给
    /// `feed_bytes_to_lines`（模拟 pty 分包），再 `parse_line_bytes`，验证控制字节
    /// （SO \017、backspace \010、CRLF、UTF-8）在分包 + 拼接 + 解析全程不丢、不产生
    /// replacement char。这覆盖渲染前的字节保真。
    #[test]
    fn feed_real_samples_byte_fidelity_across_chunks() {
        let chunk_sizes = [1usize, 2, 3, 7, 16, 31, 64, 256];
        for name in ["real-htop", "real-git_lg", "real-ls_la", "real-codex"] {
            let bytes = std::fs::read(format!("tests/samples/{name}.txt"))
                .unwrap_or_else(|e| panic!("读取样例 {name} 失败: {e}"));
            let mut baseline = None;
            for chunk_size in chunk_sizes {
                let mut buf: Vec<u8> = Vec::new();
                let mut outputs = Vec::new();
                for chunk in bytes.chunks(chunk_size) {
                    for line in feed_bytes_to_lines(&mut buf, chunk) {
                        if let Some(crate::core::runtime::tmux::protocol::Message::Output {
                            content,
                            ..
                        }) = parse_line_bytes(&line)
                        {
                            outputs.push(content);
                        }
                    }
                }
                if !buf.is_empty() {
                    let tail = std::mem::take(&mut buf);
                    if let Some(crate::core::runtime::tmux::protocol::Message::Output {
                        content,
                        ..
                    }) = parse_line_bytes(&tail)
                    {
                        outputs.push(content);
                    }
                }
                assert!(
                    !outputs.is_empty(),
                    "{name} chunk_size={chunk_size}: no outputs"
                );
                let combined: Vec<u8> = outputs.into_iter().flatten().collect();
                assert!(
                    !combined.windows(3).any(|w| w == b"\xef\xbf\xbd"),
                    "{name} chunk_size={chunk_size}: replacement char"
                );
                if let Some(expected) = &baseline {
                    assert_eq!(
                        &combined, expected,
                        "{name} chunk_size={chunk_size}: chunking changed output bytes"
                    );
                } else {
                    baseline = Some(combined.clone());
                }
                assert!(
                    combined.contains(&0x1b),
                    "{name} chunk_size={chunk_size}: ESC was lost"
                );
                match name {
                    "real-htop" => assert!(
                        combined.contains(&0x0f),
                        "{name} chunk_size={chunk_size}: SO was lost"
                    ),
                    "real-git_lg" => assert!(
                        combined.contains(&0x0d),
                        "{name} chunk_size={chunk_size}: CR was lost"
                    ),
                    "real-codex" => assert!(
                        combined.windows(3).any(|w| w == b"\xe2\x9d\xaf"),
                        "{name} chunk_size={chunk_size}: UTF-8 prompt was lost"
                    ),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn feed_bytes_incomplete_line_is_capped_at_maximum() {
        let mut buf = Vec::new();
        let prefix = vec![b'a'; MAX_INCOMPLETE_LINE_BYTES];
        assert!(feed_bytes_to_lines(&mut buf, &prefix).is_empty());
        assert_eq!(buf.len(), MAX_INCOMPLETE_LINE_BYTES);

        assert!(feed_bytes_to_lines(&mut buf, b"b").is_empty());
        assert_eq!(buf.len(), MAX_INCOMPLETE_LINE_BYTES);
        assert_eq!(buf.last(), Some(&b'b'));

        let mut with_newline = vec![b'c'; MAX_INCOMPLETE_LINE_BYTES];
        with_newline.push(b'\n');
        let lines = feed_bytes_to_lines(&mut buf, &with_newline);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_INCOMPLETE_LINE_BYTES);
        assert!(lines[0].iter().all(|&b| b == b'c'));
        assert!(buf.is_empty());
    }

    #[test]
    fn feed_bytes_preserves_empty_lines_and_truncated_tail() {
        let mut buf = Vec::new();
        let lines = feed_bytes_to_lines(&mut buf, b"\r\n\n%output %1 \"tail");
        assert_eq!(lines, vec![Vec::<u8>::new(), Vec::<u8>::new()]);
        assert_eq!(buf, br#"%output %1 "tail"#.to_vec());

        let lines = feed_bytes_to_lines(&mut buf, b"\"\r\n");
        assert_eq!(lines, vec![br#"%output %1 "tail""#.to_vec()]);
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn process_line_preserves_raw_output_and_response_boundary() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut response = None;

        process_line(b"%begin 10 99 0", &tx, &mut response).await;
        process_line(b"response \xff", &tx, &mut response).await;
        process_line(b"%output %9 \"\x94\x80\"", &tx, &mut response).await;
        process_line(b"%end 10 99 0", &tx, &mut response).await;

        assert!(response.is_none());
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(events.iter().any(|event| matches!(
            event,
            TmuxEvent::Message(Message::Output {
                pane: crate::core::types::PaneId(9),
                content,
                ..
            }) if content == b"\x94\x80"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TmuxEvent::ResponseBlock { number: 99, lines, .. }
                if lines.iter().any(|line| line.contains('\u{fffd}'))
        )));
    }
}
