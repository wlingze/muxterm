#![allow(dead_code)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::borrowed_box)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::needless_pass_by_value)]
//! Core 层：非 GUI 逻辑，与平台无关。

#[cfg(test)]
use std::sync::Mutex;

/// 测试中修改进程级 PATH 时使用同一把锁，避免影响并行测试。
#[cfg(test)]
pub(crate) static PATH_ENV_LOCK: Mutex<()> = Mutex::new(());

pub mod attention;
pub mod buffer_cap;
pub mod catalog;
pub mod config;
pub mod config_service;
pub mod discovery;
pub mod executable;
pub mod fault;
pub mod logging;
#[cfg(feature = "ffi")]
pub mod muxterm;
pub mod projects;
pub mod protocol;
pub mod quickconnect;
pub mod render_policy;
pub mod runtime;
pub mod transport;
pub mod types;
pub mod url_detect;
pub mod workspace;
