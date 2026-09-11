//! Workspace：池里一格，标准内部结构 Tab → Pane 的宿主。
//!
//! W1 先立住「一个 Workspace = 一个 Runtime+ 本工作区
//! pane 文本副本」。WorkspacePool 在 W2 加入。

mod pane_buf;
mod pool;
mod provenance;
mod spec;
mod template;
mod template_apply;
mod terminal_model;
#[allow(clippy::module_inception)] // 计划目录约定：workspace/workspace.rs 放 Workspace 本体
mod workspace;

pub use pane_buf::PaneBuf;
pub use pool::{
    WorkspaceCapacityCandidate, WorkspaceEvictionReason, WorkspaceLifecycle, WorkspacePool,
    WorkspacePoolPolicy,
};
pub use provenance::WorkspaceProvenance;
pub use spec::WorkspaceSpec;
pub use template::{
    PaneTemplate, TabTemplate, TemplateLayout, TemplateName, TemplateRegistry, WorkspaceTemplate,
};
pub use template_apply::{TemplateApplication, TemplateApplyReport, TemplateSkip};
pub use terminal_model::TerminalModel;
pub use workspace::{SearchHit, Workspace};
