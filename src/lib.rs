#![allow(dead_code)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::borrowed_box)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::needless_pass_by_value)]
//! Muxterm library root.
pub mod buffer_cap;
pub mod config;
pub mod discovery;
pub mod platform;
pub mod protocol;
pub mod runtime;
pub mod terminal;
pub mod transport;
pub mod types;
