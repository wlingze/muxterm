//! 内置 Driver / Transport 插件（包装现有实现，不复制协议解析）。
//!
//! `with_builtins` 按 tmux, herdr, shell / local, ssh 顺序登记；表是数组。

pub mod herdr;
pub mod local;
pub mod shell;
pub mod ssh;
pub mod tmux;

use super::driver::RuntimeDriver;
use super::transport::Transport;

/// 生产入口用的内置插件表（顺序锁死，不要排序）。
pub fn builtin_runtimes() -> Vec<Box<dyn RuntimeDriver>> {
    vec![
        Box::new(tmux::TmuxDriver),
        Box::new(herdr::HerdrDriver),
        Box::new(shell::ShellDriver),
    ]
}

/// 内置 Transport 插件表（顺序锁死）。
pub fn builtin_transports() -> Vec<Box<dyn Transport>> {
    vec![Box::new(local::LocalTransport), Box::new(ssh::SshTransport)]
}
