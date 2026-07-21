//! Terminal 层纯模型：两层抽象的核心。
//!
//! - [`layout`]：布局树（session/window/pane 嵌套分割），纯数据结构
//! - [`state`]：状态快照 + `State` trait + `StateChange` 事件
//! - [`task`]：`Task` enum，纯操作描述
//! - [`backend`]：`Backend` trait，统一 TmuxBackend / LocalBackend
//!
//! 本模块**无 I/O、无 GUI 依赖**，所有代码可在无 DISPLAY 环境下 `cargo test`。
//! TerminalModel（Step 2 引入）将组合这些 trait，提供可测试的纯逻辑层。
//!
//! 当前为 Step 1：trait/类型已定义，尚未被平台层使用，故暂挂 `dead_code` 以
//! 避免编译警告；Step 2 接入 TerminalModel 后逐步移除。

#![allow(dead_code)]
#![allow(unused_imports)]

pub mod backend;
pub mod layout;
pub mod state;
pub mod task;

// 便捷 re-export（Step 2+ 起被 TerminalModel / 平台层使用）
pub use backend::Backend;
pub use layout::{LayoutNode, SplitDir, WindowLayout};
pub use state::{BackendStatus, PaneInfo, SessionInfo, State, StateChange, WindowInfo};
pub use task::{Task, TaskOutcome};
