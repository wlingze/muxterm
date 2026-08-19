//! Herdr pane control 流：连 client socket，Hello 后取得终端控制权；同一
//! socket 发送原始输入/resize，并把 `terminal.frame` ANSI 送进 channel。
//!
//! 生产代码**不** `Command::new("herdr")`；这里直接实现 herdr 0.8.0
//! （协议 19）的 bincode 线协议（参考 `~/Developer/terminal/herdr`）。

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use anyhow::{bail, Context, Result};

use crate::core::types::PaneId;

use super::wire::{
    read_message, write_message, ClientKeybindings, ClientLaunchMode, ClientMessage,
    RenderEncoding, ServerMessage, HERDR_PROTOCOL_VERSION, MAX_FRAME_SIZE,
};

/// observe 流事件（reader 线程 → runtime）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveEvent {
    /// 一帧 ANSI 字节（full=true 是全量重绘，否则是增量 diff）。
    Frame {
        pane: PaneId,
        bytes: Vec<u8>,
        width: u16,
        height: u16,
        full: bool,
    },
    /// server 关闭流。
    Closed {
        pane: PaneId,
        reason: Option<String>,
    },
    /// 读流出错。
    Error { pane: PaneId, message: String },
}

/// 一条 pane 的 observe 流：持有 reader 线程 + 共享 channel。
pub struct ObserveStream {
    pane: PaneId,
    command_stream: Option<UnixStream>,
    shutdown_stream: Option<UnixStream>,
    handle: Option<JoinHandle<()>>,
}

impl ObserveStream {
    /// 连接 client socket、握手、发 `ObserveTerminal`，然后起 reader 线程。
    pub fn start(
        socket_path: &Path,
        target: &str,
        pane: PaneId,
        cols: u16,
        rows: u16,
        tx: Sender<ObserveEvent>,
    ) -> Result<Self> {
        let mut stream = UnixStream::connect(socket_path)
            .with_context(|| format!("连接 Herdr client socket 失败: {}", socket_path.display()))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .context("设置 observe 读超时失败")?;

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
                    bail!("Herdr 拒绝 observe 握手（version={version}）: {e}");
                }
                if encoding != RenderEncoding::TerminalAnsi {
                    bail!("Herdr 协商了非 ANSI 编码: {encoding:?}");
                }
            }
            other => bail!("Herdr 握手响应不是 Welcome: {other:?}"),
        }

        let control = ClientMessage::ControlTerminal {
            target: target.to_string(),
            takeover: true,
        };
        write_message(&mut stream, &control).context("写 ControlTerminal 失败")?;
        let command_stream = stream
            .try_clone()
            .context("复制 Herdr control 写 socket 失败")?;
        let shutdown_stream = stream
            .try_clone()
            .context("复制 Herdr control shutdown socket 失败")?;

        let handle = std::thread::spawn(move || {
            let mut stream = stream;
            loop {
                match read_message::<_, ServerMessage>(&mut stream, MAX_FRAME_SIZE) {
                    Ok(ServerMessage::Terminal(frame)) => {
                        if tx
                            .send(ObserveEvent::Frame {
                                pane,
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
                        let _ = tx.send(ObserveEvent::Closed { pane, reason });
                        return;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        let _ = tx.send(ObserveEvent::Error {
                            pane,
                            message: err.to_string(),
                        });
                        return;
                    }
                }
            }
        });

        Ok(Self {
            pane,
            command_stream: Some(command_stream),
            shutdown_stream: Some(shutdown_stream),
            handle: Some(handle),
        })
    }

    pub fn pane(&self) -> PaneId {
        self.pane
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
pub fn channel() -> (Sender<ObserveEvent>, Receiver<ObserveEvent>) {
    mpsc::channel()
}
