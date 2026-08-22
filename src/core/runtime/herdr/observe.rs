//! Herdr pane stream：连 client socket，Hello 后取得终端流；同一 socket 发送
//! 原始输入/resize，并把 `terminal.frame` ANSI 送进 channel。
//!
//! 生产代码**不** `Command::new("herdr")`；这里直接实现 herdr 0.8.0
//! （协议 19）的 bincode 线协议（参考 `~/Developer/terminal/herdr`）。
//!
//! 流有两种模式：
//! - [`StreamMode::Observe`]：只读，可有多个 observer，不拥有输入/resize/takeover 权；
//! - [`StreamMode::Control`]：可写，一个终端同时只有一个 controller。
//!
//! 所有权纪律：reader 事件的每个事件都带 `(pane, generation, event_ordinal)`，
//! Frame 保留 Herdr wire `seq`；runtime 只接受 current generation 且 ordinal
//! 严格递增的事件。start/handshake 由 generation-tagged worker 完成，调用线程
//! 只登记 `Starting`，不能同步等 socket。

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use anyhow::{bail, Context, Result};

use crate::core::types::PaneId;

use super::wire::{
    read_message, write_message, ClientKeybindings, ClientLaunchMode, ClientMessage,
    RenderEncoding, ServerMessage, HERDR_PROTOCOL_VERSION, MAX_FRAME_SIZE,
};

/// 一条 pane 流的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamMode {
    /// 只读 observer（可有多个；不拥有输入/resize/takeover 权）。
    Observe,
    /// 可写 controller（一个终端同时只有一个）。
    Control,
}

impl StreamMode {
    pub fn is_control(&self) -> bool {
        matches!(self, StreamMode::Control)
    }
}

/// observe/control 流事件（reader 线程 → runtime）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneStreamEvent {
    /// 一帧 ANSI 字节（full=true 是全量重绘，否则是增量 diff）。
    Frame {
        pane: PaneId,
        generation: u64,
        /// reader 线程从 1 开始的单调序号（不跨 generation 比较）。
        event_ordinal: u64,
        /// Herdr wire 帧序号（原样取自 TerminalFrame.seq；随新流重置）。
        wire_seq: u64,
        bytes: Vec<u8>,
        width: u16,
        height: u16,
        full: bool,
    },
    /// server 关闭流。
    Closed {
        pane: PaneId,
        generation: u64,
        event_ordinal: u64,
        reason: Option<String>,
    },
    /// 读流出错。
    Error {
        pane: PaneId,
        generation: u64,
        event_ordinal: u64,
        message: String,
    },
}

/// start worker 的完成结果（generation-tagged）。
pub enum StreamStartResult {
    Started {
        pane: PaneId,
        generation: u64,
        stream: ObserveStream,
    },
    Failed {
        pane: PaneId,
        generation: u64,
        message: String,
    },
}

