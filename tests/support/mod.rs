//! 测试支持模块：共享 long-chain 集成测试支持。
//!
//! - behavior_driver: 共享 2tab3pane 行为场景和断言
//! - sshd_test_support: SSH loopback sshd 连接参数
//! - tmux_test_support: 独立 tmux socket/session 管理 + 硬超时

#[allow(dead_code)]
pub mod behavior_driver;
pub mod sshd_test_support;
pub mod tmux_test_support;
