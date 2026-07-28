//! 测试支持模块：共享 harness for E2E 测试。
//!
//! 提供：
//! - 独立 tmux socket 管理（绝不复用宿主默认 socket）
//! - 硬超时包装
//! - SSH loopback sshd harness

pub mod sshd_harness;
pub mod tmux_harness;
