//! Herdr Runtime：连 Herdr named session 的 Unix socket，把 Herdr workspace
//! 填成 Muxterm Workspace（tab/pane/字节）。
//!
//! 仅此目录允许出现 `herdr.sock` / `w2:p1` / `terminal.frame`。
//! 生产代码**禁止** `Command::new("herdr")`：API 走 socket JSON，
//! 直播字节走 client socket 的 observe 流（bincode 帧）。

pub mod session;
