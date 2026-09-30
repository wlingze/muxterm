//! Herdr Runtime：连 Herdr named session 的 Unix socket，把 Herdr workspace
//! 填成 Muxterm Workspace（tab/pane/字节）。
//!
//! 仅此目录允许出现 `herdr.sock` / `w2:p1` / `terminal.frame`。
//! 生产代码不在本机执行 `Command::new("herdr")`：API 走 socket JSON，
//! 直播字节走 client socket 的 observe 流（bincode 帧）。SSH 的显式
//! CreateIfMissing 经 TargetConnection::Exec 启动远端 headless server。

mod channel;
pub mod events;
pub mod forward;
mod locale;
pub mod mutation;
pub mod observe;
pub mod provider;
pub mod registry;
pub mod runtime;
pub mod session;
pub mod wire;
mod wire_compat;

pub(super) use provider::HerdrDriver;
#[allow(unused_imports)]
pub use runtime::HerdrRuntime;
#[allow(unused_imports)]
pub use session::HerdrSession;

mod update;
