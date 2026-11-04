//! GPUI 前端（跨平台，一份 widget 树）。方案见 `docs/FRONTEND-GPUI.md`。
//!
//! 只经 `frontend::utils::corebridge` 使用 Core；不 `use` Core 内部模块。

pub mod app;
pub mod chrome;
pub mod state;
pub mod terminal;
pub mod theme;

pub use app::run;
