//! Core 层：非 GUI 逻辑，与平台无关。
//!
//! 当前批次：buffer_cap + types。
//! 后续批次逐步迁入 model / runtime / transport / protocol / config / discovery。

pub mod buffer_cap;
pub mod types;
