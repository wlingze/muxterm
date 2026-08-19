//! 测试支持模块：共享 long-chain 集成测试支持。
//!
//! - behavior_driver: 共享 2tab3pane 行为场景和断言
//! - sshd_test_support: SSH loopback sshd 连接参数
//! - tmux_test_support: 独立 tmux socket/session 管理 + 硬超时
//! - linux_gtk: Linux GTK4 测试共享助手（无 DISPLAY 跳过、widget 树查找、按键模拟）

#[allow(dead_code)]
pub mod attach_history_contract;
#[allow(dead_code)]
pub mod behavior_driver;
pub mod feature_e2e_contract;
pub mod herdr_test_support;
#[cfg(feature = "gtk")]
pub mod linux_gtk;
pub mod runtime_transport_matrix;
pub mod ssh_tmux_contract;
pub mod sshd_test_support;
pub mod tmux_test_support;
pub mod workspace_attach_contract;
