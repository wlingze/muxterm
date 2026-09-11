#![allow(dead_code)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::borrowed_box)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::needless_pass_by_value)]
//! Muxterm library root.

#[cfg(test)]
use std::sync::Mutex;

/// 测试中修改进程级 PATH 时使用同一把锁，避免影响并行测试。
#[cfg(test)]
pub(crate) static PATH_ENV_LOCK: Mutex<()> = Mutex::new(());

#[path = "core/activity/mod.rs"]
pub mod activity;
#[path = "core/buffer_cap.rs"]
pub mod buffer_cap;
#[path = "core/catalog/mod.rs"]
pub mod catalog;
#[path = "core/config/mod.rs"]
pub mod config;
#[path = "core/discovery/mod.rs"]
pub mod discovery;
#[path = "core/executable.rs"]
pub mod executable;
#[path = "core/fault.rs"]
pub mod fault;
#[path = "core/logging.rs"]
pub mod logging;
#[cfg(feature = "ffi")]
#[path = "core/muxterm.rs"]
pub mod muxterm;
#[path = "core/projects/mod.rs"]
pub mod projects;
#[path = "core/protocol/mod.rs"]
pub mod protocol;
#[path = "core/render_policy.rs"]
pub mod render_policy;
#[path = "core/runtime/mod.rs"]
pub mod runtime;
#[path = "core/transport/mod.rs"]
pub mod transport;
#[path = "core/url_detect.rs"]
pub mod url_detect;
#[path = "core/workspace/mod.rs"]
pub mod workspace;

pub mod frontend;

/// Public C-ABI facade. Core implementation modules remain behind this boundary
/// for frontend callers that use the FFI contract.
#[cfg(feature = "ffi")]
pub mod ffi {
    pub use crate::protocol::ffi::*;
}

/// Test-only compatibility exports for the existing integration contract suite.
#[doc(hidden)]
pub mod test_support {
    pub mod core {
        #[cfg(feature = "ffi")]
        pub use crate::muxterm;
        pub use crate::{
            activity, buffer_cap, catalog, config, discovery, executable, fault, logging, projects,
            protocol, render_policy, runtime, transport, url_detect, workspace,
        };
    }

    pub mod platform {
        pub use crate::frontend::*;
    }
}
