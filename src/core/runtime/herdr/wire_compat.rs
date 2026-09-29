//! Herdr private client-socket formats supported by the pane stream.
//!
//! The JSON API snapshot advertises the server's wire protocol. The direct
//! terminal format changed in 0.8.2 (20) and 0.9.0 (22), so a version number
//! alone cannot make an old bincode enum safe to decode.

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use super::wire::{
    read_message, write_message, AttachScrollDirection, AttachScrollSource, ClientKeybindings,
    ClientLaunchMode, ClientMessage, RenderEncoding, ServerMessage, TerminalFrame, MAX_FRAME_SIZE,
};

const MAX_V22_FRAME_SIZE: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireProtocol {
    V19,
    V20,
    V22,
}

impl WireProtocol {
    pub fn from_number(version: u32) -> Result<Self> {
        match version {
            19 => Ok(Self::V19),
            20 => Ok(Self::V20),
            22 => Ok(Self::V22),
            _ => bail!("Herdr socket protocol {version} is unsupported (supported: 19, 20, 22)"),
        }
    }

    pub fn number(self) -> u32 {
        match self {
            Self::V19 => 19,
            Self::V20 => 20,
            Self::V22 => 22,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StreamMessage {
    Welcome {
        version: u32,
        encoding: RenderEncoding,
        error: Option<String>,
    },
    Terminal(TerminalFrame),
    Shutdown(Option<String>),
    MouseCapture {
        enabled: bool,
        sgr_pixels: bool,
    },
    Ignored,
}

#[derive(Serialize)]
enum LaunchMode20 {
    App,
    AppDirectGraphics,
    TerminalAttach,
}

#[derive(Serialize)]
enum Hello20 {
    Hello {
        version: u32,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        requested_encoding: RenderEncoding,
        keybindings: ClientKeybindings,
        launch_mode: LaunchMode20,
    },
}

// Indices 0..8 match Herdr v0.9.1 src/protocol/wire.rs. Unused earlier
// variants are placeholders; only the messages below are ever serialized.
#[derive(Serialize)]
enum ClientMessage22 {
    TerminalHello {
        version: u32,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    Input {
        data: Vec<u8>,
    },
    ClipboardImage,
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    Detach,
    AttachTerminal,
    AttachScroll {
        source: AttachScrollSource,
        direction: AttachScrollDirection,
        lines: u16,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    },
    ObserveTerminal {
        target: String,
    },
    ControlTerminal {
        target: String,
        takeover: bool,
    },
}

pub fn write_hello<W: Write>(
    writer: &mut W,
    protocol: WireProtocol,
    cols: u16,
    rows: u16,
) -> Result<()> {
    match protocol {
        WireProtocol::V19 => write_message(
            writer,
            &ClientMessage::Hello {
                version: 19,
                cols,
                rows,
                cell_width_px: 0,
                cell_height_px: 0,
                requested_encoding: RenderEncoding::TerminalAnsi,
                keybindings: ClientKeybindings::Server,
                launch_mode: ClientLaunchMode::TerminalAttach,
            },
        ),
        WireProtocol::V20 => write_message(
            writer,
            &Hello20::Hello {
                version: 20,
                cols,
                rows,
                cell_width_px: 0,
                cell_height_px: 0,
                requested_encoding: RenderEncoding::TerminalAnsi,
                keybindings: ClientKeybindings::Server,
                launch_mode: LaunchMode20::TerminalAttach,
            },
        ),
        WireProtocol::V22 => write_message(
            writer,
            &ClientMessage22::TerminalHello {
                version: 22,
                cols,
                rows,
                cell_width_px: 0,
                cell_height_px: 0,
                pixel_mouse: false,
            },
        ),
    }
}

pub fn write_client_message<W: Write>(
    writer: &mut W,
    protocol: WireProtocol,
    message: &ClientMessage,
) -> Result<()> {
    if protocol != WireProtocol::V22 {
        return write_message(writer, message);
    }
    let message = match message {
        ClientMessage::Input { data } => ClientMessage22::Input { data: data.clone() },
        ClientMessage::Resize {
            cols,
            rows,
            cell_width_px,
            cell_height_px,
        } => ClientMessage22::Resize {
            cols: *cols,
            rows: *rows,
            cell_width_px: *cell_width_px,
            cell_height_px: *cell_height_px,
            pixel_mouse: false,
        },
        ClientMessage::Detach => ClientMessage22::Detach,
        ClientMessage::ObserveTerminal { target } => ClientMessage22::ObserveTerminal {
            target: target.clone(),
        },
        ClientMessage::ControlTerminal { target, takeover } => ClientMessage22::ControlTerminal {
            target: target.clone(),
            takeover: *takeover,
        },
        ClientMessage::AttachScroll {
            source,
            direction,
            lines,
            column,
            row,
            modifiers,
        } => ClientMessage22::AttachScroll {
            source: source.clone(),
            direction: *direction,
            lines: *lines,
            column: *column,
            row: *row,
            modifiers: *modifiers,
        },
        _ => bail!("unsupported Herdr v22 pane-stream command"),
    };
    write_message(writer, &message)
}

pub fn read_stream_message<R: Read>(
    reader: &mut R,
    protocol: WireProtocol,
) -> Result<StreamMessage> {
    if protocol == WireProtocol::V19 {
        let message: ServerMessage = read_message(reader, MAX_FRAME_SIZE)?;
        return Ok(match message {
            ServerMessage::Welcome {
                version,
                encoding,
                error,
            } => StreamMessage::Welcome {
                version,
                encoding,
                error,
            },
            ServerMessage::Terminal(frame) => StreamMessage::Terminal(frame),
            ServerMessage::ServerShutdown { reason } => StreamMessage::Shutdown(reason),
            ServerMessage::MouseCapture { enabled } => StreamMessage::MouseCapture {
                enabled,
                sgr_pixels: false,
            },
            _ => StreamMessage::Ignored,
        });
    }

    let mut length = [0u8; 4];
    reader
        .read_exact(&mut length)
        .context("read Herdr frame length")?;
    let length = u32::from_le_bytes(length) as usize;
    let max = if protocol == WireProtocol::V22 {
        MAX_V22_FRAME_SIZE
    } else {
        MAX_FRAME_SIZE
    };
    if length > max {
        bail!("Herdr frame too large: {length} > {max}");
    }
    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .context("read Herdr frame")?;
    let (tag, _): (u32, usize) =
        bincode::serde::decode_from_slice(&payload, bincode::config::standard())
            .context("decode Herdr message tag")?;
    let is_v22 = protocol == WireProtocol::V22;
    let max_known_tag = if is_v22 { 19 } else { 14 };
    match tag {
        0 => {
            let (_, version, encoding, error): (u32, u32, RenderEncoding, Option<String>) =
                decode_payload(&payload)?;
            Ok(StreamMessage::Welcome {
                version,
                encoding,
                error,
            })
        }
        1 if is_v22 => {
            let (_, frame): (u32, TerminalFrame) = decode_payload(&payload)?;
            Ok(StreamMessage::Terminal(frame))
        }
        2 if !is_v22 => {
            let (_, frame): (u32, TerminalFrame) = decode_payload(&payload)?;
            Ok(StreamMessage::Terminal(frame))
        }
        3 if is_v22 => {
            let (_, reason): (u32, Option<String>) = decode_payload(&payload)?;
            Ok(StreamMessage::Shutdown(reason))
        }
        4 if !is_v22 => {
            let (_, reason): (u32, Option<String>) = decode_payload(&payload)?;
            Ok(StreamMessage::Shutdown(reason))
        }
        8 if is_v22 => {
            let (_, enabled, sgr_pixels): (u32, bool, bool) = decode_payload(&payload)?;
            Ok(StreamMessage::MouseCapture {
                enabled,
                sgr_pixels,
            })
        }
        9 if !is_v22 => {
            let (_, enabled, sgr_pixels): (u32, bool, bool) = decode_payload(&payload)?;
            Ok(StreamMessage::MouseCapture {
                enabled,
                sgr_pixels,
            })
        }
        tag if tag <= max_known_tag => Ok(StreamMessage::Ignored),
        _ => bail!(
            "unknown Herdr protocol {} message tag {tag}",
            protocol.number()
        ),
    }
}

fn decode_payload<T: serde::de::DeserializeOwned>(payload: &[u8]) -> Result<T> {
    let (message, consumed): (T, usize) =
        bincode::serde::decode_from_slice(payload, bincode::config::standard())
            .context("decode Herdr message")?;
    if consumed != payload.len() {
        bail!(
            "Herdr message has {} trailing bytes",
            payload.len() - consumed
        );
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_selection_preserves_known_formats() {
        assert_eq!(WireProtocol::from_number(19).unwrap(), WireProtocol::V19);
        assert_eq!(WireProtocol::from_number(20).unwrap(), WireProtocol::V20);
        assert_eq!(WireProtocol::from_number(22).unwrap(), WireProtocol::V22);
        assert!(WireProtocol::from_number(23).is_err());
    }

    #[test]
    fn v22_hello_and_control_have_official_tags() {
        let mut hello = Vec::new();
        write_hello(&mut hello, WireProtocol::V22, 120, 40).unwrap();
        assert_eq!(hello[4], 0);
        assert_eq!(hello[5], 22);
        let mut control = Vec::new();
        write_client_message(
            &mut control,
            WireProtocol::V22,
            &ClientMessage::ControlTerminal {
                target: "w1:p1".into(),
                takeover: true,
            },
        )
        .unwrap();
        assert_eq!(control[4], 8);
    }

    #[test]
    fn v20_hello_uses_direct_attach_launch_mode_two() {
        let mut hello = Vec::new();
        write_hello(&mut hello, WireProtocol::V20, 80, 24).unwrap();
        assert_eq!(hello[4], 0);
        assert_eq!(hello[5], 20);
        assert_eq!(hello.last(), Some(&2));
    }

    #[test]
    fn v20_and_v22_mouse_messages_decode_their_own_layout() {
        for (protocol, tag) in [(WireProtocol::V20, 9u32), (WireProtocol::V22, 8u32)] {
            let payload =
                bincode::serde::encode_to_vec((tag, true, true), bincode::config::standard())
                    .unwrap();
            let mut framed = (payload.len() as u32).to_le_bytes().to_vec();
            framed.extend(payload);
            assert_eq!(
                read_stream_message(&mut framed.as_slice(), protocol).unwrap(),
                StreamMessage::MouseCapture {
                    enabled: true,
                    sgr_pixels: true
                },
            );
        }
    }
}
