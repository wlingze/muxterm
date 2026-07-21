//! Terminal 层纯模型：两层抽象的核心。
//!
//! - [`layout`]：布局树（session/window/pane 嵌套分割），纯数据结构
//! - [`state`]：状态快照 + `State` trait + `StateChange` 事件
//! - [`task`]：`Task` enum，纯操作描述
//! - [`backend`]：`Backend` trait，统一 TmuxBackend / LocalBackend
//! - [`terminal_model`]：`TerminalModel`，编排 task → backend → state → 事件流
//!
//! 本模块**无 I/O、无 GUI 依赖**，所有代码可在无 DISPLAY 环境下 `cargo test`。
//!
//! Step 2：TerminalModel 已接入，MockBackend 覆盖常见 Task 行为；后续 Step 3+
//! 接入 LocalBackend / TmuxBackend 后，逐步把平台层切到 TerminalModel。
// 重构过渡期：core::model 的部分 API 尚未被 GTK 前端使用（GTK 仍走旧路径），
// 保留全部 API 供 TUI 前端 + 未来 GTK 切换。统一放宽 dead_code。
#![allow(dead_code)]

pub mod backend;
pub mod layout;
pub mod state;
pub mod task;
pub mod terminal_model;

// 便捷 re-export（被 TerminalModel / 平台层使用）
#[allow(unused_imports)]
pub use backend::Backend;
#[allow(unused_imports)]
pub use layout::{LayoutNode, RemoveRootError, SplitDir, WindowLayout};
#[allow(unused_imports)]
pub use state::{BackendStatus, PaneInfo, SessionInfo, State, StateChange, WindowInfo};
#[allow(unused_imports)]
pub use task::{Task, TaskOutcome};
#[allow(unused_imports)]
pub use terminal_model::{StateChangeCallback, TerminalModel};
