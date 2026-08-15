//! QuickConnect / status bar / 连接池的纯逻辑模块（无 GTK 依赖）。
//!
//! 这些模型与 macOS Chrome 层行为一致，Linux GTK 前端与单元测试共用。

pub mod directory;
pub mod event_policy;
pub mod font;
pub mod options;
pub mod project_flow;
pub mod status_style;
pub mod tab_gate;

/// 目标模型与 `~/.config/muxterm/quickconnect.toml` 在 core，平台层再导出。
pub use crate::core::quickconnect::{model, store};
