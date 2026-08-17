//! Herdr client socket 的二进制线协议（bincode 2 standard + u32LE 长度前缀）。
//!
//! 只实现 observe 流需要的子集：Hello / ObserveTerminal 请求，
//! Welcome / Terminal / ServerShutdown 响应。变体顺序与 herdr 0.8.0
//! （协议 19）一致；未用变体用占位类型保持索引不变。
//!
//! 参考（只读）：`~/Developer/terminal/herdr/src/protocol/wire.rs`。

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// 本机 herdr 0.8.0 的 socket 协议版本。
pub const HERDR_PROTOCOL_VERSION: u32 = 19;

/// 普通帧上限（与 herdr 一致：2 MiB）。
pub const MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;

/// 渲染编码：observe 流协商 TerminalAnsi。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderEncoding {
    SemanticFrame,
    TerminalAnsi,
}

/// 键位配置：observe 用 Server。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKeybindings {
    Server,
    Local { keys_toml: String },
}

/// 启动模式（协议 19 顺序：App=0, TerminalAttach=1, AppDirectGraphics=2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientLaunchMode {
    App,
    TerminalAttach,
    AppDirectGraphics,
}

// 未用变体的占位类型（保持 ClientMessage 变体索引与 herdr 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachScrollSource {
    Wheel,
    PageKey { input: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientInputEvent {
    Key {
        code: u8,
        modifiers: u8,
        kind: u8,
        repeat_count: u16,
    },
}

/// Client → Server 消息（变体索引必须与 herdr 协议 19 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello {
        version: u32,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        requested_encoding: RenderEncoding,
        keybindings: ClientKeybindings,
        launch_mode: ClientLaunchMode,
    },
    Input {
        data: Vec<u8>,
    },
    ClipboardImage {
        extension: String,
        data: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    Detach,
    AttachTerminal {
        terminal_id: String,
        takeover: bool,
    },
    AttachScroll {
        source: AttachScrollSource,
        direction: AttachScrollDirection,
        lines: u16,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    },
    InputEvents {
        events: Vec<ClientInputEvent>,
    },
    ObserveTerminal {
        target: String,
    },
}

/// Terminal ANSI 帧。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TerminalFrame {
    pub seq: u64,
    pub width: u16,
    pub height: u16,
    pub full: bool,
    pub bytes: Vec<u8>,
}

// 未用变体的占位类型（保持 ServerMessage 变体索引与 herdr 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum NotifyKind {
    Sound,
    Toast,
    SystemToast,
}

/// Server → Client 消息（变体索引必须与 herdr 协议 19 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum ServerMessage {
    Welcome {
        version: u32,
        encoding: RenderEncoding,
        error: Option<String>,
    },
    Frame(serde_json::Value),
    Terminal(TerminalFrame),
    Graphics {
        bytes: Vec<u8>,
    },
    ServerShutdown {
        reason: Option<String>,
    },
    Notify {
        kind: NotifyKind,
        message: String,
        body: Option<String>,
    },
    Clipboard {
        data: String,
    },
    WindowTitle {
        title: Option<String>,
    },
    ReloadSoundConfig,
    MouseCapture {
        enabled: bool,
        sgr_pixels: bool,
    },
    KittyKeyboardReportAll {
        enabled: bool,
    },
    PrefixInputSource {
        active: bool,
    },
    TerminalBell {
        count: u16,
    },
    GraphicsFile {
        path: String,
        expected_len: u64,
        image_id: u32,
        transfer_id: u64,
        leading: Vec<u8>,
        control: String,
    },
    GraphicsTransmissionRetired {
        transfer_id: u64,
        image_id: u32,
    },
}

/// 写一条长度前缀帧：`[u32LE len][bincode payload]`。
pub fn write_message<W: Write, M: Serialize>(writer: &mut W, msg: &M) -> Result<()> {
    let payload = bincode::serde::encode_to_vec(msg, bincode::config::standard())
        .context("bincode 编码 Herdr 消息失败")?;
    if payload.len() > u32::MAX as usize {
        bail!("Herdr 消息过大: {} bytes", payload.len());
    }
    writer
        .write_all(&(payload.len() as u32).to_le_bytes())
        .context("写 Herdr 帧长度失败")?;
    writer.write_all(&payload).context("写 Herdr 帧失败")?;
    writer.flush().context("flush Herdr 帧失败")?;
    Ok(())
}

/// 读一条长度前缀帧并反序列化。
pub fn read_message<R: Read, M: for<'de> Deserialize<'de>>(
    reader: &mut R,
    max_frame_size: usize,
) -> Result<M> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .context("读 Herdr 帧长度失败")?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > max_frame_size {
        bail!("Herdr 帧过大: {len} > {max_frame_size}");
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).context("读 Herdr 帧失败")?;
    let (msg, consumed) = bincode::serde::decode_from_slice(&payload, bincode::config::standard())
        .context("bincode 解码 Herdr 消息失败")?;
    if consumed != len {
        bail!("Herdr 帧尾随字节: decoded {consumed} of {len}");
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 协议 19 的 Hello 字节（strace 实测 herdr 0.8.0 observe 握手）：
    /// `00 13 50 28 00 00 01 00 01`。
    #[test]
    fn hello_encodes_protocol19_bytes() {
        let hello = ClientMessage::Hello {
            version: 19,
            cols: 80,
            rows: 40,
            cell_width_px: 0,
            cell_height_px: 0,
            requested_encoding: RenderEncoding::TerminalAnsi,
            keybindings: ClientKeybindings::Server,
            launch_mode: ClientLaunchMode::TerminalAttach,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &hello).unwrap();
        assert_eq!(buf, b"\x09\x00\x00\x00\x00\x13P(\x00\x00\x01\x00\x01");
    }

    /// ObserveTerminal 变体索引 = 8（协议 19）。
    #[test]
    fn observe_terminal_encodes_variant_8() {
        let msg = ClientMessage::ObserveTerminal {
            target: "w1:p1".into(),
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        assert_eq!(buf, b"\x07\x00\x00\x00\x08\x05w1:p1");
    }

    /// Welcome 响应（协议 19）：`00 13 01 00`。
    #[test]
    fn welcome_decodes_protocol19_bytes() {
        let payload = b"\x00\x13\x01\x00";
        let mut len = (payload.len() as u32).to_le_bytes().to_vec();
        len.extend_from_slice(payload);
        let msg: ServerMessage = read_message(&mut &len[..], MAX_FRAME_SIZE).unwrap();
        assert!(matches!(
            msg,
            ServerMessage::Welcome {
                version: 19,
                encoding: RenderEncoding::TerminalAnsi,
                error: None
            }
        ));
    }
}
