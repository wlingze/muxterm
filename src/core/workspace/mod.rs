//! Workspace：池里一格，标准内部结构 Tab → Pane 的宿主。
//!
//! W1 先立住「一个 Workspace = 一个 Runtime+ 本工作区
//! pane 文本副本」。WorkspacePool 在 W2 加入。

pub mod id;
pub mod pane_buf;
pub mod pool;
pub mod spec;
#[allow(clippy::module_inception)] // 计划目录约定：workspace/workspace.rs 放 Workspace 本体
pub mod workspace;