/// 一条 pane 的流：持有 reader 线程 + 共享 channel。
pub struct ObserveStream {
    pane: PaneId,
    generation: u64,
    mode: StreamMode,
    command_stream: Option<UnixStream>,
    shutdown_stream: Option<UnixStream>,
    handle: Option<JoinHandle<()>>,
    /// Drop/replace 时置位：reader 据此把「主动关闭造成的 EOF/Error」静默
    /// 掉，不向 Runtime 误报流死亡（否则 replace 替换流会残留一个假的
    /// Error/Closed，把刚重建的新流也一起删掉）。
    dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl ObserveStream {
    /// 连接 client socket、握手、按 mode 发 `ObserveTerminal`/`ControlTerminal`，
    /// 然后起 reader 线程。**阻塞**：只允许在 start worker 线程里调用。
    ///
    /// `takeover` 只对 Control 有意义：只有新的本地 focus edge 或真实 input
    /// 建立的 intent 首次 promote 才允许 true；open/reattach/activate 的首个
    /// handshake 必须 false。
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        socket_path: &Path,
        target: &str,
        pane: PaneId,
        generation: u64,
        mode: StreamMode,
        takeover: bool,
        cols: u16,
        rows: u16,
        tx: Sender<PaneStreamEvent>,
    ) -> Result<Self> {
        let mut stream = UnixStream::connect(socket_path)
            .with_context(|| format!("连接 Herdr client socket 失败: {}", socket_path.display()))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .context("设置流读超时失败")?;

        let hello = ClientMessage::Hello {
            version: HERDR_PROTOCOL_VERSION,
            cols,
            rows,
            cell_width_px: 0,
            cell_height_px: 0,
            requested_encoding: RenderEncoding::TerminalAnsi,
            keybindings: ClientKeybindings::Server,
            launch_mode: ClientLaunchMode::TerminalAttach,
        };
        write_message(&mut stream, &hello).context("写 Herdr Hello 失败")?;
        let welcome: ServerMessage =
            read_message(&mut stream, MAX_FRAME_SIZE).context("读 Herdr Welcome 失败")?;
        match welcome {
            ServerMessage::Welcome {
                version,
                encoding,
                error,
            } => {
                if let Some(e) = error {
                    bail!("Herdr 拒绝握手（version={version}）: {e}");
                }
                if encoding != RenderEncoding::TerminalAnsi {
                    bail!("Herdr 协商了非 ANSI 编码: {encoding:?}");
                }
                // 协议 20+ Welcome 或任何 version 不匹配必须明确拒绝，不能
                // 继续按 protocol-19 解码（wire 变体索引会漂移）。
                if version != HERDR_PROTOCOL_VERSION {
                    bail!("Herdr 协议版本不匹配: client 19, server {version}；拒绝继续");
                }
            }
            other => bail!("Herdr 握手响应不是 Welcome: {other:?}"),
        }

        let message = match mode {
            StreamMode::Observe => ClientMessage::ObserveTerminal {
                target: target.to_string(),
            },
            StreamMode::Control => ClientMessage::ControlTerminal {
                target: target.to_string(),
                takeover,
            },
        };
        write_message(&mut stream, &message).context("写 Herdr terminal 请求失败")?;
        let command_stream = stream.try_clone().context("复制 Herdr 写 socket 失败")?;
        let shutdown_stream = stream
            .try_clone()
            .context("复制 Herdr shutdown socket 失败")?;

        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_dropped = std::sync::Arc::clone(&dropped);
        let handle = std::thread::spawn(move || {
            let mut stream = stream;
            let mut event_ordinal: u64 = 0;
            loop {
                // 主动 shutdown 期间的 EOF/Error 不发，避免误报流死亡。
                let alive = !reader_dropped.load(std::sync::atomic::Ordering::Acquire);
                match read_message::<_, ServerMessage>(&mut stream, MAX_FRAME_SIZE) {
                    Ok(ServerMessage::Terminal(frame)) => {
                        event_ordinal = event_ordinal.saturating_add(1);
                        if tx
                            .send(PaneStreamEvent::Frame {
                                pane,
                                generation,
                                event_ordinal,
                                wire_seq: frame.seq,
                                bytes: frame.bytes,
                                width: frame.width,
                                height: frame.height,
                                full: frame.full,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(ServerMessage::ServerShutdown { reason }) => {
                        if alive {
                            event_ordinal = event_ordinal.saturating_add(1);
                            let _ = tx.send(PaneStreamEvent::Closed {
                                pane,
                                generation,
                                event_ordinal,
                                reason,
                            });
                        }
                        return;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        if alive {
                            event_ordinal = event_ordinal.saturating_add(1);
                            let _ = tx.send(PaneStreamEvent::Error {
                                pane,
                                generation,
                                event_ordinal,
                                message: err.to_string(),
                            });
                        }
                        return;
                    }
                }
            }
        });

        Ok(Self {
            pane,
            generation,
            mode,
            command_stream: Some(command_stream),
            shutdown_stream: Some(shutdown_stream),
            handle: Some(handle),
            dropped,
        })
    }

    /// 在 worker 线程里完成 connect+handshake；结果经 `start_tx` 送回调用线程。
    /// 调用线程只登记 `Starting`，不能同步等 socket。
    #[allow(clippy::too_many_arguments)]
    pub fn start_async(
        socket_path: PathBuf,
        target: String,
        pane: PaneId,
        generation: u64,
        mode: StreamMode,
        takeover: bool,
        cols: u16,
        rows: u16,
        event_tx: Sender<PaneStreamEvent>,
        start_tx: Sender<StreamStartResult>,
    ) {
        std::thread::spawn(move || {
            let result = Self::start(
                &socket_path,
                &target,
                pane,
                generation,
                mode,
                takeover,
                cols,
                rows,
                event_tx,
            );
            match result {
                Ok(stream) => {
                    let _ = start_tx.send(StreamStartResult::Started {
                        pane,
                        generation,
                        stream,
                    });
                }
                Err(err) => {
                    let _ = start_tx.send(StreamStartResult::Failed {
                        pane,
                        generation,
                        message: err.to_string(),
                    });
                }
            }
        });
    }

    pub fn pane(&self) -> PaneId {
        self.pane
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn mode(&self) -> StreamMode {
        self.mode
    }

    pub fn send_input(&mut self, data: &[u8]) -> Result<()> {
        let stream = self
            .command_stream
            .as_mut()
            .context("Herdr control stream 已关闭")?;
        write_message(
            stream,
            &ClientMessage::Input {
                data: data.to_vec(),
            },
        )
        .context("写 Herdr terminal input 失败")
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let stream = self
            .command_stream
            .as_mut()
            .context("Herdr control stream 已关闭")?;
        write_message(
            stream,
            &ClientMessage::Resize {
                cols: cols.max(2),
                rows: rows.max(1),
                cell_width_px: 0,
                cell_height_px: 0,
            },
        )
        .context("写 Herdr terminal resize 失败")
    }
}

impl Drop for ObserveStream {
    fn drop(&mut self) {
        // 先置位：reader 不再上报 EOF/Error，避免 replace 残留事件误删新流。
        self.dropped
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(mut stream) = self.command_stream.take() {
            let _ = write_message(&mut stream, &ClientMessage::Detach);
        }
        // 主线程持有同一 socket 的 clone；shutdown 会打断 reader 的阻塞读，
        // 这样 resize 时替换 observer 不会留下重复流。仍不 join，避免 Drop
        // 因平台 socket 行为阻塞 GTK 线程。
        if let Some(stream) = self.shutdown_stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        self.handle.take();
    }
}

/// 空 channel 工厂（测试用）。
pub fn channel() -> (Sender<PaneStreamEvent>, Receiver<PaneStreamEvent>) {
    mpsc::channel()
}
