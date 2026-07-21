//! Muxterm library root — re-exports core modules for integration tests.
//!
//! main.rs 仍然作为 bin 入口；这个 lib.rs 让集成测试能 `use muxterm::core::...`。

pub mod cli;
pub mod core;
