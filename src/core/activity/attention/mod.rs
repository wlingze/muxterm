//! 阶段 B 注意力：信号 → 状态机 → 聚合。
//!
//! - [`signal`]：TerminalState 产出的注意力信号
//! - [`state`]：pane 状态机
//! - [`screen`]：tmux agent 屏幕规则（对照 Herdr manifest，无 hook）
//! - [`engine`]：跨工作区聚合（C2.3 加入）
//! - [`clock`]：可注入时钟（C2.3 加入）

pub mod clock;
pub mod engine;
pub mod input;
pub mod screen;
pub mod signal;
pub mod state;
