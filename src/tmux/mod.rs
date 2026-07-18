//! tmux 控制协议子模块
//!
//! 对外暴露：
//! - [`protocol`]：消息类型与行解析器（`Message` / `parse_line`）
//! - [`command`]：强类型命令构造器（`TmuxCommand` 及各 newtype ID）
//! - [`client`]：异步 tmux `-CC` 客户端（`TmuxClient`）
//! - [`pty`]：PTY 辅助（为 tmux -CC 分配伪终端）

pub mod client;
pub mod command;
pub mod protocol;
pub mod pty;

#[allow(unused_imports)]
pub use client::{ConnectMode, TmuxClient, TmuxClientConfig, TmuxClientHandle, TmuxEvent};
#[allow(unused_imports)]
pub use command::{Key, PaneId, SessionId, TmuxCommand, WindowId};
#[allow(unused_imports)]
pub use protocol::{
    parse_line, ControlEscapeDecoder, ControlEscapeError, LayoutChange, Message, NotificationKind,
    ProtocolError, ResponseBoundary,
};
