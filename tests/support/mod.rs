//! 测试支持模块：共享 E2E 测试支持。
//!
//! 提供：
//! - 独立 tmux socket 管理（绝不复用宿主默认 socket）
//! - 硬超时包装
//! - SSH loopback sshd 测试支持

pub mod sshd_test_support;
pub mod tmux_test_support;
