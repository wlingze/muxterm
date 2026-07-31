//! C ABI 导出（`feature = "ffi"`）。
//!
//! 让 macOS (Swift) / Windows (C#) 等通过 FFI 调用核心：
//! - [`types`]：`#[repr(C)]` 友好类型
//! - [`api`]：生命周期 / 执行 / 轮询 / 状态查询
//! - [`callbacks`]：可选事件回调

#![allow(unused_imports)] // pub use 供外部 crate / C 头文件侧使用

pub mod api;
pub mod callbacks;
pub mod types;

pub use api::*;
pub use callbacks::*;
pub use types::*;
