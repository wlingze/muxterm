//! Herdr observe 流：连 client socket，Hello 握手后 `ObserveTerminal`，
//! 后台线程把 `terminal.frame` 的 ANSI 字节送进 channel。
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

        let observe = ClientMessage::ObserveTerminal {
            target: target.to_string(),
        };
        write_message(&mut stream, &observe).context("写 ObserveTerminal 失败")?;

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
            handle: Some(handle),
        })
    }

    pub fn pane(&self) -> PaneId {
        self.pane
    }
}

impl Drop for ObserveStream {
    fn drop(&mut self) {
        // 不 join：reader 可能阻塞在 30s 读超时上；drop JoinHandle 即 detach，
        // server 关闭后线程自然 EOF 退出。
        self.handle.take();
    }
}

/// 空 channel 工厂（测试用）。
pub fn channel() -> (Sender<ObserveEvent>, Receiver<ObserveEvent>) {
    mpsc::channel()
}
