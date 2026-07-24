#![allow(dead_code)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::borrowed_box)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::needless_pass_by_value)]
//! Muxterm library root — re-exports core modules for integration tests.
//!
//! main.rs 仍然作为 bin 入口；这个 lib.rs 让集成测试能 `use muxterm::core::...`。

pub mod cli;
pub mod core;
pub mod main_entry;

/// 平台前端（GTK / TUI），供集成测试与库调用方使用。
#[cfg(any(feature = "gtk", feature = "tui"))]
pub mod platform;
